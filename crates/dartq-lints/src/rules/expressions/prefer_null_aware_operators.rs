use super::{Check, NodeKind, RuleSpec, slice};

/// Official: `pkg/linter/lib/src/rules/prefer_null_aware_operators.dart:22-76`
/// — `a == null ? null : a.b` (`!=` mirrors to the then arm) reports when
/// peeling accesses off the result expression reaches the null-check
/// operand. Message: `diagnostic.g.dart` `preferNullAwareOperators`.
///
/// dartq faces over the production tiling: the shape gate rides
/// [`super::prefer_if_null_operators::null_check_parts`] (the arm
/// segments split at the `?`/`:` payload separators, the condition
/// recovered from the owner window), the result arm peels one access
/// per step — a trailing access marker (`a.b`, `a?.b`), a `!` postfix
/// (`a!`), or an invocation's `Send` anchor with its argument suffix
/// (the `MethodInvocation.target` receiver behind the callee's last
/// dot; a bare-callee `f()` invocation has no target and never peels)
/// — and each peeled run compares to the operand with the
/// marker-filtered token-lexeme walk (the anchored `toString()` form).
/// The bare `a == null ? null : a` form never matches (the comparison
/// runs only after a peel).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_null_aware_operators",
    &[NodeKind::Conditional],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some((op, _binary, operand, condition, arms)) =
        super::prefer_if_null_operators::null_check_parts(c, node)
    else {
        return;
    };
    // `x == null ? null : y` / `x != null ? y : null` (:45-58).
    let result = match op {
        "==" if super::prefer_if_null_operators::is_null_literal_run(c, &arms.0) => arms.1,
        "!=" if super::prefer_if_null_operators::is_null_literal_run(c, &arms.1) => arms.0,
        _ => return,
    };
    // Peel one access at a time; compare only after a peel (:60-77 —
    // the bare `a == null ? null : a` form never matches).
    let mut current = result;
    while let Some(stripped) = peel_step(c, &current) {
        if super::prefer_if_null_operators::runs_token_equal(c, &stripped, &operand) {
            // The official reports at the conditional expression; the
            // production row opens at the `?`, so the condition's
            // first token bounds the start — the operand runs'
            // earliest head, never the binary's delta-tile anchor.
            let Some(start) = slice::run_utf16_start(c, &condition) else {
                return;
            };
            let range = start..super::prefer_if_null_operators::conditional_end(c, node);
            c.report(range, &[]);
            return;
        }
        current = stripped;
    }
}

/// One peel step (the official switch, :63-72): the rows behind a
/// trailing access marker, the operand rows behind a `!` postfix, or
/// the invocation target behind an invocation send's callee dot.
fn peel_step(c: &Check<'_>, run: &[u32]) -> Option<Vec<u32>> {
    let (&last, stripped) = run.split_last()?;
    match c.kind(last) {
        // `a.b`, `a?.b` → the receiver rows: the marker comes last but
        // its selector's send group (`b` + wrappers + `Send`) tiles
        // immediately behind it — `a.b` peels to `a`, not `a` + the
        // orphan `b` group. `a!` and `a.b!` strip the bang alone (the
        // operand is the whole prefix).
        NodeKind::DotAccess | NodeKind::Qualified => {
            // Selector send group trailing the marker: walk back over
            // `Send`, an optional `Arguments`/`NoArguments`, an optional
            // `TypeArguments`/`NoTypeArguments`, to the selector's
            // `Identifier`. A zero-width `NoArguments` can tile outside
            // the arm's piece run (anchored at the `:` boundary), so the
            // slots walk by kind rather than fixed positions.
            let mut start = stripped.len();
            let prev = |i: usize| stripped.get(i.wrapping_sub(1)).copied();
            let kind = |row: u32| c.kind(row);
            if prev(start).is_some_and(|r| kind(r) == NodeKind::Send) {
                start -= 1;
            }
            if prev(start)
                .is_some_and(|r| matches!(kind(r), NodeKind::Arguments | NodeKind::NoArguments))
            {
                start -= 1;
            }
            if prev(start).is_some_and(|r| {
                matches!(kind(r), NodeKind::TypeArguments | NodeKind::NoTypeArguments)
            }) {
                start -= 1;
            }
            match prev(start).filter(|&r| kind(r) == NodeKind::Identifier) {
                // `a.b` peels to `a` — drop the selector group and the
                // marker. A receiver with no bare selector group
                // (`(x).b`) strips the marker alone.
                Some(_) if start < stripped.len() => Some(stripped[..start - 1].to_vec()),
                _ => Some(stripped.to_vec()),
            }
        }
        NodeKind::NonNullAssert => Some(stripped.to_vec()),
        // `a.b()` → the invocation target behind the callee's last dot;
        // a `NoArguments` tail is the bare-name group, whose bare-callee
        // invocation has no target.
        NodeKind::Send => {
            let mut end = run.len().checked_sub(2)?;
            if c.kind(run[end]) != NodeKind::Arguments {
                return None;
            }
            if let Some(prev) = end.checked_sub(1)
                && matches!(
                    c.kind(run[prev]),
                    NodeKind::NoTypeArguments | NodeKind::TypeArguments
                )
            {
                end = prev;
            }
            match run.get(end.checked_sub(1)?).map(|&row| c.kind(row)) {
                Some(NodeKind::DotAccess | NodeKind::Qualified) => {
                    Some(run[..end - 1].to_vec())
                }
                _ => None,
            }
        }
        _ => None,
    }
}
