use dartq_semantic::model::{ArgListId, Ty};

use super::{Check, NodeKind, RuleSpec, query_site, slice, static_type_faces};
use crate::rules::utf16_token_span;

/// Official: `pkg/linter/lib/src/rules/await_only_futures.dart:38-56` —
/// `await` on an expression whose static type is not assignable to
/// `Future` reports at the `await` keyword, with the type as the
/// argument (:55).
///
/// The static-type faces: scalar literals (int / double / String /
/// bool — the exact types, reading through parenthesized expressions),
/// plus the bridge type face when mounted — the awaited expression's
/// type answers for an identifier read, a bare send (callee), or a
/// single-name member access; a decided non-`Future` type reports with
/// its rendered display (`await x;` over a `String` variable). The
/// official exemptions port: `await null` skips (:39), `dynamic` skips
/// (:43), `Future`/`FutureOr` assign (:44-45). A literal collection
/// reports through its own face (never a `Future` — `List`/`Set`/`Map`
/// with explicit or uniform element faces, `dynamic` fill otherwise). A
/// type with explicit type arguments, a chain beyond the first hop, and
/// the extension-type exemption (:46) under-report. Message:
/// `diagnostic.g.dart` `awaitOnlyFutures`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "await_only_futures",
    &[NodeKind::AwaitExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The awaited expression tiles inside the container's window; its
    // root is the last row (a container completes after its subtree).
    let window = c.children(node);
    if window.start >= window.end {
        return;
    }
    let mut root = window.end - 1;
    // Read through transparent `(...)` wrappers (the content is the
    // wrapper window's last row).
    while c.kind(root) == NodeKind::ParenthesizedExpression {
        let inner = c.children(root);
        if inner.start >= inner.end {
            return;
        }
        root = inner.end - 1;
    }
    if let Some(type_name) = scalar_literal_type(c, root) {
        // The `await` keyword is the width-0 kind's span head.
        let range = utf16_token_span(c, c.head_token(node));
        c.report(range, &[type_name]);
        return;
    }
    // The collection-literal face: a literal collection is never a
    // `Future` (`await []`, `await {1: 2}`) — the official reports the
    // inferred type. The element displays read the scalar/read faces
    // with a `dynamic` fill (a mixed or unfaceable element's LUB is
    // not modeled — the display argument is best-effort, the report
    // row itself is exact).
    if matches!(
        c.kind(root),
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
    ) {
        let Some(analysis) = c.semantics() else {
            return;
        };
        let rows: Vec<u32> = (window.start..window.end)
            .filter(|&row| {
                row != root && {
                    let span = c.byte_span(row);
                    let literal = c.byte_span(root);
                    span.start >= literal.start && span.end <= literal.end
                }
            })
            .collect();
        if let Some(display) = collection_display(c, analysis, root, &rows) {
            let range = utf16_token_span(c, c.head_token(node));
            c.report(range, &[display.as_str()]);
        }
        return;
    }
    // The bridge static-type face (the identifier / call / member
    // access family).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let run: Vec<u32> = (window.start..window.end).collect();
    let Some(ty) = static_type_faces::expr_type(c, analysis, &run) else {
        return;
    };
    let display = match ty {
        // The official bails (:43).
        Ty::Dynamic => return,
        Ty::Named {
            name,
            nullable,
            args,
        } => {
            let spelled = analysis.strings().resolve(name);
            // Assignable to `Future<dynamic>` (:48-55); `FutureOr` skips
            // (:44).
            if spelled == "Future" || spelled == "FutureOr" {
                return;
            }
            // The display argument must be exact: a type with explicit
            // arguments renders through the type-argument pool the
            // analysis does not publish — under-report.
            if args != ArgListId::EMPTY {
                return;
            }
            if nullable {
                format!("{spelled}?")
            } else {
                spelled.to_owned()
            }
        }
        // `Null` (the literal skipped above), `Never` (assignable), and
        // `void` carry no awaited value face.
        _ => return,
    };
    let range = utf16_token_span(c, c.head_token(node));
    c.report(range, &[display.as_str()]);
}

/// The exact static type of a scalar literal (`int`, `double`,
/// `String`, `bool`); `None` when the syntax carries no verdict.
/// `await null` skips upstream (official :39).
fn scalar_literal_type(c: &Check<'_>, row: u32) -> Option<&'static str> {
    match c.kind(row) {
        NodeKind::LiteralInt => Some("int"),
        NodeKind::LiteralDouble => Some("double"),
        NodeKind::StringLiteral => Some("String"),
        NodeKind::LiteralBool => Some("bool"),
        _ => None,
    }
}

/// The literal collection's display: `List`/`Set`/`Map` with its
/// element faces — explicit type arguments win, else the uniform
/// scalar/read element faces, else `dynamic` per slot. The rows are
/// the literal's interior tiles (entries, elements, wrappers) inside
/// the root's own span; on the production tiling the containers are
/// childless, so the key/value and element boundaries rebuild from the
/// sibling run — the `collection_elements`/`entry_sides` pattern: an
/// entry's marker seals last in its segment and `payload[0]` (the
/// colon) is the only reliable key/value split anchor.
fn collection_display(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    root: u32,
    rows: &[u32],
) -> Option<String> {
    // A row swallowed by a nested literal belongs to that literal, not
    // to this one (`{{1: 2}: 'a'}` — the inner `MapEntry` is not an
    // entry of the outer map). Control-flow markers stay transparent:
    // an `if` element's own rows tile flat inside this literal.
    let nested = |row: u32| {
        let span = c.byte_span(row);
        rows.iter().any(|&other| {
            other != row
                && matches!(
                    c.kind(other),
                    NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
                )
                && {
                    let outer = c.byte_span(other);
                    outer.start <= span.start && span.end <= outer.end
                }
        })
    };
    let has_entry = rows
        .iter()
        .copied()
        .any(|row| c.kind(row) == NodeKind::MapEntry && !nested(row));
    // The literal's explicit type arguments (`<int>[…]`, `<K, V>{…}`):
    // hand-built trees carry the `TypeArguments` row as a child; the
    // production tiling leaves it an owner-sibling sealing at the
    // literal head (the literal span opens at `[`/`{`).
    let typed: Vec<String> = c
        .collect_direct(root)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeArguments)
        .or_else(|| {
            let tokens = c.unit.ast.token_range(root);
            let synthetic_head = c.token_is_synthetic(tokens.start);
            let window = c.children(c.unit.ast.parent(root)?);
            (window.start..window.end)
                .filter(|&row| c.kind(row) == NodeKind::TypeArguments)
                .filter(|&row| {
                    let end = c.unit.ast.token_range(row).end;
                    // The `[]` index-token literal anchors at synthetic
                    // brackets past the `>` — its `TypeArguments`
                    // sibling only satisfies the looser before-head
                    // bound.
                    end == tokens.start || (synthetic_head && end <= tokens.start)
                })
                .max_by_key(|&row| c.unit.ast.token_range(row).end)
        })
        .map(|args| {
            c.collect_direct(args)
                .into_iter()
                .filter(|&row| c.kind(row) == NodeKind::Type)
                .map(|row| c.lexeme(c.head_token(row)).to_owned())
                .collect()
        })
        .unwrap_or_default();
    let (kind, arity) = match c.kind(root) {
        NodeKind::ListLiteral => ("List", 1usize),
        NodeKind::SetOrMapLiteral => {
            if has_entry || typed.len() == 2 {
                ("Map", 2)
            } else if typed.len() == 1
                || rows
                    .iter()
                    .any(|&row| !matches!(c.kind(row), NodeKind::NoTypeArguments))
            {
                ("Set", 1)
            } else {
                // A bare `{}` carries no entries — the upstream
                // inference settles it as `Map<dynamic, dynamic>`.
                ("Map", 2)
            }
        }
        _ => return None,
    };
    // Explicit type arguments decide (`<int>[…]`, `<K, V>{…}`).
    if typed.len() == arity {
        return Some(render_display(
            kind,
            &typed.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }
    let mut slots: Vec<Option<String>> = vec![None; arity];
    // The display of one comma-separated element or one entry side: the
    // run's last row is the root (completion order); `(...)` and `!`
    // are transparent wrappers, and the bare-name send group reads
    // through to its identifier.
    let side_display = |side: &[u32]| -> Option<String> {
        let mut side = side;
        let mut nonnull = false;
        let root = loop {
            let &root = side.last()?;
            match c.kind(root) {
                NodeKind::ParenthesizedExpression => side = &side[..side.len() - 1],
                NodeKind::NonNullAssert => {
                    nonnull = true;
                    side = &side[..side.len() - 1];
                }
                _ => break root,
            }
        };
        let mut display = match c.kind(root) {
            NodeKind::LiteralInt => "int".to_owned(),
            NodeKind::LiteralDouble => "double".to_owned(),
            NodeKind::StringLiteral => "String".to_owned(),
            NodeKind::LiteralBool => "bool".to_owned(),
            NodeKind::Send
                if side.len() >= 4 && slice::is_tear_off_group(c, side, side.len() - 4) =>
            {
                identifier_display(c, analysis, side[side.len() - 4])?
            }
            NodeKind::Identifier => identifier_display(c, analysis, root)?,
            // A nested literal spells its own collection face.
            NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => {
                let span = c.byte_span(root);
                let inner: Vec<u32> = rows
                    .iter()
                    .copied()
                    .filter(|&row| {
                        row != root && {
                            let piece = c.byte_span(row);
                            piece.start >= span.start && piece.end <= span.end
                        }
                    })
                    .collect();
                collection_display(c, analysis, root, &inner)?
            }
            _ => return None,
        };
        if nonnull && display.ends_with('?') {
            display.pop();
        }
        Some(display)
    };
    let fill = |slot: &mut Option<String>, displays: Vec<Option<String>>| {
        let mut decided: Option<Option<String>> = None;
        for display in displays.into_iter().flatten() {
            match &decided {
                Some(Some(previous)) if *previous != display => {
                    decided = Some(None); // mixed — the LUB is not modeled
                }
                Some(None) => {}
                _ => decided = Some(Some(display)),
            }
        }
        if let Some(decided) = decided {
            *slot = decided;
        }
    };
    if kind == "Map" {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        // Each `MapEntry` marker seals last in its own run; the pieces
        // accumulated since the previous marker split at the colon.
        let mut run_start = 0usize;
        for (index, &row) in rows.iter().enumerate() {
            if c.kind(row) != NodeKind::MapEntry || nested(row) {
                continue;
            }
            let run = &rows[run_start..index];
            run_start = index + 1;
            let Some(colon) = c.payload(row).first().copied() else {
                continue;
            };
            let split = run.partition_point(|&r| c.unit.ast.token_range(r).end <= colon + 1);
            keys.push(side_display(&run[..split]));
            values.push(side_display(&run[split..]));
        }
        fill(&mut slots[0], keys);
        fill(&mut slots[1], values);
    } else {
        let elements = slice::comma_segments(c, root, rows)
            .iter()
            .map(|segment| side_display(segment))
            .collect();
        fill(&mut slots[0], elements);
    }
    let rendered: Vec<String> = slots
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| "dynamic".to_owned()))
        .collect();
    Some(render_display(
        kind,
        &rendered.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
}

/// The display of a bare-name read (`x`, the element or key/value
/// face): the semantic `type_of` over the identifier's own span; a
/// named type renders its name, `?` kept.
fn identifier_display(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    row: u32,
) -> Option<String> {
    match analysis.type_of(query_site(c, row))? {
        Ty::Named { name, nullable, .. } => {
            let mut display = analysis.strings().resolve(name).to_owned();
            if nullable {
                display.push('?');
            }
            Some(display)
        }
        _ => None,
    }
}

/// The `Kind<T, …>` rendering over slot names.
fn render_display(kind: &str, slots: &[&str]) -> String {
    let mut display = String::from(kind);
    display.push('<');
    display.push_str(&slots.join(", "));
    display.push('>');
    display
}
