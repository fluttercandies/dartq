use dartq_semantic::model::Ty;

use super::{Check, NodeKind, query_site, slice, static_type_faces as faces};
use crate::rules::{NONE_TOKEN, same_file_declaration, utf16_token_span};

// The official message templates (`diagnostic.g.dart`): the Future
// shapes report `discardedFutures`/`unawaitedFutures`, the FutureOr
// shapes the paired `*FutureOr` codes under the same rule names.
const SYNC_FUTURE_PROBLEM: &str = "'Future'-returning calls in a non-'async' function.";
const SYNC_FUTURE_CORRECTION: &str = "Try either making the enclosing function 'async' and awaiting the expression or wrapping the expression in 'unawaited'.";
const SYNC_FUTURE_OR_PROBLEM: &str = "'FutureOr'-returning calls in a non-'async' function.";
const SYNC_FUTURE_OR_CORRECTION: &str =
    "Try making the enclosing function 'async' and awaiting the expression.";
const ASYNC_FUTURE_PROBLEM: &str =
    "Missing an 'await' for the 'Future' computed by this expression.";
const ASYNC_FUTURE_CORRECTION: &str =
    "Try adding an 'await' or wrapping the expression with 'unawaited' (if it is a 'Future').";
const ASYNC_FUTURE_OR_PROBLEM: &str =
    "Missing an 'await' for the 'FutureOr' computed by this expression.";
const ASYNC_FUTURE_OR_CORRECTION: &str = "Try adding an 'await'.";

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
    if is_unawaited_wrapper(c, analysis, &run, root)
        || is_map_put_if_absent(c, analysis, &run, root)
    {
        return;
    }
    // The relevance gate (:88-90): the static type is `Future<...>` or
    // `FutureOr<...>` — the written-name face; a same-file declaration
    // of either name is not the core type and stays silent.
    let Some(Ty::Named { name, .. }) = faces::expr_type(c, analysis, &run) else {
        return;
    };
    let written = analysis.strings().resolve(name);
    if (written != "Future" && written != "FutureOr")
        || same_file_declaration(c, written).is_some()
    {
        return;
    }
    let (problem, correction) = texts(want_async, written);
    c.report_texts(selector_span(c, &run, root), problem, correction, &[]);
}

/// Whether the nearest enclosing function body carries `async` (the
/// official `thisOrAncestorOfType<FunctionBody>()` walk): the
/// `BlockBody`/`ExpressionBody` payload holds the modifier words
/// (block `[async, star]`, expression `[arrow, end, async, star]` —
/// the `async_words` layout, the absent word stored as
/// [`NONE_TOKEN`]), and `sync*` is a synchronous body.
fn enclosing_body_is_async(c: &Check<'_>) -> Option<bool> {
    let body =
        c.path.iter().rev().copied().find(|&row| {
            matches!(c.kind(row), NodeKind::BlockBody | NodeKind::ExpressionBody)
        })?;
    let slot = match c.kind(body) {
        NodeKind::BlockBody => 0,
        _ => 2,
    };
    let kw = c.payload(body).get(slot).copied().unwrap_or(NONE_TOKEN);
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
            if let Some(pos) = run.iter().rposition(|&row| row == root)
                && let Some((head, _)) = slice::send_parts(c, run, pos)
            {
                return utf16_token_span(c, c.head_token(run[head]));
            }
            c.utf16_span(root)
        }
        NodeKind::DotAccess => match c.unit.ast.token_range(root).next_back() {
            Some(selector) => utf16_token_span(c, selector),
            None => c.utf16_span(root),
        },
        _ => c.utf16_span(root),
    }
}

fn texts(want_async: bool, written: &str) -> (&'static str, &'static str) {
    match (want_async, written) {
        (false, "FutureOr") => (SYNC_FUTURE_OR_PROBLEM, SYNC_FUTURE_OR_CORRECTION),
        (false, _) => (SYNC_FUTURE_PROBLEM, SYNC_FUTURE_CORRECTION),
        (true, "FutureOr") => (ASYNC_FUTURE_OR_PROBLEM, ASYNC_FUTURE_OR_CORRECTION),
        (true, _) => (ASYNC_FUTURE_PROBLEM, ASYNC_FUTURE_CORRECTION),
    }
}
