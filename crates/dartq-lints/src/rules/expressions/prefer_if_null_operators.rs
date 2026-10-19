use super::{Check, Group, NodeKind, RuleSpec, slice, token_seq_equal};

/// Official: `pkg/linter/lib/src/rules/prefer_if_null_operators.dart:22-63`
/// — `x == null ? a : x` (or the `!=`/then mirror) reports at the
/// conditional when the null-check operand and the non-null arm have the
/// same token-lexeme sequence (the official `toString()` comparison).
///
/// dartq faces over the production tiling: the arms are the direct
/// children's head-token segments split at the `?`/`:` payload
/// separators, and the condition is the null-check binary recovered
/// from the owner window — the parser opens the Conditional only at the
/// `?`, after the condition completed, so the condition rows sit
/// immediately before the Conditional's slot and the binary's own
/// window holds just the operands. The operand run is the non-null
/// side's pieces — the binary's adopted subtree plus the non-adopted
/// pieces below it — with the bare-name group's marker rows dropped
/// for the comparison. Message: `diagnostic.g.dart`
/// `preferIfNullOperators`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_if_null_operators",
    group: Group::Recommended,
    problem: "Use the '??' operator rather than '?:' when testing for 'null'.",
    correction: "Try rewriting the code to use '??'.",
    kinds: &[NodeKind::Conditional],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some((op, _binary, operand, condition, arms)) = null_check_parts(c, node) else {
        return;
    };
    // `==` tests the else arm, `!=` the then arm (:47-49).
    let arm = if op == "==" { arms.1 } else { arms.0 };
    if runs_token_equal(c, &arm, &operand) {
        // The official reports at the conditional expression; the
        // production row opens at the `?`, so the condition's first
        // token bounds the start — the operand runs' earliest head,
        // never the binary's delta-tile anchor (a member operand's
        // covered span opens at the selector). The node's covered end
        // can swallow the enclosing terminator (a send group or access
        // marker rides `;`/`)` …), so the end steps back over
        // separators.
        let Some(start) = slice::run_utf16_start(c, &condition) else {
            return;
        };
        let range = start..conditional_end(c, node);
        c.report(range, &[]);
    }
}

/// The `(operator, condition binary, non-null operand run, condition
/// run, (then, else))` of a null-check conditional, or `None`. The runs
/// are unfiltered piece runs — the comparison helpers ([`real_rows`] /
/// [`runs_token_equal`]) drop the marker pieces. The condition run is
/// the binary's two operand runs together (both null-check sides).
pub(super) fn null_check_parts<'a>(
    c: &'a Check<'a>,
    node: u32,
) -> Option<(&'a str, u32, Vec<u32>, Vec<u32>, (Vec<u32>, Vec<u32>))> {
    let payload = c.payload(node);
    let &question = payload.first()?;
    let &colon = payload.get(1)?;
    // The arms over the direct children, split at the `?`/`:` payload
    // separators by head token; rows left of `?` exist only in
    // hand-built trees (the condition among the node's children). The
    // `:` boundary is inclusive: the then arm's stragglers (a send
    // covering `[b, :]` or a `NoTypeArguments` wrapper anchored at the
    // colon itself) head at or before the colon while the else's first
    // piece heads strictly after it.
    let mut condition_rows = Vec::new();
    let mut then_run = Vec::new();
    let mut else_run = Vec::new();
    for row in c.collect_direct(node) {
        let head = c.head_token(row);
        if head < question {
            condition_rows.push(row);
        } else if head <= colon {
            then_run.push(row);
        } else {
            else_run.push(row);
        }
    }
    // The condition's null-check binary: the last row completed before
    // the Conditional's slot — among the node's children (hand-built
    // trees) or immediately before the node in the owner window
    // (production parses).
    let (binary, below) = match condition_rows.pop() {
        Some(binary) => (binary, condition_rows),
        None => {
            let parent = c.ancestor(1)?;
            let kids = c.collect_direct(parent);
            let pos = kids.iter().position(|&kid| kid == node)?;
            let before = pos.checked_sub(1)?;
            (kids[before], kids[..before].to_vec())
        }
    };
    if c.kind(binary) != NodeKind::BinaryExpression {
        return None;
    }
    let op_token = c.payload_word(binary, 0)?;
    let op = c.lexeme(op_token);
    if op != "==" && op != "!=" {
        return None;
    }
    // The operands: the binary-window rows split at the operator token.
    // The left run keeps the whole adopted segment — a container
    // operand's pieces AND its own row seal before the operator, so a
    // `split_first` root would take the container's first interior row
    // and drop the container row (the `(a) == null` misfire and the
    // `a as int == null` miss). The owner-side pieces below the adopted
    // window (a bare name's `Identifier` row, a member operand's
    // receiver and selector pieces — the binary's covered span opens
    // at the selector, so a `head >= binary_head` bound would drop the
    // receiver) join statement-run bounded, then token-adjacency
    // bounded ([`slice::contiguous_tail`]: the operand tiles
    // contiguously back to the operator, cutting earlier expression
    // siblings while statement rows stay cut by the statement run).
    let window = c.collect_direct(binary);
    let window_start = c.children(binary).start;
    let run: Vec<u32> = crate::rules::slice::statement_run(c, &below)
        .into_iter()
        .filter(|&row| row < window_start)
        .collect();
    let kept = crate::rules::slice::contiguous_tail(c, &run, op_token);
    let left: Vec<u32> = kept
        .iter()
        .copied()
        .chain(
            window
                .iter()
                .copied()
                .filter(|&row| c.head_token(row) < op_token),
        )
        .collect();
    let right: Vec<u32> = window
        .iter()
        .copied()
        .filter(|&row| c.head_token(row) >= op_token)
        .collect();
    // Upstream order (:39-45): the left `null` literal takes the right
    // operand, else the right `null` literal takes the left operand.
    // The gate reads each run's root row — the run tail, since a piece
    // run seals root-last — so a wrapped literal keeps its container
    // row (`(null) == x` never reads as a null test) while the
    // bare-name group's markers riding the operator stay harmless.
    let lhs_null = left
        .last()
        .is_some_and(|&row| c.kind(row) == NodeKind::LiteralNull);
    let rhs_null = right
        .last()
        .is_some_and(|&row| c.kind(row) == NodeKind::LiteralNull);
    // The condition run keeps both sides — the callers anchor report
    // starts and comparison windows at its earliest head token.
    let mut condition = left.clone();
    condition.extend_from_slice(&right);
    let operand = if lhs_null {
        right
    } else if rhs_null {
        left
    } else {
        return None;
    };
    Some((op, binary, operand, condition, (then_run, else_run)))
}

/// The run's comparison rows: the bare-name send group's marker pieces
/// dropped (`NoTypeArguments` anchors at the follower token,
/// `NoArguments` is zero-width, the group's `Send` rides its follower),
/// the real expression rows kept in order.
pub(super) fn real_rows(c: &Check<'_>, run: &[u32]) -> Vec<u32> {
    run.iter()
        .copied()
        .filter(|&row| {
            !matches!(
                c.kind(row),
                NodeKind::NoTypeArguments | NodeKind::NoArguments | NodeKind::Send
            )
        })
        .collect()
}

/// Whether two piece runs carry the same token-lexeme sequence — the
/// anchored stand-in for the official `toString()` comparisons — over
/// the runs' comparison rows ([`real_rows`]). A `DotAccess`/
/// `CascadeAccess` marker's covered range swallows its follower token
/// (`.b` covers `[b, next-operator]`), so it contributes only its own
/// pair — the operator token and the selector (`payload[0]`,
/// `payload[0]+1`).
pub(super) fn runs_token_equal(c: &Check<'_>, a: &[u32], b: &[u32]) -> bool {
    let (a, b) = (real_rows(c, a), real_rows(c, b));
    a.len() == b.len() && a.iter().zip(&b).all(|(&x, &y)| row_tokens_equal(c, x, y))
}

/// The per-row token compare behind [`runs_token_equal`].
fn row_tokens_equal(c: &Check<'_>, x: u32, y: u32) -> bool {
    if c.kind(x) != c.kind(y) {
        return false;
    }
    // `DotAccess`/`CascadeAccess` cover `[selector, follower]` — the
    // follower rides the covered span but is not the access's own
    // token; `payload[0]`/`payload[0]+1` name the operator and the
    // selector. `Qualified` covers `[receiver, .]` exactly, so it
    // falls through to the whole-range compare.
    if matches!(c.kind(x), NodeKind::DotAccess | NodeKind::CascadeAccess) {
        let own = |row: u32| -> Option<(u32, u32)> {
            let dot = c.payload(row).first().copied()?;
            Some((dot, dot + 1))
        };
        let Some((dx, nx)) = own(x) else { return false };
        let Some((dy, ny)) = own(y) else { return false };
        return c.lexeme(dx) == c.lexeme(dy) && c.lexeme(nx) == c.lexeme(ny);
    }
    token_seq_equal(c, x, y)
}

/// The UTF-16 end of a production `Conditional` row: the else arm's own
/// last token. The covered range swallows enclosing terminators (`;`,
/// `)`, `,`, `]`, `}`, and an outer conditional's or case label's `:`)
/// into the tiling's send groups and access markers, so the covered
/// `end` cannot be read directly — and stepping back over separators
/// would also cross the arm's own closer (`f(g() == null ? a : g())`
/// ends `))`, the inner `)` belonging to the arm). The bound is
/// therefore structural: the else run's last real row's own end token —
/// an access marker ends at its selector (`payload[0] + 1`), every
/// other real row at its range end.
pub(super) fn conditional_end(c: &Check<'_>, node: u32) -> u32 {
    let Some(&colon) = c.payload(node).get(1) else {
        return c.utf16_span(node).end;
    };
    let last = c
        .collect_direct(node)
        .into_iter()
        .filter(|&row| c.head_token(row) > colon)
        .filter(|&row| {
            !matches!(
                c.kind(row),
                NodeKind::NoTypeArguments | NodeKind::NoArguments | NodeKind::Send
            )
        })
        .last();
    let Some(last) = last else {
        return c.utf16_span(node).end;
    };
    let token = if matches!(c.kind(last), NodeKind::DotAccess | NodeKind::CascadeAccess) {
        c.payload(last)
            .first()
            .map(|&dot| dot + 1)
            .unwrap_or_else(|| c.unit.ast.token_range(last).end - 1)
    } else {
        c.unit.ast.token_range(last).end - 1
    };
    crate::rules::utf16_token_span(c, token).end
}

/// Whether the run is exactly the `null` literal (the upstream
/// `thenExpression is NullLiteral` / `elseExpression is NullLiteral`
/// gates). Zero-width send-group wrappers (`NoArguments` anchored at
/// the arm's first token) can tile beside the literal, so the check
/// reads the comparison rows.
pub(super) fn is_null_literal_run(c: &Check<'_>, run: &[u32]) -> bool {
    matches!(real_rows(c, run).as_slice(), [only] if c.kind(*only) == NodeKind::LiteralNull)
}
