use super::{Check, NodeKind, RuleSpec, unused_futures};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/discarded_futures.dart` — a `Future`- or
/// `FutureOr`-typed expression dropped as a statement reports through
/// the shared `UnusedFuturesVisitor` (`util/unused_futures.dart`). The
/// rule is only concerned with code in sync functions (:38-40 — the
/// `isInteresting` gate; `async` bodies belong to `unawaited_futures`)
/// and registers expression statements, cascades, and interpolation
/// expressions (:42-44).
///
/// dartq faces over the production tiling: the expression-statement
/// face — the statement row trails its expression pieces, whose static
/// type is the shared piece-run face (a bare invocation's callee, a
/// member invocation, a property access, or a bare read). The escapes
/// read structurally: an assignment or `await` root
/// (`util/unused_futures.dart:49-50`), a bare core
/// `unawaited`/`safeUnawaited` wrapper (the official
/// `isAwaitNotRequired` face — a resolved binding is a user
/// declaration), and a `Map.putIfAbsent` invocation over a `Map`-typed
/// receiver (:83-86) stay silent; a same-file `Future`/`FutureOr`
/// declaration is not the core type. Approximations (documented): the
/// cascade and interpolation arms stay silent (the M1 face cut), and
/// the `Future.delayed(d, computation)` creation exemption (:73-77)
/// folds into the same silence — a creation carries no
/// declared-return face and never reports. Message:
/// `diagnostic.g.dart` `discardedFutures` (the `FutureOr` shape
/// reports the paired `discarded_future_or` texts).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "discarded_futures",
    &[NodeKind::ExpressionStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    unused_futures::check(c, false);
}
