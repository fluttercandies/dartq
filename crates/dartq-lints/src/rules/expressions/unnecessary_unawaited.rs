use dartq_semantic::model::Ty;

use super::{Check, NodeKind, RuleSpec, query_site, slice, static_type_faces as faces};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_unawaited.dart` — a bare
/// `unawaited(...)` invocation over exactly one argument whose
/// expression carries no await requirement reports at the method name
/// (:36-57; `hasAwaitNotRequired` — the argument is not `Future`
/// typed). A prefixed invocation (`node.target != null`, :38) and any
/// other argument count skip (:42).
///
/// dartq faces over the production tiling: the argument's static type
/// is the piece-run face (an identifier read, an invocation, or a
/// single-name member access — the official switch's shapes); a
/// written `Future`/`FutureOr` keeps the wrapper necessary and
/// everything without a decided name stays silent. The dart:async
/// library gate (:62-64) is the resolution face: a name that resolves
/// to any declaration (same file or imported) is not the core
/// `unawaited` and stays silent. Approximations (documented,
/// under-report): `dynamic`-typed arguments, operator expressions,
/// and creations stay silent (the official switch's element faces);
/// a cross-library `Future` alias reads by its written name
/// (over-report, the alias family). Message: `diagnostic.g.dart`
/// `unnecessaryUnawaited`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_unawaited",
    &[NodeKind::Send],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, Some(arguments))) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // The bare-invocation shape (:38) and the dart:async gate
    // (:61-64): a resolved binding of the name is a user declaration.
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some()
        || analysis.binding_of(query_site(c, kids[head])).is_some()
    {
        return;
    }
    if c.lexeme(c.head_token(kids[head])) != "unawaited" {
        return;
    }
    // Exactly one argument (:42).
    let raw = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &raw);
    let [range] = &ranges[..] else {
        return;
    };
    let value: &[u32] = if c.kind(raw[range.end - 1]) == NodeKind::NamedArgument {
        &raw[range.start + 1..range.end - 1]
    } else {
        &raw[range.clone()]
    };
    // The not-Future face (:44-54): a decided written name that is
    // neither `Future` nor `FutureOr` reports.
    let Some(Ty::Named { name, .. }) = faces::expr_type(c, analysis, value) else {
        return;
    };
    let written = analysis.strings().resolve(name);
    if written == "Future" || written == "FutureOr" {
        return;
    }
    let range = utf16_token_span(c, c.head_token(kids[head]));
    c.report(range, &[]);
}
