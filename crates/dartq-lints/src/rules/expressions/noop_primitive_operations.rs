use dartq_semantic::model::Ty;

use super::{Check, Group, NodeKind, RuleSpec, query_site, slice, static_type_faces as faces};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/noop_primitive_operations.dart`
/// — three noop families report:
///
/// - a middle empty literal of an adjacent-strings group (:42-47 — the
///   ends pass, sdk#55541; the literal `stringValue` must be the empty
///   string, so an interpolated literal never fires);
/// - a `target.toString()` with no arguments and a non-`super` target
///   inside a string interpolation (:97-105);
/// - typed invocations (:70-94): `toString` on a non-nullable core
///   `String` receiver, the `int` invariants (`toInt`/`round`/`ceil`/
///   `floor`/`truncate`) on a core `int` receiver (nullability does not
///   gate — `isDartCoreInt` reads through `?`), and `toDouble` on a
///   core `double` receiver; with no target, the core `print` face
///   descends into its single argument for the same `toString` shape
///   (:59-66 — `print(1.toString())`).
///
/// dartq faces over the production tiling (the send-family handles are
/// childless leaves over flat owner slices; a member invocation tiles
/// `[receiver pieces, selector Identifier, Arguments | NoArguments,
/// Send, trailing same-span DotAccess marker]`): the dispatch rows are
/// [`NodeKind::Send`] and [`NodeKind::StringLiteral`]. The receiver
/// reads in two layers — a literal receiver decides closed through the
/// shared literal faces, everything else through the semantic model
/// ([`faces::expr_type`]: a bare name's read or callee row, a
/// single-name member chain, a receiver call), gated on the written
/// name not being a same-file declaration. Approximations (documented):
/// a member-chain receiver beyond the first hop, an unknown-type
/// (`var`) binding, and a written core-type name shadowed by a
/// same-file declaration stay silent; the core `print` identity is
/// the name-table face (`isDartCorePrint`) — a bare `print` binding
/// into the synthetic SDK face is core, a workspace binding shadows
/// it, which under-reports a shadowing import. Message:
/// `diagnostic.g.dart` `noopPrimitiveOperations`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "noop_primitive_operations",
    group: Group::Standalone,
    problem: "The expression has no effect and can be removed.",
    correction: "Try removing the expression.",
    kinds: &[NodeKind::Send, NodeKind::StringLiteral],
    check,
};

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::Send => check_send(c, analysis),
        NodeKind::StringLiteral => check_string_literal(c),
        _ => {}
    }
}

/// The send faces (:57-95): the typed receiver families over a member
/// invocation, and the bare `print` descent into its single argument.
fn check_send(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((selector, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    let name = c.lexeme(c.head_token(kids[selector]));
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_none() {
        // The bare-invocation face (:59-66): core `print` with exactly
        // one argument descends for the toString shape. The identity is
        // the name-table face (`isDartCorePrint`): a bare `print`
        // resolving into the synthetic SDK face is the core function —
        // descend; a resolved workspace binding shadows it (kept
        // under-reported).
        if name != "print" || arguments.is_none() {
            return;
        }
        if analysis
            .binding_of(query_site(c, kids[selector]))
            .is_some_and(|binding| !analysis.is_sdk_binding(&binding))
        {
            return;
        }
        // Exactly one argument (:60-61): the `Arguments` row is a single
        // positional argument when no `NamedArgument` marker and no
        // top-level comma tile in it — `slice::arguments` mis-splits a
        // member-chain argument's send groups, so the count reads the
        // comma token directly.
        let args = arguments.expect("checked Some above");
        let raw = c.collect_direct(args);
        let single = c.unit.ast.token_range(args).all(|tok| {
            c.unit
                .tokens
                .get(tok as usize)
                .is_none_or(|t| t.kind != dartq_scanner::TokenKind::Comma)
        }) && !raw
            .iter()
            .any(|&row| c.kind(row) == NodeKind::NamedArgument);
        if !single || raw.is_empty() {
            return;
        }
        let run = &raw[..];
        // The argument's root invocation: the widest `Send` inside the
        // argument's piece range.
        let Some(root) = run
            .iter()
            .copied()
            .filter(|&row| c.kind(row) == NodeKind::Send)
            .max_by_key(|&row| {
                let span = c.byte_span(row);
                span.end - span.start
            })
        else {
            return;
        };
        if let Some(name_range) = noop_to_string_name(c, run, root) {
            c.report(name_range, &[]);
        }
        return;
    }
    // The typed faces (:70-94): the member name decides the family.
    // The upstream predicate reads no argument count and no null-aware
    // marker — the receiver type alone decides, with the non-nullable
    // gate on the `toString` family only.
    let (wanted, non_null) = match name {
        "toString" => ("String", true),
        "toDouble" => ("double", false),
        "toInt" | "round" | "ceil" | "floor" | "truncate" => ("int", false),
        _ => return,
    };
    let Some((core, nullable)) = receiver_core_type(c, analysis, &kids, selector) else {
        return;
    };
    if core != wanted || (non_null && nullable) {
        return;
    }
    let name_range = utf16_token_span(c, c.head_token(kids[selector]));
    c.report(name_range, &[]);
}

/// The receiver's core type name and nullability, or `None` when no
/// face answers: a literal decides closed (non-nullable), everything
/// else reads the semantic model — a bare name's read or callee row, a
/// single-name member chain, a receiver call — with the written core
/// spelling shadow-guarded against same-file declarations.
fn receiver_core_type(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    kids: &[u32],
    selector: usize,
) -> Option<(&'static str, bool)> {
    let mut i = selector.checked_sub(1)?;
    while matches!(
        c.kind(kids[i]),
        NodeKind::NoTypeArguments | NodeKind::TypeArguments
    ) {
        i = i.checked_sub(1)?;
    }
    match c.kind(kids[i]) {
        NodeKind::StringLiteral => Some(("String", false)),
        NodeKind::LiteralInt => Some(("int", false)),
        NodeKind::LiteralDouble => Some(("double", false)),
        _ => {
            // A wrapped literal decides through the shared literal face.
            if let Some(literal) = slice::literal_faces(c, kids[i]) {
                return (literal & slice::STRING != 0).then_some(("String", false));
            }
            // The bare-name receiver group immediately before the
            // selector (`[name, NoTypeArguments, NoArguments, Send]`):
            // the name's flow row answers. The whole-window piece run
            // would read the earlier statements' strays.
            if c.kind(kids[i]) == NodeKind::Send
                && i >= 3
                && slice::is_tear_off_group(c, kids, i - 3)
            {
                let Ty::Named { name, nullable, .. } =
                    analysis.type_of(query_site(c, kids[i - 3]))?
                else {
                    return None;
                };
                let written = analysis.strings().resolve(name);
                if super::same_file::declares_name(c, written) {
                    return None;
                }
                let core = match written {
                    "String" => "String",
                    "int" => "int",
                    "double" => "double",
                    _ => return None,
                };
                return Some((core, nullable));
            }
            let Ty::Named { name, nullable, .. } =
                faces::expr_type(c, analysis, &kids[..selector])?
            else {
                return None;
            };
            let written = analysis.strings().resolve(name);
            // A same-file declaration owns the spelling — not the core
            // type.
            if super::same_file::declares_name(c, written) {
                return None;
            }
            let core = match written {
                "String" => "String",
                "int" => "int",
                "double" => "double",
                _ => return None,
            };
            Some((core, nullable))
        }
    }
}

/// The `_checkToStringInvocation` face (:97-105): `run`'s send is a
/// member invocation whose member is `toString` with no arguments and a
/// present non-`super` target; returns the member-name range.
fn noop_to_string_name(c: &Check<'_>, run: &[u32], send: u32) -> Option<core::ops::Range<u32>> {
    let pos = run.iter().position(|&row| row == send)?;
    // A target is required (a bare `toString()` has no marker).
    slice::access_marker(c, run, pos, NodeKind::DotAccess)?;
    let (selector, arguments) = slice::send_parts(c, run, pos)?;
    if c.lexeme(c.head_token(run[selector])) != "toString" {
        return None;
    }
    if !zero_arguments(c, arguments) {
        return None;
    }
    // The target is present and not `super`: the first receiver piece
    // before the selector (wrappers skip back) decides.
    let mut i = selector.checked_sub(1)?;
    while matches!(
        c.kind(run[i]),
        NodeKind::NoTypeArguments | NodeKind::TypeArguments
    ) {
        i = i.checked_sub(1)?;
    }
    if c.kind(run[i]) == NodeKind::SuperExpression {
        return None;
    }
    Some(utf16_token_span(c, c.head_token(run[selector])))
}

/// The upstream zero-argument face: no argument list marker (the
/// width-0 `NoArguments`) or an empty `()` pair.
fn zero_arguments(c: &Check<'_>, arguments: Option<u32>) -> bool {
    match arguments {
        None => true,
        Some(args) => c.unit.ast.token_range(args).count() == 2,
    }
}

/// The string-literal faces: the adjacent-strings middle empty literal,
/// then the interpolation face over the literal's own interpolation
/// expressions (their rows tile as the literal's children).
fn check_string_literal(c: &mut Check<'_>) {
    let literal = c.node();
    if middle_empty_literal(c) {
        let range = c.utf16_span(literal);
        c.report(range, &[]);
        return;
    }
    let kids = c.collect_direct(literal);
    for row in kids {
        if c.kind(row) != NodeKind::Send {
            continue;
        }
        let Some(parent) = c.unit.ast.parent(row) else {
            continue;
        };
        let run = c.collect_direct(parent);
        if let Some(name_range) = noop_to_string_name(c, &run, row) {
            c.report(name_range, &[]);
        }
    }
}

/// The adjacent-strings middle face (:42-47): a literal with adjacent
/// string siblings on both sides whose own spelling is the empty
/// literal.
fn middle_empty_literal(c: &Check<'_>) -> bool {
    let node = c.node();
    // Adjacent strings tile across an adopted boundary: the last
    // literal of `'a' '' 'b'` completes inside the `Assignment` row, so
    // same-parent siblings miss it. The run is the enclosing statement's
    // string literals in token order; the node is the middle when it is
    // flanked on both sides by directly-adjacent spellings.
    let mut scope = node;
    let group: Vec<u32> = loop {
        let Some(parent) = c.unit.ast.parent(scope) else {
            return false;
        };
        let kids = c.collect_direct(parent);
        let strings: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::StringLiteral)
            .collect();
        if strings.contains(&node) {
            // The group's right literal may live in a nested operator
            // row — collect every string literal under this statement
            // subtree in token order.
            let mut all = Vec::new();
            collect_string_literals(c, parent, &mut all);
            if all.contains(&node) {
                break all;
            }
        }
        scope = parent;
    };
    let Some(pos) = group.iter().position(|&kid| kid == node) else {
        return false;
    };
    let adjacent = |left: u32, right: u32| {
        c.unit.ast.token_range(left).end == c.unit.ast.token_range(right).start
    };
    let has_left = pos > 0 && adjacent(group[pos - 1], group[pos]);
    let has_right = pos + 1 < group.len() && adjacent(group[pos], group[pos + 1]);
    has_left && has_right && is_empty_literal(c, node)
}

/// Every `StringLiteral` row under `row`, in token order (subtree walk —
/// the adjacent-strings group's literals scatter across the adopted
/// operator boundary).
fn collect_string_literals(c: &Check<'_>, row: u32, out: &mut Vec<u32>) {
    let mut kids: Vec<u32> = c.collect_direct(row);
    kids.sort_by_key(|&kid| c.head_token(kid));
    for kid in kids {
        if c.kind(kid) == NodeKind::StringLiteral {
            out.push(kid);
        }
        collect_string_literals(c, kid, out);
    }
}

/// Whether the literal's spelling is the empty string: one string
/// segment that is only its quote run (`''`/`""` or the triple-quoted
/// forms; an interpolation's first segment is a lone opening quote and
/// fails the shape).
fn is_empty_literal(c: &Check<'_>, literal: u32) -> bool {
    let spelling = c.lexeme(c.head_token(literal));
    let body = spelling.strip_prefix('r').unwrap_or(spelling);
    let Some(quote) = body.chars().next() else {
        return false;
    };
    (quote == '\'' || quote == '"')
        && body.chars().all(|ch| ch == quote)
        && matches!(body.len(), 2 | 6)
}
