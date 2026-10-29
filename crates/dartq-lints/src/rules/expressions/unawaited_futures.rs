use super::{Check, NodeKind, RuleSpec, unused_futures};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unawaited_futures.dart` — the same
/// `Future`/`FutureOr` statement face as `discarded_futures` (the
/// shared `UnusedFuturesVisitor`), only concerned with code in async
/// functions (:38-40 — the `isInteresting` gate).
///
/// dartq faces: the expression-statement face with the async context
/// gate — the nearest enclosing function body (closures included) is
/// read for its `async` modifier word off the body payload; a body
/// with no decided word stays silent. The escapes and approximations
/// are the shared walk's (see `discarded_futures`). Message:
/// `diagnostic.g.dart` `unawaitedFutures` (the `FutureOr` shape
/// reports the paired `unawaited_future_or` texts).
pub(crate) static SPEC: RuleSpec =
    crate::spec!("unawaited_futures", &[NodeKind::ExpressionStatement], check,);

fn check(c: &mut Check<'_>) {
    unused_futures::check(c, true);
}
