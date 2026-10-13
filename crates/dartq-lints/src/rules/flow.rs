//! The flow-control family: `curly_braces_in_flow_control_structures`,
//! `control_flow_in_finally`, `no_duplicate_case_values`,
//! `use_rethrow_when_possible`, `avoid_returning_null_for_void`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, last_child};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

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
        if let Some(else_stmt) = arms.1
            && c.kind(else_stmt) != NodeKind::IfStatement
        {
            check_body(c, else_stmt, "an if");
        }
        return;
    }
    // An `if` without `else` that IS the else branch of an outer if: the
    // outer chain already requires blocks, so the then-arm is checked
    // unconditionally (:55-61).
    if let Some(parent) = c.ancestor(1)
        && c.kind(parent) == NodeKind::IfStatement
        && c.payload(parent).first().copied() != Some(NONE_TOKEN)
        && arms.2 == Some(node)
    {
        check_body(c, then, "an if");
        return;
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
    /// Approximation (documented): the switch's member-label scope walk
    /// reads the members over the production tiling (the same rows the
    /// `exhaustive_cases` member walk collects); a label declared inside a
    /// case BODY (a labeled statement within a member) scopes normally.
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
        // The switch-member tiling overlaps windows (each member's window
        // starts at the previous member's arena row), which spills the
        // member's body rows both at the switch-block level and under the
        // member row the next member's window adopts — the try's subtree
        // can dispatch twice. Keep only the canonical (`ast.parent`)
        // descent; the spilled one bails.
        if c.unit.ast.parent(try_node) != c.path.get(try_idx - 1).copied() {
            return;
        }
        let report = match c.kind(node) {
            NodeKind::Break | NodeKind::Continue => match jump_target(c, node, node_idx) {
                // `enablerNode = target.thisOrAncestorMatching(n ==
                // tryStatement)` (:79-91): the target's own ancestor chain
                // containing the try keeps the jump local —
                // `thisOrAncestorMatching` includes the target itself, so a
                // label bound onto the try (`a: try …`) also counts as
                // within. Outside it the jump crosses the finally and
                // reports.
                Some(target) => !chain_contains(c, target, try_node),
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

    /// The resolved jump target's row, `None` for the official
    /// `target == null` face (`_lookupBreakOrContinueTarget`,
    /// `scope_analyzer.dart:1042-1093`).
    fn jump_target(c: &Check<'_>, node: u32, node_idx: usize) -> Option<u32> {
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
            // The label scope chain, innermost first: a labeled statement's
            // labels bind the statement after the labels
            // (`visitLabeledStatement`, scope_analyzer.dart:631-639); a
            // switch statement pre-nests EVERY member's labels over the
            // whole statement (scope_analyzer.dart:846-850), so a forward
            // `continue L` to a later member resolves there — the members
            // walk in reverse source order, a later duplicate shadowing.
            for (idx, &ancestor) in c.path[..node_idx].iter().enumerate().rev() {
                match c.kind(ancestor) {
                    NodeKind::LabeledStatement if declares_label(c, ancestor, label) => {
                        return Some(c.path[idx + 1]);
                    }
                    NodeKind::SwitchStatement => {
                        if let Some(member) = switch_member_with_label(c, ancestor, label) {
                            return Some(member);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        let is_continue = c.kind(node) == NodeKind::Continue;
        for (idx, &ancestor) in c.path[..node_idx].iter().enumerate().rev() {
            match c.kind(ancestor) {
                NodeKind::WhileStatement
                | NodeKind::ForStatement
                | NodeKind::ForIn
                | NodeKind::DoWhileStatement => return Some(c.path[idx]),
                // An untargeted `continue` never binds a switch
                // (scope.dart:336-341).
                NodeKind::SwitchStatement if !is_continue => return Some(c.path[idx]),
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

    /// The switch member whose label prefix declares `label`, later members
    /// first (the official scope chain nests member labels in source order,
    /// so the last declared wins); `None` when no member carries it. The
    /// members read over the production tiling — the plain child walk loses
    /// members (the `exhaustive_cases` member walk's documented wrinkle).
    fn switch_member_with_label(c: &Check<'_>, switch_row: u32, label: &str) -> Option<u32> {
        let block = c
            .collect_direct(switch_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)?;
        super::exhaustive_cases::switch_case_rows(c, block)
            .into_iter()
            .rev()
            .find(|&member| declares_label(c, member, label))
    }

    /// Whether `try_row` sits on `target`'s ancestor chain — the official
    /// `target.thisOrAncestorMatching((n) => n == tryStatement)` (the
    /// target itself counts). The walk ascends strictly to the root.
    fn chain_contains(c: &Check<'_>, target: u32, try_row: u32) -> bool {
        let mut current = target;
        loop {
            if current == try_row {
                return true;
            }
            match c.unit.ast.parent(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Whether a labeled statement / switch member declares `label` among
    /// its label-prefix children. The production tiling emits each label as
    /// an `Identifier` (the name) + `Label` (the colon) leaf pair — the
    /// collapse map's `2·label_count` adoption — while hand-built trees
    /// carry a bare `Label` row spanning `name:`; the prefix is the
    /// leading run of both shapes, and the name reads off whichever row
    /// anchors at the identifier token.
    fn declares_label(c: &Check<'_>, node: u32, label: &str) -> bool {
        c.collect_direct(node)
            .into_iter()
            .take_while(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::Label))
            .any(|kid| c.lexeme(c.head_token(kid)) == label)
    }

    /// Whether a closure (function expression / local function) sits on the
    /// path between the finally block and the statement — the official
    /// `FunctionBody inside the finally block` exemption (:69-77); the
    /// function-kind family is the `jump_target` boundary list below.
    fn closure_wraps(c: &Check<'_>, finally_idx: usize, node_idx: usize) -> bool {
        (finally_idx + 1..=node_idx).any(|i| {
            matches!(
                c.kind(c.path[i]),
                NodeKind::FunctionExpression
                    | NodeKind::NamedFunctionExpression
                    | NodeKind::LocalFunctionDeclaration
            )
        })
    }
}

pub(crate) mod no_duplicate_case_values {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, first_child, last_child};

    /// Official: `pkg/linter/lib/src/rules/no_duplicate_case_values.dart:37-58`
    /// — a `case` whose constant value equals an earlier `case` of the same
    /// switch reports at the duplicate expression with both sources spelled
    /// into `{0}`/`{1}`. The official visitor processes only legacy members
    /// (`if (member is SwitchCase)`, :41); the `AstBuilder` materializes that
    /// member shape only when `Feature.patterns` is off (`ast_builder.dart:1295`
    /// — patterns shipped in 3.0), so at the default language version every
    /// case is a `SwitchPatternCase` and the lint is silent — the official
    /// test pins exactly that (all default-version duplicates expect no lint;
    /// the reports live in `NoDuplicateCaseValuesTestLanguage219`). The gate
    /// here is the same version face at unit granularity (the members of one
    /// switch are uniformly one shape): the check runs only with `patterns`
    /// off. The official keys the map by the computed constant
    /// (`computeConstantValue`, :44); the face here is an in-rule constant
    /// folder over the syntax substrate — integer/string/bool/null
    /// literals, `+ - * ~/ %` and unary `-`, parentheses (64-bit wrapping
    /// integers, Dart `~/` truncation and `%` divisor-sign semantics),
    /// descending through `ConstantPattern`/`ParenthesizedExpression` and
    /// the `CaseExpression` operand wrapper the fixed parse surface puts
    /// around every legacy case operand (the bridge's parse posture is
    /// patterns-on at every version, so the operand never sits bare under
    /// the `SwitchCase`).
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
        // The official `member is SwitchCase` gate (:41): with `Feature.
        // patterns` on (the 3.0+ default) every member is a pattern case and
        // the visitor finds nothing to process.
        if c.unit.features.patterns {
            return;
        }
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
            NodeKind::ConstantPattern
            | NodeKind::ParenthesizedExpression
            | NodeKind::CaseExpression => const_value(c, first_child(c, expression)?),
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
            .map(u64::cast_signed)
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
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/type_literal_in_constant_pattern.dart:40-63`
    /// — a constant pattern whose expression's static type is the `Type`
    /// type reports at the pattern. The official bails for a
    /// `const`-marked pattern (:45-47) and for a `Type`-typed matched
    /// value (:50-59, the raw-type exemption).
    ///
    /// The type-literal test (`expression.staticType.isDartCoreType`,
    /// :57-63) resolves over the same-file channel: a bare name answers
    /// through the innermost value binding first (a value is never a type
    /// literal — the `const String = 'x'` shadow over-report is dead), a
    /// top-level `const` folds its initializer through the alias channel
    /// (`const t = int; case t:` — a Type-valued constant — reports like
    /// the official static type), a typed binding reports when written
    /// `Type`, and a name with no value binding is a type literal: a
    /// same-file type declaration (`case MyType:` — the user-type
    /// under-report is dead) or an unshadowed built-in type name. A
    /// two-segment name folds a same-file class-static alias (`A.t`);
    /// anything else there (an enum member, a prefixed spelling — the
    /// `Type` row's head is the prefix) stays silent, the prefixed type
    /// literal being the documented under-report. The type-literal test is
    /// the official `isDartCoreType` element read — nullability-blind, so a
    /// `Type`-written binding reports through `Type?` too. The raw-type
    /// exemption reads the matched value through the scrutinee face and —
    /// for a constant nested in an object pattern (`A(field: int)`) — the
    /// field's same-file written type, both the nullability-sensitive
    /// `isSubtypeOf` face (`Type?` exempts nothing). Known corners: a
    /// class-scope `static const` shadowing a type name is invisible to
    /// the binding walk (statics are not instance bindings) and keeps the
    /// type reading; the object-pattern field face reads instance fields
    /// only (a getter, a prefixed container, a name-less field, and
    /// list/map element positions keep the name reading). Message:
    /// `diagnostic.g.dart` `typeLiteralInConstantPattern`.
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
    /// identifier) — the unshadowed fallback the same-file channel cannot
    /// resolve better (dart:core is auto-imported, so an unshadowed
    /// spelling of these IS the library type). The official test corpus
    /// exercises the dart:core names.
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
        if !constant_is_type_literal(c, expression) {
            return;
        }
        // OK to use raw types if the matched value is of type `Type`
        // (:50-59) — the scrutinee face or the object-pattern field face.
        if matched_value_is_type(c) || pattern_field_is_type(c) {
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
            // written-type face — exactly `Type`, not the nullable `Type?`
            // (the official `isSubtypeOf(matchedValueType, typeType)`).
            if let Some(&head) = scrutinee.first()
                && c.kind(head) == NodeKind::Identifier
            {
                let face = crate::rules::written_type_face(c, head, c.lexeme(c.head_token(head)));
                if face.is_some_and(|face| face.name == Some("Type") && !face.nullable) {
                    return true;
                }
            }
            return false;
        }
        false
    }

    /// The static-type question over the expression's name segments (the
    /// module doc carries the decision table).
    fn constant_is_type_literal(c: &Check<'_>, expression: u32) -> bool {
        let range = c.unit.ast.token_range(expression);
        let Some((leading_dot, names)) =
            super::exhaustive_cases::segments(c, super::exhaustive_cases::strip_const(c, range))
        else {
            return false;
        };
        if leading_dot {
            return false;
        }
        match names.as_slice() {
            [name] => bare_name_is_type_literal(c, expression, name),
            [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
                .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0)),
            _ => false,
        }
    }

    /// The bare-name face: the innermost value binding first, then the
    /// type reading.
    fn bare_name_is_type_literal(c: &Check<'_>, use_row: u32, name: &str) -> bool {
        use crate::rules::expressions::same_file::BindingScope;
        match crate::rules::expressions::same_file::binding(c, use_row, name) {
            Some((scope, typed)) => match scope {
                // A `const` binding folds its initializer through the alias
                // channel (a Type-valued constant); any other value ends
                // the reading off the type family.
                BindingScope::TopLevel => super::exhaustive_cases::top_level_const_init(c, name)
                    .map_or(typed_face_is_type(c, typed), |init| {
                        folds_to_type(c, init, &mut Vec::new(), 0)
                    }),
                BindingScope::Local => {
                    crate::rules::expressions::same_file::local_const_initializer(c, use_row, name)
                        .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0))
                }
                BindingScope::Parameter | BindingScope::Field => typed_face_is_type(c, typed),
            },
            // No same-file value binding: a type literal — a same-file type
            // declaration or an unshadowed built-in type name.
            None => {
                crate::rules::expressions::same_file::declares_type(c, name)
                    || is_builtin_type_name(name)
            }
        }
    }

    /// The object-pattern field face of the matched-value exemption: a
    /// constant pattern nested in an object pattern (`A(field: int)`)
    /// matches the field's own value, whose static type is the field's
    /// written type — a `Type`-written field exempts the nested constant
    /// (`type_literal_in_constant_pattern_test.dart`
    /// `test_constType_matchType_nested`). The bridge leaks the
    /// sub-pattern rows past the field windows (the constant's tree parent
    /// is the statement container; the `PatternField` / `ObjectPattern`
    /// rows are span-only overlays completing right after the leak), so
    /// the wrapping pair resolves by span containment over the container's
    /// arena window — the innermost containing rows. The field reads
    /// through the same-file written-type face of the object pattern's
    /// class; a prefixed spelling (`prefix.A(...)`, payload word 1), a
    /// name-less field (`A(: sub)`, payload word 0 is the colon), and an
    /// unresolvable class keep the name reading.
    fn pattern_field_is_type(c: &Check<'_>) -> bool {
        let node = c.node();
        let span = c.byte_span(node);
        let contains = |row: u32| {
            let outer = c.byte_span(row);
            outer.start <= span.start && span.end <= outer.end
        };
        let Some(container) = c.path.len().checked_sub(2).map(|i| c.path[i]) else {
            return false;
        };
        let window = c.children(container).start..container;
        // Post-order rows: the innermost containing overlay completes
        // first.
        let Some(object) = window
            .clone()
            .filter(|&row| c.kind(row) == NodeKind::ObjectPattern && contains(row))
            .min()
        else {
            return false;
        };
        let Some(field) = (c.children(container).start..object)
            .filter(|&row| c.kind(row) == NodeKind::PatternField && contains(row))
            .min()
        else {
            return false;
        };
        // A prefixed container (`prefix.A(...)`) resolves outside the file.
        let words = c.payload(object);
        let Some(&class_tok) = words.first() else {
            return false;
        };
        if class_tok == NONE_TOKEN || words.get(1).copied() != Some(NONE_TOKEN) {
            return false;
        }
        // The name-less field face carries the colon token, not a name.
        let Some(&name_tok) = c.payload(field).first() else {
            return false;
        };
        if name_tok == NONE_TOKEN || c.lexeme(name_tok) == ":" {
            return false;
        }
        let Some(class_row) = same_file_class_named(c, c.lexeme(class_tok)) else {
            return false;
        };
        instance_field_is_type(c, class_row, c.lexeme(name_tok))
    }

    /// The same-file top-level class declaration named `name` — the local
    /// class-only face of the `exhaustive_cases::class_like_named` walk
    /// (module-private there): the object-pattern head resolves against a
    /// class's instance members.
    fn same_file_class_named(c: &Check<'_>, name: &str) -> Option<u32> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if c.kind(kid) == NodeKind::ClassDeclaration
                    && crate::rules::class_declaration_name(c, kid) == Some(name)
                {
                    return Some(kid);
                }
            }
        }
        None
    }

    /// Whether the instance field `field` of the same-file `class_row` is
    /// written exactly `Type` (the non-static `Fields` walk of
    /// `type_face_of_field`'s index builder, class-row parameterized — the
    /// member's single written type covers all its declarators). Untyped
    /// fields, every other written type, and the nullable `Type?` (not a
    /// subtype of `Type`) keep the name reading; a getter or an
    /// inferred-from-initializer type is invisible (the documented channel
    /// face).
    fn instance_field_is_type(c: &Check<'_>, class_row: u32, field: &str) -> bool {
        let body = c
            .collect_direct(class_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
            .unwrap_or(class_row);
        for member in crate::rules::rows_with_parent(c, body) {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
            {
                continue;
            }
            let written = c
                .collect_direct(member)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type);
            let Some(written) = written else {
                continue;
            };
            let face = crate::rules::type_face(c, written);
            if face.name != Some("Type") || face.nullable {
                continue;
            }
            for decl in c
                .collect_direct(member)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
            {
                for var in c
                    .collect_direct(decl)
                    .into_iter()
                    .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                {
                    if crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|name| c.lexeme(name) == field)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// A typed value binding is a type literal whenever written `Type` —
    /// nullability-blind, the official `isDartCoreType` reading the type's
    /// element (`Type?` still names dart:core `Type`, :792-794) — while
    /// the matched-value exemption is the nullability-sensitive
    /// `isSubtypeOf` face.
    fn typed_face_is_type(c: &Check<'_>, typed: Option<u32>) -> bool {
        typed.is_some_and(|row| {
            c.kind(row) == dartq_ast::NodeKind::Type
                && crate::rules::type_face(c, row).name == Some("Type")
        })
    }

    /// The alias fold over a top-level/class-static const initializer
    /// chain: the chain landing on a type name (a same-file type or an
    /// unshadowed built-in) makes the constant Type-valued. `seen` breaks
    /// alias cycles, `depth` bounds them.
    fn folds_to_type(
        c: &Check<'_>,
        init: core::ops::Range<u32>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> bool {
        if depth > super::exhaustive_cases::MAX_ALIAS_DEPTH || seen.contains(&init.start) {
            return false;
        }
        let init = super::exhaustive_cases::strip_const(c, init);
        let Some((false, names)) = super::exhaustive_cases::segments(c, init.clone()) else {
            return false;
        };
        match names.as_slice() {
            // Within a top-level initializer only top-level names resolve:
            // a const alias recurses, a non-const value ends the chain off
            // the type family.
            [name] => match super::exhaustive_cases::top_level_const_init(c, name) {
                Some(next) => {
                    seen.push(init.start);
                    folds_to_type(c, next, seen, depth + 1)
                }
                None => {
                    crate::rules::expressions::same_file::declares_type(c, name)
                        || is_builtin_type_name(name)
                }
            },
            [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
                .is_some_and(|next| folds_to_type(c, next, seen, depth + 1)),
            _ => false,
        }
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
    //! Mounted face: the element facts ride the same-file declarative
    //! channel — the scrutinee's written type resolves through
    //! [`same_file::written_type`](crate::rules::expressions::same_file::written_type)
    //! (the `type_literal_in_constant_pattern` idiom), the
    //! candidate's constructor/field facts walk the same-file declaration
    //! rows, and constant-value identity interns canonical initializer
    //! texts — `computeConstantValue` same-file edition: two constants
    //! share a value when their initializer chains land on equal terminal
    //! texts, so `static const b = a` aliases group with `a`
    //! (extensions.dart:626-642). A case head covers through the official
    //! variable faces only (`Identifier` / `PropertyAccess` /
    //! `DotShorthandPropertyAccess`, :59-74) — any other constant pattern
    //! covers nothing.
    //!
    //! Known silences (the same-file under-report posture, each the
    //! documented fallback of the channel it rides): a scrutinee whose type
    //! leaves the file (an import, a `var`/for-in binding, a call result —
    //! `written_type`'s contract), a candidate or a subclass declared in
    //! another unit of the library (`asEnumLikeType` sees the whole
    //! defining library, extensions.dart:259-270), and constant values
    //! spelled differently over the same instance (`E._(0x1)` vs `E._(1)`).

    use dartq_ast::{NodeKind, mod_flags};
    use dartq_scanner::{KeywordStyle, TokenKind};

    use super::{Group, NONE_TOKEN, RuleSpec, first_child};
    use crate::engine::Check;
    use crate::rules::{
        SupertypeClause, class_declaration_name, initialized_identifier_name, member_annotations,
        rows_with_parent, supertypes, utf16_token_span, variables_declaration_keyword,
    };

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "exhaustive_cases",
        group: Group::Recommended,
        problem: "Missing case clauses for some constants in '{0}'.",
        correction: "Try adding case clauses for the missing constants.",
        kinds: &[NodeKind::SwitchStatement],
        check,
    };

    /// The class-family `abstract` modifier bit (bridge `mod_bits`; the
    /// parser lane owns the values).
    const ABSTRACT_BIT: u16 = 1 << 2;

    /// The alias-hop cap: a legitimate constant alias chain is a few names
    /// deep; past this it is pathological and computes to nothing.
    pub(super) const MAX_ALIAS_DEPTH: usize = 8;

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(block_pos) = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
        else {
            return;
        };
        let block = kids[block_pos];
        // The scrutinee's written type → the same-file enum-like candidate
        // (`statement.expression.staticType` → `InterfaceType.element`,
        // :39-46; a type outside the file stays silent — the channel's
        // documented fallback).
        let Some(scrutinee) = scrutinee_identifier(c, &kids[..block_pos]) else {
            return;
        };
        let name = c.lexeme(c.head_token(scrutinee));
        let Some(written) = crate::rules::expressions::same_file::written_type(c, scrutinee, name)
        else {
            return;
        };
        let Some((candidate_row, candidate_kind)) = class_like_named(
            c,
            written,
            &[
                NodeKind::ClassDeclaration,
                NodeKind::ExtensionTypeDeclaration,
            ],
        ) else {
            return;
        };
        let is_extension_type = candidate_kind == NodeKind::ExtensionTypeDeclaration;
        let constructors = constructor_face(c, candidate_row);
        let field_faces = class_const_fields(c, candidate_row);
        let mut values = ConstValues::default();
        let constants = candidate_constants(c, &mut values, &field_faces, written);
        let candidate = EnumLikeCandidate {
            is_extension_type,
            is_abstract: !is_extension_type
                && mod_flags(c.unit.ast, candidate_row) & ABSTRACT_BIT != 0,
            has_subclass_in_defining_unit: subclassed_in_file(c, candidate_row),
            constants: &constants,
            constructors: &constructors,
        };
        let cases = switch_case_rows(c, block);
        let members: Vec<SwitchMember> = cases
            .into_iter()
            .map(|case| member_face(c, &mut values, &field_faces, case))
            .collect();
        // Owned names: the report mutates the check the verdict borrowed.
        let missing: Vec<String> = check_switch(&candidate, &members)
            .into_iter()
            .map(String::from)
            .collect();
        if missing.is_empty() {
            return;
        }
        // The report span: the `switch` head through the scrutinee's `)`
        // (official `reportAtOffset(statement.offset,
        // rightParenthesis.end - statement.offset)`, :81-90).
        let start = c.utf16_span(node).start;
        let Some(rparen) = c
            .head_token(block)
            .checked_sub(1)
            .filter(|&token| token > c.head_token(node))
        else {
            return;
        };
        let range = start..utf16_token_span(c, rparen).end;
        for name in &missing {
            c.report(range.clone(), &[name]);
        }
    }

    /// The block's `SwitchCase` rows, source order. The plain tiling walk
    /// loses members: each case's completion window starts AT the previous
    /// case's arena row (`end_switch_case` snapshots the material start
    /// before the previous case seals), so the reverse tiling jump lands on
    /// that row and the decrement-first walk skips it as window-start
    /// interior — a two-case switch surfaces only the last case. The
    /// jump-landed row is this block's member, not the case's child: the
    /// walk pushes it and continues below it. Shared with the
    /// `control_flow_in_finally` member-label walk.
    pub(super) fn switch_case_rows(c: &Check<'_>, block: u32) -> Vec<u32> {
        let window = c.children(block);
        let mut cases: Vec<u32> = Vec::new();
        let mut i = window.end;
        while i > window.start {
            i -= 1;
            let start = c.children(i).start;
            if start < i && c.kind(start) == NodeKind::SwitchCase {
                cases.push(start);
            }
            if c.kind(i) == NodeKind::SwitchCase {
                cases.push(i);
            }
            i = start;
        }
        // The start-checks push in jump order, not id order.
        cases.sort_unstable();
        cases
    }

    /// The bare-identifier scrutinee row: the leading expression piece
    /// (`matched_value_is_type`'s proven `scrutinee.first()` face,
    /// flow.rs:1011). The bridge leaks the send-group leaves past the
    /// condition wrapper as owner siblings — the `Identifier`, the
    /// `NoTypeArguments` leaf anchored at the follower `)`, and the `Send`
    /// leaf that spans through it — so the piece run says nothing about the
    /// expression shape beyond its head. Only a selector continuation
    /// (a `DotAccess` piece: `a.b`, `e.multi()`) disqualifies the bare
    /// name; any other scrutinee has no same-file written type and stays
    /// silent through `written_type`'s `None`.
    fn scrutinee_identifier(c: &Check<'_>, pieces: &[u32]) -> Option<u32> {
        let &first = pieces.first()?;
        if c.kind(first) != NodeKind::Identifier {
            return None;
        }
        let selector = pieces[1..]
            .iter()
            .any(|&piece| c.kind(piece) == NodeKind::DotAccess);
        (!selector).then_some(first)
    }

    /// The same-file class-like declaration named `name`: `(row, kind)` —
    /// the top-level wrapper walk of `same_file_declaration` under the
    /// caller's kind filter. The candidate face accepts only the
    /// enum-like-eligible kinds (a class or an extension type; a same-file
    /// enum is the analyzer's own exhaustiveness error, a mixin cannot
    /// carry const instances); the superclass chain additionally walks
    /// mixin applications.
    fn class_like_named(c: &Check<'_>, name: &str, kinds: &[NodeKind]) -> Option<(u32, NodeKind)> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if kinds.contains(&c.kind(kid)) && declaration_name(c, kid) == Some(name) {
                    return Some((kid, c.kind(kid)));
                }
            }
        }
        None
    }

    /// The written name of a class-like declaration row: the sibling
    /// identifier for the class family (`class_declaration_name`), the
    /// name-token payload word for an extension type (bridge
    /// `begin_extension_type_declaration`).
    fn declaration_name<'a>(c: &'a Check<'_>, row: u32) -> Option<&'a str> {
        if c.kind(row) == NodeKind::ExtensionTypeDeclaration {
            let name = c.payload(row).get(2).copied()?;
            return (name != NONE_TOKEN).then(|| c.lexeme(name));
        }
        class_declaration_name(c, row)
    }

    /// The `(is_private, is_generative)` constructor face of the candidate:
    /// the declared constructors and factories plus the header primary
    /// constructor (the official `constructors.every((c) => c.isPrivate &&
    /// c.isGenerative)` walk, extensions.dart:620). An empty face is the
    /// element model's synthesized default constructor — public, so the
    /// gate fails on it.
    fn constructor_face(c: &Check<'_>, candidate_row: u32) -> Vec<(bool, bool)> {
        let mut ctors = Vec::new();
        let mut push = |row: u32| match c.kind(row) {
            NodeKind::Constructor => ctors.push((constructor_name_private(c, row), true)),
            // A factory is never generative — the gate fails on it however
            // its name is spelled (:620).
            NodeKind::Factory => ctors.push((false, false)),
            // A primary constructor names through `._` (private) or not at
            // all (the public type-named form).
            NodeKind::PrimaryConstructor => {
                ctors.push((c.payload(row).get(2) == Some(&1), true));
            }
            _ => {}
        };
        // The header face (an extension type's representation declaration
        // hangs off the declaration row) and the declared members.
        for row in rows_with_parent(c, candidate_row) {
            push(row);
        }
        for row in crate::rules::body_members(c, candidate_row) {
            push(row);
        }
        if ctors.is_empty() {
            ctors.push((false, true));
        }
        ctors
    }

    /// Whether the constructor named at the row head is private: the head
    /// is the class-name token, and the element name is that token — bare
    /// for an unnamed constructor, `.`-suffixed for a named one — so the
    /// official name test (`name.isPrivateName`) reduces to the head
    /// lexeme leading with `_`: a library-private class is private in
    /// every spelling (`_Priv(`, `_Priv._(`, `_Priv.named(`), a public
    /// class only through the `._` tail (`E(` and `E.named(` are public).
    fn constructor_name_private(c: &Check<'_>, row: u32) -> bool {
        let head = c.head_token(row);
        if lexeme_at(c, head).starts_with('_') {
            return true;
        }
        lexeme_at(c, head + 1) == "." && lexeme_at(c, head + 2).starts_with('_')
    }

    /// The lexeme of `token`, empty past the table (the tail guard of the
    /// constructor-name walk).
    fn lexeme_at<'a>(c: &'a Check<'_>, token: u32) -> &'a str {
        c.unit
            .tokens
            .get(token as usize)
            .map_or("", |t| &c.unit.source[t.byte_range()])
    }

    /// Whether any same-file class's superclass chain reaches the candidate
    /// — `_hasSubclassInDefiningCompilationUnit` (extensions.dart:259-270)
    /// over the current unit (the official walk sees the whole library;
    /// another unit's subclass stays unseen).
    fn subclassed_in_file(c: &Check<'_>, candidate: u32) -> bool {
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if !matches!(
                    c.kind(kid),
                    NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication
                ) {
                    continue;
                }
                if chain_reaches(c, &superclass_faces(c, kid), candidate) {
                    return true;
                }
            }
        }
        false
    }

    /// The superclass-chain faces of a class row: the written `extends`
    /// names (`with` mixins ride on the extends face without joining the
    /// chain — `classType.superclass`, extensions.dart:263), or a mixin
    /// application's `=` name (its superclass face).
    fn superclass_faces<'a>(c: &'a Check<'_>, row: u32) -> Vec<&'a str> {
        match c.kind(row) {
            NodeKind::ClassDeclaration => supertypes(c, row)
                .into_iter()
                .filter(|&(_, clause)| clause == SupertypeClause::Extends)
                .map(|(name, _)| name)
                .collect(),
            NodeKind::NamedMixinApplication => supertypes(c, row)
                .into_iter()
                .next()
                .map(|(name, _)| vec![name])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Whether the written superclass names reach `target` over the
    /// same-file class rows (a cycle-guarded walk; a name resolving
    /// outside the file dead-ends).
    fn chain_reaches(c: &Check<'_>, names: &[&str], target: u32) -> bool {
        let chain_class = |name: &str| {
            class_like_named(
                c,
                name,
                &[NodeKind::ClassDeclaration, NodeKind::NamedMixinApplication],
            )
            .map(|(row, _)| row)
        };
        let mut seen: Vec<u32> = Vec::new();
        let mut queue: Vec<u32> = Vec::new();
        for &name in names {
            if let Some(row) = chain_class(name) {
                if row == target {
                    return true;
                }
                if !seen.contains(&row) {
                    seen.push(row);
                    queue.push(row);
                }
            }
        }
        while let Some(row) = queue.pop() {
            for name in superclass_faces(c, row) {
                if let Some(next) = chain_class(name) {
                    if next == target {
                        return true;
                    }
                    if !seen.contains(&next) {
                        seen.push(next);
                        queue.push(next);
                    }
                }
            }
        }
        false
    }

    /// One `static const` field of a same-file class-like scope, in
    /// declaration order (the official field walk,
    /// extensions.dart:627-642).
    struct ConstFieldFace<'a> {
        name: &'a str,
        /// The initializer's token range; `None` when absent (the
        /// `computeConstantValue() == null` skip).
        init: Option<core::ops::Range<u32>>,
        /// The written type's head name (`E` of `E?`/`E<int>`); empty for a
        /// function/void type (never the candidate type), `None` unwritten.
        written: Option<&'a str>,
        /// `metadata.hasDeprecated` — `@deprecated` / `@Deprecated`.
        deprecated: bool,
    }

    /// The `static const` fields of the class-like `class_row`.
    fn class_const_fields<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<ConstFieldFace<'a>> {
        let body = c
            .collect_direct(class_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
            .unwrap_or(class_row);
        let mut fields = Vec::new();
        for member in rows_with_parent(c, body) {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::StaticField as u32))
            {
                continue;
            }
            let deprecated = member_annotations(c, body, member).iter().any(|&meta| {
                c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && matches!(c.lexeme(c.head_token(piece)), "deprecated" | "Deprecated")
                })
            });
            let written = c
                .collect_direct(member)
                .into_iter()
                .find_map(|kid| match c.kind(kid) {
                    NodeKind::Type => Some(c.lexeme(c.head_token(kid))),
                    NodeKind::FunctionType | NodeKind::VoidType => Some(""),
                    _ => None,
                });
            for decl in c
                .collect_direct(member)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
            {
                let Some(kw) = variables_declaration_keyword(c, decl) else {
                    continue;
                };
                if c.lexeme(kw) != "const" {
                    continue;
                }
                for var in c
                    .collect_direct(decl)
                    .into_iter()
                    .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                {
                    let Some(name_row) = initialized_identifier_name(c, var) else {
                        continue;
                    };
                    let init = initializer_range(c, var);
                    fields.push(ConstFieldFace {
                        name: c.lexeme(name_row),
                        init,
                        written,
                        deprecated,
                    });
                }
            }
        }
        fields
    }

    /// The initializer's token range of an `InitializedIdentifier` row —
    /// the tokens after the `=` up to the declarator's end: the first
    /// depth-0 `,` (a sibling declarator's separator) or the terminating
    /// `;`, whichever comes first. The row's own tail can leak past the
    /// declarator (the bridge seals the row at the last completed node,
    /// which a name-shaped initializer — `newFoo`, `E.e` — never
    /// completes, while a creation `E._(1)` does), so a leaked sibling
    /// (`b = E._(2)`) must not join the value text; the `,` cut only
    /// fires outside brackets, depth tracked over `(`, `[`, `{` — inside
    /// them a `,` belongs to the expression (argument lists, list/map/set
    /// literals). `<`/`>` are deliberately not tracked: a depth-0 `,`
    /// inside `<...>` only occurs in a parameterized tear-off or type
    /// literal (`map<int, String>`), whose value can never head the
    /// candidate type, so cutting there is invisible to coverage — while
    /// tracking `<` would swallow the relational shape's real separator
    /// (`a = b < c, d = E._(2)`). A `;` cannot occur inside a constant
    /// initializer (function literals are not const; a string literal's
    /// semicolons ride inside the string token), so it terminates at any
    /// depth.
    pub(crate) fn initializer_range(c: &Check<'_>, var: u32) -> Option<core::ops::Range<u32>> {
        let range = c.unit.ast.token_range(var);
        let mut token = range.start + 1;
        while token < range.end {
            if c.lexeme(token) == "=" {
                let mut end = token + 1;
                let mut depth = 0usize;
                while end < range.end {
                    match c.lexeme(end) {
                        "(" | "[" | "{" => depth += 1,
                        ")" | "]" | "}" => depth = depth.saturating_sub(1),
                        ";" => break,
                        "," if depth == 0 => break,
                        _ => {}
                    }
                    end += 1;
                }
                return Some(token + 1..end);
            }
            token += 1;
        }
        None
    }

    /// The candidate's constant fields, valued (extensions.dart:626-642):
    /// each field's value id resolves through the alias channel, and
    /// `of_this_type` reads the written type or — unwritten — the canonical
    /// terminal's head (`field.type != thisType`).
    fn candidate_constants<'a>(
        c: &Check<'a>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'a>],
        candidate_name: &str,
    ) -> Vec<ConstantField<'a>> {
        field_faces
            .iter()
            .map(|face| {
                let value = face
                    .init
                    .clone()
                    .and_then(|init| init_value(c, values, field_faces, init, &mut Vec::new(), 0));
                let of_this_type = match face.written {
                    Some(name) => name == candidate_name,
                    None => value.is_some_and(|id| {
                        values.text(id).split(' ').next() == Some(candidate_name)
                    }),
                };
                ConstantField {
                    name: face.name,
                    is_static_const: true,
                    of_this_type,
                    deprecated: face.deprecated,
                    value,
                }
            })
            .collect()
    }

    /// One switch member's verdict face (the official member walk,
    /// exhaustive_cases.dart:49-77): `default` silence, and coverage
    /// through the head's constant-value identity.
    fn member_face(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        case: u32,
    ) -> SwitchMember {
        let is_default = c.payload(case).first().copied() != Some(NONE_TOKEN);
        let mut covers = None;
        if !is_default {
            // The first case head only — the official reads
            // `member.expression` / the single `guardedPattern` (:51-57).
            let head = c
                .collect_direct(case)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::CaseExpression);
            if let Some(head) = head
                && let Some(range) = head_range(c, head)
                && let Some(face) = head_face(c, range)
            {
                covers = alias_value(c, values, field_faces, face, &mut Vec::new(), 0);
            }
        }
        SwitchMember { is_default, covers }
    }

    /// The head expression's token face from the `CaseExpression` row: the
    /// constant pattern's span, with the bridge's two tiling wrinkles
    /// normalized —
    ///
    /// - a parenthesized head (`case (E.e):`) rides a `ParenthesizedPattern`
    ///   leaf spanning the parens; the inner constant pattern (the window's
    ///   first tile) carries the unparenthesized face (the official
    ///   `pattern.unParenthesized`, :52);
    /// - a direct head's adopted window holds only the trailing operand
    ///   subtree, so a dot-shorthand member name (`case .e:`) leaks as
    ///   preceding `CaseExpression` children (the `Identifier` /
    ///   `DotShorthandHead` leaves — the `type_literal_in_constant_pattern`
    ///   idiom); the range widens over them. Literal/creation heads grow no
    ///   leak, and a `when` guard completes after the pattern, outside the
    ///   widened prefix.
    fn head_range(c: &Check<'_>, head: u32) -> Option<core::ops::Range<u32>> {
        let kids = c.collect_direct(head);
        let pos = kids.iter().position(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ConstantPattern | NodeKind::ParenthesizedPattern
            )
        })?;
        let row = kids[pos];
        if c.kind(row) == NodeKind::ParenthesizedPattern {
            let mut inner = first_child(c, row)?;
            while c.kind(inner) == NodeKind::ParenthesizedPattern {
                inner = first_child(c, inner)?;
            }
            return (c.kind(inner) == NodeKind::ConstantPattern)
                .then(|| c.unit.ast.token_range(inner));
        }
        let mut range = c.unit.ast.token_range(row);
        for &kid in &kids[..pos] {
            let kid_range = c.unit.ast.token_range(kid);
            range.start = range.start.min(kid_range.start);
            range.end = range.end.max(kid_range.end);
        }
        Some(range)
    }

    /// The head's alias face — the official variable faces only
    /// (`Identifier` / `PropertyAccess` / `DotShorthandPropertyAccess`,
    /// :59-74): identifier segments joined by `.`, an optional `const`
    /// introducer stripped (`case const E.e:` covers through the same
    /// expression). `None` — list/map/object patterns, literals,
    /// creations — covers nothing.
    fn head_face<'a>(c: &'a Check<'a>, range: core::ops::Range<u32>) -> Option<AliasFace<'a>> {
        let (leading_dot, names) = segments(c, strip_const(c, range))?;
        match names.as_slice() {
            [name] if leading_dot => Some(AliasFace::Shorthand(name)),
            [name] => Some(AliasFace::Bare(name)),
            // A leading-dot chain (`.a.b`) is a selector on a shorthand,
            // not one of the official variable faces.
            [.., class, field] if !leading_dot => Some(AliasFace::Qualified(class, field)),
            _ => None,
        }
    }

    /// The token range past a leading `const` introducer — `const` heads
    /// and initializers carry their constant through the same expression
    /// as the bare spelling.
    pub(super) fn strip_const(
        c: &Check<'_>,
        range: core::ops::Range<u32>,
    ) -> core::ops::Range<u32> {
        if range.start < range.end && c.lexeme(range.start) == "const" {
            return range.start + 1..range.end;
        }
        range
    }

    /// The dot-joined identifier segments of a token range: `(leading_dot,
    /// names)` — `E.e` → `(false, ["E", "e"])`, `prefix.E.e` →
    /// `(false, ["prefix", "E", "e"])`, `.e` → `(true, ["e"])`. `None` for
    /// any other expression (a non-name token inside a segment — the
    /// literal/creation/interpolation faces).
    pub(super) fn segments<'a>(
        c: &'a Check<'_>,
        range: core::ops::Range<u32>,
    ) -> Option<(bool, Vec<&'a str>)> {
        let mut names: Vec<&'a str> = Vec::new();
        let mut shorthand = false;
        let mut current: Option<&'a str> = None;
        for token in range {
            match c.lexeme(token) {
                "." if current.is_none() && names.is_empty() => shorthand = true,
                "." => names.push(current.take()?),
                lex => {
                    if current.is_some() || !is_name_token(c, token) {
                        return None;
                    }
                    current = Some(lex);
                }
            }
        }
        if let Some(last) = current.take() {
            names.push(last);
        }
        (!names.is_empty()).then_some((shorthand, names))
    }

    /// Whether the token lexes as a plain name (an identifier, or a
    /// built-in keyword — a constant named `set` binds;
    /// `initialized_identifier_name`'s tolerance).
    fn is_name_token(c: &Check<'_>, token: u32) -> bool {
        match c.unit.tokens.get(token as usize).map(|t| t.kind) {
            Some(TokenKind::Identifier) => true,
            Some(TokenKind::Keyword(kw)) if kw.style() == KeywordStyle::BuiltIn => true,
            _ => false,
        }
    }

    /// The alias face of a constant — a bare name (`static const b = a`),
    /// a shorthand (`= .e`; never resolvable standalone), or a
    /// `Class.member` face.
    #[derive(Clone, Copy, Debug)]
    enum AliasFace<'a> {
        Bare(&'a str),
        Shorthand(&'a str),
        Qualified(&'a str, &'a str),
    }

    /// The constant value id of one initializer: a dot-joined name segment
    /// aliases into the named constant's value, and any other expression —
    /// the terminal face (`E._(1)`, a literal) — is its own value text (the
    /// `computeConstantValue` identity: equal texts denote the same
    /// instance). A `.e` shorthand has no standalone value. `seen` breaks
    /// alias cycles, `depth` bounds them.
    fn init_value(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        init: core::ops::Range<u32>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> Option<u32> {
        if depth > MAX_ALIAS_DEPTH || seen.contains(&init.start) {
            return None;
        }
        let init = strip_const(c, init);
        match segments(c, init.clone()) {
            None => Some(values.intern(c, init.clone())),
            Some((true, _)) => None,
            Some((false, names)) => {
                seen.push(init.start);
                let target = match names.as_slice() {
                    [name] => field_faces
                        .iter()
                        .find(|field| field.name == *name)
                        .and_then(|field| field.init.clone())
                        .or_else(|| top_level_const_init(c, name)),
                    [.., class, field] => class_const_init(c, class, field),
                    _ => return None,
                }?;
                init_value(c, values, field_faces, target, seen, depth + 1)
            }
        }
    }

    /// The value of a head's alias face: resolve the named constant, then
    /// its initializer's value (unresolvable — nothing covered).
    fn alias_value(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        face: AliasFace<'_>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> Option<u32> {
        if depth > MAX_ALIAS_DEPTH {
            return None;
        }
        let target = match face {
            // A bare head names the candidate's own constants first (class
            // scope shadows the top level), then the top-level scope.
            AliasFace::Bare(name) => field_faces
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| field.init.clone())
                .or_else(|| top_level_const_init(c, name)),
            // A shorthand head resolves against the scrutinee's type — the
            // candidate's constants.
            AliasFace::Shorthand(name) => field_faces
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| field.init.clone()),
            AliasFace::Qualified(class, field) => class_const_init(c, class, field),
        }?;
        init_value(c, values, field_faces, target, seen, depth + 1)
    }

    /// The initializer of the `static const` field `field` of the same-file
    /// class-like `class` — the `E.e` head face and the `Class.member`
    /// alias target.
    pub(super) fn class_const_init(
        c: &Check<'_>,
        class: &str,
        field: &str,
    ) -> Option<core::ops::Range<u32>> {
        let (row, _) = class_like_named(
            c,
            class,
            &[
                NodeKind::ClassDeclaration,
                NodeKind::ExtensionTypeDeclaration,
            ],
        )?;
        class_const_fields(c, row)
            .into_iter()
            .find(|known| known.name == field)?
            .init
    }

    /// The initializer of the same-file top-level `const` named `name` —
    /// the bare-name alias target outside class scope.
    pub(super) fn top_level_const_init(c: &Check<'_>, name: &str) -> Option<core::ops::Range<u32>> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if c.kind(kid) != NodeKind::TopLevelFields {
                    continue;
                }
                for decl in c
                    .collect_direct(kid)
                    .into_iter()
                    .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
                {
                    let Some(kw) = variables_declaration_keyword(c, decl) else {
                        continue;
                    };
                    if c.lexeme(kw) != "const" {
                        continue;
                    }
                    for var in c
                        .collect_direct(decl)
                        .into_iter()
                        .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                    {
                        if let Some(name_row) = initialized_identifier_name(c, var)
                            && c.lexeme(name_row) == name
                        {
                            return initializer_range(c, var);
                        }
                    }
                }
            }
        }
        None
    }

    /// The canonical constant-value table: initializer texts interned to
    /// dense ids.
    #[derive(Default)]
    struct ConstValues {
        texts: Vec<String>,
    }

    impl ConstValues {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "interned ids are bounded by one unit's constant count; \
                      u32 is the payload width"
        )]
        fn intern(&mut self, c: &Check<'_>, range: core::ops::Range<u32>) -> u32 {
            let text = token_text(c, range);
            if let Some(id) = self.texts.iter().position(|known| *known == text) {
                return id as u32;
            }
            self.texts.push(text);
            (self.texts.len() - 1) as u32
        }

        fn text(&self, id: u32) -> &str {
            &self.texts[id as usize]
        }
    }

    /// The canonical text of a token range — lexemes joined with a space
    /// (whitespace-free, comment-free value identity).
    fn token_text(c: &Check<'_>, range: core::ops::Range<u32>) -> String {
        let mut text = String::new();
        for token in range {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(c.lexeme(token));
        }
        text
    }

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

        // —— the mounted face: the official test sources through the
        // production bridge (`Fixture::parse`) ——

        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("exhaustive_cases"))
        }

        /// `test_class_enumLike_allCases` — every value covered, silent.
        #[test]
        fn mounted_all_cases_silent() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void ok(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
      break;
    case E.g:
      break;
  }
}
"#)
                .is_empty()
            );
        }

        /// `test_class_enumLike_missingCase` — the uncovered value reports
        /// at the statement head through the scrutinee's `)`, with the
        /// official message and correction.
        #[test]
        fn mounted_missing_case_reports() {
            let source = r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void e(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding case clauses for the missing constants.")
            );
            // The span: the `switch` head through the scrutinee's `)`
            // (the sixth close paren — after the four creation spellings
            // and the parameter list).
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert_eq!(
                lints[0].range.end,
                utf16_at(&f, tok(source, &f.tokens, ")", 5)) + 1
            );
        }

        /// `test_class_enumLike_withDefault` — a `default` member silences
        /// the whole check.
        #[test]
        fn mounted_default_silences() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okDefault(E e) {
  switch (e) {
    case E.e:
      break;
    default:
      break;
  }
}
"#)
                .is_empty()
            );
        }

        /// `test_class_enumLike_parenthesizedCases` — parenthesized case
        /// heads cover (the `unParenthesized` walk).
        #[test]
        fn mounted_parenthesized_cases_cover() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okParenthesized(E e) {
  switch (e) {
    case (E.e):
      break;
    case ((E.f)):
      break;
    case (E.g):
      break;
  }
}
"#)
                .is_empty()
            );
        }

        /// `test_class_enumLike_deprecatedAliases` — the deprecated alias
        /// groups with its live twin; exactly one report (`bar`, the group
        /// the second switch leaves open), the other two switches silent.
        #[test]
        fn mounted_deprecated_aliases() {
            let source = r#"
class DeprecatedFields {
  final int i;
  const DeprecatedFields._(this.i);

  @deprecated
  static const oldFoo = newFoo;
  static const newFoo = DeprecatedFields._(1);
  static const bar = DeprecatedFields._(2);
  static const baz = DeprecatedFields._(3);
}

void dep(DeprecatedFields e) {
  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.oldFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'bar'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 1))
            );
        }

        /// `test_class_notEnumLike_publicConstructor` / `_singleConstant` /
        /// `_subclassed` — the enum-like gate rejections, silent.
        #[test]
        fn mounted_not_enum_like_silent() {
            assert!(
                run(r#"
class PublicCons {
  const PublicCons();
  static const e = PublicCons();
  static const f = PublicCons();
}

void p(PublicCons e) {
  switch (e) {
    case PublicCons.e:
  }
}
"#)
                .is_empty()
            );
            assert!(
                run(r#"
class TooFew {
  const TooFew._();

  static const e = TooFew._();
}

void t(TooFew e) {
  switch (e) {
    case TooFew.e:
  }
}
"#)
                .is_empty()
            );
            assert!(
                run(r#"
class Subclassed {
  const Subclassed._();

  static const e = Subclassed._();
  static const f = Subclassed._();
  static const g = Subclassed._();
}

class Subclass extends Subclassed {
  Subclass() : super._();
}

void s(Subclassed e) {
  switch (e) {
    case Subclassed.e:
  }
}
"#)
                .is_empty()
            );
        }

        /// `test_class_enumLike_dotShorthand_missingCase` /
        /// `_withDefault` — the `.e` head face covers and misses like the
        /// spelled form.
        #[test]
        fn mounted_dot_shorthand_heads() {
            let source = r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    case .f:
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    default:
      break;
  }
}
"#)
                .is_empty()
            );
        }

        /// The `test_extensionType_*` family — enum-like extension types
        /// (private representation, two-plus same-typed constants) report
        /// the missing constant; the public-representation and
        /// single-constant shapes stay silent.
        #[test]
        fn mounted_extension_type_family() {
            let source = r#"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
  static const E c = E._(3);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'c'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert!(
                run(r#"
extension type const E(int i) {
  static const E a = E(1);
  static const E b = E(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
"#)
                .is_empty()
            );
            assert!(
                run(r#"
extension type const E._(int i) {
  static const E a = E._(1);
}

void f(E e) {
  switch (e) {}
}
"#)
                .is_empty()
            );
            assert!(
                run(r#"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    default:
  }
}
"#)
                .is_empty()
            );
        }

        /// The shorthand extension-type head (`.a`) — the official
        /// `test_extensionType_enumLike_dotShorthand_missingCase`.
        #[test]
        fn mounted_extension_type_dot_shorthand() {
            let source = r#"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case .a:
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }

        /// `test_enum_missingCase` — a real enum never enters (the
        /// element walk's default arm; the analyzer's own exhaustiveness
        /// error covers it, and that diagnostic is out of this face).
        #[test]
        fn mounted_enum_scrutinee_silent() {
            assert!(
                run(r#"
enum Color { red, green }

void f(Color c) {
  switch (c) {
    case Color.red:
  }
}
"#)
                .is_empty()
            );
        }

        /// The value-based coverage faces: a bare top-level alias head
        /// (`case alias:`) and a cross-class `Class.member` head cover by
        /// value identity — the remaining values report in declaration
        /// order.
        #[test]
        fn mounted_value_based_heads() {
            let alias_source = r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

const alias = E.e;

void fn(E e) {
  switch (e) {
    case alias:
  }
}
"#;
            let lints = run(alias_source);
            assert_eq!(lints.len(), 2, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'f'."
            );
            assert_eq!(
                lints[1].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

class Other {
  static const z = E.e;
}

void fn(E e) {
  switch (e) {
    case Other.z:
  }
}
"#)
                .iter()
                .map(|lint| lint.message.as_str())
                .collect::<Vec<_>>(),
                [
                    "Missing case clauses for some constants in 'f'.",
                    "Missing case clauses for some constants in 'g'.",
                ]
            );
        }

        /// The same-file silences: a non-identifier scrutinee (no written
        /// type on the channel) and an out-of-file candidate (an import
        /// prefix) — the documented under-report family.
        #[test]
        fn mounted_channel_fallbacks_silent() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
}

E make() => E._(1);

void fn() {
  switch (make()) {
    case E.e:
  }
}
"#)
                .is_empty()
            );
            assert!(
                run(r#"
import 'e.dart' as prefixed;

void fn(prefixed.E e) {
  switch (e) {
    case prefixed.E.e:
  }
}
"#)
                .is_empty()
            );
        }

        /// A creation-only multi-declarator row (`static const a =
        /// E._(1), b = E._(2);`) seals cleanly per declarator, and the
        /// depth-0 `,` cut keeps each value's own tokens: full coverage
        /// is silent, a partial switch reports the uncovered declarator.
        #[test]
        fn mounted_multi_declarator_creations() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
      break;
  }
}
"#)
                .is_empty()
            );
            let source = r#"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
"#;
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }

        /// The mixed-alias multi-declarator leak pin: a name-shaped
        /// initializer (`newFoo`) never completes a bridge node, so `a`'s
        /// row span can seal past the sibling `,` — without the depth-0
        /// cut the value text interns as `newFoo , b = E._(2)` and `a`'s
        /// alias to `E._(1)` breaks. The official corpus has no
        /// multi-declarator case; the expectation follows
        /// `asEnumLikeType`'s value resolution — `a`'s
        /// `computeConstantValue()` is the `E._(1)` instance, `newFoo`'s
        /// identity, so `case E.newFoo:` covers `a` and the row is
        /// silent, while leaving the shared group open reports it under
        /// its first name.
        #[test]
        fn mounted_multi_declarator_alias_no_false_positive() {
            assert!(
                run(r#"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.newFoo:
      break;
    case E.b:
      break;
  }
}
"#)
                .is_empty()
            );
            let source = r#"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.b:
  }
}
"#;
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'a'."
            );
        }

        /// A library-private class's unnamed constructor (`_Priv(`) is
        /// private — the official `isPrivate` is the element name's
        /// leading `_`, which every constructor of the class leads with —
        /// so the enum-like gate holds and the missing case reports; the
        /// public spelling (`Priv(`) stays gate-silent.
        #[test]
        fn mounted_private_class_unnamed_constructor() {
            let source = r#"
class _Priv {
  final int i;
  const _Priv(this.i);

  static const a = _Priv(1);
  static const b = _Priv(2);
}

void f(_Priv e) {
  switch (e) {
    case _Priv.a:
  }
}
"#;
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }
    }
}

pub(crate) mod always_put_control_body_on_new_line {
    use super::{Check, Group, NodeKind, RuleSpec, first_child, last_child};

    /// Official: `pkg/linter/lib/src/rules/always_put_control_body_on_new_line.dart:30-78`
    /// — a control structure's body statement on the same line as the
    /// control structure reports at the body's first token. Faces (:38-53):
    /// the `do` body (compared against the `do` keyword's end), the
    /// `for`/`while` bodies and the `if` then-branch (against the closing
    /// `)`), and the `else` branch when it is not another `if` (against the
    /// `else` keyword's end). An empty block body skips (:56).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "always_put_control_body_on_new_line",
        group: Group::Standalone,
        problem: "Statement should be on a separate line.",
        correction: "Try moving the statement to a new line.",
        kinds: &[
            NodeKind::IfStatement,
            NodeKind::WhileStatement,
            NodeKind::ForStatement,
            NodeKind::ForIn,
            NodeKind::DoWhileStatement,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::IfStatement => {
                // The then-branch is the direct child after the condition
                // (collapse map §3.4 IfStatement row).
                let kids = c.collect_direct(node);
                if let Some(condition) = kids
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
                    && let Some(&then) = kids.get(condition + 1)
                {
                    let control_end = control_end_before(c, then);
                    check_body(c, then, control_end);
                }
                // The else branch (:50-53), skipped for `else if` chains.
                let Some(else_kw) = c
                    .payload(node)
                    .first()
                    .copied()
                    .filter(|&kw| kw != super::NONE_TOKEN)
                else {
                    return;
                };
                if let Some(else_stmt) = last_child(c, node)
                    && c.kind(else_stmt) != NodeKind::IfStatement
                {
                    check_body(c, else_stmt, Some(token_end(c, else_kw)));
                }
            }
            NodeKind::WhileStatement | NodeKind::ForStatement | NodeKind::ForIn => {
                if let Some(body) = last_child(c, node) {
                    let control_end = control_end_before(c, body);
                    check_body(c, body, control_end);
                }
            }
            NodeKind::DoWhileStatement => {
                // Compared against the `do` keyword's end (:38-40); the body
                // is the clause's first child.
                if let Some(body) = first_child(c, node) {
                    check_body(c, body, Some(token_end(c, c.head_token(node))));
                }
            }
            _ => {}
        }
    }

    /// The end offset of the token preceding the body — the control
    /// structure's closing `)`.
    fn control_end_before(c: &Check<'_>, body: u32) -> Option<u32> {
        let head = c.head_token(body);
        (head > 0).then_some(token_end(c, head - 1))
    }

    fn token_end(c: &Check<'_>, token: u32) -> u32 {
        let t = c.unit.tokens.get(token as usize).expect("token in table");
        t.utf16_offset + c.lexeme(token).encode_utf16().count() as u32
    }

    /// The official `_checkNodeOnNextLine` (:55-66): an empty block skips,
    /// the compared body offset is the block's first statement's offset,
    /// and the report sits at the body's begin token.
    fn check_body(c: &mut Check<'_>, body: u32, control_end: Option<u32>) {
        let Some(control_end) = control_end else {
            return;
        };
        let first = if c.kind(body) == NodeKind::Block {
            match super::first_child(c, body) {
                Some(first) => first,
                // `node.statements.isEmpty` (:56).
                None => return,
            }
        } else {
            body
        };
        let body_offset = c
            .unit
            .tokens
            .get(c.head_token(first) as usize)
            .expect("token in table")
            .utf16_offset;
        let same_line = |offset: u32| c.unit.lines.line_col_utf16(c.unit.source, offset).0;
        if same_line(control_end) == same_line(body_offset) {
            c.report(crate::rules::utf16_token_span(c, c.head_token(body)), &[]);
        }
    }
}

pub(crate) mod avoid_catches_without_on_clauses {
    use super::{Check, Group, NodeKind, RuleSpec, last_child};

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
    /// inside the throw's subtree; a shadowing local of the same name
    /// inside a closure body over-reports.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_catches_without_on_clauses",
        group: Group::Standalone,
        problem: "Catch clause should use 'on' to specify the type of exception being caught.",
        correction: "Try adding an 'on' clause before the 'catch'.",
        kinds: &[NodeKind::CatchClause],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `onKeyword != null` (:69-70): the on-form opens at `on`
        // (head-when-present, collapse map §3.4 CatchClause row).
        if c.lexeme(c.head_token(node)) == "on" {
            return;
        }
        // `catchKeyword == null` (:86-88).
        let Some(catch_kw) = c
            .payload(node)
            .first()
            .copied()
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
            .payload(exception)
            .get(1)
            .filter(|&&tok| tok != super::NONE_TOKEN)
            .map(|&tok| c.lexeme(tok))
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
            let head = c.head_token(row);
            if throws.iter().any(|&throw| {
                row < throw
                    && head > c.head_token(throw)
                    && window
                        .clone()
                        .filter(|&stmt| {
                            stmt < throw
                                && c.unit.ast.parent(stmt) == c.unit.ast.parent(throw)
                                && crate::rules::is_statement_row(c, stmt)
                        })
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
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    /// A legacy (`Feature.patterns` off, `<=2.19`) fixture — the library
    /// version whose switch members are legacy `SwitchCase` shapes, the only
    /// face `no_duplicate_case_values` processes.
    fn legacy(mut f: Fixture) -> Fixture {
        f.features.patterns = false;
        f
    }

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

    /// The default-version face (official `test_duplicateIntConstant_ok`/
    /// `test_duplicateStringConstant_ok`/`test_duplicateConstClassValue_ok`/
    /// `test_duplicateEnumValue_ok`): with `Feature.patterns` on (3.0+) every
    /// member is a pattern case and the lint is silent — even on repeated
    /// constant values.
    #[test]
    fn duplicate_case_value_silent_at_default_version() {
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
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// `test/rules/no_duplicate_case_values_test.dart` `test_duplicateCase`
    /// (`NoDuplicateCaseValuesTestLanguage219`) — in a legacy (`patterns`
    /// off) library the duplicate reports with both sources in the message.
    #[test]
    fn duplicate_case_value_reports_legacy() {
        let source = "switch (x) { case 1: break; case 1: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
        let f = legacy(Fixture::build(
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
        ));
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

    /// Distinct values and a `default:` clause (no expressions) pass — the
    /// legacy (`Language219`) face.
    #[test]
    fn distinct_and_default_cases_pass() {
        let source = "switch (x) { case 1: break; case 2: break; default: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 211 :12 break13 ;14
        // default15 :16 break17 ;18 }19
        let f = legacy(Fixture::build(
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
        ));
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// The official keys by computed constant (`computeConstantValue`, :44):
    /// `case 1 + 1:` collides with `case 2:` — the later expression reports
    /// with both sources (legacy face).
    #[test]
    fn computed_duplicate_case_values_report() {
        let source = "switch (x) { case 1 + 1: break; case 2: break; }";
        // switch0 (1 x2 )3 {4 case5 16 +7 18 :9 break10 ;11 case12 213 :14
        // break15 ;16 }17
        let f = legacy(Fixture::build(
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
        ));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('2') is equal to the value of an earlier case clause ('1 + 1')."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 13));
    }

    /// String concatenation folds: `case 'a' + 'b':` collides with
    /// `case 'ab':` (legacy face).
    #[test]
    fn string_concatenation_duplicate_cases_report() {
        let source = "switch (x) { case 'a' + 'b': break; case 'ab': break; }";
        // switch0 (1 x2 )3 {4 case5 'a'6 +7 'b'8 :9 break10 ;11 case12
        // 'ab'13 :14 break15 ;16 }17
        let f = legacy(Fixture::build(
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
        ));
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
    /// identifiers stay silent (legacy face).
    #[test]
    fn non_constant_case_expressions_pass() {
        let source = "switch (x) { case y: break; case y: break; }";
        // switch0 (1 x2 )3 {4 case5 y6 :7 break8 ;9 case10 y11 :12 break13
        // ;14 }15
        let f = legacy(Fixture::build(
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
        ));
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

    // —— the converted audit faces (exemption-reaudit F4/F5): the
    // production-bridge pins for the resolution-backed decisions ——

    /// The forward `continue L` to a LATER switch member resolves through
    /// the member-label scope (scope_analyzer.dart:846-850); the target
    /// member's chain contains the try, so the finally jump stays local
    /// and stays silent (the oracle probe face).
    #[test]
    fn finally_forward_continue_member_label_passes() {
        assert!(
            Fixture::parse(
                r#"
void f(bool c) {
  try {
    print(0);
  } finally {
    switch (c) {
      case true:
        continue L;
      L:
      case false:
        print(1);
    }
  }
}
"#
            )
            .run(&one_rule("control_flow_in_finally"))
            .is_empty()
        );
    }

    /// The same forward jump with the target member OUTSIDE the try: the
    /// target's chain misses the try and the jump reports (the oracle
    /// probe reports `break L` at the same shape).
    #[test]
    fn finally_forward_continue_outside_try_reports() {
        let f = Fixture::parse(
            r#"
void f(bool c) {
  switch (c) {
    case true:
      try {
        print(0);
      } finally {
        continue L;
      }
    L:
    case false:
      print(1);
  }
}
"#,
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Use of 'continue' in a 'finally' clause.");
    }

    /// A forward `continue L` to a LATER labeled statement (no switch)
    /// stays undefined officially (LABEL_UNDEFINED — labeled statements
    /// bind only within their statement) — the unresolved-target face
    /// reports.
    #[test]
    fn finally_forward_continue_to_later_labeled_statement_reports() {
        let f = Fixture::parse(
            r#"
void f(bool c) {
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
    L:
    while (c) {
      print(1);
    }
  }
}
"#,
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// `continue L` onto the try statement itself (`L2: try … finally {
    /// continue L2; }`): the target IS the try — the enabler matches and
    /// the jump stays silent (the oracle probe face).
    #[test]
    fn finally_continue_onto_labeled_try_passes() {
        assert!(
            Fixture::parse(
                r#"
void f(bool c) {
  L:
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
  }
}
"#
            )
            .run(&one_rule("control_flow_in_finally"))
            .is_empty()
        );
    }

    /// The resolution-backed type-literal test: a same-file user type
    /// reports (the under-report face is dead), a same-file value shadow
    /// of a built-in name stays silent (the over-report face is dead).
    #[test]
    fn type_literal_resolution_backed_faces() {
        // `case MyType` on a same-file class reports.
        let f = Fixture::parse(
            r#"
class MyType {}

void f(Object o) {
  if (o case MyType) {
    print(0);
  }
}
"#,
        );
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        // `const String = 'x';` shadows the built-in — the pattern is the
        // String constant, not a type literal.
        let f = Fixture::parse(
            r#"
const String = 'x';

void f(Object o) {
  if (o case String) {
    print(0);
  }
}
"#,
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        // A local `var int` shadow — the name resolves to the value.
        let f = Fixture::parse("void f(Object o) { var int = 5; if (o case int) { print(int); } }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    /// The const-alias fold: a Type-valued constant reports like the
    /// official static type (`case t` where `t` is `int`) — top-level,
    /// local, and qualified-class spellings; a value alias stays silent.
    #[test]
    fn type_literal_const_alias_faces() {
        // Top-level `const t = int;` reports.
        let f = Fixture::parse(
            r#"
const t = int;

void f(Object o) {
  if (o case t) {
    print(0);
  }
}
"#,
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );

        // Local `const t = int;` reports (the oracle probe face).
        let f = Fixture::parse(
            r#"
void f(Object o) {
  const t = int;
  if (o case t) {
    print(0);
  }
}
"#,
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );

        // A value alias (`const s = 'x';`) stays silent.
        let f = Fixture::parse(
            r#"
const s = 'x';

void f(Object o) {
  if (o case s) {
    print(0);
  }
}
"#,
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    // —— always_put_control_body_on_new_line ————————————————————————————

    /// `test/rules/always_put_control_body_on_new_line.dart` — a then-body
    /// on the control line reports at the body's first token.
    #[test]
    fn control_body_reports_a_same_line_then_body() {
        let source = "void f(bool c) { if (c) x = 1; }";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Statement should be on a separate line.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try moving the statement to a new line.")
        );
        let x = tok(&f.source, &f.tokens, "x", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// A non-empty block whose first statement shares the control line
    /// reports; the body on its own line passes.
    #[test]
    fn control_body_block_faces() {
        let f = Fixture::parse("void f(bool c) { if (c) { x = 1; } }");
        assert_eq!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .len(),
            1
        );
        let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  }\n}");
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    /// An empty block skips (:56).
    #[test]
    fn control_body_empty_block_passes() {
        let f = Fixture::parse("void f(bool c) { if (c) {} }");
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    /// `while`, `for`, and `for-in` bodies are checked against the
    /// closing `)`.
    #[test]
    fn control_body_loop_faces() {
        let f = Fixture::parse(
            "void f(bool c) { while (c) x = 1; }\n\
             void g() { for (var i = 0; i < 1; i++) x = i; }\n\
             void h() { for (var e in [1]) print(e); }",
        );
        assert_eq!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .len(),
            3
        );
    }

    /// The `do` body is compared against the `do` keyword's end (:38-40).
    #[test]
    fn control_body_do_face() {
        let f = Fixture::parse("void f(bool c) { int x = 0; do x += 1; while (c); }");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // The reported token is the body's first `x` (the second in the
        // source), not the declaration's.
        let x = tok(&f.source, &f.tokens, "x", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// The else branch checks against the `else` keyword (:50-53) — the
    /// report sits at the else clause's begin, the block's `{`; an
    /// `else if` chain skips.
    #[test]
    fn control_body_else_faces() {
        let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  } else { y = 2; }\n}");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "{", 2))
        );
        let f = Fixture::parse(
            "void f(bool c, bool d) {\n  if (c) {\n    x = 1;\n  } else if (d) {\n    y = 2;\n  }\n}",
        );
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    // —— avoid_catches_without_on_clauses ———————————————————————————————

    /// A bare `catch` clause with no valid use reports at the catch
    /// keyword.
    #[test]
    fn catch_without_on_reports_a_bare_catch() {
        let source = "void f() { try {} catch (e) {} }";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("avoid_catches_without_on_clauses"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Catch clause should use 'on' to specify the type of exception being caught."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding an 'on' clause before the 'catch'.")
        );
        let kw = tok(&f.source, &f.tokens, "catch", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, kw));
    }

    /// `on`-clause forms skip — with or without a catch clause.
    #[test]
    fn catch_without_on_on_forms_pass() {
        let f = Fixture::parse(
            "void f() { try {} on int catch (e) {} }\nvoid g() { try {} on int {} }",
        );
        assert!(
            f.run(&one_rule("avoid_catches_without_on_clauses"))
                .is_empty()
        );
    }

    /// `rethrow;` and `throw e;` bodies are valid uses (:96-117).
    #[test]
    fn catch_without_on_valid_uses_pass() {
        let f = Fixture::parse(
            "void f() { try {} catch (e) { rethrow; } }\n\
             void g() { try {} catch (e) { throw e; } }",
        );
        assert!(
            f.run(&one_rule("avoid_catches_without_on_clauses"))
                .is_empty()
        );
    }

    /// Any other body reports; the two-parameter catch form reports too.
    /// (The :96-101 nested-catch rethrow fence is implemented but untested
    /// here: the production parser currently mis-nests a `try` inside a
    /// `catch` body — the builder rejects the row pairing — a parser-side
    /// defect tracked outside this rule.)
    #[test]
    fn catch_without_on_other_bodies_report() {
        let f = Fixture::parse(
            "void f() { try {} catch (e) { print(e); } }\nvoid g() { try {} catch (e, s) {} }",
        );
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            2
        );
    }

    /// The object-pattern field face of the raw-type exemption — the
    /// official `test_constType_matchType_nested`: a constant nested in an
    /// object pattern against a `Type`-written field stays silent (if-case
    /// and switch-case containers); a non-`Type` field keeps the report.
    #[test]
    fn type_literal_object_pattern_type_field_passes() {
        let type_field = |src: &str| {
            let f = Fixture::parse(src);
            assert!(
                f.run(&one_rule("type_literal_in_constant_pattern"))
                    .is_empty(),
                "the matched value is the Type-typed field: {src}"
            );
        };
        type_field(
            r#"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
"#,
        );
        type_field(
            r#"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  switch (x) {
    case A(type: int):
      break;
  }
}
"#,
        );
        // Nested object patterns: the innermost container decides — B's
        // `Type` field exempts, the outer field's own type is irrelevant.
        type_field(
            r#"
class A {
  final B b;
  A(this.b);
}

class B {
  final Type type;
  B(this.type);
}

void f(A x) {
  if (x case A(b: B(type: int))) {}
}
"#,
        );

        // A non-`Type` field keeps the name reading.
        let f = Fixture::parse(
            r#"
class A {
  final int type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
"#,
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "an int-typed field is no exemption"
        );
        // An untyped field keeps the name reading too.
        let f = Fixture::parse(
            r#"
class A {
  final type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
"#,
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "an untyped field is no exemption"
        );
        // The nullable `Type?` is not a subtype of `Type` (the official
        // `isSubtypeOf` gate) — the nested constant reports.
        let f = Fixture::parse(
            r#"
class A {
  final Type? type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
"#,
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "a Type?-typed field is no exemption"
        );
    }

    /// The matched-value exemption's nullability face on the scrutinee:
    /// `Type x` exempts, `Type? x` reports (the official
    /// `isSubtypeOf(matchedValueType, typeType)` — `Type?` is no subtype
    /// of `Type`; `test_constType_matchObjectNullable` shape).
    #[test]
    fn type_literal_nullable_type_scrutinee_reports() {
        let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );
        let f = Fixture::parse("void f(Type? x) { if (x case int) {} }");
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "a Type? matched value is not of type Type"
        );
        let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }
}
