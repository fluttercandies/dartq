use super::{Check, NONE_TOKEN, NodeKind, PayloadField, RuleSpec, last_child};

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
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "control_flow_in_finally",
    &[
        NodeKind::Break,
        NodeKind::Continue,
        NodeKind::ReturnStatement,
    ],
    check,
);

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
    // `finallyBlock == null` bails (:48-54): the `FinallyKeyword`
    // word is the `finally` token (`TryStatementEnd`).
    if c.field(try_node, PayloadField::FinallyKeyword) == Some(NONE_TOKEN) {
        return;
    }
    // The node must be inside the finally block — the try's last child
    // (body block, catch clauses, then the finally block).
    let Some(finally_block) = last_child(c, try_node) else {
        return;
    };
    if c.kind(finally_block) != NodeKind::Block || c.path.get(try_idx + 1) != Some(&finally_block) {
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
        // The official reports the whole statement (`reportAtNode`) —
        // analyzer statement spans carry their `;`. The production rows
        // anchor Break/Continue at the keyword alone (the label and `;`
        // are derivable), so the end extends through the statement's `;`
        // when one follows; hand-built tables already span it (the probe
        // token then is not a `;` and the node span stands).
        let span = c.utf16_span(node);
        let end = semicolon_end(c, node).unwrap_or(span.end);
        c.report(span.start..end, &[kind]);
    }
}

/// The UTF-16 end of the statement's `;` when the token follows the
/// node's span (past a targeted jump's label, the payload `HasTarget`
/// bit), `None` otherwise.
fn semicolon_end(c: &Check<'_>, node: u32) -> Option<u32> {
    let mut probe = c.unit.ast.token_range(node).end;
    // `HasTarget` is a `Break`/`Continue` field only — `ReturnStatement`
    // has an empty payload (fields.rs), so its raw slot always read 0
    // (no label to skip past).
    let targeted = matches!(c.kind(node), NodeKind::Break | NodeKind::Continue)
        && c.field(node, PayloadField::HasTarget)
            .is_some_and(|bit| bit != 0);
    if targeted {
        probe += 1;
    }
    let token = c.unit.tokens.get(probe as usize)?;
    if token.kind != dartq_scanner::TokenKind::Semicolon {
        return None;
    }
    c.utf16_end_of_token(probe)
}

/// The resolved jump target's row, `None` for the official
/// `target == null` face (`_lookupBreakOrContinueTarget`,
/// `scope_analyzer.dart:1042-1093`).
fn jump_target(c: &Check<'_>, node: u32, node_idx: usize) -> Option<u32> {
    // The payload carries only the `HasTarget` bit (a childless
    // `BreakStatementHandle`); the label identifier is the token after the
    // `break`/`continue` keyword, and a synthetic recovery token never
    // names a label.
    if c.field(node, PayloadField::HasTarget).unwrap_or(0) != 0 {
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
