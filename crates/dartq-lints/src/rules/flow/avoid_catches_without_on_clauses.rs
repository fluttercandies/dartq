use super::{PayloadField, Check, NodeKind, RuleSpec, last_child};

/// Official: `pkg/linter/lib/src/rules/avoid_catches_without_on_clauses.dart:30-104`
/// — a bare `catch` clause (no `on`) reports at the `catch` keyword
/// unless the caught exception has a *valid use* in the clause body:
/// a `rethrow` reached before any nested catch clause (:96-101, the
/// `_canRethrow` fence) or a `throw` expression referencing the caught
/// exception (:107-117). A clause without an exception parameter or
/// without a `catch` keyword skips (:69-73, :86-88).
///
/// Approximation (documented): the remaining valid-use faces of the
/// official `_ValidUseVisitor` (:111-155) key on static types —
/// `Never`-typed invocations, `Future.error`, `FlutterError.reportError`,
/// `Completer.completeError` — and stay silent here (under-report). A
/// throw references the exception when an identifier of its name sits
/// inside the throw's subtree AND no nearer binder of that name captures
/// the occurrence (the official `node.element == caughtException`
/// identity over the same-file scope walk): a closure parameter, a
/// nested block's local, a nested catch's parameter, a loop variable,
/// or a case pattern variable of the same lexeme re-binds every lexeme
/// inside its extent, so those reads are the binder's, never the caught
/// exception's.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_catches_without_on_clauses",
    &[NodeKind::CatchClause],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `onKeyword != null` (:69-70): the on-form opens at `on`
    // (head-when-present).
    if c.lexeme(c.head_token(node)) == "on" {
        return;
    }
    // `catchKeyword == null` (:86-88).
    let Some(catch_kw) = c
        .field(node, PayloadField::CatchKeyword)
        .filter(|&kw| kw != super::NONE_TOKEN)
    else {
        return;
    };
    // `exceptionParameter == null` (:72-73): the clause's first
    // `FormalParameter` row (the catch formals ride one
    // `FormalParameters` child, the production probe shape).
    let window = c.children(node);
    let Some(exception) = window
        .clone()
        .find(|&row| c.kind(row) == NodeKind::FormalParameter)
    else {
        return;
    };
    let Some(name) = c
        .field(exception, PayloadField::Name)
        .filter(|&tok| tok != super::NONE_TOKEN)
        .map(|tok| c.lexeme(tok))
    else {
        return;
    };
    let Some(body) = last_child(c, node) else {
        return;
    };
    if c.kind(body) != NodeKind::Block {
        return;
    }

    // The first nested catch clause row fences the rethrow face
    // (:96-98 `_canRethrow` — false from the first nested `catch` on,
    // never restored).
    let nested_catch = window
        .clone()
        .filter(|&row| row != node && c.kind(row) == NodeKind::CatchClause)
        .min();
    // The bridge flattens a throw's operand rows as siblings completed
    // before the width-0 `throw` row, so a throw references the
    // exception (:107-117) when an identifier of its name sits between
    // the throw's token and the throw row itself — the operand run.
    // Rows after an earlier statement row cannot belong to the throw,
    // which bounds each throw to its own statement.
    let throws: Vec<u32> = window
        .clone()
        .filter(|&row| c.kind(row) == NodeKind::ThrowExpression)
        .collect();
    let mut valid = false;
    for row in window.clone() {
        // The collapse keeps one rethrow kind (the statement form
        // covers `=> rethrow` bodies too).
        if c.kind(row) == NodeKind::RethrowStatement
            && nested_catch.is_none_or(|first| row < first)
        {
            valid = true;
            break;
        }
        if c.kind(row) != NodeKind::Identifier || c.lexeme(c.head_token(row)) != name {
            continue;
        }
        // A nearer binder of the same lexeme re-owns the occurrence (the
        // element identity): the read is not the caught exception.
        if shadowed_by_nearer_binder(c, row, node, name) {
            continue;
        }
        let head = c.head_token(row);
        if throws.iter().any(|&throw| {
            row < throw
                && head > c.head_token(throw)
                && window
                    .clone()
                    .filter(|&stmt| stmt < throw && crate::rules::is_statement_row(c, stmt))
                    .all(|stmt| stmt < row)
        }) {
            valid = true;
            break;
        }
    }
    if valid {
        return;
    }
    c.report(crate::rules::utf16_token_span(c, catch_kw), &[]);
}

/// Whether a binder of `name` nested between the catch header and the
/// identifier row `row` captures the occurrence — the same-file stand-in
/// for the official element identity (`node.element == caughtException`).
/// The walk descends the subtree extents from the catch clause: the
/// production tiling keeps the catch body flat (every material row's
/// parent row is the body block), so `row` lies in `s`'s subtree iff
/// `children(s).start <= row <= s` — each descending scope that contains
/// the row contributes its binder set (a block's sibling binding
/// statements, a nested catch's and a closure's own parameters, a
/// named/local function's own name and parameters, for-loop heads, and
/// switch-case pattern variables).
fn shadowed_by_nearer_binder(c: &Check<'_>, row: u32, catch_clause: u32, name: &str) -> bool {
    let mut child = row;
    let mut scope = catch_clause;
    loop {
        let Some(next) = c
            .collect_direct(scope)
            .into_iter()
            .find(|&kid| kid > row && c.unit.ast.children(kid).start <= row)
        else {
            return false;
        };
        let binds = match c.kind(next) {
            // A function-expression body tiles as `BlockBody`; both
            // block kinds carry sibling statement binders.
            NodeKind::Block | NodeKind::BlockBody => c
                .collect_direct(next)
                .into_iter()
                .any(|stmt| stmt != child && statement_binds(c, stmt, name)),
            NodeKind::CatchClause | NodeKind::FunctionExpression => c
                .collect_direct(next)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                .is_some_and(|params| parameters_bind(c, params, name)),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                function_name_binds(c, next, name)
                    || c.collect_direct(next)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                        .is_some_and(|params| parameters_bind(c, params, name))
            }
            NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, next, name),
            NodeKind::ForStatement => c
                .collect_direct(next)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::ForParts)
                .is_some_and(|parts| {
                    c.collect_direct(parts)
                        .into_iter()
                        .any(|stmt| statement_binds(c, stmt, name))
                }),
            NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                c.collect_direct(next).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::DeclaredVariablePattern
                        && c.field(kid, PayloadField::Variable).is_some_and(|v| {
                            v != crate::rules::NONE_TOKEN && c.lexeme(v) == name
                        })
                })
            }
            _ => false,
        };
        if binds {
            return true;
        }
        child = next;
        scope = next;
    }
}

/// Whether a statement declares `name`: a variable declaration, a
/// pattern-variable declaration statement, a local/named function (its
/// own name binds through its body), or a labeled statement wrapping
/// one of those.
fn statement_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    match c.kind(stmt) {
        NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::InitializedIdentifier
                && crate::rules::initialized_identifier_name(c, decl)
                    // The helper answers a TOKEN id — compare its
                    // lexeme directly, never re-head it.
                    .is_some_and(|n| c.lexeme(n) == name)
        }),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, stmt, name)
        }
        NodeKind::PatternVarDeclStatement => c.children(stmt).any(|descendant| {
            c.kind(descendant) == NodeKind::DeclaredVariablePattern
                && c
                    .field(descendant, PayloadField::Variable)
                    .is_some_and(|v| v != crate::rules::NONE_TOKEN && c.lexeme(v) == name)
        }),
        NodeKind::LabeledStatement => c
            .collect_direct(stmt)
            .into_iter()
            .any(|kid| c.kind(kid) != NodeKind::Label && statement_binds(c, kid, name)),
        _ => false,
    }
}

/// The name a local/named function binds (`FunctionName` child — the
/// production tiling nests the stamped `Identifier` inside it).
fn function_name_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    crate::rules::declarations::member_declaration_name(c, node)
        .is_some_and(|n| c.lexeme(c.head_token(n)) == name)
}

/// Whether a formal-parameter list declares `name` (plain and
/// function-typed formals; the optional named group's formals read
/// through their wrapper).
fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
    c.collect_direct(params)
        .into_iter()
        .any(|param| match c.kind(param) {
            NodeKind::FormalParameter => c
                .field(param, PayloadField::Name)
                .filter(|&tok| tok != crate::rules::NONE_TOKEN)
                .is_some_and(|tok| c.lexeme(tok) == name),
            NodeKind::FunctionTypedFormalParameter => c
                .collect_direct(param)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                .is_some_and(|kid| c.lexeme(c.head_token(kid)) == name),
            NodeKind::OptionalFormalParameters => parameters_bind(c, param, name),
            _ => false,
        })
}

/// Whether the for-in loop variable binds `name`: the bare form is an
/// `Identifier` child completing before the parts' `in` token, the `var
/// x` form a `DeclaredVariablePattern` child.
fn for_in_binds(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let kids = c.collect_direct(scope);
    let Some(in_token) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
        .and_then(|parts| c.field(parts, PayloadField::InKeyword))
        .filter(|&token| token != crate::rules::NONE_TOKEN)
    else {
        return false;
    };
    kids.into_iter().any(|kid| match c.kind(kid) {
        NodeKind::DeclaredVariablePattern => c
            .field(kid, PayloadField::Variable)
            .is_some_and(|v| v < in_token && c.lexeme(v) == name),
        // The `var x` form tiles as a `VariablesDeclaration` completing
        // before the `ForInParts` row (its `InitializedIdentifier`
        // carries the loop-variable name on its span head).
        NodeKind::VariablesDeclaration => c.collect_direct(kid).into_iter().any(|var| {
            c.kind(var) == NodeKind::InitializedIdentifier
                && crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|tok| tok < in_token && c.lexeme(tok) == name)
        }),
        NodeKind::Identifier => {
            c.unit.ast.token_range(kid).end <= in_token && c.lexeme(c.head_token(kid)) == name
        }
        _ => false,
    })
}
