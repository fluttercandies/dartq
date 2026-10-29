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

use dartq_ast::{NodeKind, PayloadField};

use crate::engine::Check;
use crate::registry::RuleSpec;

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// Contract (delta-tile anchors): `token_range(node).start` on a
/// send-family container (`Send`, `Arguments`, `BinaryExpression`,
/// `Assignment`, `DotAccess`, …) is the *delta tile anchor* — the token the
/// row was emitted over, typically the previous segment's last token or the
/// operator token — never the operand head. A `func!()` invocation's send
/// covers `! ()` and opens at `!`; a `p.m != null` binary covers
/// `m != null` and opens at the selector. Consumers needing an operand's
/// head token must rebuild it from the piece run or owner slice (the first
/// operand row's head), never from the container's covered start.
///
/// The dispatch node's owner slice: the parent's flattened child window and
/// the node's position in it.
pub(crate) fn owner_slice(c: &Check<'_>) -> Option<(Vec<u32>, usize)> {
    let parent = c.ancestor(1)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == c.node())?;
    Some((kids, pos))
}

/// The operand run before a statement-level operator container: the pieces
/// after the last statement row (statement tiling trails the container row
/// in the enclosing block, so everything up to it belongs to earlier
/// statements). Runs with no statement row keep every piece (the
/// initializer / argument single-expression parents).
pub(crate) fn statement_run(c: &Check<'_>, run: &[u32]) -> Vec<u32> {
    let start = run
        .iter()
        .rposition(|&row| crate::rules::is_statement_row(c, row))
        .map_or(0, |i| i + 1);
    run[start..].to_vec()
}

/// The operand run bounded at the parent flow statement's body: under a
/// non-block body (an `if` arm, a loop body, a switch-member body) the
/// header fragment rows (the condition group, the `ParenthesizedCondition`,
/// the for-parts leaf, the `case` head row) tile beside the body's rows in
/// the owner window with no statement row to cut at, so the raw run would
/// open at the header. The cut restarts the run after the last flow-header
/// row; block, initializer, and argument parents hold none and keep the run
/// intact.
pub(crate) fn flow_body_run<'a>(c: &Check<'_>, run: &'a [u32]) -> &'a [u32] {
    let cut = run.iter().rposition(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ParenthesizedCondition
                | NodeKind::ForParts
                | NodeKind::ForInParts
                | NodeKind::CaseExpression
        )
    });
    match cut {
        Some(i) => &run[i + 1..],
        None => run,
    }
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
/// window and would join the left run; a statement-run parent is bounded at
/// its last statement row ([`statement_run`]), a flow parent (a non-block
/// `if`/`while`/`for` arm, a switch-member body) restarts after the header
/// fragment rows ([`flow_body_run`]), a `for`/for-in header owner trims the
/// run to the operator-adjacent tail, and an initializer parent owns a
/// single expression, so the run is exact there.
pub(crate) fn binary_operands(c: &Check<'_>) -> Option<(Vec<u32>, Vec<u32>)> {
    binary_operands_at(c, c.node())
}

/// [`binary_operands`] keyed at an explicit operator container row — for
/// the rules that scan a body window for candidate containers whose
/// operands must be read off the production tiling while the dispatch
/// node is elsewhere (`parameter_assignments`'s earlier-`??=` scan).
pub(crate) fn binary_operands_at(c: &Check<'_>, node: u32) -> Option<(Vec<u32>, Vec<u32>)> {
    let op_token = c.field(node, PayloadField::Operator)?;
    let op_start = c.unit.tokens.get(op_token as usize)?.byte_offset as usize;
    let window = c.children(node);
    let parent = c.unit.ast.parent(node)?;
    let kids = c.collect_direct(parent);
    let owner_kids;
    let raw: &[u32] = match kids.iter().position(|&kid| kid == node) {
        Some(pos) if pos > 0 => &kids[..pos],
        _ => {
            let grand = c.unit.ast.parent(parent)?;
            owner_kids = c.collect_direct(grand);
            let container_pos = owner_kids.iter().position(|&kid| kid == parent)?;
            &owner_kids[..container_pos]
        }
    };
    // A classic-`for`/for-in header owner: the parts leaf seals after the
    // whole header, so it sorts after the fragments and never cuts the
    // container's below slice — the fragments (an initializer, a condition)
    // would join the left run. The run trims to the token-adjacent tail at
    // the operator instead: the left operand's rows chain back to it, the
    // fragments sit behind the separating `;`/`in` token. Other owners keep
    // the standard bounds ([`flow_body_run`] + [`statement_run`]).
    let bounded = statement_run(c, flow_body_run(c, raw));
    let below: Vec<u32> = if matches!(c.kind(parent), NodeKind::ForStatement | NodeKind::ForIn) {
        contiguous_tail(c, &bounded, op_token).to_vec()
    } else {
        bounded
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
            && c.field(piece, PayloadField::Operator).is_some_and(|dot| {
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

/// The first token of a piece run's expression: the minimum head token
/// across the run's rows. A run seals in completion order (root last) and
/// a container row's head is its delta-tile anchor — the operator or the
/// previous segment's tail leaf, never the operand head (the contract
/// above) — so neither `run[0]`'s covered start nor a container row's
/// `token_range().start` answers where the expression opens. Report starts
/// and operand-window anchors go through this helper.
pub(crate) fn run_start_token(c: &Check<'_>, run: &[u32]) -> Option<u32> {
    run.iter().map(|&row| c.head_token(row)).min()
}

/// The trailing rows of `run` token-adjacent to `floor`: walking backward,
/// a row stays while its covered range still reaches the accumulated
/// floor (the floor lowers to each kept row's start). An operand tiles
/// contiguously back to the operator that adopts it, so the walk cuts
/// earlier expression siblings (an argument list's leading `0` ends at
/// its `,`) — though a sibling's own send group can swallow the
/// separating `,` (`f(g, x == null ? …)`) and read as contiguous, a
/// documented under-report residual.
pub(crate) fn contiguous_tail<'a>(c: &Check<'_>, run: &'a [u32], floor: u32) -> &'a [u32] {
    let mut floor = floor;
    let kept = run
        .iter()
        .rev()
        .take_while(|&&row| {
            let range = c.unit.ast.token_range(row);
            let joins = range.end >= floor;
            if joins {
                floor = range.start.min(floor);
            }
            joins
        })
        .count();
    &run[run.len() - kept..]
}

/// The UTF-16 report start of a piece run — [`run_start_token`] resolved
/// to its span offset.
pub(crate) fn run_utf16_start(c: &Check<'_>, run: &[u32]) -> Option<u32> {
    Some(crate::rules::utf16_token_span(c, run_start_token(c, run)?).start)
}

/// The report start of an operator container's operand run: the
/// [`contiguous_tail`] at the operator token bounds the run first — the
/// owner-side rows below the adopted window can carry an enclosing
/// statement's or declaration's strays, and only the token-adjacent tail
/// is the operand — then the earliest head token answers the operand's
/// first token.
pub(crate) fn operand_start_token(c: &Check<'_>, run: &[u32], op_token: u32) -> Option<u32> {
    run_start_token(c, contiguous_tail(c, run, op_token))
}

/// The UTF-16 report start of an operator container's operand run —
/// [`operand_start_token`] resolved to its span offset.
pub(crate) fn operand_utf16_start(c: &Check<'_>, run: &[u32], op_token: u32) -> Option<u32> {
    Some(crate::rules::utf16_token_span(c, operand_start_token(c, run, op_token)?).start)
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
    // The argument list's own window: pieces tiling inside it (a forwarded
    // parameter's send group) belong to the argument content, not the
    // callee — bound the selector walk at the list's start.
    let args_start = arguments.map(|row| c.byte_span(row).start);
    let mut head = pos.checked_sub(2)?;
    loop {
        let row = kids[head];
        if let Some(start) = args_start
            && c.byte_span(row).start >= start
        {
            head = head.checked_sub(1)?;
            continue;
        }
        match c.kind(row) {
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

/// Whether the row is one of the bare-identifier send group's marker
/// pieces (`NoTypeArguments | TypeArguments`, `NoArguments`, `Send`):
/// `parse_send` emits the full group for every bare name, and the group's
/// pieces tile beside the access-chain rows — the operand-run readers skip
/// them when they look for the `[target, name]` faces.
pub(crate) fn is_send_group_piece(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
            | NodeKind::NoArguments
            | NodeKind::Send
    )
}

/// The access-chain operand run over an owner slice: the pieces minus the
/// childless access markers and the bare-identifier send group's marker
/// pieces ([`is_send_group_piece`]) — what remains for the
/// `[ThisExpression, name]` / `[name]` face matches.
pub(crate) fn access_run(c: &Check<'_>, rows: &[u32]) -> Vec<u32> {
    rows.iter()
        .copied()
        .filter(|&row| {
            !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified)
                && !is_send_group_piece(c, row)
        })
        .collect()
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

/// The comma-separated element segments of a collection literal's interior
/// `rows` (the span-filtered piece run, in completion order). A `,` token
/// inside the literal's own range covered by no piece row is an element
/// separator — an interior comma (a nested literal's or a call's argument
/// comma) sits inside its root's span and never splits. Each segment holds
/// one element's pieces with its root last; a `MapEntry` marker trails its
/// key/value pieces inside a map literal's segment.
pub(crate) fn comma_segments(c: &Check<'_>, literal: u32, rows: &[u32]) -> Vec<Vec<u32>> {
    let tokens = c.unit.ast.token_range(literal);
    let mut separators = Vec::new();
    for token in tokens.start..tokens.end {
        if c.lexeme(token) != "," {
            continue;
        }
        let comma = crate::rules::byte_span_of_token(c, token);
        let covered = rows.iter().any(|&row| {
            let piece = c.byte_span(row);
            piece.start <= comma.start && comma.end <= piece.end
        });
        if !covered {
            separators.push(comma.start);
        }
    }
    let mut segments = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut end = 0usize;
    for &row in rows {
        let piece = c.byte_span(row);
        if !current.is_empty()
            && separators
                .iter()
                .any(|&sep| end <= sep && sep < piece.start)
        {
            segments.push(core::mem::take(&mut current));
            end = 0;
        }
        end = end.max(piece.end);
        current.push(row);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
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

/// The core-family bit of a written type name: dart:core's `Iterable`
/// implementers (`Iterable`, `List`, `Set`, and dart:collection's `Queue`,
/// whose unshadowed written spelling resolves there), `String`, and the
/// map family (never in a receiver family — a written `Map` answers `MAP`,
/// which no rule's mask requests).
fn written_family_bits(name: &str) -> u8 {
    match name {
        "Iterable" | "List" | "Set" | "Queue" => ITERABLE,
        "String" => STRING,
        "Map" | "LinkedHashMap" | "LinkedHashSet" => MAP,
        _ => 0,
    }
}

/// The receiver's bare-identifier row when the receiver is a plain name
/// (the tear-off group face [`is_tear_off_group`] proves): `None` for any
/// other receiver shape — a member chain, a call, a literal (literals read
/// through [`receiver_faces`]).
pub(crate) fn bare_receiver(c: &Check<'_>, pieces: &[u32], name: usize) -> Option<u32> {
    let mut i = name.checked_sub(1)?;
    while matches!(
        c.kind(pieces[i]),
        NodeKind::NoTypeArguments | NodeKind::TypeArguments
    ) {
        i = i.checked_sub(1)?;
    }
    if i >= 3 && is_tear_off_group(c, pieces, i - 3) {
        return Some(pieces[i - 3]);
    }
    None
}

/// The resolution-backed receiver verdict the typed-receiver rules share
/// (`prefer_contains` :143-149, `prefer_iterable_whereType` :40-41 — the
/// official reads the receiver's static type and BAILS when it cannot):
/// literal receivers decide closed through [`receiver_faces`]; a
/// bare-identifier receiver resolves its written type over the same-file
/// channel (the innermost binding — locals shadow fields, fields shadow
/// nothing else) and must land on `required`; an unshadowed written family
/// name is the library type. Any other shape — a member chain, an untyped
/// (`var`) binding, an import-only name — carries no verdict and answers
/// `None`, which the caller keeps silent (the official's
/// unresolvable-receiver face).
pub(crate) fn resolved_receiver_family(
    c: &Check<'_>,
    pieces: &[u32],
    name: usize,
    required: u8,
) -> Option<bool> {
    if let Some(faces) = receiver_faces(c, pieces, name) {
        return Some(faces & required != 0);
    }
    let row = bare_receiver(c, pieces, name)?;
    let receiver_name = c.lexeme(c.head_token(row));
    let written = crate::rules::expressions::same_file::written_type(c, row, receiver_name)?;
    if crate::rules::expressions::same_file::declares_name(c, written) {
        // A same-file declaration owns the name — the written family
        // spelling is not the core type.
        return Some(false);
    }
    Some(written_family_bits(written) & required != 0)
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
            if end == i + 1
                && let Some(offset) = pieces[i + 1..].iter().position(|&piece| {
                    let span = c.byte_span(piece);
                    span.start <= head_span.start && head_span.end <= span.end
                })
            {
                end = i + offset + 2;
            }
            args.push(i..end);
            i = end;
        }
    }
    args
}

pub(crate) mod no_self_assignments {
    use super::{Check, NodeKind, PayloadField, RuleSpec};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/no_self_assignments.dart`
    /// — an `=` assignment whose both sides are identifiers with the same
    /// name reports (`_Visitor.visitAssignmentExpression`: `=` only, both
    /// operands `Identifier`, `lhs.name == rhs.name`). The visitor
    /// processes `AssignmentExpression` nodes only, so a constructor field
    /// initializer (`: x = x` — a `ConstructorFieldInitializer` upstream)
    /// never reports. Message: `diagnostic.g.dart` `noSelfAssignments`.
    ///
    /// dartq face: the bare-identifier assignment (`x = x`) — the
    /// [`NodeKind::Assignment`] op payload word is the operator token and
    /// the operand pieces tile flat around the childless-adopted
    /// container ([`super::binary_operands`] + [`super::access_run`]
    /// leaves the semantic rows). The prefixed faces (`a.b = a.b`, which
    /// the official's name-only comparison also flags — even across
    /// receivers) stay silent here: they tile as send groups, not bare
    /// identifiers. Compound assignments read the left operand and are
    /// already excluded by the `=` gate. The constructor-initializer rows
    /// the production tiling does materialize under the
    /// [`NodeKind::FieldInitializer`] wrapper bail on the ancestor — the
    /// official's node-kind gate.
    pub(crate) static SPEC: RuleSpec =
        crate::spec!("no_self_assignments", &[NodeKind::Assignment], check,);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if c.field(node, PayloadField::Operator)
            .is_none_or(|op| c.lexeme(op) != "=")
        {
            return;
        }
        // `ConstructorFieldInitializer` is not an `AssignmentExpression`
        // upstream — the initializer-list write (`: x = x`) never reports.
        // The tiling wraps the write in an `Initializer` row (`FieldInitializer`
        // is the `:` head's own face), so the wrapper on the path is the gate.
        if c.path.iter().any(|&row| {
            matches!(
                c.kind(row),
                NodeKind::Initializer | NodeKind::FieldInitializer
            )
        }) {
            return;
        }
        let Some((left, right)) = super::binary_operands(c) else {
            return;
        };
        let (left, right) = (super::access_run(c, &left), super::access_run(c, &right));
        let ([lhs], [rhs]) = (left.as_slice(), right.as_slice()) else {
            return;
        };
        if c.kind(*lhs) != NodeKind::Identifier || c.kind(*rhs) != NodeKind::Identifier {
            return;
        }
        if c.lexeme(c.head_token(*lhs)) == c.lexeme(c.head_token(*rhs)) {
            // The official reports at the whole `AssignmentExpression` —
            // the range opens at the left operand's head and ends at the
            // right operand's own tail (the container row's seal runs past
            // it when the bare-send tail parses through the follower, so
            // the node's own span is not the bound).
            c.report(c.utf16_span(*lhs).start..c.utf16_span(*rhs).end, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        /// The flow-parent arms: the header fragment rows no longer open the
        /// operand run (the [`flow_body_run`] bound), so a self-assignment
        /// as a non-block `if`/`while` arm reports like the block-body face
        /// (the #1995 wave-1 residual shape `if (c) x = x;`).
        #[test]
        fn reports_under_flow_parent_arms() {
            let f = Fixture::parse("void f(int a) { if (a < 1) a = a; }");
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let write = tok(&f.source, &f.tokens, "a", 2);
            assert_eq!(lints[0].range.start, utf16_at(&f, write));
            // The span ends at the right operand's own tail (oracle span
            // length 5), never at the container's sealed `;`.
            let rhs = tok(&f.source, &f.tokens, "a", 3);
            assert_eq!(lints[0].range.end, utf16_at(&f, rhs) + 1);

            let f = Fixture::parse("void f(int a) { while (a < 1) a = a; }");
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let write = tok(&f.source, &f.tokens, "a", 2);
            assert_eq!(lints[0].range.start, utf16_at(&f, write));
            let rhs = tok(&f.source, &f.tokens, "a", 3);
            assert_eq!(lints[0].range.end, utf16_at(&f, rhs) + 1);
        }

        /// The flow-body arms of the header-fragment cut: a self-assignment
        /// write in a classic-`for` updater — with and without an
        /// initializer preceding it in the header window (the parts leaf
        /// sorts after the fragments, so the header owner drops them by
        /// kind) — reads its operands whole.
        #[test]
        fn reports_in_classic_for_updater_writes() {
            let f = Fixture::parse("void f(int i) {\n  for (;; i = i) {\n    i = 1;\n  }\n}");
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let write = tok(&f.source, &f.tokens, "i", 1);
            assert_eq!(lints[0].range.start, utf16_at(&f, write));
            // The header write ends at its right operand (the sealed row
            // runs past the `)` — the rhs tail is the bound).
            let rhs = tok(&f.source, &f.tokens, "i", 2);
            assert_eq!(lints[0].range.end, utf16_at(&f, rhs) + 1);

            let f = Fixture::parse(
                "void f(int i, bool b) {\n  for (i = 0; b; i = i) {\n    i = 2;\n  }\n}",
            );
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let write = tok(&f.source, &f.tokens, "i", 2);
            assert_eq!(lints[0].range.start, utf16_at(&f, write));
            let rhs = tok(&f.source, &f.tokens, "i", 3);
            assert_eq!(lints[0].range.end, utf16_at(&f, rhs) + 1);
        }

        /// A switch-member body self-assignment reads whole under a
        /// guard-less head, a guarded head, and `default`: the `case` head
        /// row seals before the member opens and [`flow_body_run`] cuts it
        /// from the operand run; `default` owns no head row.
        #[test]
        fn reports_in_case_member_bodies() {
            let f = Fixture::parse(
                "void f(int i, bool b) {\n  switch (i) {\n    case 1:\n      i = i;\n      break;\n    case 2 when b:\n      i = i;\n      break;\n    default:\n      i = i;\n      break;\n  }\n}",
            );
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 3, "{lints:?}");
            for (nth, lint) in lints.iter().enumerate() {
                let write = tok(&f.source, &f.tokens, "i", 2 + nth * 2);
                assert_eq!(lint.range.start, utf16_at(&f, write), "member {nth}");
            }
        }

        /// `test/rules/no_self_assignments_test.dart` `test_withAssignment`.
        #[test]
        fn reports_a_bare_self_assignment() {
            let f = Fixture::parse("void f(int a) { a = a; }");
            let lints = f.run(&one_rule("no_self_assignments"));
            assert_eq!(lints.len(), 1);
            assert_eq!(
                lints[0].message,
                "The variable or property is being assigned to itself."
            );
            assert_eq!(lints[0].range.start, 16);
            // Oracle span length 5: `a = a` — the `;` stays out.
            assert_eq!(lints[0].range.end, 21);
        }

        #[test]
        fn ignores_distinct_names() {
            let f = Fixture::parse("void f(int a, int b) { a = b; }");
            assert!(f.run(&one_rule("no_self_assignments")).is_empty());
        }

        #[test]
        fn ignores_compound_assignment() {
            let f = Fixture::parse("void f(int a) { a += a; }");
            assert!(f.run(&one_rule("no_self_assignments")).is_empty());
        }

        /// The official's `Identifier` gate covers plain names only in the
        /// dartq face: a member-write target tiles as a send group.
        #[test]
        fn ignores_member_target() {
            let f = Fixture::parse("class C { int x = 0; void m() { this.x = x; } }");
            assert!(f.run(&one_rule("no_self_assignments")).is_empty());
        }

        /// A constructor field initializer (`: x = x`) is a
        /// `ConstructorFieldInitializer` upstream — the visitor processes
        /// `AssignmentExpression` nodes only and stays silent (the
        /// `lib/constructor_family.dart` `Point` row).
        #[test]
        fn ignores_a_constructor_field_initializer() {
            let f = Fixture::parse("class Point {\n  int? x;\n  Point(int? x) : x = x;\n}");
            assert!(f.run(&one_rule("no_self_assignments")).is_empty());
        }
    }
}

pub(crate) mod parameter_assignments {
    use super::{Check, NodeKind, PayloadField, RuleSpec};
    use crate::rules::{NONE_TOKEN, initialized_identifier_name};
    use dartq_scanner::TokenKind;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/parameter_assignments.dart` — a write to a
    /// function's or method's own parameter inside its body reports at the
    /// write (`_DeclarationVisitor`: assignment expressions with the
    /// parameter as the bare left identifier, `++`/`--` operands, and
    /// pattern-assignment variable targets). Message: `diagnostic.g.dart`
    /// `parameterAssignments`.
    ///
    /// dartq face: the write faces are the bare-identifier
    /// [`NodeKind::Assignment`] left operand, the [`NodeKind::UnaryPrefix`]
    /// / [`NodeKind::UnaryPostfix`] `++`/`--` operand, and the
    /// [`NodeKind::AssignedVariablePattern`] rows of a
    /// [`NodeKind::PatternAssignment`]. The target parameter is the nearest
    /// enclosing function-like container whose parameter list binds the
    /// lexeme — a bare name falls through nested functions that do not
    /// bind it — and the scope binders on the path (block locals, catch
    /// parameters, loop variables, case pattern variables) keep the write
    /// silent: the name-based stand-in for the official's element
    /// identity. The official walks the body only, so
    /// constructor-initializer writes stay silent. The optional faces
    /// follow `_defaultValueIsNull`: a plain or compound assignment
    /// reports on an optional parameter without a default clause,
    /// `++`/`--` do not, and the FIRST `??=` stays silent — the
    /// null-replacement initializer idiom — while a second `??=` reports:
    /// the official's per-(parameter, body) `hasBeenAssigned` flag
    /// (a fresh visitor per body), read statelessly as an
    /// earlier-completed `??=` write in the binding body's completion
    /// window. The count is name-faced: a same-name `??=` write shadowed
    /// by a nested closure's own binder counts into it (the candidate
    /// residual — no corpus shape today). The
    /// primary-constructor face stays unported. A `_` formal binds nothing
    /// under `wildcard_variables` (Dart 3.7) — writes to `_` stay silent;
    /// pre-3.7 the name binds and reports.
    ///
    /// The arm-statement compensation face ([`statement`]): the production
    /// tiling seals a non-block statement body (an `if` arm, a loop body)
    /// as a childless `ExpressionStatement` leaf whose `Assignment` row
    /// spills beside it. The row face answers through the bounded peel
    /// ([`flow_body_run`]), so this face stays silent on the covered arms
    /// (its probe reads the row face's own operand peel); it remains the
    /// recovery path for any arm shape whose peel still answers nothing.
    /// The leaf's token face answers the bare-identifier `=` write — the
    /// shapes the row covers never double-report.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
        "parameter_assignments",
        &[
            NodeKind::Assignment,
            NodeKind::PatternAssignment,
            NodeKind::UnaryPrefix,
            NodeKind::UnaryPostfix,
            NodeKind::ExpressionStatement,
        ],
        check,
    );

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::Assignment => assignment(c),
            NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => unary(c),
            NodeKind::PatternAssignment => pattern(c),
            NodeKind::ExpressionStatement => statement(c),
            _ => {}
        }
    }

    /// The `=` / compound / `??=` assignment face: the bare left
    /// identifier is the written name.
    fn assignment(c: &mut Check<'_>) {
        let node = c.node();
        let Some(op) = c.field(node, PayloadField::Operator) else {
            return;
        };
        let Some((lhs, name)) = assignment_lhs(c, node) else {
            return;
        };
        let Some((body, null_default)) = enclosing_parameter(c, name) else {
            return;
        };
        // The first `??=` on the null-default face is the initializer
        // idiom; a later one reports (the official's `hasBeenAssigned`
        // flag). Every other operator reports there too.
        if null_default && c.lexeme(op) == "??=" && !earlier_null_aware_write(c, body, name) {
            return;
        }
        // The official reports at the whole `AssignmentExpression` — the
        // adopted node's own span covers only the sealed tail, so the
        // report range opens at the left operand's head.
        c.report(c.utf16_span(lhs).start..c.utf16_span(node).end, &[name]);
    }

    /// The arm-statement compensation face: the production tiling seals a
    /// non-block statement body (an `if` arm, a loop body) as a childless
    /// [`NodeKind::ExpressionStatement`] leaf whose write's
    /// [`NodeKind::Assignment`] row spills beside it (span the sealed RHS
    /// tail). The row face answers through the bounded peel
    /// ([`flow_body_run`]); where it does, this face's probe reads the peel
    /// as covered and stays silent. The statement's own token face answers:
    /// `name = …` with a bare identifier target, reporting at the write
    /// (the head through the token before the `;`) — the recovery path for
    /// any arm shape whose peel still answers nothing.
    /// Plain `=` only; the compound and `??=` faces stay on the row.
    fn statement(c: &mut Check<'_>) {
        let node = c.node();
        let head = c.head_token(node);
        let Some(first) = c.unit.tokens.get(head as usize) else {
            return;
        };
        let Some(second) = head
            .checked_add(1)
            .and_then(|token| c.unit.tokens.get(token as usize))
        else {
            return;
        };
        if first.kind != TokenKind::Identifier || second.kind != TokenKind::Eq {
            return;
        }
        let name = c.lexeme(head);
        if enclosing_parameter(c, name).is_none() {
            return;
        }
        // The row-face coverage probe: the write's `Assignment` row seals
        // with the RHS tail as its span — inside the statement (block
        // bodies) or spilled beside the childless leaf (flow arms) — and
        // reports through the assignment face only when its operand peel
        // succeeds. Probe the same peel over the rows inside this
        // statement's token face: one that answers covers the write (the
        // row face reports — never double-report); one that doesn't is
        // exactly the shape this face compensates.
        let range = c.unit.ast.token_range(node);
        let start = first.byte_offset as usize;
        let stop = range
            .end
            .checked_sub(1)
            .map(|last| crate::rules::byte_span_of_token(c, last).end);
        let covered = c
            .unit
            .ast
            .parent(node)
            .map(|parent| c.collect_direct(parent))
            .unwrap_or_default()
            .into_iter()
            .chain(c.collect_direct(node))
            .filter(|&kid| c.kind(kid) == NodeKind::Assignment)
            .filter(|&kid| c.byte_span(kid).start >= start)
            .any(|kid| {
                c.byte_span(kid).end <= stop.unwrap_or(0) && assignment_lhs(c, kid).is_some()
            });
        if covered {
            return;
        }
        // The write ends at the statement's `;` (the official reports the
        // `AssignmentExpression` — `name = value`, no semicolon).
        let Some(close) = (head + 2..range.end).find(|&token| c.lexeme(token) == ";") else {
            return;
        };
        let (Some(start16), Some(end16)) = (c.utf16_of_token(head), c.utf16_of_token(close)) else {
            return;
        };
        c.report(start16..end16, &[name]);
    }

    /// The written bare-identifier row and name of the assignment at
    /// `node` — the left operand run off the production tiling
    /// ([`super::binary_operands_at`]) peeled to its semantic rows
    /// ([`super::access_run`]); `Some` only for the single-identifier
    /// face the official's `SimpleIdentifier` gate covers.
    fn assignment_lhs<'a>(c: &Check<'a>, node: u32) -> Option<(u32, &'a str)> {
        let (left, _) = super::binary_operands_at(c, node)?;
        match super::access_run(c, &left).as_slice() {
            [name] if c.kind(*name) == NodeKind::Identifier => {
                Some((*name, c.lexeme(c.head_token(*name))))
            }
            _ => None,
        }
    }

    /// Whether an earlier-completed `??=` write naming `name` sits in
    /// `body` — the stateless stand-in for the official's per-(parameter,
    /// body) `hasBeenAssigned` flag (`_DeclarationVisitor` is fresh per
    /// body and walks the whole body in traversal order, so "first `??=`
    /// silent, second onward reports" reads as "some earlier `??=` write
    /// exists"). Ids ascend in completion order, which equals the
    /// official's traversal order away from nested-write shapes; the
    /// name-faced count folds same-name writes shadowed by a nested
    /// closure's own binder into the flag domain (the documented residual
    /// — no corpus shape today).
    fn earlier_null_aware_write(c: &Check<'_>, body: u32, name: &str) -> bool {
        let current = c.node();
        for id in c.children(body) {
            if id >= current {
                return false;
            }
            if c.kind(id) != NodeKind::Assignment {
                continue;
            }
            let Some(op) = c.field(id, PayloadField::Operator) else {
                continue;
            };
            if c.lexeme(op) == "??=" && assignment_lhs(c, id).is_some_and(|(_, n)| n == name) {
                return true;
            }
        }
        false
    }

    /// The `++`/`--` face: the bare operand is the written name; the
    /// null-default face stays silent (the official's
    /// `!_defaultValueIsNull` gate). The operator row is a leaf sealing
    /// last in the owner slice, so the operand's bare-identifier
    /// tear-off group (`Identifier, NoTypeArguments, NoArguments, Send`)
    /// occupies the four owner siblings right before it — the same
    /// posture for `++p` and `p++` (completion order, not byte order).
    fn unary(c: &mut Check<'_>) {
        let node = c.node();
        let operator = match c.kind(node) {
            // The operator is the span head (width-0 kind).
            NodeKind::UnaryPrefix => c.lexeme(c.head_token(node)),
            // The operator trails the operand.
            _ => c.lexeme(c.unit.ast.token_range(node).end - 1),
        };
        if operator != "++" && operator != "--" {
            return;
        }
        let Some(parent) = c.unit.ast.parent(node) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
            return;
        };
        let Some(start) = pos.checked_sub(4) else {
            return;
        };
        if !super::is_tear_off_group(c, &siblings, start) {
            return;
        }
        let name = c.lexeme(c.head_token(siblings[start]));
        if enclosing_parameter(c, name).is_some_and(|(_, null_default)| !null_default) {
            c.report(c.utf16_span(node), &[name]);
        }
    }

    /// The pattern-assignment face: every assigned-variable row naming a
    /// parameter reports at the pattern (the official reports per matching
    /// field at `node.pattern`). The childless marker anchors over the
    /// `=` (the row's `Equals` word); the pattern's pieces are the owner siblings
    /// before it, up to the `=` byte — the statement run bounded at the
    /// previous statement row.
    fn pattern(c: &mut Check<'_>) {
        let node = c.node();
        let Some(equals) = c.field(node, PayloadField::Equals) else {
            return;
        };
        let Some(eq_start) = c
            .unit
            .tokens
            .get(equals as usize)
            .map(|t| t.byte_offset as usize)
        else {
            return;
        };
        let Some(parent) = c.unit.ast.parent(node) else {
            return;
        };
        let siblings = c.collect_direct(parent);
        let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
            return;
        };
        let run = super::statement_run(c, &siblings[..pos]);
        let left: Vec<u32> = run
            .into_iter()
            .filter(|&row| c.byte_span(row).end <= eq_start)
            .collect();
        // The outermost pattern row seals last in the left run.
        let Some(&pattern_row) = left.last() else {
            return;
        };
        let mut seen: Vec<&str> = Vec::new();
        for row in left {
            if c.kind(row) != NodeKind::AssignedVariablePattern {
                continue;
            }
            let name = c.lexeme(c.head_token(row));
            if seen.contains(&name) || enclosing_parameter(c, name).is_none() {
                continue;
            }
            seen.push(name);
            // `reportAtNode(node.pattern)` (parameter_assignments.dart:63,
            // :116) — the outer pattern's span, from its head token to the
            // token before the `=` (the record-pattern row's span ends at
            // its last child under flat tiling, one closer short).
            let head = c.head_token(pattern_row);
            if let Some(range) = crate::rules::declarations::utf16_token_run_span(c, head..equals) {
                c.report(range, &[name]);
            }
        }
    }

    /// The nearest enclosing function-like container binding `name` as a
    /// parameter, past the scope binders on the path. `Some((body,
    /// null_default))` — the body node holding the write (the flag domain
    /// of the official's fresh per-(parameter, body) visitor) and the
    /// official's `_defaultValueIsNull` (optional without a default
    /// clause); `None` when a binder shadows the name first or no
    /// parameter binds. A `_` formal binds nothing under
    /// `wildcard_variables` (Dart 3.7 — the official answers by element
    /// identity and a wildcard parameter has none); pre-3.7 the name binds
    /// normally.
    fn enclosing_parameter(c: &Check<'_>, name: &str) -> Option<(u32, bool)> {
        if name == "_" && c.unit.features.wildcard_variables {
            return None;
        }
        let write_start = c.byte_span(c.node()).start;
        for depth in 1..256 {
            let row = c.ancestor(depth)?;
            match c.kind(row) {
                NodeKind::TopLevelMethod
                | NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::FunctionExpression
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::NamedFunctionExpression => {
                    // The official walks the body only: initializer and
                    // non-body arms fall through to the outer scopes.
                    let in_body = c.collect_direct(row).into_iter().find(|&kid| {
                        matches!(c.kind(kid), NodeKind::BlockBody | NodeKind::ExpressionBody) && {
                            let span = c.byte_span(kid);
                            span.start <= write_start && write_start < span.end
                        }
                    });
                    if let Some(body) = in_body {
                        for (parameter, optional) in parameter_rows(c, row) {
                            if parameter_name(c, parameter) == Some(name) {
                                return Some((body, optional && !has_default_clause(c, parameter)));
                            }
                        }
                    }
                }
                // Scope binders on the path capture the name first (the
                // function body block binds locals like any block).
                NodeKind::Block | NodeKind::BlockBody => {
                    if c.collect_direct(row)
                        .into_iter()
                        .any(|stmt| statement_binds(c, stmt, name, write_start))
                    {
                        return None;
                    }
                }
                NodeKind::CatchClause => {
                    if c.collect_direct(row).into_iter().any(|kid| {
                        c.kind(kid) == NodeKind::FormalParameters
                            && c.collect_direct(kid)
                                .into_iter()
                                .any(|p| parameter_name(c, p) == Some(name))
                    }) {
                        return None;
                    }
                }
                NodeKind::ForStatement => {
                    if c.collect_direct(row)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::ForParts)
                        .is_some_and(|parts| {
                            c.collect_direct(parts)
                                .into_iter()
                                .any(|stmt| statement_binds(c, stmt, name, write_start))
                        })
                    {
                        return None;
                    }
                }
                NodeKind::ForIn
                | NodeKind::ForControlFlow
                | NodeKind::SwitchCase
                | NodeKind::SwitchExpressionCase => {
                    if c.collect_direct(row).into_iter().any(|kid| {
                        c.kind(kid) == NodeKind::DeclaredVariablePattern
                            && c.lexeme(c.head_token(kid)) == name
                    }) {
                        return None;
                    }
                }
                NodeKind::PatternVarDeclStatement
                    if c.children(row).any(|kid| {
                        c.kind(kid) == NodeKind::DeclaredVariablePattern
                            && c.lexeme(c.head_token(kid)) == name
                    }) =>
                {
                    return None;
                }
                _ => {}
            }
        }
        None
    }

    /// Whether a statement binds `name` before the write (the shadow
    /// binder set: local variable declarations, pattern variable
    /// statements, local functions; the span filter keeps later
    /// declarations inert).
    fn statement_binds(c: &Check<'_>, stmt: u32, name: &str, write_start: usize) -> bool {
        if c.byte_span(stmt).end > write_start {
            return false;
        }
        match c.kind(stmt) {
            NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
                c.kind(decl) == NodeKind::InitializedIdentifier
                    && initialized_identifier_name(c, decl)
                        .is_some_and(|token| c.lexeme(token) == name)
            }),
            NodeKind::PatternVarDeclStatement => c.children(stmt).any(|kid| {
                c.kind(kid) == NodeKind::DeclaredVariablePattern
                    && c.lexeme(c.head_token(kid)) == name
            }),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                crate::rules::declarations::member_declaration_name(c, stmt)
                    .is_some_and(|row| c.lexeme(c.head_token(row)) == name)
            }
            NodeKind::LabeledStatement => c
                .collect_direct(stmt)
                .into_iter()
                .any(|kid| statement_binds(c, kid, name, write_start)),
            _ => false,
        }
    }

    /// The formal rows of a function-like container: the direct plain rows
    /// plus the required/optional group nests, each with its optionality
    /// (the analyzer's `parameters.parameters` list). The optional
    /// section nests inside `FormalParameters` on the production tiling
    /// (`(int a, [int? b])` — the `[...]` group's own row), so the walk
    /// recurses one level.
    pub(crate) fn parameter_rows(c: &Check<'_>, owner: u32) -> Vec<(u32, bool)> {
        fn collect(c: &Check<'_>, list: u32, optional: bool, rows: &mut Vec<(u32, bool)>) {
            for kid in c.collect_direct(list) {
                match c.kind(kid) {
                    NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                        rows.push((kid, optional));
                    }
                    NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                        collect(
                            c,
                            kid,
                            c.kind(kid) == NodeKind::OptionalFormalParameters,
                            rows,
                        );
                    }
                    _ => {}
                }
            }
        }
        let mut rows = Vec::new();
        for kid in c.collect_direct(owner) {
            match c.kind(kid) {
                NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => {
                    rows.push((kid, false));
                }
                NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                    collect(
                        c,
                        kid,
                        c.kind(kid) == NodeKind::OptionalFormalParameters,
                        &mut rows,
                    );
                }
                _ => {}
            }
        }
        rows
    }

    /// A formal parameter's name (`FormalParameter` payload word 1; the
    /// function-typed form's name — the production
    /// `FunctionTypedFormalParameter` anchors at its name token but may
    /// still tile the written return type's `Identifier` (a
    /// `TypeReference` leaf) as a child, so the read skips the type leaf
    /// and otherwise answers the head token; a hand-built table carries a
    /// name `Identifier` child).
    pub(crate) fn parameter_name<'a>(c: &'a Check<'a>, parameter: u32) -> Option<&'a str> {
        match c.kind(parameter) {
            NodeKind::FormalParameter => {
                let name = c.field(parameter, PayloadField::Name)?;
                (name != NONE_TOKEN).then(|| c.lexeme(name))
            }
            NodeKind::FunctionTypedFormalParameter => {
                let name_row = c.collect_direct(parameter).into_iter().find(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context)
                            != Some(dartq_parser::IdentifierContext::TypeReference as u32)
                });
                Some(c.lexeme(
                    name_row.map_or_else(|| c.head_token(parameter), |kid| c.head_token(kid)),
                ))
            }
            _ => None,
        }
    }

    /// Whether the parameter carries a default clause (`= init`; the
    /// bridge keeps the default clause extent in the row's
    /// `InitializerStart`/`InitializerEnd` words).
    fn has_default_clause(c: &Check<'_>, parameter: u32) -> bool {
        c.kind(parameter) == NodeKind::FormalParameter
            && c.field(parameter, PayloadField::InitializerStart)
                .is_some_and(|token| token != NONE_TOKEN)
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/parameter_assignments_test.dart` `test_withAssignment`.
        #[test]
        fn reports_a_parameter_reassignment() {
            let f = Fixture::parse("void f(int a) { a = 1; }");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Invalid assignment to the parameter 'a'.");
            assert_eq!(lints[0].range.start, 16);
        }

        /// The arm-statement face: a bare-identifier write as a non-block
        /// `if` arm reports at the write (the
        /// `lib/expressions_family.dart` `labelUser` row). The arm's
        /// `Assignment` row answers through the bounded peel
        /// ([`flow_body_run`]), so exactly one report lands (row face; the
        /// leaf's probe reads it covered and stays silent).
        #[test]
        fn reports_a_write_in_a_bare_if_arm() {
            let f =
                Fixture::parse("void f(String? label) {\n  if (label == null) label = 'none';\n}");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Invalid assignment to the parameter 'label'."
            );
            let start = f.source.find("label = 'none'").unwrap() as u32;
            assert_eq!(lints[0].range, start..start + 14);

            let f = Fixture::parse("void f(int a) {\n  while (true) a = 1;\n}");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }

        /// The for-in arm: the header rows (the loop variable's row, the
        /// iterable fragments, the `ForInParts` leaf) no longer open the
        /// operand run, so a compound write as a non-block loop body reports
        /// through the assignment face — a shape the statement leaf cannot
        /// compensate (plain `=` only).
        #[test]
        fn reports_a_write_in_a_bare_for_in_body() {
            let f = Fixture::parse("void f(List<int> l, int t) {\n  for (final s in l) t += s;\n}");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "Invalid assignment to the parameter 't'.");
            let start = f.source.find("t += s").unwrap() as u32;
            assert_eq!(lints[0].range.start, start);
        }

        /// The shapes the `Assignment` row survives dispatch through the
        /// assignment face alone — the arm-statement leaf never
        /// double-reports them.
        #[test]
        fn arm_leaf_never_double_reports_row_shapes() {
            let f = Fixture::parse("void f(int a) {\n  if (true) {\n    a = 1;\n  }\n}");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");

            let f = Fixture::parse("void f(int a) {\n  if (true) ; else a = 1;\n}");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
        }

        /// The arm-statement face's gates: a local target and a member
        /// write stay silent.
        #[test]
        fn arm_leaf_non_parameter_writes_stay_silent() {
            let f = Fixture::parse("void f() {\n  int x = 0;\n  if (x == 0) x = 1;\n}");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let f = Fixture::parse(
                "class C {\n  int b = 0;\n  void m(int a) {\n    if (a == 0) this.b = a;\n  }\n}",
            );
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());
        }

        /// Compound assignments and increments are writes too.
        #[test]
        fn reports_compound_and_increment_writes() {
            let f = Fixture::parse("void f(int a) { a += 1; a++; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 2);
        }

        /// Prefix `++p` and postfix `p++` both write the parameter (the
        /// operand reads off the owner-sibling tiling in both directions);
        /// a local's increment stays silent.
        #[test]
        fn increments_report_and_local_increments_stay_silent() {
            let f = Fixture::parse("void f(int p) { ++p; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f(int p) { p++; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f() { int x = 0; x++; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());
        }

        /// The optional-null-default faces: `??=` is the initializer
        /// idiom and `++` cannot follow `null`, but a plain assignment
        /// reports; a defaulted parameter has no null default.
        #[test]
        fn optional_null_default_faces() {
            let f = Fixture::parse("void f([int? a]) { a ??= 1; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let f = Fixture::parse("void f([int? a]) { a = 1; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f([int? a]) { a++; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let f = Fixture::parse("void f([int a = 0]) { a++; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }

        /// The canonical double-`??=` face (upstream
        /// `test_function_positional_optional_assignedTwice`): the first
        /// write is the initializer idiom and stays silent, the second
        /// reports.
        #[test]
        fn second_null_aware_write_reports() {
            let f = Fixture::parse("void f([int? a]) { a ??= 8; a ??= 16; }");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Invalid assignment to the parameter 'a'.");
            assert_eq!(lints[0].range.start, 28);
        }

        /// The flag spans branch bodies and nested closures (the official
        /// walks the whole body per parameter), so a `??=` inside either
        /// arms the write after it.
        #[test]
        fn null_aware_flag_spans_branches_and_closures() {
            let f = Fixture::parse("void f([int? a]) { if (b) { a ??= 8; } a ??= 16; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f([int? a]) { void g() { a ??= 8; } a ??= 16; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }

        /// A non-`??=` write after a first `??=` reports immediately and
        /// never arms the flag itself (upstream
        /// `..._re_incremented`/`..._reassigned`).
        #[test]
        fn plain_write_after_null_aware_reports() {
            let f = Fixture::parse("void f([int? a]) { a ??= 8; a = 16; }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }

        /// The flag is keyed per parameter (the official's fresh visitor
        /// per (parameter, body)): a second parameter's first `??=` is its
        /// own initializer idiom and stays silent beside the first
        /// parameter's reported second write, and same-name parameters of
        /// different functions arm independent flags.
        #[test]
        fn null_aware_flag_is_per_parameter() {
            let f = Fixture::parse("void f([int? a, int? b]) { a ??= 8; a ??= 16; b ??= 1; }");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].range.start, 36);

            let f = Fixture::parse("void f([int? a]) { a ??= 1; } void g([int? a]) { a ??= 2; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());
        }

        /// Pattern-assignment variable targets write the parameter; the
        /// report covers the whole outer pattern (`(a, b)` — the official
        /// `reportAtNode(node.pattern)`, parameter_assignments.dart:63).
        #[test]
        fn reports_a_pattern_assignment_target() {
            let f = Fixture::parse("void f(int a) { (a, _) = (1, 2); }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f(int a) {\n  var b = 0;\n  (a, b) = (1, 2);\n}");
            let lints = f.run(&one_rule("parameter_assignments"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            let start = f.source.find("(a, b)").unwrap() as u32;
            assert_eq!(lints[0].range, start..start + 6);
        }

        /// `test_assignment_wildcard` — a `_` formal binds nothing under
        /// `wildcard_variables` (Dart 3.7), so the write stays silent;
        /// pre-3.7 the name binds and reports.
        #[test]
        fn wildcard_parameter_stays_silent() {
            let f = Fixture::parse("void f(int _) { _ = 8; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let mut f = Fixture::parse("void f(int _) { _ = 8; }");
            f.features.wildcard_variables = false;
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }

        /// A nested function's bare write falls through to the outer
        /// parameter (the official's element identity); a local binder on
        /// the path shadows it instead.
        #[test]
        fn closure_capture_reports_and_local_binder_shadows() {
            let f = Fixture::parse("void f(int a) { void g() { a = 1; } }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);

            let f = Fixture::parse("void f(int a) { int a = 0; a = 1; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());
        }

        /// Member-write targets are not bare identifiers, and the official
        /// walks the body only — constructor-initializer writes stay
        /// silent while body writes report.
        #[test]
        fn member_and_initializer_faces() {
            let f = Fixture::parse("void f(int a) { a.b = 1; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let f = Fixture::parse("class C { int x; C(this.x) : x = 0; }");
            assert!(f.run(&one_rule("parameter_assignments")).is_empty());

            let f = Fixture::parse("class C { C(int c) { c = 1; } }");
            assert_eq!(f.run(&one_rule("parameter_assignments")).len(), 1);
        }
    }
}
