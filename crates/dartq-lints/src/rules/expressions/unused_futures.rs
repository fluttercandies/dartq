use dartq_semantic::model::Ty;

use super::{
    Check, NodeKind, PayloadField, query_site, same_file, slice, static_type_faces as faces,
};
use crate::rules::declarations::{annotation_names, top_level_annotations};
use crate::rules::{
    MemberQuery, NONE_TOKEN, body_members, class_declaration_name, member_annotations,
    member_matches, same_file_ancestors, same_file_declaration, utf16_token_span,
};

// The official message templates (3.13.7 `diagnostic.g.dart`): each
// rule publishes exactly one code — the `Future`/`FutureOr` static-type
// split lands in the same row (the paired `*FutureOr` texts are a later
// SDK face, not this pin).
const SYNC_FUTURE_PROBLEM: &str = "'Future'-returning calls in a non-'async' function.";
const SYNC_FUTURE_CORRECTION: &str = "Try converting the enclosing function to be 'async' and then 'await' the future, or wrap the expression in 'unawaited'.";
const ASYNC_FUTURE_PROBLEM: &str =
    "Missing an 'await' for the 'Future' computed by this expression.";
const ASYNC_FUTURE_CORRECTION: &str =
    "Try adding an 'await' or wrapping the expression with 'unawaited'.";

pub(super) fn check(c: &mut Check<'_>, want_async: bool) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    // The context gate (the official `isInteresting` filter): the
    // nearest enclosing function body decides, closures included; no
    // body ancestor is the official `?? false` silence.
    if enclosing_body_is_async(c) != Some(want_async) {
        return;
    }
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    // The statement's expression pieces tile before its row.
    let run = slice::statement_run(c, &kids[..pos]);
    let Some(&root) = run.last() else {
        return;
    };
    let root = faces::unparenthesized(c, root);
    // A stored or awaited value is not discarded (:49-50).
    if matches!(
        c.kind(root),
        NodeKind::Assignment | NodeKind::AwaitExpression
    ) {
        return;
    }
    // The relevance gate (:88-90 + the `isInteresting` type face): the
    // static type's name walk lands on the core `Future`/`FutureOr`
    // face — the written name itself or a written supertype of it (the
    // upstream `asInstanceOf` cut: `Future2 implements Future` is
    // Future-relevant). [`same_file::resolve`] walks the written
    // clauses; a same-file declaration named `Future`/`FutureOr` is a
    // user shadow, never the core face (`NotCore`).
    let Some(Ty::Named { name, .. }) = faces::expr_type(c, analysis, &run) else {
        return;
    };
    let written = analysis.strings().resolve(name);
    if !matches!(
        same_file::resolve(c, written, &["Future", "FutureOr"]),
        same_file::Verdict::Core(_)
    ) {
        return;
    }
    if is_unawaited_wrapper(c, analysis, &run, root)
        || is_await_not_required(c, analysis, &run, root)
        || is_map_put_if_absent(c, analysis, &run, root)
    {
        return;
    }
    let (problem, correction) = texts(want_async);
    c.report_texts(selector_span(c, &run, root), problem, correction, &[]);
}

/// Whether the nearest enclosing function body carries `async` (the
/// official `thisOrAncestorOfType<FunctionBody>()` walk): both body
/// rows carry their `async` modifier in the `AsyncKeyword` word (the
/// absent word stored as [`NONE_TOKEN`]), and `sync*` is a
/// synchronous body.
fn enclosing_body_is_async(c: &Check<'_>) -> Option<bool> {
    let body = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| matches!(c.kind(row), NodeKind::BlockBody | NodeKind::ExpressionBody))?;
    let kw = c
        .field(body, PayloadField::AsyncKeyword)
        .unwrap_or(NONE_TOKEN);
    Some(kw != NONE_TOKEN && c.lexeme(kw) == "async")
}

/// The `unawaited`/`safeUnawaited` wrapper escape (the official
/// `isAwaitNotRequired` face, :55): a bare send of either name with no
/// resolved binding is the dart:async helper. A resolved binding is a
/// user declaration and reads through the ordinary type face.
fn is_unawaited_wrapper(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    root: u32,
) -> bool {
    if c.kind(root) != NodeKind::Send {
        return false;
    }
    let Some(pos) = run.iter().rposition(|&row| row == root) else {
        return false;
    };
    if slice::access_marker(c, run, pos, NodeKind::DotAccess).is_some() {
        return false;
    }
    let Some((head, _)) = slice::send_parts(c, run, pos) else {
        return false;
    };
    let selector = run[head];
    matches!(
        c.lexeme(c.head_token(selector)),
        "unawaited" | "safeUnawaited"
    ) && analysis.binding_of(query_site(c, selector)).is_none()
}

/// The `@awaitNotRequired` escape (the official `isAwaitNotRequired`,
/// extensions.dart:396-431): the dropped expression's resolved member
/// or declaration carries the annotation, or a member further up its
/// container's supertype chain does (the `superMembers.any` walk).
/// Three faces by root shape: a member invocation/property access
/// resolves through `Analysis::member_of`; a bare call's binding is a
/// top-level declaration (same-file metadata read) or an implicit-
/// `this` member of the enclosing class-like (`member_has_annotation`).
/// A same-file declaration that shadows the chain still walks its
/// written ancestors; a cross-file shadowing under-reports (the
/// annotation stays silent, matching the member face's miss posture).
fn is_await_not_required(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    root: u32,
) -> bool {
    match c.kind(root) {
        NodeKind::Send => {
            let Some(pos) = run.iter().rposition(|&row| row == root) else {
                return false;
            };
            let Some((head, _)) = slice::send_parts(c, run, pos) else {
                return false;
            };
            let selector_row = run[head];
            let selector = c.head_token(selector_row);
            if slice::access_marker(c, run, pos, NodeKind::DotAccess).is_some() {
                // The member-invocation face (`receiver.m()` — upstream
                // walks `superTypes.getMethod(name)` for this shape).
                let Some(receiver) = faces::single_receiver(c, &run[..head], selector) else {
                    return false;
                };
                member_is_await_not_required(
                    c,
                    analysis,
                    receiver,
                    c.lexeme(selector),
                    MemberQuery::Method,
                )
            } else {
                // The bare-call face: a top-level declaration or the
                // enclosing container's implicit-`this` member.
                match analysis.binding_of(query_site(c, selector_row)) {
                    Some(binding) if binding.kind == dartq_semantic::BindingKind::Decl => {
                        same_file_decl_annotated(c, c.lexeme(selector))
                    }
                    Some(binding) if binding.kind == dartq_semantic::BindingKind::Member => {
                        enclosing_member_annotated(
                            c,
                            analysis,
                            c.lexeme(selector),
                            MemberQuery::Method,
                        )
                    }
                    _ => false,
                }
            }
        }
        // The property-access face (`receiver.p`): the root is the
        // whole-access marker — the dot rides `Operator`, the member
        // name is the token after it. Upstream walks
        // `superTypes.getGetter(name)` for this shape.
        NodeKind::DotAccess => {
            let Some(pos) = run.iter().rposition(|&row| row == root) else {
                return false;
            };
            let Some(dot) = c.field(root, PayloadField::Operator) else {
                return false;
            };
            let selector = dot + 1;
            let Some(receiver) = faces::single_receiver(c, &run[..pos], selector) else {
                return false;
            };
            member_is_await_not_required(
                c,
                analysis,
                receiver,
                c.lexeme(selector),
                MemberQuery::Getter,
            )
        }
        _ => false,
    }
}

/// The `receiver.name` member face of the `@awaitNotRequired` check:
/// the resolved member's own captured star, then the same-file
/// supertype chain's same-named members (the upstream
/// `superMembers.any(hasAwaitNotRequired)` walk — a member declared on
/// `receiver`'s own type without the annotation does not end the
/// question when an ancestor declares the annotated one). `query`
/// splits the ancestor read the way upstream splits it
/// (`getMethod` for invocations, `getGetter` for property reads — a
/// field group answers through `getGetter`'s induced accessor, so the
/// getter arm reads `Variable` rows too).
fn member_is_await_not_required(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    receiver: u32,
    name: &str,
    query: MemberQuery,
) -> bool {
    let member = analysis.strings().intern(name);
    let Some(binding) = analysis.member_of(query_site(c, receiver), member) else {
        return false;
    };
    let annotation = analysis.strings().intern("awaitNotRequired");
    if binding
        .entry
        .annotations
        .iter()
        .any(|row| row.name == annotation)
    {
        return true;
    }
    same_file_ancestors_annotated(
        c,
        analysis.strings().resolve(binding.container),
        name,
        query,
    )
}

/// The bare-call member face of the `@awaitNotRequired` check: the
/// selector resolved to a member of the enclosing class-like
/// (implicit `this`). The semantic walk answers the self/ancestor hit;
/// the same-file ancestor walk covers the shadowed-ancestor shape the
/// first hit cannot see past.
fn enclosing_member_annotated(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    name: &str,
    query: MemberQuery,
) -> bool {
    let class_row = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    });
    let Some(class_row) = class_row else {
        return false;
    };
    let Some(container) = class_declaration_name(c, class_row) else {
        return false;
    };
    if analysis.member_has_annotation(
        analysis.strings().intern(container),
        analysis.strings().intern(name),
        analysis.strings().intern("awaitNotRequired"),
    ) == Some(true)
    {
        return true;
    }
    same_file_ancestors_annotated(c, container, name, query)
}

/// The same-file supertype-chain half of the `@awaitNotRequired` walk:
/// every written ancestor of the container named `container` whose own
/// `name` member carries the annotation's metadata star. `query`
/// mirrors upstream's `getMethod`/`getGetter` split — the getter arm
/// reads `Variable` rows too (a field's induced getter claims the
/// field's star, extensions.dart:361-362). Cross-file ancestors stay
/// inside `member_lookup`'s walk (the resolved entry already landed
/// through it).
fn same_file_ancestors_annotated(
    c: &Check<'_>,
    container: &str,
    name: &str,
    query: MemberQuery,
) -> bool {
    let queries: &[MemberQuery] = match query {
        MemberQuery::Getter => &[MemberQuery::Getter, MemberQuery::Variable],
        _ => &[query],
    };
    let Some(class_row) = same_file_declaration(c, container) else {
        return false;
    };
    same_file_ancestors(c, class_row, false)
        .into_iter()
        .any(|ancestor| {
            body_members(c, ancestor).into_iter().any(|member| {
                let named = queries
                    .iter()
                    .any(|&q| !member_matches(c, member, q, name).is_empty());
                if !named {
                    return false;
                }
                let Some(body) = c.unit.ast.parent(member) else {
                    return false;
                };
                member_annotations(c, body, member)
                    .into_iter()
                    .any(|meta| annotation_names(c, meta, "awaitNotRequired"))
            })
        })
}

/// Whether the same-file top-level declaration named `name` carries
/// the `awaitNotRequired` star — the declaration-side arm of the
/// official `element.metadata.hasAwaitNotRequired` check for a bare
/// callee resolved to this unit. A cross-file declaration keeps the
/// member face's miss posture (no declaration-level annotation query
/// reaches it at this face cut).
fn same_file_decl_annotated(c: &Check<'_>, name: &str) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    c.collect_direct(root).into_iter().any(|top| {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            return false;
        }
        c.collect_direct(top).into_iter().any(|row| {
            crate::rules::member_name(c, row).is_some_and(|name_row| {
                c.lexeme(c.head_token(name_row)) == name
                    && top_level_annotations(c, row)
                        .into_iter()
                        .any(|meta| annotation_names(c, meta, "awaitNotRequired"))
            })
        })
    })
}

/// The `Map.putIfAbsent` exemption (:83-86): a member invocation named
/// `putIfAbsent` over a `Map`-typed receiver — the written-name cut of
/// the official dart:core class face.
fn is_map_put_if_absent(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    root: u32,
) -> bool {
    if c.kind(root) != NodeKind::Send {
        return false;
    }
    let Some(pos) = run.iter().rposition(|&row| row == root) else {
        return false;
    };
    if slice::access_marker(c, run, pos, NodeKind::DotAccess).is_none() {
        return false;
    }
    let Some((head, _)) = slice::send_parts(c, run, pos) else {
        return false;
    };
    let selector = c.head_token(run[head]);
    if c.lexeme(selector) != "putIfAbsent" {
        return false;
    }
    let Some(receiver) = faces::single_receiver(c, &run[..head], selector) else {
        return false;
    };
    matches!(
        analysis.type_of(query_site(c, receiver)),
        Some(Ty::Named { name, .. }) if analysis.strings().resolve(name) == "Map"
    )
}

/// The report face (`_reportOnExpression`, :92-101): the selector — a
/// method name, property name, or callee — or the expression itself.
fn selector_span(c: &Check<'_>, run: &[u32], root: u32) -> core::ops::Range<u32> {
    match c.kind(root) {
        NodeKind::Send => {
            if let Some(pos) = run.iter().rposition(|&row| row == root) {
                if let Some((head, _)) = slice::send_parts(c, run, pos) {
                    return utf16_token_span(c, c.head_token(run[head]));
                }
                // A `NoArguments` tear-off send with a trailing
                // whole-access marker is the property-access tile
                // (`c.p`): `send_parts` bails on the childless group
                // and the marker's `Operator` word is the dot — the
                // property name is the token after it (the
                // `access_type`/`selector_name` idiom).
                if let Some(marker) = slice::access_marker(c, run, pos, NodeKind::DotAccess)
                    && let Some(dot) = c.field(marker, PayloadField::Operator)
                {
                    return utf16_token_span(c, dot + 1);
                }
            }
            run_span(c, run)
        }
        // The whole-access marker over a property read (`c.p`): the dot
        // rides `Operator`, so the property name is the token after it —
        // the same idiom the `Send` arm and the `awaitNotRequired` walk
        // use. The node's own token range is unusable here: the #585
        // contract ends a completed send's re-access one token past the
        // member (the follower, `;`), so the range tail is not the name.
        NodeKind::DotAccess => match c.field(root, PayloadField::Operator) {
            Some(dot) => utf16_token_span(c, dot + 1),
            None => c.utf16_span(root),
        },
        _ => run_span(c, run),
    }
}

/// The whole-expression span of a piece run: the union of every row's
/// span (min start, max end) — the `_ => expr` report face anchors the
/// full expression (`-a`, `a + b`), not the container row's own token.
fn run_span(c: &Check<'_>, run: &[u32]) -> core::ops::Range<u32> {
    let mut span = match run.first() {
        Some(&first) => c.utf16_span(first),
        None => return 0..0,
    };
    for &row in &run[1..] {
        let piece = c.utf16_span(row);
        span.start = span.start.min(piece.start);
        span.end = span.end.max(piece.end);
    }
    span
}

fn texts(want_async: bool) -> (&'static str, &'static str) {
    if want_async {
        (ASYNC_FUTURE_PROBLEM, ASYNC_FUTURE_CORRECTION)
    } else {
        (SYNC_FUTURE_PROBLEM, SYNC_FUTURE_CORRECTION)
    }
}
