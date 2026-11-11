//! Ticket #3890 pins: the `mapLiterals`/`futureUnion` family's
//! context-inference seam — controls over the faces the routed fixes
//! will touch, plus `#[ignore]`-gated acceptance pins for each routed
//! root (they fail while the bug stands, pass when the owning lane
//! lands, and stay out of the green suite until then).
//!
//! The family's premise — "the receive side lives in
//! `collection_literal.rs`" — does not hold: the granted file hosts
//! exactly one spelling probe (`collection_literal_starts_at`, the
//! `??=`/`=` write-target's literal check), and every divergent row
//! recomputes to a root OUTSIDE it. All roots below are sibling lanes'
//! exclusive scope this round, so they are ROUTED with the divergence
//! named and evidenced (zero silent drops; counts recomputed from the
//! gated census `census-20261104-092425` over the nine case ids
//! `test_mapLiterals` + `test_mapLiterals_topLevel` +
//! `test_mapLiteralsCanInferNull` + the six `test_futureUnion_*`
//! variants — the collection/argument-code rows only, 20 missing /
//! 12 extra: C1a 7 missing, C1b 5 missing, C2 2 missing + 1 extra,
//! C4 6 missing + 11 extra).
//!
//! # The four routed divergences
//!
//! **C1a — index-argument check vs `[]`/`[]=` on a `Map` receiver: 7
//! missing rows.** `x['hi'] = 'w'` over `Map<int, String>` must check
//! the index against `[]=`'s first formal `K` (upstream
//! `checkIndexExpressionIndex`, Dart SDK source
//! `pkg/analyzer/lib/src/error/error_detection_helpers.dart`:260-289 —
//! resolved through `resolveForWrite`, `resolver.dart`). A
//! USER-DECLARED `[]=` fires correctly today (`m2['hi'] = 'w'` over
//! `class M2 { void operator []=(int k, String v) {} }` reports
//! `argument_type_not_assignable`, oracle-verified 3.13.7); an SDK
//! `Map<K, V>` receiver stays silent — the `[]` element's `Object?`
//! key formal and the `[]=` element's `K` formal both resolve through
//! [`Engine::resolved_member`] at `invocation.rs`:10237-10256, and the
//! receiver-instantiation substitution (`recv_map`, built from
//! `container_params`/`foreign_container_params`) cannot name an SDK
//! container's parameters, so the bare `K` reaches
//! [`Engine::argument_verdict`] undecidable and `continue`s past the
//! `core_index_formal_ty` fallback (the `or_else` runs only when the
//! resolved-member chain answers `None`, never when the verdict is
//! undecidable). A `List<E>` receiver fires today because its `[]`
//! resolves `E operator [](int)` with a NON-parameter first formal.
//! Owner: the `invocation.rs` lane (B4) — either substitute the
//! receiver's written arguments before the verdict, or fall through to
//! `core_index_formal_ty` when the substituted formal is still a bare
//! parameter.
//!
//! **C1b — the `[]=` VALUE formal check does not exist anywhere: 5
//! missing rows.** `x[3] = 42` over `Map<int, String>` must check the
//! right side against `[]=`'s second formal `V` (upstream
//! `checkAssignment` reads `writeElement.type` — the resolved `[]=`
//! element's value type — and reports `invalid_assignment`; reproduces
//! for `List<int>` too: `l[0] = 'bad'` is an oracle
//! `invalid_assignment` we never emit). [`Engine::check_assignment`]
//! returns early on an `Intent::Operator` target
//! (`assignability.rs`:4562) and no other face owns the ask. Owner:
//! the `assignability.rs`/`listener.rs` seam (lanes V/P) — the write's
//! value checks against the `[]=` element's second formal under the
//! receiver's written arguments.
//!
//! **C2 — collection-literal leaf typing under-reads non-row leaves
//! and joins unrelated classes at `dynamic`, not their true LUB: 2
//! missing rows + 1 extra row (the two verdict sites of the same
//! inference).** `var x = {1: 'x', 2: 'y',
//! 3.0: new RegExp('.')}` must bind `Map<num, Pattern>` — the values'
//! least upper bound of `String` and `RegExp` is `Pattern` (both
//! implement it; upstream `least_upper_bound.dart` walks the
//! superinterface sets and picks the minimal-depth shared class). Our
//! leaf faces read the leaf row's RAW `ty` slot
//! (`leaf_row_ty_in`, `resolution.rs`:4115; `leaf_row_ty`,
//! `engine_core.rs`:4827; `exact_leaf_ty`, `engine_core.rs`:4611):
//! an instance-creation/send leaf carries `Ty::UNKNOWN` there (the
//! resolved type lives behind [`Engine::operand_static_ty`]'s callee
//! walk), so the leaf answers `None` and the fold collapses the whole
//! argument to `dynamic` (`entry_leaf_tys`, `init_entry_leaf_tys`).
//! And even with the leaf typed, the join faces
//! (`least_leaf_ty_from`, `init_leaf_least_from`, `exact_leaf_lub`)
//! never run the shared-supertype walk — unrelated non-nullable class
//! leaves lift to bare `Object`/`dynamic` where the oracle spells the
//! named superinterface. Result today: `x` binds `Map<num, dynamic>`,
//! and `Map<int, String> y = x` reports `'Map<num, dynamic>'` where
//! the oracle reports `'Map<num, Pattern>'` (the function-scope
//! variant emits the wrong-spelled row, 1 extra; the top-level `var
//! x2` binds nothing at all — `initializer_literal_ty` gates on
//! `resolve_local`, which never names a top-level variable — so the
//! same verdict site stays fully silent there instead, 1 missing).
//! Owner: the `resolution.rs`/`engine_core.rs` leaf-type faces (lanes
//! P/V) — read [`Engine::operand_static_ty`] for the leaf, then join
//! through the real LUB (the same superinterface-set walk the type
//! system's `leastUpperBound` performs).
//!
//! **C3 — the literal's context read skips `const`-prefixed
//! initializers: 5 missing rows (ADJACENT, outside the 32)**
//! (`test_downwardsInferenceOnMapLiterals`, the
//! `map_key_type_not_assignable`/`map_value_type_not_assignable`
//! entries). [`Engine::literal_context_ty`] (`listener.rs`:15004)
//! donates the declared type only when the byte before the literal's
//! `{`/`[` is `=` (or `=>`/`return`) — `const Map<int, String> l =
//! const {"hello": "w"};` precedes the braces with `t`, so the entry
//! checks never see the `Map<int, String>` context. The default-value
//! arm has the same gap (`[Map<int, String> m = const {...}]`). Owner:
//! the `listener.rs` lane (P) — the context read must step past a
//! `const` keyword (and into the default-value world), exactly as
//! upstream `typed_literal_resolver.dart`'s
//! `_computeFromContextType`/`_fromContextType` donates the declared
//! type to const and non-const literals alike.
//!
//! **C4 — `futureUnion`: the conditional's greatest-closure arm and
//! the async return's `FutureOr` context never reach the row fold: 6
//! missing + 11 extra rows across three sub-shapes.**
//!
//! * C4a, conditional typing under the async return context (4 extra +
//!   2 missing): `return x ? 42 : new Future.value(42)` inside
//!   `Future<int> g() async` resolves `Object` today and reports
//!   `return_of_invalid_type` at `g1`/`g2` (4 extras across the two
//!   `asyncConditional` variants) — upstream analyzes the conditional
//!   under the async body's context type `FutureOr<int>` (the
//!   flattened return; `body_inference_context.dart`), so `S =
//!   greatestClosure(K)` is `FutureOr<int>`, `T1 = int <: S` and
//!   `T2 = Future<int> <: S`, and `inference_update_3` types the node
//!   `S` (Dart SDK source
//!   `pkg/analyzer/lib/src/generated/static_type_analyzer.dart`:74-112
//!   — `T <: S ? T : (T1 <: S && T2 <: S ? S : T)`). [`Engine::
//!   fold_conditional`] (`engine_core.rs`:11159) joins only
//!   `LUB(then, else)` — it receives no context at all — so the same
//!   conditional in `var y = x ? 42 : new Future.value(42); return y;`
//!   leaves `y` bound to nothing usable and the oracle's
//!   `return_of_invalid_type` at `return y` goes missing (2 rows).
//! * C4b, `Future.value`'s `T` binds from neither argument nor
//!   context (2 extra): `return new Future.value([3])` inside
//!   `Future<List<int>> g3() async` reports `'Future<dynamic>'` — the
//!   send-close instantiation does not solve `T := List<int>` from
//!   the `[3]` argument (upstream `invocation_inferrer.dart`'s
//!   argument-constraint solve at :345/:354), nor adopt the return
//!   context. Owner: the `invocation.rs` instantiation faces (B4).
//! * C4c, the `then` callback's `S`/`T` formals reach the verdict
//!   raw (4 missing + 5 extra): `f.then((_) => new
//!   Future.value('hi'))` under a `Future<int>`/`MyFuture` receiver —
//!   the `FutureOr<S>`-returning callback's inner send must see
//!   `FutureOr<int>`/`int` through the resolved `S`, but the
//!   unsubstituted `T?`/`FutureOr<S>` formals reach
//!   `argument_verdict`: the oracle's `'String'` vs
//!   `'FutureOr<int>?'`/`'int?'` rows go missing while our `'String'`
//!   vs `'T?'` rows are the extras. Owner: the `invocation.rs`
//!   generic-send formal substitution (B4), with the callback-return
//!   context plumbing beside C4a in `engine_core.rs` (lane V).
//!
//! Offsets are UTF-16; `at` shifts by [`CORE_PRELUDE`] because the rig
//! carries the fixture behind it on one line. All fixtures are ASCII.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` in the fixture, shifted past the core prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— controls (green today; guard the faces the routed work touches) ——

/// The var-bind leaf fold types an all-primitive map literal exactly:
/// `var x = {1: 'x', 2: 'y'}` binds `Map<int, String>`, so the
/// disagreeing declaration reports the inferred instantiation.
#[test]
fn control_var_map_literal_binds_primitive_args() {
    let source = r#"
void f() {
  var x = {1: 'x', 2: 'y'};
  Map<int, int> bad = x;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'Map<int, String>'") && rows[0].2.contains("'Map<int, int>'"),
        "{}",
        rows[0].2
    );
}

/// The C1a control: a USER-declared `[]=` checks its index argument
/// through `resolved_member` — the oracle reports
/// `argument_type_not_assignable` on `m2['hi']`, and we already do.
/// The routed fix must not regress this path while unblocking the SDK
/// `Map` receiver.
#[test]
fn control_index_write_user_operator_checks_key() {
    let source = r#"
class M2 {
  void operator []=(int k, String v) {}
  String operator [](int k) => '';
}
void f(M2 m2) {
  m2['hi'] = 'w';
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        rows[0].0,
        at(source, "m2['hi'") + "m2[".len() as u32,
        "{:#?}",
        rows
    );
}

/// The C2 control: the leaf fold keeps a UNIFORM non-int/double leaf
/// type — `{3.0: 'a', 1.0: 'b'}` keys join at `double`, values at
/// `String`, and `Map<double, String>` is what the disagreeing
/// declaration reports. The LUB extension must keep this arm exact.
///
/// Oracle 3.13.7: `Map<double, String>` reports `invalid_assignment`
/// against `Map<int, String>` (covariant `double <: int` refuses); the
/// `Map<num, String>` twin stays silent because `double <: num`
/// accepts. The sink here must be `Map<int, String>` so the verdict
/// fires and names the inferred face.
#[test]
fn control_map_literal_uniform_leaves_join_exact() {
    let source = r#"
void f() {
  var x = {3.0: 'a', 1.0: 'b'};
  Map<int, String> bad = x;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(rows[0].2.contains("'Map<double, String>'"), "{}", rows[0].2);
}

/// The C3 control: a NON-const literal under a `Map<int, String>`
/// declaration donates the entry context and reports
/// `map_key_type_not_assignable` at the wrong-typed key — the arm
/// `const` must join, not disturb.
#[test]
fn control_declared_map_context_checks_entries() {
    let source = r#"
void f() {
  Map<int, String> m = {'hi': 'w'};
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "map_key_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        rows[0].0,
        at(source, "= {'hi'") + "= {".len() as u32,
        "{:#?}",
        rows
    );
}

/// The C1a contract on the rig's own `Map`: the prelude `Map` declares
/// `[]` but no `[]=`, so `resolved_member` misses and
/// `core_index_formal_ty`'s fallback already answers `int` — the row
/// fires here. The live divergence is the resolved-member
/// substitution arm upstream of this fallback (`recv_map`), which the
/// rig does not model; this pin holds the contract the routed fix
/// must keep.
#[test]
fn control_index_write_map_receiver_checks_key() {
    let source = r#"
void f(Map<int, String> m) {
  m['hi'] = 'w';
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("argument type 'String'") && rows[0].2.contains("parameter type 'int'"),
        "{}",
        rows[0].2
    );
}

// —— routed acceptance pins (fail-before, pass-after; #[ignore]-gated
// —— until the owning lane lands) ——

/// C1b acceptance: `l[0] = 'bad'` over `List<int>` reports the VALUE
/// against `[]=`'s `E` formal — `invalid_assignment`, `'String'` into
/// `'int'`'s slot — and `m[3] = 42` over `Map<int, String>` reports
/// `'int'` into `'String'`'s slot; upstream anchors the whole right
/// side (`checkAssignment` reads `writeElement.type`).
#[test]
fn index_write_value_checks_against_value_formal() {
    let source = r#"
void f(List<int> l, Map<int, String> m) {
  l[0] = 'bad';
  m[3] = 42;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'String'") && rows[0].2.contains("'int'"),
        "{}",
        rows[0].2
    );
    assert!(
        rows[1].2.contains("'int'") && rows[1].2.contains("'String'"),
        "{}",
        rows[1].2
    );
}

/// C2 acceptance: `new RegExp('.')` as a map VALUE leaf contributes
/// its creation type, and the values' LUB of `String` and `RegExp`
/// spells the shared superinterface `Pattern` — the var binds
/// `Map<num, Pattern>`, so `Map<num, Pattern>` accepts it silently
/// and `Map<int, String>` reports that spelling.
#[test]
#[ignore = "leaf faces read row.ty raw (resolution.rs:4115, engine_core.rs:4827) and join unrelated classes at dynamic/Object instead of the superinterface LUB (least_upper_bound.dart) — 1 extra + 2 missing census rows; re-verified 2026-11-05: still fails"]
fn creation_leaf_joins_shared_superinterface() {
    let source = r#"
class Pattern {}
class RegExp implements Pattern {
  RegExp(String source) {}
}
void f() {
  var x = {1: 'x', 2: 'y', 3.0: new RegExp('.')};
  Map<num, Pattern> good = x;
  Map<int, String> bad = x;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'Map<num, Pattern>'"),
        "expected the LUB-joined 'Map<num, Pattern>' face, got {}",
        rows[0].2
    );
}

/// C3 acceptance: a `const`-prefixed literal under a declared
/// `Map<int, String>` still donates the entry context — the wrong key
/// reports `map_key_type_not_assignable` at `'hi'` exactly as the
/// non-const control above does.
#[test]
#[ignore = "literal_context_ty (listener.rs:15004) only reads the byte before the braces; a const prefix and the default-value world donate no context — 5 census rows; re-verified 2026-11-05: still fails (0 map_key_type_not_assignable rows, expected 1)"]
fn const_map_literal_inherits_declared_context() {
    let source = r#"
void f() {
  const Map<int, String> m = const {'hi': 'w'};
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "map_key_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
}
