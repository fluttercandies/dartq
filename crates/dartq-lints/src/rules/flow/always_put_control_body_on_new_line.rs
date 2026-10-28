use super::{Check, NodeKind, PayloadField, RuleSpec, first_child, last_child};

/// Official: `pkg/linter/lib/src/rules/always_put_control_body_on_new_line.dart:30-78`
/// — a control structure's body statement on the same line as the
/// control structure reports at the body's first token. Faces (:38-53):
/// the `do` body (compared against the `do` keyword's end), the
/// `for`/`while` bodies and the `if` then-branch (against the closing
/// `)`), and the `else` branch when it is not another `if` (against the
/// `else` keyword's end). An empty block body skips (:56).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "always_put_control_body_on_new_line",
    &[
        NodeKind::IfStatement,
        NodeKind::WhileStatement,
        NodeKind::ForStatement,
        NodeKind::ForIn,
        NodeKind::DoWhileStatement,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::IfStatement => {
            // The then-branch is the direct child after the condition
            // (the `IfStatement` layout).
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
                .field(node, PayloadField::ElseKeyword)
                .filter(|&kw| kw != super::NONE_TOKEN)
            else {
                return;
            };
            if let Some(else_stmt) = last_child(c, node)
                && c.kind(else_stmt) != NodeKind::IfStatement
            {
                check_body(c, else_stmt, token_end(c, else_kw));
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
                check_body(c, body, token_end(c, c.head_token(node)));
            }
        }
        _ => {}
    }
}

/// The end offset of the token preceding the body — the control
/// structure's closing `)`.
fn control_end_before(c: &Check<'_>, body: u32) -> Option<u32> {
    let head = c.head_token(body);
    (head > 0).then(|| token_end(c, head - 1)).flatten()
}

fn token_end(c: &Check<'_>, token: u32) -> Option<u32> {
    c.utf16_end_of_token(token)
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
    let Some(body_offset) = c.utf16_of_token(c.head_token(first)) else {
        return;
    };
    let same_line = |offset: u32| c.unit.lines.line_col_utf16(c.unit.source, offset).0;
    if same_line(control_end) == same_line(body_offset) {
        c.report(crate::rules::utf16_token_span(c, c.head_token(body)), &[]);
    }
}
