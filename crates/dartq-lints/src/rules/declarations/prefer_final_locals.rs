use super::{
    Check, NodeKind, RuleSpec, local_written_type, name_written_in_window, utf16_token_run_span,
};
use crate::rules::{
    NONE_TOKEN, initialized_identifier_name, last_child, utf16_token_span,
    variables_declaration_keyword,
};
use dartq_ast::PayloadField;

/// Official: `pkg/linter/lib/src/rules/prefer_final_locals.dart` — three
/// faces:
/// * `visitVariableDeclarationList` (:166-189): a non-`const`/non-`final`
///   local variable list (:167) inside a function body (:169) where every
///   variable carries `=` and an initializer (:173-175) and none is a
///   wildcard or mutated in scope (:178-179) reports at the keyword, else
///   the written type (:183-188).
/// * `visitDeclaredVariablePattern` (:79-141): a pattern's declared
///   variable — an if-case clause, a switch-case clause, or a for-each
///   pattern — not declared `final` (:84, :223-231), not inside a
///   switch-expression case (:90-92), not the second binder of a join
///   (:63-70) and not mutated in the enclosing function reports at the
///   pattern node (:123, :139).
/// * `visitPatternVariableDeclaration` (:144-163): a statement
///   `var <pattern> = ...` whose declared variables are never mutated
///   (:158) reports at the `var` keyword (:160); a pattern that contains
///   just wildcards stays silent (:159, :199-219).
///
/// Approximation (documented): the mutation face is the enclosing
/// function's token scan after the declaration — the same lexical
/// `isPotentiallyMutatedInScope` stand-in every face of this rule uses
/// (a shadowing redeclaration inside the window under-reports); plain
/// reads — including inside closures — pass. The just-wildcards face is
/// approximated by "the pattern binds no variable" (no
/// `DeclaredVariablePattern` row): a constant-only pattern (`var [1]`),
/// which upstream's recursive `containsJustWildcards` also reports,
/// stays silent. The join dedupe spans one clause; a join across sibling
/// case clauses of one switch body can double-report. Message:
/// `diagnostic.g.dart` `preferFinalLocals`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_final_locals",
    &[
        NodeKind::VariablesDeclaration,
        NodeKind::DeclaredVariablePattern,
        NodeKind::PatternVarDeclStatement,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::DeclaredVariablePattern => declared_pattern(c),
        NodeKind::PatternVarDeclStatement => pattern_var_declaration(c),
        _ => variable_declaration_list(c),
    }
}

/// `visitVariableDeclarationList` (:166-189).
fn variable_declaration_list(c: &mut Check<'_>) {
    let node = c.node();
    // `thisOrAncestorOfType<FunctionBody>` (:169) — the local faces
    // (statement, block body, C-style for initializer).
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if !matches!(
        c.kind(parent),
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement
    ) {
        return;
    }
    // `node.isConst || node.isFinal` (:167).
    if variables_declaration_keyword(c, node)
        .is_some_and(|keyword| matches!(c.lexeme(keyword), "final" | "const"))
    {
        return;
    }
    let Some(owner) = enclosing_owner(c) else {
        return;
    };
    let mut names = Vec::new();
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        // `variable.equals == null || variable.initializer == null`
        // (:173-175).
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        if initializer.is_none_or(|init| last_child(c, init).is_none()) {
            return;
        }
        // `declaredElement.isWildcardVariable` (:178).
        let Some(name_token) = initialized_identifier_name(c, var) else {
            return;
        };
        let name = c.lexeme(name_token);
        if name == "_" {
            return;
        }
        names.push(name);
    }
    if names.is_empty() {
        return;
    }
    // `function.isPotentiallyMutatedInScope` (:179).
    let window = c.unit.ast.token_range(owner);
    let decl = c.unit.ast.token_range(node);
    if names
        .iter()
        .any(|name| name_written_in_window(c, decl.end..window.end, name))
    {
        return;
    }
    // `rule.reportAtToken(keyword)`, else the type (:183-188).
    if let Some(keyword) = variables_declaration_keyword(c, node) {
        c.report(utf16_token_span(c, keyword), &[]);
    } else {
        let Some((start, end)) = local_written_type(c, node) else {
            return;
        };
        let Some(range) = utf16_token_run_span(c, start..end) else {
            return;
        };
        c.report(range, &[]);
    }
}

/// The nearest function-like owner on the visit path — the official
/// `thisOrAncestorOfType<FunctionBody>` stand-in (:86, :148) shared by
/// every face's mutation window.
fn enclosing_owner(c: &Check<'_>) -> Option<u32> {
    c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::TopLevelMethod
                | NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::FunctionExpression
                | NodeKind::NamedFunctionExpression
        )
    })
}

/// The clause a declared-variable pattern sits in — the official parent
/// walk (:89-101) under flat tiling: the first case/loop/statement
/// container on the path decides the report arm.
enum PatternContext {
    /// An if-case's clause or a switch case — the official `CaseClause`
    /// arm (:104-124) and the switch-case else-arm (:125-140) share the
    /// mutation-gated report at the node.
    Case(u32),
    /// A for-each pattern (:126-132).
    ForEach(u32),
    /// Anything else (statement-position patterns).
    Statement,
}

fn pattern_context(c: &Check<'_>) -> PatternContext {
    for &row in c.path[..c.path.len() - 1].iter().rev() {
        match c.kind(row) {
            NodeKind::SwitchCase => return PatternContext::Case(row),
            NodeKind::ForIn => return PatternContext::ForEach(row),
            NodeKind::IfStatement => {
                // Only a case clause's pattern rows report through the
                // clause arm; a plain `if` holds no declared patterns of
                // its own.
                let has_case = c
                    .collect_direct(row)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
                    .and_then(|cond| c.field(cond, PayloadField::CaseKeyword))
                    .is_some_and(|case| case != NONE_TOKEN);
                if has_case {
                    return PatternContext::Case(row);
                }
            }
            NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement => {
                return PatternContext::Statement;
            }
            _ => {}
        }
    }
    PatternContext::Statement
}

/// `visitDeclaredVariablePattern` (:79-141).
fn declared_pattern(c: &mut Check<'_>) {
    let node = c.node();
    let Some(variable) = c
        .field(node, PayloadField::Variable)
        .filter(|&token| token != NONE_TOKEN)
    else {
        return;
    };
    let name = c.lexeme(variable);
    // Switch-expression cases never report (:90-92).
    if c.path[..c.path.len() - 1]
        .iter()
        .any(|&row| c.kind(row) == NodeKind::SwitchExpressionCase)
    {
        return;
    }
    // The rows a pattern-variable declaration owns report through that
    // statement's face (:80-83). Under flat tiling they are siblings, so
    // the enclosing statement container's subtree is probed for a
    // `PatternVarDeclStatement` covering the variable token (a
    // switch-expression initializer inside the statement is covered by
    // the :90-92 bail either way).
    let Some(container) = c.path[..c.path.len() - 1]
        .iter()
        .rev()
        .copied()
        .find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::Block
                    | NodeKind::BlockBody
                    | NodeKind::SwitchCase
                    | NodeKind::IfStatement
                    | NodeKind::ForIn
                    | NodeKind::ForStatement
            )
        })
    else {
        return;
    };
    if (c.children(container).start..=container).any(|row| {
        c.kind(row) == NodeKind::PatternVarDeclStatement
            && c.unit.ast.token_range(row).contains(&variable)
    }) {
        return;
    }
    let Some(owner) = enclosing_owner(c) else {
        return;
    };
    let context = pattern_context(c);
    // `isDeclaredFinal` (:223-231): the row's own keyword, or the
    // for-each pattern's.
    let head = c.head_token(node);
    if let PatternContext::ForEach(forin) = context {
        let parts_final = c
            .collect_direct(forin)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
            .and_then(|parts| c.field(parts, PayloadField::PatternKeyword))
            .is_some_and(|keyword| keyword != NONE_TOKEN && c.lexeme(keyword) == "final");
        if parts_final {
            return;
        }
    }
    if c.lexeme(head) == "final" {
        return;
    }
    let window = c.unit.ast.token_range(owner);
    let decl = c.unit.ast.token_range(node);
    match context {
        // The clause arm: only the first binder of a join reports
        // (:63-70, :104-124, :138-140).
        PatternContext::Case(clause) => {
            let joined_earlier = (c.children(clause).start..node).any(|row| {
                c.kind(row) == NodeKind::DeclaredVariablePattern
                    && c.field(row, PayloadField::Variable)
                        .is_some_and(|token| token != NONE_TOKEN && c.lexeme(token) == name)
            });
            if !joined_earlier && !name_written_in_window(c, decl.end..window.end, name) {
                report_pattern(c, node, head, variable);
            }
        }
        // The for-each arm: any binder of the pattern mutated in the
        // function spares every binder (:126-132).
        PatternContext::ForEach(forin) => {
            let mutated = c.collect_direct(forin).into_iter().any(|row| {
                c.kind(row) == NodeKind::DeclaredVariablePattern
                    && c.field(row, PayloadField::Variable).is_some_and(|token| {
                        token != NONE_TOKEN
                            && name_written_in_window(c, decl.end..window.end, c.lexeme(token))
                    })
            });
            if !mutated {
                report_pattern(c, node, head, variable);
            }
        }
        // The statement arm (:133-135, :138-140).
        PatternContext::Statement => {
            if !name_written_in_window(c, decl.end..window.end, name) {
                report_pattern(c, node, head, variable);
            }
        }
    }
}

/// `reportAtNode(node)` (:123, :139) — the official node span covers a
/// leading type (`int x`); the bridge anchors a typed row at the name, so
/// an adjacent `Type` sibling row extends the span back over the type.
fn report_pattern(c: &mut Check<'_>, node: u32, head: u32, variable: u32) {
    let start = if matches!(c.lexeme(head), "var" | "final") {
        head
    } else {
        c.unit
            .ast
            .parent(node)
            .and_then(|parent| {
                let siblings = c.collect_direct(parent);
                let pos = siblings.iter().position(|&kid| kid == node)?;
                let prev = siblings.get(pos.checked_sub(1)?)?;
                (c.kind(*prev) == NodeKind::Type && c.unit.ast.token_range(*prev).end == head)
                    .then_some(c.head_token(*prev))
            })
            .unwrap_or(head)
    };
    if let Some(range) = utf16_token_run_span(c, start..variable + 1) {
        c.report(range, &[]);
    }
}

/// `visitPatternVariableDeclaration` (:144-163).
fn pattern_var_declaration(c: &mut Check<'_>) {
    let node = c.node();
    // `node.keyword.isFinal` (:146) — the row anchors at the keyword.
    let head = c.head_token(node);
    if c.lexeme(head) == "final" {
        return;
    }
    let Some(owner) = enclosing_owner(c) else {
        return;
    };
    // The pattern rows tile before the statement row, bounded at the
    // previous statement.
    let Some(parent) = c.unit.ast.parent(node) else {
        return;
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&kid| kid == node) else {
        return;
    };
    let names: Vec<&str> = crate::rules::slice::statement_run(c, &siblings[..pos])
        .into_iter()
        .filter(|&row| c.kind(row) == NodeKind::DeclaredVariablePattern)
        .filter_map(|row| {
            c.field(row, PayloadField::Variable)
                .filter(|&token| token != NONE_TOKEN)
                .map(|token| c.lexeme(token))
        })
        .collect();
    // `node.pattern.containsJustWildcards` (:159) — the just-wildcards
    // pattern binds nothing; the no-binder approximation (rule doc).
    if names.is_empty() {
        return;
    }
    // `hasPotentiallyMutatedDeclaredVariableInScope` (:158).
    let window = c.unit.ast.token_range(owner);
    let decl = c.unit.ast.token_range(node);
    if names
        .iter()
        .any(|name| name_written_in_window(c, decl.end..window.end, name))
    {
        return;
    }
    // `reportAtToken(node.keyword)` (:160).
    c.report(utf16_token_span(c, head), &[]);
}
