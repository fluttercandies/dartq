//! Parent-slice reconstruction over the production invocation tiling.
//!
//! The bridge collapse keeps the send-family handles childless — `Send`,
//! `DotAccess`, `CascadeAccess`, and `NamedArgument` own empty child
//! windows; their pieces tile flat into the enclosing owner's slice
//! (finding #468: every read of `collect_direct` off a send is structurally
//! dead on production parses, while hand-built test trees stay green). The
//! shape was validated on `null_closures`, `unnecessary_this`, and
//! `prefer_collection_literals`; these helpers are the shared form:
//!
//! - owner slice: the dispatch node's parent window and its position;
//! - send parts: the argument list at `pos - 1` (`None` for a `NoArguments`
//!   tear-off), the callee head before that, skipping the type-argument
//!   wrapper;
//! - the bare-identifier tear-off group (`Identifier`,
//!   `NoTypeArguments | TypeArguments`, `NoArguments`, `Send`) — the
//!   production tiling of a bare receiver;
//! - the named-argument re-anchor: the marker is childless and anchors at
//!   the value head, so `name : value` are three consecutive tokens and the
//!   name is the identifier two tokens back.
//!
//! Approximation boundary: an argument whose expression is a member chain
//! (`f(a.b)`) tiles beyond the four-piece group and counts piece-wise; the
//! validated walkers accept that divergence, and so does this module.

use dartq_ast::NodeKind;

use crate::engine::Check;

/// The dispatch node's owner slice: the parent's flattened child window and
/// the node's position in it.
pub(crate) fn owner_slice(c: &Check<'_>) -> Option<(Vec<u32>, usize)> {
    let parent = c.ancestor(1)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == c.node())?;
    Some((kids, pos))
}

/// The invocation send over the owner slice with the send at `pos`: the
/// argument list sits immediately before it and the callee head before
/// that, skipping the type-argument wrapper. `arguments` is `None` for a
/// `NoArguments` tear-off send. Returns `(head, arguments)`.
pub(crate) fn send_parts(
    c: &Check<'_>,
    kids: &[u32],
    pos: usize,
) -> Option<(usize, Option<u32>)> {
    let arguments = match kids.get(pos.checked_sub(1)?)? {
        &args if matches!(c.kind(args), NodeKind::Arguments | NodeKind::NoArguments) => {
            (c.kind(args) == NodeKind::Arguments).then_some(args)
        }
        _ => return None,
    };
    let mut head = pos.checked_sub(2)?;
    while matches!(
        c.kind(kids[head]),
        NodeKind::TypeArguments | NodeKind::NoTypeArguments
    ) {
        head = head.checked_sub(1)?;
    }
    Some((head, arguments))
}

/// Whether a same-span access marker of `kind` trails the send at `pos`
/// (the production tiling of a member receiver: `xs.map(f)` carries a
/// `DotAccess` over the whole access right after the invocation send).
pub(crate) fn access_marker(
    c: &Check<'_>,
    kids: &[u32],
    pos: usize,
    kind: NodeKind,
) -> Option<u32> {
    let &access = kids.get(pos.checked_add(1)?)?;
    (c.kind(access) == kind && c.utf16_span(access) == c.utf16_span(kids[pos])).then_some(access)
}

/// Whether the four pieces starting at `start` are the bare-identifier
/// tear-off group — the production tiling of a bare receiver
/// (`Identifier, NoTypeArguments | TypeArguments, NoArguments, Send`).
pub(crate) fn is_tear_off_group(c: &Check<'_>, pieces: &[u32], start: usize) -> bool {
    let Some(group) = pieces.get(start..start.saturating_add(4)) else {
        return false;
    };
    let [name, tyargs, no_args, send] = group else {
        return false;
    };
    matches!(
        (c.kind(*name), c.kind(*tyargs), c.kind(*no_args), c.kind(*send)),
        (
            NodeKind::Identifier,
            NodeKind::NoTypeArguments | NodeKind::TypeArguments,
            NodeKind::NoArguments,
            NodeKind::Send,
        )
    )
}

/// The name position of the childless named-argument marker at `marker` in
/// `pieces`: the marker anchors at the value head, so the name is the
/// identifier two tokens back.
pub(crate) fn named_arg_name(c: &Check<'_>, pieces: &[u32], marker: usize) -> Option<usize> {
    let value_head = c.head_token(pieces[marker]);
    pieces[..marker].iter().rposition(|&p| {
        c.kind(p) == NodeKind::Identifier && c.head_token(p) + 2 == value_head
    })
}

/// The arguments over an argument list's flattened pieces: leading callee
/// type-argument wrappers, then arguments in order — a bare tear-off wraps
/// in the four-piece group and counts once, a named argument flattens as
/// `name value… NamedArgument` and counts once. Each argument is the half-
/// open piece range it occupies (a named group includes its name and value
/// pieces; the marker is the last piece). Piece granularity matches the
/// validated `null_closures` walker, including its member-chain boundary
/// (`f(a.b)` tiles past the group head and counts piece-wise).
pub(crate) fn arguments(c: &Check<'_>, pieces: &[u32]) -> Vec<core::ops::Range<usize>> {
    // Named groups resolve first: marker → the name two tokens back.
    let named: Vec<(usize, usize)> = (0..pieces.len())
        .filter(|&i| c.kind(pieces[i]) == NodeKind::NamedArgument)
        .filter_map(|i| named_arg_name(c, pieces, i).map(|name| (name, i)))
        .collect();
    let mut args = Vec::new();
    let mut i = 0usize;
    while matches!(
        pieces.get(i),
        Some(&piece)
            if matches!(
                c.kind(piece),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            )
    ) {
        i += 1;
    }
    while i < pieces.len() {
        if let Some(&(name, marker)) = named.iter().find(|&&(name, _)| name == i) {
            args.push(name..marker + 1);
            i = marker + 1;
        } else if is_tear_off_group(c, pieces, i) {
            args.push(i..i + 4);
            i += 4;
        } else {
            args.push(i..i + 1);
            i += 1;
        }
    }
    args
}
