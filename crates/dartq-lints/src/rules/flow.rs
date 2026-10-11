//! The flow-control family: `curly_braces_in_flow_control_structures`,
//! `control_flow_in_finally`, `no_duplicate_case_values`,
//! `use_rethrow_when_possible`, `avoid_returning_null_for_void`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, last_child};

/// Official: `pkg/linter/lib/src/rules/curly_braces_in_flow_control_structures.dart:36-95`
/// — control-flow bodies must be blocks, except a single-line `if` without
/// `else` (`if (c) return;` on one line is allowed, :59-70). Message:
/// `diagnostic.g.dart` `curlyBracesInFlowControlStructures` with the
/// construct spelled into `{0}`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "curly_braces_in_flow_control_structures",
    group: Group::Core,
    problem: "Statements in {0} should be enclosed in a block.",
    correction: "Try wrapping the statement in a block.",
    kinds: &[
        NodeKind::IfStatement,
        NodeKind::WhileStatement,
        NodeKind::ForStatement,
        NodeKind::DoWhileStatement,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::WhileStatement => {
            // Body is the last child (the condition is the first,
            // collapse map §3.4 WhileStatement row).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a while");
            }
        }
        NodeKind::ForStatement => {
            // Body is the last child after the for-parts (:82-84).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a for");
            }
        }
        NodeKind::DoWhileStatement => {
            // Body is the first child, before the `while (...)` condition
            // (collapse map §3.4 DoWhileStatement row: body = the children
            // segment before the While token).
            if let Some(body) = first_child(c, node) {
                check_body(c, body, "a do");
            }
        }
        NodeKind::IfStatement => check_if(c, node),
        _ => {}
    }
}

/// `_check` (curly_braces...dart:93-95): a non-block body reports.
fn check_body(c: &mut Check<'_>, body: u32, where_: &str) {
    if c.kind(body) != NodeKind::Block {
        let range = c.utf16_span(body);
        c.report(range, &[where_]);
    }
}

fn check_if(c: &mut Check<'_>, node: u32) {
    let else_present = c.payload(node).first().copied() != Some(NONE_TOKEN);
    // Direct children: condition, then-arm, [else-arm]. The scratch guard is
    // scoped so the reporting calls below can take `&mut Check`.
    let arms = {
        let kids = c.direct_children(node);
        (
            kids.get(1).copied(),
            kids.get(2).copied(),
            last_child(c, node),
        )
    };
    let Some(then) = arms.0 else {
        return;
    };
    if else_present {
        // With an `else`, both arms are checked; an else-if chain arm is
        // skipped (`elseStatement is! IfStatement`, :71-77).
        check_body(c, then, "an if");
        if let Some(else_stmt) = arms.1 {
            if c.kind(else_stmt) != NodeKind::IfStatement {
                check_body(c, else_stmt, "an if");
            }
        }
        return;
    }
    // An `if` without `else` that IS the else branch of an outer if: the
    // outer chain already requires blocks, so the then-arm is checked
    // unconditionally (:55-61).
    if let Some(parent) = c.ancestor(1) {
        if c.kind(parent) == NodeKind::IfStatement
            && c.payload(parent).first().copied() != Some(NONE_TOKEN)
            && arms.2 == Some(node)
        {
            check_body(c, then, "an if");
            return;
        }
    }
    // Single-line form allowed (:62-69): report only when the statement
    // spans lines (`lineOf(if) != lineOf(then.end)`).
    if c.kind(then) == NodeKind::Block {
        return;
    }
    let if_line = c
        .unit
        .lines
        .line_col_utf16(c.unit.source, c.utf16_span(node).start)
        .0;
    let then_end = c.utf16_span(then).end;
    let then_line = c.unit.lines.line_col_utf16(c.unit.source, then_end).0;
    if if_line != then_line {
        let range = c.utf16_span(then);
        c.report(range, &["an if"]);
    }
}

pub(crate) mod control_flow_in_finally {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, last_child};

    /// Official: `pkg/linter/lib/src/rules/control_flow_in_finally.dart:24-107`
    /// — `break`/`continue`/`return` inside a `finally` block reports, unless
    /// the statement sits inside a closure within the block (its jump binds
    /// locally), or — for targeted `break`/`continue` — the resolved target
    /// sits within the try statement (the jump stays local to it). Targets
    /// resolve through the official label machinery
    /// (`_lookupBreakOrContinueTarget`, `scope_analyzer.dart:1042-1093`): a
    /// labeled jump through the lexical label scope — the nearest enclosing
    /// labeled statement or switch member declaring the label, where a
    /// statement's label binds the statement after the labels
    /// (`visitLabeledStatement`, :631-639) — an untargeted jump through the
    /// break/continue context chain (the nearest enclosing loop; `continue`
    /// skips switches, `scope.dart:333-341`; the chain resets at every
    /// function body, :224). Message: `diagnostic.g.dart`
    /// `controlFlowInFinally`.
    ///
    /// Approximation (documented): the switch's member labels scope across
    /// the whole statement officially (`scope_analyzer.dart:847-850`), so a
    /// forward `continue L` to a later member resolves there but not in
    /// this lexical walk — a member label matches only when the member
    /// encloses the jump.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "control_flow_in_finally",
        group: Group::Recommended,
        problem: "Use of '{0}' in a 'finally' clause.",
        correction: "Try restructuring the code.",
        kinds: &[
            NodeKind::Break,
            NodeKind::Continue,
            NodeKind::ReturnStatement,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let node_idx = c.path.len() - 1;
        let kind = match c.kind(node) {
            NodeKind::Break => "break",
            NodeKind::Continue => "continue",
            NodeKind::ReturnStatement => "return",
            _ => return,
        };
        // `node.thisOrAncestorOfType<TryStatement>()` (:45-47).
        let Some(try_idx) = c
            .path
            .iter()
            .rposition(|n| c.kind(*n) == NodeKind::TryStatement)
        else {
            return;
        };
        let try_node = c.path[try_idx];
        // `finallyBlock == null` bails (:48-54): payload word 0 is the
        // `finally` token (collapse map §3.4 TryStatementEnd).
        if c.payload(try_node).first() == Some(&NONE_TOKEN) {
            return;
        }
        // The node must be inside the finally block — the try's last child
        // (body block, catch clauses, then the finally block).
        let Some(finally_block) = last_child(c, try_node) else {
            return;
        };
        if c.kind(finally_block) != NodeKind::Block
            || c.path.get(try_idx + 1) != Some(&finally_block)
        {
            return;
        }
        let report = match c.kind(node) {
            NodeKind::Break | NodeKind::Continue => match jump_target(c, node, node_idx) {
                // `target.thisOrAncestorMatching(n == tryStatement)` (:79-91):
                // the enabler (the target's own chain contains the try) keeps
                // the jump local — `thisOrAncestorMatching` includes the
                // target itself, so a label bound onto the try (`a: try …`)
                // also counts as within. Outside it the jump crosses the
                // finally and reports.
                Some(target_idx) => target_idx < try_idx,
                // Unresolved target (`target == null`): the closure exemption
                // decides (the `ancestor == null` branch, :69-77).
                None => !closure_wraps(c, try_idx + 1, node_idx),
            },
            _ => !closure_wraps(c, try_idx + 1, node_idx),
        };
        if report {
            let range = c.utf16_span(node);
            c.report(range, &[kind]);
        }
    }

    /// The resolved jump target's path index, `None` for the official
    /// `target == null` face (`_lookupBreakOrContinueTarget`,
    /// `scope_analyzer.dart:1042-1093`).
    fn jump_target(c: &Check<'_>, node: u32, node_idx: usize) -> Option<usize> {
        // The payload carries only the `has_target` bit (collapse map §3.4
        // BreakStatementHandle); the label identifier is the token after the
        // `break`/`continue` keyword, and a synthetic recovery token never
        // names a label.
        if c.payload(node).first().copied().unwrap_or(0) != 0 {
            let label_token = c.head_token(node) + 1;
            if c.token_is_synthetic(label_token) {
                return None;
            }
            let label = c.lexeme(label_token);
            let Some(binding) = c.path[..node_idx].iter().rposition(|&n| {
                matches!(c.kind(n), NodeKind::LabeledStatement | NodeKind::SwitchCase)
                    && declares_label(c, n, label)
            }) else {
                return None;
            };
            // `visitLabeledStatement` (:631-639): a statement's label binds
            // the statement after the labels — the next path node; a switch
            // member's label binds the member itself.
            return Some(match c.kind(c.path[binding]) {
                NodeKind::LabeledStatement => binding + 1,
                _ => binding,
            });
        }
        let is_continue = c.kind(node) == NodeKind::Continue;
        for (idx, &ancestor) in c.path[..node_idx].iter().enumerate().rev() {
            match c.kind(ancestor) {
                NodeKind::WhileStatement
                | NodeKind::ForStatement
                | NodeKind::ForIn
                | NodeKind::DoWhileStatement => return Some(idx),
                // An untargeted `continue` never binds a switch
                // (scope.dart:336-341).
                NodeKind::SwitchStatement if !is_continue => return Some(idx),
                // The context chain resets at every function body (:224) —
                // the jump cannot bind past a closure (`target == null`).
                NodeKind::FunctionExpression
                | NodeKind::NamedFunctionExpression
                | NodeKind::LocalFunctionDeclaration => return None,
                _ => {}
            }
        }
        None
    }

    /// Whether a labeled statement / switch member declares `label` among
    /// its `Label` children prefix (collapse map §3.4: the label count is
    /// the Label-children prefix length).
    fn declares_label(c: &Check<'_>, node: u32, label: &str) -> bool {
        c.collect_direct(node)
            .into_iter()
            .take_while(|&kid| c.kind(kid) == NodeKind::Label)
            .any(|kid| c.lexeme(c.head_token(kid)) == label)
    }

    /// Whether a closure (function expression / local function) sits on the
    /// path between the finally block and the statement — the official
    /// `FunctionBody inside the finally block` exemption (:69-77).
    fn closure_wraps(c: &Check<'_>, finally_idx: usize, node_idx: usize) -> bool {
        (finally_idx + 1..=node_idx).any(|i| {
            matches!(
                c.kind(c.path[i]),
                NodeKind::FunctionExpression | NodeKind::LocalFunctionDeclaration
            )
        })
    }
}

pub(crate) mod no_duplicate_case_values {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, first_child, last_child};

    /// Official: `pkg/linter/lib/src/rules/no_duplicate_case_values.dart:37-58`
    /// — a `case` whose constant value equals an earlier `case` of the same
    /// switch reports at the duplicate expression with both sources spelled
    /// into `{0}`/`{1}`. The official keys the map by the computed constant
    /// (`computeConstantValue`, :44); the face here is an in-rule constant
    /// folder over the syntax substrate — integer/string/bool/null
    /// literals, `+ - * ~/ %` and unary `-`, parentheses (64-bit wrapping
    /// integers, Dart `~/` truncation and `%` divisor-sign semantics),
    /// descending through `ConstantPattern`/`ParenthesizedExpression`.
    /// Expressions outside the folder's grammar evaluate unknown and never
    /// dedupe — the documented under-report family (const identifiers,
    /// doubles, interpolation, adjacent strings, const constructors),
    /// mirroring the official `hasKnownValue` skip (:47-49). Message:
    /// `diagnostic.g.dart` `noDuplicateCaseValues`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_duplicate_case_values",
        group: Group::Core,
        problem: "The value of the case clause ('{0}') is equal to the value of an earlier case clause ('{1}').",
        correction: "Try removing or changing the value.",
        kinds: &[NodeKind::SwitchStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let Some(block) = c
            .collect_direct(c.node())
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
        else {
            return;
        };
        // Collect first: each case re-enters `direct_children` (the scratch
        // RefCell forbids a live outer guard).
        let cases: Vec<u32> = c
            .direct_children(block)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::SwitchCase)
            .collect();
        let mut seen: Vec<(ConstValue, u32)> = Vec::new();
        for case in cases {
            // Expressions follow the label prefix (`labelCount` pruned to
            // the Label children, collapse map §3.4 SwitchCaseEnd); word 2
            // counts them.
            let count = c.payload(case).get(2).copied().unwrap_or(0) as usize;
            if count == 0 {
                continue;
            }
            let kids = c.collect_direct(case);
            let expressions: Vec<u32> = kids
                .into_iter()
                .filter(|&kid| c.kind(kid) != NodeKind::Label)
                .take(count)
                .collect();
            for expression in expressions {
                // `computeConstantValue` → `hasKnownValue` or skip (:44-49).
                let Some(value) = const_value(c, expression) else {
                    continue;
                };
                if let Some(&(_, original)) = seen.iter().find(|(seen, _)| *seen == value) {
                    let dup = source_slice(c, expression).to_owned();
                    let orig = source_slice(c, original).to_owned();
                    let range = c.utf16_span(expression);
                    c.report(range, &[&dup, &orig]);
                } else {
                    seen.push((value, expression));
                }
            }
        }
    }

    /// One computed case value — the map key the official builds from
    /// `DartObject`s (:38, :51).
    #[derive(PartialEq)]
    enum ConstValue {
        Null,
        Bool(bool),
        Int(i64),
        Str(Box<str>),
    }

    /// The constant value of one case expression, `None` when it is outside
    /// the folder's grammar (unknown — the official `hasKnownValue` skip).
    fn const_value(c: &Check<'_>, expression: u32) -> Option<ConstValue> {
        match c.kind(expression) {
            NodeKind::LiteralNull => Some(ConstValue::Null),
            NodeKind::LiteralBool => Some(ConstValue::Bool(
                c.lexeme(c.head_token(expression)) == "true",
            )),
            NodeKind::LiteralInt => Some(ConstValue::Int(int_literal(
                c.lexeme(c.head_token(expression)),
            )?)),
            NodeKind::StringLiteral => string_literal(c, expression),
            NodeKind::ConstantPattern | NodeKind::ParenthesizedExpression => {
                const_value(c, first_child(c, expression)?)
            }
            NodeKind::UnaryPrefix => {
                // The operator is the span head (collapse map §3.5,
                // derivable-word0 face).
                if c.lexeme(c.head_token(expression)) != "-" {
                    return None;
                }
                match const_value(c, first_child(c, expression)?)? {
                    ConstValue::Int(value) => Some(ConstValue::Int(value.wrapping_neg())),
                    _ => None,
                }
            }
            NodeKind::BinaryExpression => {
                // The operator token is payload word 0 (collapse map §3.5
                // BinaryExpressionEnd); the operands are children ×2.
                let operator = c.payload(expression).first().copied()?;
                if operator == NONE_TOKEN {
                    return None;
                }
                let left = const_value(c, first_child(c, expression)?)?;
                let right = const_value(c, last_child(c, expression)?)?;
                fold_binary(c.lexeme(operator), left, right)
            }
            _ => None,
        }
    }

    /// The folder's binary faces: `+` over integers and string
    /// concatenation, `- * ~/ %` over integers — 64-bit wrapping like the
    /// VM's int, `~/` truncating toward zero, `%` taking the divisor's
    /// sign. Division by zero throws officially, so the value is not a
    /// known constant.
    fn fold_binary(operator: &str, left: ConstValue, right: ConstValue) -> Option<ConstValue> {
        match (operator, left, right) {
            ("+", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_add(b)))
            }
            ("+", ConstValue::Str(a), ConstValue::Str(b)) => {
                Some(ConstValue::Str(format!("{a}{b}").into()))
            }
            ("-", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_sub(b)))
            }
            ("*", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_mul(b)))
            }
            ("~/", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
                Some(ConstValue::Int(a.wrapping_div(b)))
            }
            ("%", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
                let mut rem = a.wrapping_rem(b);
                if rem != 0 && (rem < 0) != (b < 0) {
                    rem = rem.wrapping_add(b);
                }
                Some(ConstValue::Int(rem))
            }
            _ => None,
        }
    }

    /// The Dart integer literal grammar the folder reads — decimal and hex
    /// (`0x`), digit separators dropped, wrapping to the VM's 64-bit int.
    fn int_literal(lexeme: &str) -> Option<i64> {
        let text: String = lexeme.chars().filter(|&ch| ch != '_').collect();
        let (digits, radix) = match text.get(..2) {
            Some("0x" | "0X") => (&text[2..], 16),
            _ => (&text[..], 10),
        };
        u64::from_str_radix(digits, radix)
            .ok()
            .map(|value| value as i64)
    }

    /// The value of one string literal: a single-token span (adjacent
    /// segments span several tokens and never fold), quote forms `'…'` /
    /// `"…"` and the triple variants, `r`-raw bodies verbatim, cooked
    /// bodies escape-decoded. A `$` in a cooked body is interpolation (a
    /// token-layer face) — unknown.
    fn string_literal(c: &Check<'_>, node: u32) -> Option<ConstValue> {
        let first = c.head_token(node);
        if first != crate::rules::tail_token(c, node) {
            return None;
        }
        let text = c.lexeme(first);
        let (raw, text) = match text.strip_prefix('r') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let body = if let Some(rest) = text.strip_prefix("'''") {
            rest.strip_suffix("'''")?
        } else if let Some(rest) = text.strip_prefix("\"\"\"") {
            rest.strip_suffix("\"\"\"")?
        } else {
            let quote = text.chars().next().filter(|&ch| ch == '\'' || ch == '"')?;
            text.strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))?
        };
        if raw {
            return Some(ConstValue::Str(body.into()));
        }
        if body.contains('$') {
            return None;
        }
        let mut value = String::with_capacity(body.len());
        let mut chars = body.chars();
        while let Some(ch) = chars.next() {
            if ch != '\\' {
                value.push(ch);
                continue;
            }
            match chars.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000C}'),
                'v' => value.push('\u{000B}'),
                'x' => {
                    let hex: String = chars.by_ref().take(2).collect();
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                'u' => {
                    let mut hex = String::new();
                    let first = chars.next()?;
                    if first == '{' {
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                break;
                            }
                            hex.push(ch);
                        }
                    } else {
                        // The peeked character is the first of four hex
                        // digits.
                        hex.push(first);
                        hex.extend(chars.by_ref().take(3));
                    }
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                escaped @ ('\\' | '\'' | '"') => value.push(escaped),
                _ => return None,
            }
        }
        Some(ConstValue::Str(value.into()))
    }

    /// The source text of one expression (the official `toString()`
    /// arguments).
    fn source_slice<'a>(c: &'a Check<'_>, node: u32) -> &'a str {
        let bytes = c.byte_span(node);
        &c.unit.source[bytes]
    }
}

pub(crate) mod use_rethrow_when_possible {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{first_child, member_name};

    /// Official: `pkg/linter/lib/src/rules/use_rethrow_when_possible.dart:36-46`
    /// — a `throw` of exactly the caught exception parameter, as a statement,
    /// reports at the throw. The parameter identity
    /// (`element == exceptionParameter`, :39-44) resolves lexically: the
    /// thrown identifier must name the nearest enclosing catch's parameter
    /// and not be shadowed between the throw and that catch clause — a
    /// same-lexeme local variable, pattern variable, loop-variable
    /// declaration, or closure parameter inside the catch body binds the
    /// read instead. The catch clause is the walk's boundary: its parameter
    /// is the candidate itself, never a shadow. The remaining lexical face
    /// (documented): a sibling local-function declaration sharing the name
    /// shadows in the official scope but is not walked here. Message:
    /// `diagnostic.g.dart` `useRethrowWhenPossible`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_rethrow_when_possible",
        group: Group::Recommended,
        problem: "Use 'rethrow' to rethrow a caught exception.",
        correction: "Try replacing the 'throw' with a 'rethrow'.",
        kinds: &[NodeKind::ThrowExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `node.parent is! ExpressionStatement` bails (:37).
        if c.ancestor(1)
            .is_none_or(|parent| c.kind(parent) != NodeKind::ExpressionStatement)
        {
            return;
        }
        // The thrown expression is the throw's operand child (collapse map
        // §3.5 ThrowExpressionHandle).
        let Some(operand) = first_child(c, c.node()) else {
            return;
        };
        if c.kind(operand) != NodeKind::Identifier {
            return;
        }
        // `node.thisOrAncestorOfType<CatchClause>()` (:41).
        let Some(catch_clause) = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&n| c.kind(n) == NodeKind::CatchClause)
        else {
            return;
        };
        // The exception parameter is the catch's first formal (a stack-trace
        // formal follows it; an `on`-only catch has none and never fires —
        // `exceptionParameter` null, :42-43).
        let Some(exception) = c
            .collect_direct(catch_clause)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameter)
        else {
            return;
        };
        // The parameter name is payload word 1 (collapse map §3.3
        // FormalParameterEnd).
        let Some(&name) = c.payload(exception).get(1) else {
            return;
        };
        let thrown = c.lexeme(c.head_token(operand));
        if thrown == c.lexeme(name) && !shadowed_between(c, operand, catch_clause, thrown) {
            let range = c.utf16_span(c.node());
            c.report(range, &[]);
        }
    }

    /// Whether any declaration visible between the throw's operand and the
    /// catch clause binds `name` — the lexical face of the official element
    /// identity: the innermost binding of a read inside the catch body
    /// wins, so a same-name declaration below the catch parameter makes the
    /// thrown identifier a different element.
    fn shadowed_between(c: &Check<'_>, use_node: u32, catch_clause: u32, name: &str) -> bool {
        let use_start = c.byte_span(use_node).start;
        for &ancestor in c.path.iter().rev().skip(1) {
            if ancestor == catch_clause {
                return false;
            }
            match c.kind(ancestor) {
                NodeKind::Block => {
                    if block_declares_before(c, ancestor, use_start, name) {
                        return true;
                    }
                }
                NodeKind::ForIn => {
                    if scope_declares(c, ancestor, name) {
                        return true;
                    }
                }
                NodeKind::ForParts => {
                    // `for (var e = …; …)` declares in the loop parts,
                    // outside the body block.
                    if for_parts_declares(c, ancestor, name) {
                        return true;
                    }
                }
                NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression => {
                    // Parameters scope the whole body regardless of position.
                    if parameter_names(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                NodeKind::LocalFunctionDeclaration => {
                    // The function name binds from its declaration on; the
                    // parameters scope the whole body.
                    let names_decl = member_name(c, ancestor).is_some_and(|decl| {
                        c.lexeme(c.head_token(decl)) == name && c.byte_span(decl).start < use_start
                    });
                    if names_decl || parameter_names(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Local declarations of a block that precede the use: variables
    /// (`VariablesDeclaration`) and pattern bindings.
    fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
        for decl in c.collect_direct(block) {
            match c.kind(decl) {
                NodeKind::VariablesDeclaration => {
                    for var in c.collect_direct(decl) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        if let Some(var_name) = member_name(c, var)
                            && c.lexeme(c.head_token(var_name)) == name
                            && c.byte_span(var_name).start < use_start
                        {
                            return true;
                        }
                    }
                }
                NodeKind::PatternVarDeclStatement => {
                    let window = c.children(decl);
                    for id in window.start..window.end {
                        if c.kind(id) == NodeKind::Identifier
                            && c.lexeme(c.head_token(id)) == name
                            && c.byte_span(id).start < use_start
                        {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Any name binding directly inside a for-in header.
    fn scope_declares(c: &Check<'_>, scope: u32, name: &str) -> bool {
        let window = c.children(scope);
        for id in window.start..window.end {
            if c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == name {
                return true;
            }
        }
        false
    }

    /// The loop-variable declarations of a classic for header.
    fn for_parts_declares(c: &Check<'_>, parts: u32, name: &str) -> bool {
        c.collect_direct(parts).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::VariablesDeclaration
                && c.collect_direct(decl).into_iter().any(|var| {
                    c.kind(var) == NodeKind::InitializedIdentifier
                        && member_name(c, var)
                            .is_some_and(|var_name| c.lexeme(c.head_token(var_name)) == name)
                })
        })
    }

    /// The parameter names of a closure or local function (the
    /// `FormalParameters` child's `FormalParameter` name words).
    fn parameter_names<'a>(c: &'a Check<'a>, function: u32) -> Vec<&'a str> {
        let Some(params) = c
            .collect_direct(function)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return Vec::new();
        };
        c.collect_direct(params)
            .into_iter()
            .filter(|&param| c.kind(param) == NodeKind::FormalParameter)
            .filter_map(|param| {
                let name = c.payload(param).get(1).copied()?;
                (name != super::NONE_TOKEN).then(|| c.lexeme(name))
            })
            .collect()
    }
}

pub(crate) mod avoid_returning_null_for_void {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, first_child, last_child};

    const PROBLEM: &str = "Don't return 'null' from a function with a return type of 'void'.";
    const METHOD_PROBLEM: &str = "Don't return 'null' from a method with a return type of 'void'.";
    const CORRECTION: &str = "Try removing the 'null'.";

    /// Official: `pkg/linter/lib/src/rules/avoid_returning_null_for_void.dart:41-81`
    /// — `return null` (statement or arrow body) inside a sync function or
    /// method whose declared return type is `void` reports at the return.
    /// The two official message variants (from-function / from-method,
    /// `diagnostic.g.dart:727/:739`) split on the enclosing declaration kind.
    ///
    /// Faces not ported (documented): the async `Future<void>` branch
    /// (:77-80) needs the declared type resolved (`type.isDartAsyncFuture`),
    /// and the closure face under-reports when the return type is inferred
    /// from context (`void Function() f = () { return null; };`) — a
    /// function expression has no written return type here. `async void`
    /// functions never report (neither official branch takes them), so the
    /// enclosing body's `async` token is read (collapse map §3.9:
    /// `ExpressionBody` word 2, `BlockBody` word 0; a plain statement `Block`
    /// body carries no async words).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_returning_null_for_void",
        group: Group::Recommended,
        problem: PROBLEM,
        correction: CORRECTION,
        kinds: &[NodeKind::ReturnStatement, NodeKind::ExpressionBody],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.expression case var expression?` (:47): a bare `return;`
        // has no child.
        let Some(expression) = first_child(c, node) else {
            return;
        };
        if c.kind(expression) != NodeKind::LiteralNull {
            return;
        }
        // The nearest enclosing function-like declaration decides the face
        // (:55-57 — `thisOrAncestorMatching(FunctionExpression |
        // MethodDeclaration)`).
        for &ancestor in c.path.iter().rev().skip(1) {
            match c.kind(ancestor) {
                // A closure has no written return type here — out of face.
                NodeKind::FunctionExpression => return,
                NodeKind::Method => {
                    report_void(c, node, ancestor, METHOD_PROBLEM);
                    return;
                }
                NodeKind::TopLevelMethod
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::NamedFunctionExpression => {
                    report_void(c, node, ancestor, PROBLEM);
                    return;
                }
                _ => {}
            }
        }
    }

    /// `isAsync`/`type is VoidType` (:75), then the report at `node`.
    fn report_void(c: &mut Check<'_>, node: u32, function: u32, problem: &str) {
        let Some(body) = last_child(c, function) else {
            return;
        };
        let async_kw = match c.kind(body) {
            NodeKind::ExpressionBody => c.payload(body).get(2).copied(),
            NodeKind::BlockBody => c.payload(body).first().copied(),
            _ => Some(NONE_TOKEN),
        };
        if async_kw != Some(NONE_TOKEN) {
            return;
        }
        // The written return type precedes the name; `void` is a
        // [`NodeKind::VoidType`] child (collapse map §3.3/§3.8). An inferred
        // or non-void written type never takes the official void branch.
        let declares_void = c
            .collect_direct(function)
            .into_iter()
            .any(|kid| c.kind(kid) == NodeKind::VoidType);
        if !declares_void {
            return;
        }
        let range = c.utf16_span(node);
        c.report_texts(range, problem, CORRECTION, &[]);
    }
}

pub(crate) mod type_literal_in_constant_pattern {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/type_literal_in_constant_pattern.dart:40-63`
    /// — a constant pattern whose expression is a type literal reports at
    /// the pattern. The official bails for a `const`-marked pattern (:45-47)
    /// and for a `Type`-typed matched value (:50-59, the raw-type
    /// exemption); `case int:`-style built-in names report.
    ///
    /// Approximations (documented): the type-literal test
    /// (`expression.staticType.isDartCoreType`, :61-63) is a resolution
    /// question. The lexical face reports a bare identifier whose lexeme is
    /// a built-in Dart type name — a set that can never be a type literal
    /// miss (`int`, `String`, ...), while the over-report faces stay inside
    /// the user-shadowed family: a user constant named like a built-in type
    /// (`const String = 'x'; case String:`) over-reports, and user type
    /// literals (`case MyType:` — indistinguishable from a capitalized
    /// constant without resolution) under-report. The `Type`-matched-value
    /// exemption (:50-59) reads through the syntactic face below; a
    /// type-parameter scrutinee (`T case int`) and an object-pattern field
    /// face (`A(type: int)`) stay over-reports. Message: `diagnostic.g.dart`
    /// `typeLiteralInConstantPattern`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "type_literal_in_constant_pattern",
        group: Group::Core,
        problem: "Use 'TypeName _' instead of a type literal.",
        correction: "Replace with 'TypeName _'.",
        kinds: &[NodeKind::ConstantPattern],
        check,
    };

    /// The built-in type names a bare constant-pattern identifier can spell
    /// (`dynamic` included; `void` is a keyword and never reaches an
    /// identifier). The official test corpus exercises the dart:core names.
    fn is_builtin_type_name(lexeme: &str) -> bool {
        matches!(
            lexeme,
            "int"
                | "double"
                | "num"
                | "bool"
                | "String"
                | "Object"
                | "dynamic"
                | "Never"
                | "Null"
                | "Function"
                | "Iterable"
                | "List"
                | "Map"
                | "Set"
                | "Symbol"
                | "Type"
                | "Future"
                | "Stream"
                | "Enum"
                | "Record"
                | "Uri"
                | "Pattern"
                | "RegExp"
                | "DateTime"
                | "Duration"
        )
    }

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.constKeyword != null` bails (:45-47): a const-marked pattern
        // owns the `const` keyword as its span head (collapse map §3.7).
        if c.lexeme(c.head_token(node)) == "const" {
            return;
        }
        let Some(expression) = super::first_child(c, node) else {
            return;
        };
        // The bare constant parses as a tear-off send group — the pattern
        // adopts the childless `Send` leaf (span = the identifier; the
        // `Identifier` / `NoTypeArguments` leaves precede it as owner
        // siblings), so the constant name reads off the send's span head.
        if !matches!(
            c.kind(expression),
            NodeKind::Identifier | NodeKind::Send | NodeKind::DotAccess
        ) {
            return;
        }
        if !is_builtin_type_name(c.lexeme(c.head_token(expression))) {
            return;
        }
        // OK to use raw types if the matched value is of type `Type`
        // (:50-59).
        if matched_value_is_type(c) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// The `Type`-matched-value exemption (:50-59): the enclosing match's
    /// scrutinee proves a `Type` matched value. The syntactic face: a
    /// `.runtimeType` tail is always `Type`, and a bare-identifier scrutinee
    /// resolves its written type through the same-file face — the
    /// `Type`-typed parameter / local / field / top-level shapes
    /// (`type_literal_in_constant_pattern_test.dart`
    /// `test_constType_matchType`).
    fn matched_value_is_type(c: &Check<'_>) -> bool {
        for (depth, &ancestor) in c.path.iter().enumerate().rev() {
            // The scrutinee piece run: if-case (the condition container
            // holds the scrutinee itself), a switch statement case, a
            // switch expression case.
            let scrutinee = match c.kind(ancestor) {
                NodeKind::ParenthesizedCondition => {
                    // The condition's pieces leak past the container to the
                    // enclosing statement: the scrutinee is the container's
                    // parent's direct rows before the container row, ending
                    // before the pattern starts. (The path step after the
                    // container is the container's child on the path — the
                    // pattern itself — so the owner sits one step before
                    // it.) The boundary reads the pattern's own head row
                    // (the container's adopted child when present) — the
                    // container's span can anchor wider than the constant.
                    let Some(&outer) = depth.checked_sub(1).and_then(|i| c.path.get(i)) else {
                        return false;
                    };
                    let pattern_window = c.children(c.node());
                    let pattern_start = if pattern_window.start < pattern_window.end {
                        c.byte_span(pattern_window.start).start
                    } else {
                        c.byte_span(c.node()).start
                    };
                    c.collect_direct(outer)
                        .into_iter()
                        .take_while(|&kid| kid != ancestor)
                        .filter(|&kid| c.byte_span(kid).end <= pattern_start)
                        .collect()
                }
                NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                    let boundary = if c.kind(ancestor) == NodeKind::SwitchCase {
                        NodeKind::SwitchBlock
                    } else {
                        NodeKind::SwitchExpressionBlock
                    };
                    let Some(root_index) = depth.checked_sub(2) else {
                        return false;
                    };
                    let root = c.path[root_index];
                    let kids = c.collect_direct(root);
                    let Some(boundary_pos) = kids.iter().position(|&kid| c.kind(kid) == boundary)
                    else {
                        return false;
                    };
                    kids[..boundary_pos].to_vec()
                }
                _ => continue,
            };
            // A `.runtimeType` tail is always `Type`.
            if let Some(&tail) = scrutinee.last()
                && c.kind(tail) == NodeKind::DotAccess
                && c.lexeme(c.head_token(tail)) == "runtimeType"
            {
                return true;
            }
            // A bare-identifier scrutinee resolves through the same-file
            // written-type face.
            if let Some(&head) = scrutinee.first()
                && c.kind(head) == NodeKind::Identifier
                && crate::rules::expressions::same_file::written_type(
                    c,
                    head,
                    c.lexeme(c.head_token(head)),
                ) == Some("Type")
            {
                return true;
            }
            return false;
        }
        false
    }
}

pub(crate) mod exhaustive_cases {
    //! Official: `pkg/linter/lib/src/rules/exhaustive_cases.dart` +
    //! `asEnumLikeType` (`pkg/linter/lib/src/extensions.dart:604-647`) — a
    //! switch over an enum-like type (a concrete, unsubclassed class or an
    //! extension type, with only private generative constructors and at
    //! least two `static const` fields of the type) must cover every
    //! constant value or carry a `default`. Each case head whose constant
    //! resolves to a value removes it (:49-74); a `default` member ends the
    //! whole check silent (:75-77); every remaining value then reports once
    //! — at the statement head through the scrutinee's `)` — spelled with
    //! the group's preferred (first non-deprecated) name (:80-93). Real
    //! enums never enter (the element walk's default arm — the analyzer's
    //! own exhaustiveness errors cover them).
    //!
    //! Seam posture (documented): the scrutinee's static type and the
    //! constants' computed values are element-model facts
    //! [`Unit`][crate::Unit] does not carry, so the body runs over the
    //! declarative face below — the enum-like gate is ported inside, and
    //! the mounter (the wiring ticket, #434) supplies the facts. The
    //! registry row keeps name/group alignment with `kinds: &[]`.
    //!
    //! The seam body is dead until #434 wires it (`kinds: &[]` keeps the
    //! traversal from dispatching) — the allow below is the seam, not debt.
    #![allow(dead_code)]

    use super::{Group, RuleSpec};
    use crate::rules::not_dispatched;

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "exhaustive_cases",
        group: Group::Recommended,
        problem: "Missing case clauses for some constants in '{0}'.",
        correction: "Try adding case clauses for the missing constants.",
        kinds: &[],
        check: not_dispatched,
    };

    /// One `static const` field of the candidate type (the official field
    /// walk, extensions.dart:627-642).
    pub struct ConstantField<'a> {
        pub name: &'a str,
        /// `field.isConst && field.isStatic`.
        pub is_static_const: bool,
        /// The field's type is the enclosing type (the
        /// `field.type != thisType` skip).
        pub of_this_type: bool,
        /// `metadata.hasDeprecated` — aliases are reported last.
        pub deprecated: bool,
        /// The computed constant-value identity; aliasing fields
        /// (`static const b = a`) share the id, `None` when the value is
        /// not computable (the `fieldValue == null` skip).
        pub value: Option<u32>,
    }

    /// The candidate type face — `asEnumLikeType`'s inputs
    /// (extensions.dart:604-647).
    pub struct EnumLikeCandidate<'a> {
        /// An extension type (`ExtensionTypeElement`) is always
        /// enum-like-eligible; a class must additionally pass the two
        /// class-only gates below.
        pub is_extension_type: bool,
        /// Class only: `element.isAbstract`.
        pub is_abstract: bool,
        /// Class only: a subclass exists in the defining compilation unit
        /// (`_hasSubclassInDefiningCompilationUnit`).
        pub has_subclass_in_defining_unit: bool,
        /// Every constructor as `(is_private, is_generative)` — all must
        /// hold (:620).
        pub constructors: &'a [(bool, bool)],
        /// The type's fields, declaration order (:627); the enumeration
        /// applies the official `isOriginGetterSetter` skip (getter/setter
        /// pseudo-fields are not fields and never enter the face).
        pub constants: &'a [ConstantField<'a>],
    }

    /// One switch member's face (the official member walk,
    /// exhaustive_cases.dart:49-77).
    pub struct SwitchMember {
        /// `default:` — ends the whole check silent (:75-77).
        pub is_default: bool,
        /// The constant value the head covers: a constant-pattern /
        /// legacy-case head whose expression resolves to one of the type's
        /// constant values (the `Identifier` / `PropertyAccess` /
        /// `DotShorthandPropertyAccess` faces all resolve through
        /// `computeConstantValue`). `None` — other patterns, foreign
        /// constants, unresolved heads — covers nothing.
        pub covers: Option<u32>,
    }

    /// The official verdict: the missing values' preferred names, in field
    /// declaration order — the mounter renders each into the template at
    /// the statement span (statement head through the scrutinee's `)`,
    /// :80-93). Empty when the type is not enum-like or a `default` is
    /// present.
    #[must_use]
    pub fn check_switch<'a>(
        candidate: &'a EnumLikeCandidate<'a>,
        members: &[SwitchMember],
    ) -> Vec<&'a str> {
        if !candidate.is_extension_type
            && (candidate.is_abstract || candidate.has_subclass_in_defining_unit)
        {
            return Vec::new();
        }
        if !candidate
            .constructors
            .iter()
            .all(|&(is_private, is_generative)| is_private && is_generative)
        {
            return Vec::new();
        }
        // The constant groups in declaration order, keyed by value id
        // (extensions.dart:626-642). Constant counts are tiny — linear
        // grouping beats a map.
        let mut groups: Vec<(u32, Vec<&'a ConstantField<'a>>)> = Vec::new();
        let mut value_count = 0;
        for field in candidate.constants {
            if !field.is_static_const || !field.of_this_type {
                continue;
            }
            let Some(value) = field.value else {
                continue;
            };
            value_count += 1;
            match groups.iter_mut().find(|(id, _)| *id == value) {
                Some((_, group)) => group.push(field),
                None => groups.push((value, vec![field])),
            }
        }
        // The two-constant minimum counts fields, not values (:640) — a
        // `static const b = a` pair is enum-like with one group.
        if value_count < 2 {
            return Vec::new();
        }
        let mut covered: Vec<u32> = Vec::new();
        for member in members {
            if member.is_default {
                return Vec::new();
            }
            if let Some(value) = member.covers.filter(|value| !covered.contains(value)) {
                covered.push(value);
            }
        }
        groups
            .iter()
            .filter(|(value, _)| !covered.contains(value))
            .map(|(_, group)| preferred_name(group))
            .collect()
    }

    /// The group's reporting name (:84-88): the first non-deprecated
    /// member, else the first.
    fn preferred_name<'a>(group: &[&'a ConstantField<'a>]) -> &'a str {
        group
            .iter()
            .find(|field| !field.deprecated)
            .unwrap_or(&group[0])
            .name
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const PRIVATE_GENERATIVE: (bool, bool) = (true, true);

        fn candidate<'a>(
            constructors: &'a [(bool, bool)],
            constants: &'a [ConstantField<'a>],
        ) -> EnumLikeCandidate<'a> {
            EnumLikeCandidate {
                is_extension_type: false,
                is_abstract: false,
                has_subclass_in_defining_unit: false,
                constructors,
                constants,
            }
        }

        fn const_field(name: &str, value: Option<u32>) -> ConstantField<'_> {
            ConstantField {
                name,
                is_static_const: true,
                of_this_type: true,
                deprecated: false,
                value,
            }
        }

        fn member(covers: Option<u32>) -> SwitchMember {
            SwitchMember {
                is_default: false,
                covers,
            }
        }

        /// The official `test_class_enumLike_allCases` face — every value
        /// covered, silent.
        #[test]
        fn fully_covered_switch_is_silent() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(Some(1)), member(Some(2))];
            assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
        }

        /// The official `test_class_enumLike_missingCase` — the uncovered
        /// value reports with its name.
        #[test]
        fn missing_value_reports_the_name() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(Some(1))];
            assert_eq!(check_switch(&candidate(&ctors, &fields), &members), ["g"]);
        }

        /// The official `test_class_enumLike_withDefault` — a `default`
        /// silences the whole check even with values missing (:75-77).
        #[test]
        fn default_silences() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [
                member(Some(0)),
                SwitchMember {
                    is_default: true,
                    covers: None,
                },
            ];
            assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
        }

        /// A non-constant / foreign / unresolved head covers nothing (the
        /// official `enumConstants.remove` misses) — missing values report
        /// in declaration order.
        #[test]
        fn foreign_head_covers_nothing() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(None)];
            assert_eq!(
                check_switch(&candidate(&ctors, &fields), &members),
                ["f", "g"]
            );
        }

        /// The official `test_class_enumLike_deprecatedAliases` — aliases
        /// share a value id; covering either alias covers the value, and a
        /// missing alias group reports the first non-deprecated name.
        #[test]
        fn alias_group_prefers_the_live_name() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("newFoo", Some(0)),
                ConstantField {
                    deprecated: true,
                    ..const_field("oldFoo", Some(0))
                },
                const_field("bar", Some(1)),
                const_field("baz", Some(2)),
            ];
            // newFoo + bar + baz covers everything (the deprecated alias
            // never reports).
            let all = [member(Some(0)), member(Some(1)), member(Some(2))];
            assert!(check_switch(&candidate(&ctors, &fields), &all).is_empty());
            // bar + baz leaves the alias group; the live name reports.
            let partial = [member(Some(1)), member(Some(2))];
            assert_eq!(
                check_switch(&candidate(&ctors, &fields), &partial),
                ["newFoo"]
            );
        }

        /// The enum-like gate (extensions.dart:604-647): public or
        /// non-generative constructors, an abstract class, a subclass in
        /// the defining unit, or fewer than two value-carrying
        /// `static const` fields of the type all stay silent.
        #[test]
        fn gate_rejections() {
            let fields = [const_field("e", Some(0)), const_field("f", Some(1))];
            let members = [member(Some(0))];
            // A public constructor.
            let public = [(false, true)];
            assert!(check_switch(&candidate(&public, &fields), &members).is_empty());
            // A non-generative constructor (a factory).
            let factory = [(true, false)];
            assert!(check_switch(&candidate(&factory, &fields), &members).is_empty());
            // Abstract, or subclassed in the defining unit.
            let mut abstract_candidate = candidate(&[PRIVATE_GENERATIVE], &fields);
            abstract_candidate.is_abstract = true;
            assert!(check_switch(&abstract_candidate, &members).is_empty());
            let mut subclassed = candidate(&[PRIVATE_GENERATIVE], &fields);
            subclassed.has_subclass_in_defining_unit = true;
            assert!(check_switch(&subclassed, &members).is_empty());
            // One constant only (the official
            // `test_class_notEnumLike_singleConstant`).
            let single = [const_field("e", Some(0))];
            assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &single), &members).is_empty());
            // Non-const / foreign-typed / non-computable fields do not
            // count toward the minimum.
            let uncounted = [
                ConstantField {
                    is_static_const: false,
                    ..const_field("e", Some(0))
                },
                ConstantField {
                    of_this_type: false,
                    ..const_field("f", Some(1))
                },
                const_field("g", None),
            ];
            let two = [member(Some(0)), member(Some(1))];
            assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &uncounted), &two).is_empty());
        }

        /// Extension types skip the class-only gates (the official
        /// `test_extensionType_*` family); duplicate values count fields,
        /// not groups, toward the minimum (:640).
        #[test]
        fn extension_types_and_aliased_pairs() {
            let fields = [const_field("a", Some(0)), const_field("b", Some(0))];
            let mut ext = candidate(&[PRIVATE_GENERATIVE], &fields);
            ext.is_extension_type = true;
            assert_eq!(check_switch(&ext, &[]), ["a"]);
            // As a class, the same shape passes the gates too (two fields).
            assert_eq!(
                check_switch(&candidate(&[PRIVATE_GENERATIVE], &fields), &[]),
                ["a"]
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    /// `test/rules/curly_braces_in_flow_control_structures_test.dart`
    /// `test_if_multiline` — a multi-line arm without a block reports.
    #[test]
    fn multiline_if_without_block_reports() {
        let source = "if (b)\n  x;";
        // if0 (1 b2 )3 x4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::ExpressionStatement, 4);
                b.leaf_span(NodeKind::Send, 4, 5);
                b.end(NodeKind::ExpressionStatement, 6);
                b.end(NodeKind::IfStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in an if should be enclosed in a block."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// The single-line form is allowed (:62-69).
    #[test]
    fn single_line_if_passes() {
        let source = "if (b) x;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::ExpressionStatement, 4);
                b.leaf_span(NodeKind::Send, 4, 5);
                b.end(NodeKind::ExpressionStatement, 6);
                b.end(NodeKind::IfStatement, 6);
            }),
        );
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    #[test]
    fn block_arm_passes() {
        let source = "if (b) { x; }";
        // if0 (1 b2 )3 {4 x5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.leaf_span(NodeKind::Send, 6, 8);
                b.end(NodeKind::ExpressionStatement, 8);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::IfStatement, 8);
            }),
        );
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    /// With an `else` both arms are checked unconditionally (:71-77).
    #[test]
    fn else_arm_reports_even_on_one_line() {
        let source = "if (b) {} else y;";
        // if0 (1 b2 )3 {4 }5 else6 y7 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, 6);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.leaf_span(NodeKind::Block, 4, 6);
                b.begin(NodeKind::ExpressionStatement, 7);
                b.leaf_span(NodeKind::Send, 7, 8);
                b.end(NodeKind::ExpressionStatement, 9);
                b.end(NodeKind::IfStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
    }

    /// `while` bodies always need a block (`test_while`).
    #[test]
    fn while_body_reports() {
        let source = "while (b) ;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::WhileStatement, 0);
                identifier(b, 2, 3);
                b.leaf(NodeKind::EmptyStatement, 4);
                b.end(NodeKind::WhileStatement, 5);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a while should be enclosed in a block."
        );
    }

    /// `do` bodies always need a block (`test_do`).
    #[test]
    fn do_body_reports() {
        let source = "do x; while (b);";
        // do0 x1 ;2 while3 (4 b5 )6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::DoWhileStatement, 0);
                b.begin(NodeKind::ExpressionStatement, 1);
                b.leaf_span(NodeKind::Send, 1, 2);
                b.end(NodeKind::ExpressionStatement, 3);
                identifier(b, 5, 6);
                b.end(NodeKind::DoWhileStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a do should be enclosed in a block."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// `for` bodies always need a block (`test_for`).
    #[test]
    fn for_body_reports() {
        let source = "for (;b;) ;";
        // for0 (1 ;2 b3 ;4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 1);
                b.patch(1, 2);
                b.patch(2, 4);
                b.patch(3, 0);
                identifier(b, 3, 4);
                b.end(NodeKind::ForParts, 6);
                b.leaf(NodeKind::EmptyStatement, 6);
                b.end(NodeKind::ForStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a for should be enclosed in a block."
        );
    }

    // —— control_flow_in_finally ————————————————————————————————————————

    use crate::testsupport::tok as tok_at;

    /// `try {} finally { break; }` — a bare break in a finally reports
    /// (`test/rules/control_flow_in_finally_test.dart` `test_break`).
    #[test]
    fn finally_break_reports() {
        let source = "try {} finally { break; }";
        // try0 {1 }2 finally3 {4 break5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::Break, 5);
                b.patch(0, 0);
                b.end(NodeKind::Break, 7);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TryStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Use of 'break' in a 'finally' clause.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// A loop inside the finally keeps the break local (:79-91 shape).
    #[test]
    fn finally_break_into_local_loop_passes() {
        let source = "try {} finally { for (;;) { break; } }";
        // try0 {1 }2 finally3 {4 for5 (6 ;7 ;8 )9 {10 break11 ;12 }13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ForStatement, 5);
                b.begin(NodeKind::ForParts, 6);
                b.patch(0, 6);
                b.patch(1, 7);
                b.patch(2, 8);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 10);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::Break, 11);
                b.patch(0, 0);
                b.end(NodeKind::Break, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::ForStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::TryStatement, 15);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// A loop outside the try does not keep the break local — it reports.
    #[test]
    fn finally_break_out_of_enclosing_loop_reports() {
        let source = "for (;;) { try {} finally { break; } }";
        // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 break11 ;12 }13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 2);
                b.patch(1, 2);
                b.patch(2, 3);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::TryStatement, 6);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 7, 9);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::Break, 11);
                b.patch(0, 0);
                b.end(NodeKind::Break, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::TryStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::ForStatement, 15);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
    }

    /// A closure inside the finally keeps the break local (:69-77).
    #[test]
    fn finally_break_inside_closure_passes() {
        let source = "try {} finally { f(() { break; }); }";
        // try0 {1 }2 finally3 {4 f5 (6 (7 )8 {9 break10 ;11 }12 )13 ;14 }15
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ExpressionStatement, 5);
                b.begin(NodeKind::Send, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::Block, 13);
                b.end(NodeKind::FunctionExpression, 13);
                b.end(NodeKind::Arguments, 14);
                b.end(NodeKind::Send, 14);
                b.end(NodeKind::ExpressionStatement, 15);
                b.end(NodeKind::Block, 16);
                b.end(NodeKind::TryStatement, 16);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// `return` in a finally reports (`test_return`).
    #[test]
    fn finally_return_reports() {
        let source = "try {} finally { return; }";
        // try0 {1 }2 finally3 {4 return5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.leaf(NodeKind::ReturnStatement, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::TryStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Use of 'return' in a 'finally' clause.");
    }

    /// A try without a finally never reports.
    #[test]
    fn try_without_finally_passes() {
        let source = "try { break; } catch (e) {}";
        // try0 {1 break2 ;3 }4 catch5 (6 e7 )8 {9 }10
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Block, 1);
                b.begin(NodeKind::Break, 2);
                b.patch(0, 0);
                b.end(NodeKind::Break, 4);
                b.end(NodeKind::Block, 5);
                b.begin(NodeKind::CatchClause, 5);
                b.patch(0, tok_at(source, t, "catch", 0));
                b.patch(1, NONE);
                b.end(NodeKind::CatchClause, 11);
                b.end(NodeKind::TryStatement, 11);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// A labeled loop outside the try: the break's resolved target (the
    /// loop the label binds, `visitLabeledStatement` :631-639) wraps the
    /// try, so the jump crosses the finally — reports.
    #[test]
    fn finally_labeled_break_outside_reports() {
        let source = "loop: for (;;) { try {} finally { break loop; } }";
        // loop0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 break13
        // loop14 ;15 }16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
                b.begin(NodeKind::ForStatement, 2);
                b.begin(NodeKind::ForParts, 3);
                b.patch(0, 3);
                b.patch(1, 4);
                b.patch(2, 5);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::TryStatement, 8);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 9, 11);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 1);
                b.end(NodeKind::Break, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::TryStatement, 17);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::ForStatement, 18);
                b.end(NodeKind::LabeledStatement, 18);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 13));
    }

    /// A label binding a loop inside the finally block itself: the target
    /// sits below the try (its own chain contains it) — no report.
    #[test]
    fn finally_labeled_loop_inside_finally_passes() {
        let source = "try {} finally { inner: for (;;) { break inner; } }";
        // try0 {1 }2 finally3 {4 inner5 :6 for7 (8 ;9 ;10 )11 {12 break13
        // inner14 ;15 }16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::Label, 5);
                b.end(NodeKind::Label, 7);
                b.begin_adopting(NodeKind::LabeledStatement, 5, 1);
                b.begin(NodeKind::ForStatement, 7);
                b.begin(NodeKind::ForParts, 8);
                b.patch(0, 8);
                b.patch(1, 9);
                b.patch(2, 10);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 12);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 1);
                b.end(NodeKind::Break, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::ForStatement, 17);
                b.end(NodeKind::LabeledStatement, 17);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::TryStatement, 18);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// Nested labels wrapping the try: both labels bind the same loop
    /// through the label scope, which encloses the try — the official
    /// target lookup reports.
    #[test]
    fn finally_nested_labels_wrapping_try_report() {
        let source = "a: b: for (;;) { try {} finally { break a; } }";
        // a0 :1 b2 :3 for4 (5 ;6 ;7 )8 {9 try10 {11 }12 finally13 {14
        // break15 a16 ;17 }18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin(NodeKind::Label, 2);
                b.end(NodeKind::Label, 4);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 2);
                b.begin(NodeKind::ForStatement, 4);
                b.begin(NodeKind::ForParts, 5);
                b.patch(0, 5);
                b.patch(1, 6);
                b.patch(2, 7);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 9);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::TryStatement, 10);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 11, 13);
                b.begin(NodeKind::Block, 14);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 1);
                b.end(NodeKind::Break, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::TryStatement, 19);
                b.end(NodeKind::Block, 20);
                b.end(NodeKind::ForStatement, 20);
                b.end(NodeKind::LabeledStatement, 20);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
    }

    /// The resolution the label walk closes: a loop inside the finally is
    /// the nearest enclosing loop but not the resolved target — `break
    /// outer` binds the outer loop, whose chain lacks the try — reports
    /// (the nearest-loop heuristic kept this silent).
    #[test]
    fn finally_inner_loop_does_not_mask_labeled_target() {
        let source = "outer: for (;;) { try {} finally { for (;;) { break outer; } } }";
        // outer0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 for13 (14
        // ;15 ;16 )17 {18 break19 outer20 ;21 }22 }23 }24
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
                b.begin(NodeKind::ForStatement, 2);
                b.begin(NodeKind::ForParts, 3);
                b.patch(0, 3);
                b.patch(1, 4);
                b.patch(2, 5);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::TryStatement, 8);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 9, 11);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::ForStatement, 13);
                b.begin(NodeKind::ForParts, 14);
                b.patch(0, 14);
                b.patch(1, 15);
                b.patch(2, 16);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 18);
                b.begin(NodeKind::Block, 18);
                b.begin(NodeKind::Break, 19);
                b.patch(0, 1);
                b.end(NodeKind::Break, 22);
                b.end(NodeKind::Block, 23);
                b.end(NodeKind::ForStatement, 23);
                b.end(NodeKind::Block, 24);
                b.end(NodeKind::TryStatement, 24);
                b.end(NodeKind::Block, 25);
                b.end(NodeKind::ForStatement, 25);
                b.end(NodeKind::LabeledStatement, 25);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 19));
    }

    /// A closure resets the unlabeled-break context chain (:224), so a bare
    /// break inside one has no target even with loops outside — the
    /// FunctionBody exemption (:69-77) keeps it silent (the nearest-loop
    /// heuristic reported this).
    #[test]
    fn finally_closure_break_with_outer_loop_passes() {
        let source = "for (;;) { try {} finally { f(() { break; }); } }";
        // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 f11 (12 (13 )14 {15
        // break16 ;17 }18 )19 ;20 }21 }22
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 1);
                b.patch(1, 2);
                b.patch(2, 3);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::TryStatement, 6);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 7, 9);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::ExpressionStatement, 11);
                b.begin(NodeKind::Send, 11);
                identifier(b, 11, 12);
                b.begin(NodeKind::Arguments, 12);
                b.begin(NodeKind::FunctionExpression, 13);
                b.begin(NodeKind::Block, 15);
                b.begin(NodeKind::Break, 16);
                b.patch(0, 0);
                b.end(NodeKind::Break, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::FunctionExpression, 19);
                b.end(NodeKind::Arguments, 20);
                b.end(NodeKind::Send, 20);
                b.end(NodeKind::ExpressionStatement, 21);
                b.end(NodeKind::Block, 22);
                b.end(NodeKind::TryStatement, 22);
                b.end(NodeKind::Block, 23);
                b.end(NodeKind::ForStatement, 23);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    // —— no_duplicate_case_values —————————————————————————————————————————

    /// `test/rules/no_duplicate_case_values_test.dart` `test_duplicateCase`
    /// — the duplicate reports with both sources in the message.
    #[test]
    fn duplicate_case_value_reports() {
        let source = "switch (x) { case 1: break; case 1: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (case_head, int_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                    b.begin(NodeKind::SwitchCase, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1); // one matched expression
                    b.leaf(NodeKind::LiteralInt, int_tok);
                    b.begin(NodeKind::Break, int_tok + 2);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, int_tok + 4);
                    b.end(NodeKind::SwitchCase, int_tok + 4);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        );
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('1') is equal to the value of an earlier case clause ('1')."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing or changing the value.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 11));
    }

    /// Distinct values and a `default:` clause (no expressions) pass.
    #[test]
    fn distinct_and_default_cases_pass() {
        let source = "switch (x) { case 1: break; case 2: break; default: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 211 :12 break13 ;14
        // default15 :16 break17 ;18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                // case 1
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 6);
                b.begin(NodeKind::Break, 8);
                b.patch(0, 0);
                b.end(NodeKind::Break, 10);
                b.end(NodeKind::SwitchCase, 10);
                // case 2
                b.begin(NodeKind::SwitchCase, 10);
                b.patch(0, NONE);
                b.patch(1, 12);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 11);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 0);
                b.end(NodeKind::Break, 15);
                b.end(NodeKind::SwitchCase, 15);
                // default (no expressions)
                b.begin(NodeKind::SwitchCase, 15);
                b.patch(0, 15);
                b.patch(1, 16);
                b.patch(2, 0);
                b.begin(NodeKind::Break, 17);
                b.patch(0, 0);
                b.end(NodeKind::Break, 19);
                b.end(NodeKind::SwitchCase, 19);
                b.end(NodeKind::SwitchBlock, 20);
                b.end(NodeKind::SwitchStatement, 20);
            }),
        );
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// The official keys by computed constant (`computeConstantValue`, :44):
    /// `case 1 + 1:` collides with `case 2:` — the later expression reports
    /// with both sources.
    #[test]
    fn computed_duplicate_case_values_report() {
        let source = "switch (x) { case 1 + 1: break; case 2: break; }";
        // switch0 (1 x2 )3 {4 case5 16 +7 18 :9 break10 ;11 case12 213 :14
        // break15 ;16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 6);
                b.leaf(NodeKind::LiteralInt, 8);
                b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
                b.patch(0, 7);
                b.end(NodeKind::BinaryExpression, 9);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::SwitchCase, 12);
                b.begin(NodeKind::SwitchCase, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 13);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 0);
                b.end(NodeKind::Break, 17);
                b.end(NodeKind::SwitchCase, 17);
                b.end(NodeKind::SwitchBlock, 18);
                b.end(NodeKind::SwitchStatement, 18);
            }),
        );
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('2') is equal to the value of an earlier case clause ('1 + 1')."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 13));
    }

    /// String concatenation folds: `case 'a' + 'b':` collides with
    /// `case 'ab':`.
    #[test]
    fn string_concatenation_duplicate_cases_report() {
        let source = "switch (x) { case 'a' + 'b': break; case 'ab': break; }";
        // switch0 (1 x2 )3 {4 case5 'a'6 +7 'b'8 :9 break10 ;11 case12
        // 'ab'13 :14 break15 ;16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, 1);
                b.leaf(NodeKind::StringLiteral, 6);
                b.leaf(NodeKind::StringLiteral, 8);
                b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
                b.patch(0, 7);
                b.end(NodeKind::BinaryExpression, 9);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::SwitchCase, 12);
                b.begin(NodeKind::SwitchCase, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.patch(2, 1);
                b.leaf(NodeKind::StringLiteral, 13);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 0);
                b.end(NodeKind::Break, 17);
                b.end(NodeKind::SwitchCase, 17);
                b.end(NodeKind::SwitchBlock, 18);
                b.end(NodeKind::SwitchStatement, 18);
            }),
        );
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        // The official `toString()` arguments carry the source spelling,
        // quotes included (the template's `('…')` wrap doubles them).
        assert_eq!(
            lints[0].message,
            "The value of the case clause (''ab'') is equal to the value of an earlier case clause (''a' + 'b'')."
        );
    }

    /// Expressions outside the folder's grammar evaluate unknown and never
    /// dedupe (the official `hasKnownValue` skip): repeated non-constant
    /// identifiers stay silent.
    #[test]
    fn non_constant_case_expressions_pass() {
        let source = "switch (x) { case y: break; case y: break; }";
        // switch0 (1 x2 )3 {4 case5 y6 :7 break8 ;9 case10 y11 :12 break13
        // ;14 }15
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (case_head, y_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                    b.begin(NodeKind::SwitchCase, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1);
                    identifier(b, y_tok, y_tok + 1);
                    b.begin(NodeKind::Break, colon + 1);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, colon + 3);
                    b.end(NodeKind::SwitchCase, colon + 3);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        );
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    // —— use_rethrow_when_possible ————————————————————————————————————————

    /// `test/rules/use_rethrow_when_possible_test.dart` `test_rethrow` — the
    /// caught parameter rethrown as a statement reports at the throw.
    #[test]
    fn rethrow_of_the_exception_reports() {
        let source = "try {} catch (e) { throw e; }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 throw8 e9 ;10 }11
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::ThrowExpression, 8);
                identifier(b, 9, 10);
                b.end(NodeKind::ThrowExpression, 11);
                b.end(NodeKind::ExpressionStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::CatchClause, 12);
                b.end(NodeKind::TryStatement, 12);
            }),
        );
        let lints = f.run(&one_rule("use_rethrow_when_possible"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'rethrow' to rethrow a caught exception."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 8));
    }

    /// Throwing a different name and an `on`-only catch (no exception
    /// parameter) pass.
    #[test]
    fn rethrow_other_name_and_on_form_pass() {
        let other = "try {} catch (e) { throw x; }";
        let f = Fixture::build(
            other,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::ThrowExpression, 8);
                identifier(b, 9, 10);
                b.end(NodeKind::ThrowExpression, 11);
                b.end(NodeKind::ExpressionStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::CatchClause, 12);
                b.end(NodeKind::TryStatement, 12);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());

        // try0 {1 }2 on3 E4 {5 throw6 e7 ;8 }9 — no exception parameter.
        let on_form = "try {} on E { throw e; }";
        let f = Fixture::build(
            on_form,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::Type, 4, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.begin(NodeKind::ThrowExpression, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::ThrowExpression, 9);
                b.end(NodeKind::ExpressionStatement, 9);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::CatchClause, 10);
                b.end(NodeKind::TryStatement, 10);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    /// A throw inside an argument is not a statement throw (:37).
    #[test]
    fn rethrow_in_argument_position_passes() {
        let source = "try {} catch (e) { g(throw e); }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 g8 (9 throw10 e11 )12 ;13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::Send, 8);
                identifier(b, 8, 9);
                b.begin(NodeKind::Arguments, 9);
                b.begin(NodeKind::ThrowExpression, 10);
                identifier(b, 11, 12);
                b.end(NodeKind::ThrowExpression, 13);
                b.end(NodeKind::Arguments, 13);
                b.end(NodeKind::Send, 13);
                b.end(NodeKind::ExpressionStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::CatchClause, 15);
                b.end(NodeKind::TryStatement, 15);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    /// A local shadowing the catch parameter binds the throw — the lexical
    /// face of the official element identity (`e` the local, not the
    /// parameter): no report.
    #[test]
    fn rethrow_of_shadowing_local_passes() {
        let source = "try {} catch (e) { var e = 1; throw e; }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 var8 e9 =10 111 ;12 throw13 e14 ;15 }16
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::VariablesDeclaration, 8);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 9);
                identifier(b, 9, 10);
                b.begin(NodeKind::VariableInitializer, 10);
                b.leaf(NodeKind::LiteralInt, 11);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::ThrowExpression, 13);
                identifier(b, 14, 15);
                b.end(NodeKind::ThrowExpression, 16);
                b.end(NodeKind::ExpressionStatement, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::CatchClause, 17);
                b.end(NodeKind::TryStatement, 17);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    // —— avoid_returning_null_for_void ————————————————————————————————————

    /// `test/rules/avoid_returning_null_for_void_test.dart` — a top-level
    /// function carries the function text.
    #[test]
    fn returning_null_from_void_function_reports() {
        let source = "void f() { return null; }";
        // void0 f1 (2 )3 {4 return5 null6 ;7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ReturnStatement, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::ReturnStatement, 8);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::TopLevelMethod, 9);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a function with a return type of 'void'."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing the 'null'.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// Inside a class the method variant reports.
    #[test]
    fn returning_null_from_void_method_reports() {
        let source = "class A { void m() { return null; } }";
        // class0 A1 {2 void3 m4 (5 )6 {7 return8 null9 ;10 }11 }12
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 3);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::VoidType, 3);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ReturnStatement, 8);
                b.leaf(NodeKind::LiteralNull, 9);
                b.end(NodeKind::ReturnStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 13);
                b.end(NodeKind::ClassDeclaration, 13);
                b.end(NodeKind::TopLevelDeclaration, 13);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a method with a return type of 'void'."
        );
    }

    /// A non-void declared type, a closure face, and `async void` pass.
    #[test]
    fn returning_null_non_void_and_async_pass() {
        let typed = "int f() { return null; }";
        // int0 f1 (2 )3 {4 return5 null6 ;7 }8
        let f = Fixture::build(
            typed,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ReturnStatement, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::ReturnStatement, 8);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::TopLevelMethod, 9);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

        // The closure is the nearest function — no written return type.
        let closure = "void f() { g(() { return null; }); }";
        // void0 f1 (2 )3 {4 g5 (6 (7 )8 {9 return10 null11 ;12 }13 )14 ;15 }16
        let f = Fixture::build(
            closure,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ExpressionStatement, 5);
                b.begin(NodeKind::Send, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::ReturnStatement, 10);
                b.leaf(NodeKind::LiteralNull, 11);
                b.end(NodeKind::ReturnStatement, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::FunctionExpression, 14);
                b.end(NodeKind::Arguments, 15);
                b.end(NodeKind::Send, 15);
                b.end(NodeKind::ExpressionStatement, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::TopLevelMethod, 17);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

        // `void f() async { return null; }` — the async body skips both
        // official branches (:75-80). The brace body is a `BlockBody`
        // carrying the `async` token (collapse map §3.9).
        let async_fn = "void f() async { return null; }";
        // void0 f1 (2 )3 async4 {5 return6 null7 ;8 }9
        let f = Fixture::build(
            async_fn,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::BlockBody, 4);
                b.patch(0, tok_at(async_fn, t, "async", 0));
                b.patch(1, NONE);
                b.begin(NodeKind::ReturnStatement, 6);
                b.leaf(NodeKind::LiteralNull, 7);
                b.end(NodeKind::ReturnStatement, 9);
                b.end(NodeKind::BlockBody, 10);
                b.end(NodeKind::TopLevelMethod, 10);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());
    }

    /// The arrow-body face: `void f() => null;` reports.
    #[test]
    fn returning_null_from_arrow_body_reports() {
        let source = "void f() => null;";
        // void0 f1 (2 )3 =>4 null5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::ExpressionBody, 4);
                b.patch(0, 4);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralNull, 5);
                b.end(NodeKind::ExpressionBody, 7);
                b.end(NodeKind::TopLevelMethod, 7);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    // —— type_literal_in_constant_pattern ———————————————————————————————————

    /// `case int:` — a built-in type name in constant-pattern position
    /// reports over the pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
    /// `test_BuiltInTypePattern`).
    #[test]
    fn type_literal_reports_a_builtin_type_pattern() {
        let source = "switch (x) { case int: break; }";
        // switch0 (1 x2 )3 {4 case5 int6 :7 break8 ;9 }10
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ConstantPattern, 7);
                b.end(NodeKind::SwitchCase, 8);
                b.end(NodeKind::SwitchBlock, 10);
                b.end(NodeKind::SwitchStatement, 10);
            }),
        );
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'TypeName _' instead of a type literal."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
        assert_eq!(lints[0].range.end, utf16_at(&f, 7));
    }

    /// A const-marked pattern bails (:45-47), a non-builtin constant
    /// (`case color:`) never matches the name set, and a property access
    /// (`case Foo.bar:`) is not a bare type literal.
    #[test]
    fn type_literal_const_marked_and_non_type_patterns_pass() {
        let const_marked = "switch (x) { case const int: break; }";
        // switch0 (1 x2 )3 {4 case5 const6 int7 :8 break9 ;10 }11
        let f = Fixture::build(
            const_marked,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::ConstantPattern, 8);
                b.end(NodeKind::SwitchCase, 9);
                b.end(NodeKind::SwitchBlock, 11);
                b.end(NodeKind::SwitchStatement, 11);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        let user_constant = "switch (x) { case color: break; }";
        let f = Fixture::build(
            user_constant,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ConstantPattern, 7);
                b.end(NodeKind::SwitchCase, 8);
                b.end(NodeKind::SwitchBlock, 10);
                b.end(NodeKind::SwitchStatement, 10);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        let property = "switch (x) { case Foo.bar: break; }";
        // switch0 (1 x2 )3 {4 case5 Foo6 .7 bar8 :9 break10 ;11 }12
        let f = Fixture::build(
            property,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                b.begin(NodeKind::DotAccess, 6);
                b.patch(0, 7);
                b.patch(1, NONE);
                identifier(b, 6, 7);
                identifier(b, 8, 9);
                b.end(NodeKind::DotAccess, 9);
                b.end(NodeKind::ConstantPattern, 9);
                b.end(NodeKind::SwitchCase, 10);
                b.end(NodeKind::SwitchBlock, 12);
                b.end(NodeKind::SwitchStatement, 12);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    /// Production-bridge face: the if-case `case int` reports over the
    /// pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
    /// core-type case).
    #[test]
    fn type_literal_if_case_reports_the_pattern() {
        let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'TypeName _' instead of a type literal."
        );
        let int = tok(&f.source, &f.tokens, "int", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, int));
        assert_eq!(lints[0].range.end, utf16_at(&f, int) + 3);
    }

    /// The `Type`-matched-value exemption (:50-59) through the syntactic
    /// face on real parses: a `Type`-typed parameter, a `Type`-typed local,
    /// a `.runtimeType` scrutinee, and a const-marked pattern all stay
    /// silent (`test_constType_matchType`).
    #[test]
    fn type_literal_type_matched_value_passes() {
        let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the matched value is the Type-typed parameter"
        );

        let f = Fixture::parse("void f() { Type t = int; if (t case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the matched value is the Type-typed local"
        );

        let f = Fixture::parse("void f(Object x) { if (x.runtimeType case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "a .runtimeType scrutinee is always Type"
        );

        let f = Fixture::parse("void f(Object x) { if (x case const (int)) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "a const-marked pattern bails (:45-47)"
        );
    }
}
