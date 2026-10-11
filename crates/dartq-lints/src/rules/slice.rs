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

/// The adopted-begin operator container's (`BinaryExpression`,
/// `Assignment`) operand runs. The container flattens its operand subtrees
/// into its own completion window, but a member-access operand's pieces stay
/// owner children — the adoption takes the childless trailing `DotAccess`
/// marker alone — so a full run is the owner siblings below the adopted
/// window plus the container's window rows. The split is the operator token.
/// Returns `(left, right)`.
///
/// A left member-access operand finalizes lazily: its trailing
/// `Send` + `DotAccess` completion pair fills inside the adopted window,
/// after the operator wrappers, so both rows straddle the operator byte and
/// the byte split strands them on the right. The marker's dot token anchors
/// the access left of the operator, so the pair is pulled back to the left
/// run.
///
/// A parenthesized-condition container leaks its expression pieces one level
/// up (the operator container rides inside it), so a parent run starved of
/// rows below the adopted window reads beside the container at the
/// container's parent; rows filled inside the adopted window keep the
/// `kid < window.start` bound from duplicating across both sources.
///
/// Divergence (documented): when the parent holds earlier expression
/// siblings (argument lists), their subtree roots fall below the adopted
/// window and would join the left run; every current caller's parent owns a
/// single expression (statement / initializer), so the run is exact there.
pub(crate) fn binary_operands(c: &Check<'_>) -> Option<(Vec<u32>, Vec<u32>)> {
    let node = c.node();
    let op_token = c.payload(node).first().copied()?;
    let op_start = c.unit.tokens.get(op_token as usize)?.byte_offset as usize;
    let window = c.children(node);
    let parent = c.ancestor(1)?;
    let kids = c.collect_direct(parent);
    let below: Vec<u32> = match kids.iter().position(|&kid| kid == node) {
        Some(pos) if pos > 0 => kids[..pos].to_vec(),
        _ => {
            let grand = c.ancestor(2)?;
            let owner_kids = c.collect_direct(grand);
            let Some(container_pos) = owner_kids.iter().position(|&kid| kid == parent) else {
                return None;
            };
            owner_kids[..container_pos].to_vec()
        }
    };
    let mut rows: Vec<u32> = below
        .into_iter()
        .filter(|&kid| kid < window.start)
        .collect();
    rows.extend(window.start..window.end);
    let split = rows.partition_point(|&piece| c.byte_span(piece).end <= op_start);
    if split == 0 || split == rows.len() {
        return None;
    }
    let mut left = rows[..split].to_vec();
    let mut right = rows[split..].to_vec();
    // The completion pair fills adjacently (`Send` then `DotAccess`) and
    // its row ids sit above every left row, so the extraction keeps both
    // runs ascending.
    if let Some(marker) = right.iter().position(|&piece| {
        c.kind(piece) == NodeKind::DotAccess
            && c.payload(piece).first().is_some_and(|&dot| {
                c.unit
                    .tokens
                    .get(dot as usize)
                    .is_some_and(|t| (t.byte_offset as usize) < op_start)
            })
    }) && marker >= 1
        && c.kind(right[marker - 1]) == NodeKind::Send
    {
        left.extend(right.drain(marker - 1..=marker));
    }
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

/// The invocation send over the owner slice with the send at `pos`: the
/// argument list sits immediately before it and the callee head before
/// that, skipping the type-argument wrapper. `arguments` is `None` for a
/// `NoArguments` tear-off send. Returns `(head, arguments)`.
pub(crate) fn send_parts(c: &Check<'_>, kids: &[u32], pos: usize) -> Option<(usize, Option<u32>)> {
    let arguments = match kids.get(pos.checked_sub(1)?)? {
        &args if matches!(c.kind(args), NodeKind::Arguments | NodeKind::NoArguments) => {
            (c.kind(args) == NodeKind::Arguments).then_some(args)
        }
        _ => return None,
    };
    let mut head = pos.checked_sub(2)?;
    loop {
        match c.kind(kids[head]) {
            // The callee's type-argument wrapper sits before the selector.
            NodeKind::TypeArguments | NodeKind::NoTypeArguments => {
                head = head.checked_sub(1)?;
            }
            NodeKind::Identifier => break,
            // A childless argument leaf tiles at the owner level between
            // the callee wrappers and the argument list; keep walking to
            // the selector identifier (the callers validate its lexeme).
            _ => head = head.checked_sub(1)?,
        }
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
        (
            c.kind(*name),
            c.kind(*tyargs),
            c.kind(*no_args),
            c.kind(*send)
        ),
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
    pieces[..marker]
        .iter()
        .rposition(|&p| c.kind(p) == NodeKind::Identifier && c.head_token(p) + 2 == value_head)
}

// —— the shared receiver-type face ————————————————————————————————————————

/// dart:core interface bits for the receiver-type checks the rules share:
/// `prefer_contains` (:145-149 `implementsAnyInterface([Iterable, String])`),
/// `prefer_is_empty` (:229-234 `implementsInterface(Iterable/Map/String)`),
/// `prefer_is_not_empty` (:57-62 an interface declaring `isNotEmpty`),
/// `avoid_function_literals_in_foreach_calls` (:40-45
/// `implementsInterface(Iterable)`), `prefer_iterable_whereType` (:41-49 the
/// same), and `prefer_interpolation_to_compose_strings` (:74-85
/// `isDartCoreString`).
pub(crate) const ITERABLE: u8 = 1;
pub(crate) const STRING: u8 = 2;
pub(crate) const MAP: u8 = 4;

/// The core faces the receiver expression's syntax proves: the literal
/// shapes, through the transparent `(...)`/`const` wrappers. `None` = the
/// syntax carries no verdict (a name, a chain) — every rule then keeps its
/// documented presumption (the receiver qualifies), which matches the
/// official verdict for the real-code receivers (a `List` variable) and
/// over-reports only for user classes shadowing the core members. A
/// non-collection literal is provably outside the family (`Some(0)` — the
/// closed divergence face, e.g. `(1,).length >= 1`).
pub(crate) fn literal_faces(c: &Check<'_>, expression: u32) -> Option<u8> {
    let mut current = expression;
    loop {
        match c.kind(current) {
            NodeKind::ParenthesizedExpression
            | NodeKind::ConstExpression
            | NodeKind::ConstLiteral => {
                // The paren content is the window's last row: wrapper leaves
                // may leak ahead of it inside the container.
                let window = c.children(current);
                if window.start >= window.end {
                    return None;
                }
                current = window.end - 1;
            }
            NodeKind::StringLiteral => return Some(STRING),
            NodeKind::ListLiteral => return Some(ITERABLE),
            // A set-or-map literal decides through its entries: a `MapEntry`
            // child is the map face, anything else the set face (which
            // implements `Iterable`). The empty `{}` reads as a set — a
            // documented corner (the official resolves it through context).
            NodeKind::SetOrMapLiteral => {
                // The literal is childless in the common production tiling:
                // its entries leak as parent-level siblings around it. The
                // decision scans every row inside the literal's own token
                // extent — its completion window (container tiling) and the
                // surrounding sibling run — for a `MapEntry`.
                let literal_span = c.byte_span(current);
                let mut map = (c.children(current).start..c.children(current).end)
                    .any(|row| c.kind(row) == NodeKind::MapEntry);
                if !map && let Some(parent) = c.unit.ast.parent(current) {
                    let window = c.children(parent);
                    map = (window.start..window.end).any(|row| {
                        row != current && c.kind(row) == NodeKind::MapEntry && {
                            let s = c.byte_span(row);
                            s.start >= literal_span.start && s.end <= literal_span.end
                        }
                    });
                }
                return Some(if map { MAP } else { ITERABLE });
            }
            NodeKind::LiteralInt
            | NodeKind::LiteralDouble
            | NodeKind::LiteralBool
            | NodeKind::LiteralNull
            | NodeKind::RecordLiteral
            | NodeKind::SymbolLiteral => return Some(0),
            _ => return None,
        }
    }
}

/// The receiver face over an owner slice whose member-name piece sits at
/// `name`: the selector's type-argument wrapper sits between the receiver
/// run and the name, so it is skipped back; a bare-identifier receiver's
/// tear-off group ending at the tail piece is opaque; anything else decides
/// through [`literal_faces`] at the tail piece (a parenthesized literal's
/// transparent wrapper row).
pub(crate) fn receiver_faces(c: &Check<'_>, pieces: &[u32], name: usize) -> Option<u8> {
    let mut i = name.checked_sub(1)?;
    while matches!(
        c.kind(pieces[i]),
        NodeKind::NoTypeArguments | NodeKind::TypeArguments
    ) {
        i = i.checked_sub(1)?;
    }
    if i >= 3 && is_tear_off_group(c, pieces, i - 3) {
        return None;
    }
    // A literal receiver's outermost row is its send leaf; the literal
    // itself is the owner sibling before the send's type-argument and
    // empty-argument wrappers (both width-0 at the member name).
    let mut face_row = pieces[i];
    if c.kind(face_row) == NodeKind::Send {
        let mut j = i.checked_sub(1)?;
        while matches!(
            c.kind(pieces[j]),
            NodeKind::NoTypeArguments | NodeKind::TypeArguments | NodeKind::NoArguments
        ) {
            j = j.checked_sub(1)?;
        }
        face_row = pieces[j];
    }
    literal_faces(c, face_row)
}

/// [`receiver_faces`] as the rule query: an opaque receiver keeps the rule's
/// presumption.
pub(crate) fn receiver_implements_any(
    c: &Check<'_>,
    pieces: &[u32],
    name: usize,
    required: u8,
) -> bool {
    receiver_faces(c, pieces, name).is_none_or(|faces| faces & required != 0)
}

/// The index of the member-name piece a trailing access marker names: an
/// invocation's selector identifier shares the marker's head token; a bare
/// property access has no selector row (the member name lives only on the
/// marker itself), so the marker's own position stands in and the receiver
/// query reads the piece directly before it.
pub(crate) fn access_name_index(c: &Check<'_>, pieces: &[u32], marker: u32) -> Option<usize> {
    let head = c.head_token(marker);
    pieces
        .iter()
        .position(|&p| c.kind(p) == NodeKind::Identifier && c.head_token(p) == head)
        .or_else(|| pieces.iter().position(|&p| p == marker))
}

/// The arguments over an argument list's flattened pieces: leading callee
/// type-argument wrappers, then arguments in order — a bare tear-off wraps
/// in the four-piece group and counts once, a named argument flattens as
/// `name value… NamedArgument` and counts once. A childless container
/// argument's interior rows (list/set/map/record literal element rows) tile
/// beside its root, and the root can lead (interior rows leak behind it) or
/// trail (containers complete after their children) the run: following
/// pieces inside the head piece's span join it, and when none do, the run
/// groups under the first later piece containing the head piece's span.
/// Sibling subtrees never contain each other's spans, so a later
/// containment is the head's own ancestor; chain pieces (`f(a.b)`) find
/// neither and stay piece-wise. Each argument is the half-open piece range
/// it occupies (a named group includes its name and value pieces; the
/// marker is the last piece).
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
            let head_span = c.byte_span(pieces[i]);
            let mut end = i + 1;
            while pieces.get(end).is_some_and(|&piece| {
                let span = c.byte_span(piece);
                span.start >= head_span.start && span.end <= head_span.end
            }) {
                end += 1;
            }
            if end == i + 1 {
                if let Some(offset) = pieces[i + 1..].iter().position(|&piece| {
                    let span = c.byte_span(piece);
                    span.start <= head_span.start && head_span.end <= span.end
                }) {
                    end = i + offset + 2;
                }
            }
            args.push(i..end);
            i = end;
        }
    }
    args
}
