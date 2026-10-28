use super::{Check, NodeKind, RuleSpec, query_site, slice};

/// Official: `pkg/linter/lib/src/rules/prefer_is_not_empty.dart:31-63` —
/// `!x.isEmpty` reports at the prefix expression.
///
/// The element checks (:57-62 — the property resolves to `isEmpty` and
/// its interface declares `isNotEmpty`) read through the literal faces
/// plus the bridge member set when mounted: every core family carrying
/// `isEmpty` also declares `isNotEmpty`, and a receiver whose resolved
/// type declares `isEmpty` without `isNotEmpty` stays silent (the
/// documented user-class over-report closes); an unresolvable receiver
/// keeps the documented presumption. A positional argument list clips
/// the operand run at the argument boundaries — a later argument's
/// access no longer leaks into the run (the documented false-fire
/// face). Message: `diagnostic.g.dart` `preferIsNotEmpty`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_is_not_empty",
    &[NodeKind::UnaryPrefix],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `prefix.type != BANG` bails (:44-46); the operator is the width-0
    // kind's span head.
    if c.lexeme(c.head_token(node)) != "!" {
        return;
    }
    // The `!` is a childless prefix leaf emitted before its operand, so
    // the operand rows precede it as owner siblings — the receiver's
    // tear-off pieces and the trailing access marker alike. A
    // parenthesized-condition container leaks its expression pieces one
    // level up, though: the bang then opens the container (no rows
    // before it inside) and the run reads beside the container row at
    // the container's parent.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let bang_end = c.byte_span(node).end;
    let mut kids = c.collect_direct(parent);
    // A positional argument list carries no separators: the operand
    // run is the bang's own argument — clip the sibling pieces at the
    // argument boundaries (the documented false-fire face, a later
    // argument's access leaking into the run).
    if c.kind(parent) == NodeKind::Arguments {
        let raw = kids.clone();
        let ranges = slice::arguments(c, &raw);
        if let Some(pos) = raw.iter().position(|&kid| kid == node) {
            let clip = ranges
                .iter()
                .find(|range| range.contains(&pos))
                .or_else(|| ranges.iter().find(|range| range.start >= pos));
            if let Some(range) = clip {
                kids = raw[range.clone()].to_vec();
            }
        }
    }
    let run: Vec<u32> = match kids.iter().position(|&kid| kid == node) {
        Some(pos) if pos > 0 => kids[..pos].to_vec(),
        _ => {
            let Some(grand) = c.ancestor(2) else {
                return;
            };
            let owner_kids = c.collect_direct(grand);
            let Some(container_pos) = owner_kids.iter().position(|&kid| kid == parent) else {
                return;
            };
            owner_kids[..container_pos].to_vec()
        }
    };
    let mut pieces: Vec<u32> = run
        .iter()
        .copied()
        .filter(|&piece| c.byte_span(piece).start >= bang_end)
        .collect();
    // `node.operand.unParenthesized` (:50-55): a parenthesized operand
    // carries its access chain inside the ParenthesizedExpression row —
    // unwrap until the access marker appears, the outermost container
    // bounding the report (the official `reportAtNode(node)` spans the
    // whole prefix expression, closing parens included). A receiver-side
    // paren (`!(a.b).isEmpty`) keeps its marker in the run and never
    // unwraps.
    let mut operand_end = None;
    loop {
        if pieces.iter().any(|&p| c.kind(p) == NodeKind::DotAccess) {
            break;
        }
        let Some(pos) = pieces
            .iter()
            .rposition(|&p| c.kind(p) == NodeKind::ParenthesizedExpression)
        else {
            return;
        };
        if operand_end.is_none() {
            operand_end = Some(c.utf16_span(pieces[pos]).end);
        }
        let inner = c.collect_direct(pieces[pos]);
        pieces.splice(pos..pos + 1, inner);
    }
    // `node.operand.unParenthesized` is a property access (:50-55): the
    // operand's tail is its access marker (`!(a || b)` carries none).
    let Some(marker_index) = pieces
        .iter()
        .rposition(|&p| c.kind(p) == NodeKind::DotAccess)
    else {
        return;
    };
    let marker = pieces[marker_index];
    let chain = &pieces[..=marker_index];
    // The property name is `isEmpty` (:55).
    let Some(name) = slice::access_name_index(c, chain, marker) else {
        return;
    };
    if c.lexeme(c.head_token(chain[name])) != "isEmpty" {
        return;
    }
    // The element checks (:57-62): the property resolves to `isEmpty`
    // and its interface declares `isNotEmpty`. The literal faces
    // decide closed (every core family carrying `isEmpty` declares
    // `isNotEmpty`; a non-family literal resolves nothing and stays
    // silent); a resolved member face decides the member set — a
    // receiver type with `isEmpty` but no `isNotEmpty` stays silent
    // (the documented user-class over-report closes); an
    // unresolvable receiver keeps the documented presumption.
    if let Some(faces) = slice::receiver_faces(c, chain, name) {
        if faces & (slice::ITERABLE | slice::MAP | slice::STRING) == 0 {
            return;
        }
    } else if let Some(analysis) = c.semantics()
        && let Some(receiver) = slice::bare_receiver(c, chain, name)
    {
        let strings = analysis.strings();
        let receiver_site = query_site(c, receiver);
        if analysis
            .member_of(receiver_site, strings.intern("isEmpty"))
            .is_some()
            && analysis
                .member_of(receiver_site, strings.intern("isNotEmpty"))
                .is_none()
        {
            return;
        }
    }
    // The official reports at the prefix expression — from the `!`
    // through the operand's end: the member name on the plain shape (the
    // completion marker's span trails one token past the access — the
    // lazy rows anchor at the following token — so the name row bounds
    // the end), the outermost closing paren on the parenthesized shape.
    let start = c.utf16_span(node).start;
    let end = match operand_end {
        Some(end) => end,
        None => c.utf16_span(chain[name]).end,
    };
    c.report(start..end, &[]);
}
