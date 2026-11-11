//! Ticket #3893 pins: the inference-tail family's four divergent
//! shapes, each `#[ignore]`-gated until its owning lane lands the
//! root (they fail while the bug stands, pass when the fix lands,
//! and stay out of the green suite until then).
//!
//! Every divergent row recomputes to a root OUTSIDE this lane's two
//! free files (`index_result.rs`, `operand_face.rs`) — the
//! `flatten`/`futureType` family in `engine_core.rs` (Lane V), the
//! generic-invocation inference faces in `invocation.rs` (Lane B4),
//! and the irrefutable-pattern required-type check in
//! `scopes_bodies.rs` (Lane W). Recon greps found no emission site
//! or result-row fold in either free file for these codes: the
//! `[]`-read result (`index_result.rs`) is never on the divergent
//! paths, and the numeric/nullable operand faces
//! (`operand_face.rs`) own only `unchecked_use_of_nullable_value`/
//! `invalid_use_of_null_value`/`invalid_null_aware_operator` — none
//! of which appears in the family.
//!
//! Counts recomputed from the gated census `census-20261104-092425`
//! over the four case ids `derive_future_type_test`,
//! `type_inference_A29_t02`, `test_instanceCreation_viaTypeAlias_
//! notWellBounded#0`, and `int_to_double_test`: 24 missing / 16
//! extra in-scope rows (per-shape splits below).
//!
//! # The four routed divergences
//!
//! **S1 — `flatten` never walks the "derives a future type" graph:
//! 10 missing + 12 extra rows in `derive_future_type_test`.** The
//! oracle's `await e` resolves through `TypeSystem.futureType`
//! (Dart SDK source `pkg/analyzer/lib/src/dart/element/
//! type_system.dart`:456-468): `T.asInstanceOf(futureElement)`
//! walks a class's superinterface chain and a type variable's
//! bound, then `_futureTypeOfBounded` (:1843-1875) recurses
//! through `FutureOr`/`Future?`/`FutureOr?` spellings. Our
//! [`flatten_ty`] (`engine_core.rs`:13472) answers the written
//! head only — a bare `Future`/`FutureOr` arg or the operand
//! itself — and [`Engine::flatten_param`] (`engine_core.rs`:9074)
//! adds only the in-scope parameter's captured bound. The
//! superinterface arm is documented as missing at
//! `engine_core.rs`:13469-13471. Three faces diverge today:
//!
//! * `class SubFuture … implements Future<int>` /
//!   `extension type ExtensionFuture(…) implements Future<String?>`
//!   / mixin-/enum-`implements` heads — `await x` keeps the raw
//!   receiver instead of the interface-derived `S`/`S?`. The
//!   `Exactly<int>`/`Exactly<String?>` type args then report
//!   `type_argument_not_matching_bounds` at 97/102/112/244/247/
//!   250/377/380/383 (9 extras), and `f1`'s `FutureOr<R>
//!   Function(T)` argument can't be assigned to itself at 53,
//!   `Null`→`X3` at 214, `bool`→`X` at 226 (3 more extras).
//! * `x is X1` where `X1 extends Future<Never>`: the promoted
//!   bound's future type is `Future<Never>` (rule 4.1 of the
//!   derives-future spec — the type variable's bound walks), so
//!   `await x` types `Never`, and the oracle emits
//!   `receiver_of_type_never` at `(await x)` plus `dead_code` on
//!   the trailing statements (4 missing at 313/447). We keep `X`
//!   or `Unknown` and emit `undefined_method` extras instead.
//! * `x is X1?` (nullable promoted bound): upstream
//!   `_futureTypeOfBounded(X1?)` consults `X1`'s ELEMENT bound —
//!   `Future<Never>` is non-nullable and not `FutureOr`, so the
//!   walk returns null and `flatten(X & X1?)` falls through to
//!   `flatten(X)` = `X` (:406-422). Our `flatten_param` strips
//!   the `?` and reads `X1`'s bound `Future<Never>` directly,
//!   answering `Never?` — the oracle's
//!   `type_argument_not_matching_bounds` on `Exactly<Never?>`/
//!   `Exactly<Object?>`/`Exactly<Symbol?>` at 333/338/349/467/
//!   472/483 goes missing (the routed 6M).
//!
//!   Owner: the `engine_core.rs` lane (V) — `flatten_ty` needs
//!   the `asInstanceOf(futureElement)` superinterface walk and
//!   `flatten_param` the `_futureTypeOfBounded` two-arm (element
//!   bound then promoted bound) split.
//!
//! **S2 — the map-pattern key's `const A()` runs no generic
//! constructor inference: 12 missing rows in
//! `type_inference_A29_t02`.** `<A<dynamic>, String>{const A():
//! 1}` type-checks the key under context `C = A<dynamic>` (co19
//! spec comment: map patterns with written type args key-check
//! each key under `C`). Downward inference on `A()` then solves
//! `T := A<Object?>`, which violates `T extends A<T>` — the
//! oracle emits `could_not_infer` ("Tried to infer 'A<Object?>'
//! for 'T' … bound 'A<T>' producing 'A<A<Object?>>'") and
//! `type_argument_not_matching_bounds` at the key, at all four
//! sites (31/39/46/53). The `String`-typed value `1` also earns
//! `constant_pattern_never_matches_value_type` (matched `String`
//! vs constant `int`). Our engine emits nothing at any site —
//! the pattern's key expression never reaches the generic-send
//! inference path. Owners: the `invocation.rs` inference faces
//! (B4) for `could_not_infer`/`type_argument_not_matching_
//! bounds` (emission sites `invocation.rs`:7249/:7583/:8875);
//! the `engine_core.rs`/`diagnose.rs` pattern faces (V/main)
//! for `constant_pattern_never_matches_value_type`
//! (`engine_core.rs`:12435, `diagnose.rs`:2502).
//!
//! **S3 — factory constructors reached THROUGH a type alias skip
//! the could_not_infer face: 2 missing rows in
//! `test_instanceCreation_viaTypeAlias_notWellBounded`.**
//! `typedef A<X extends G<C<X>>> = C<X>` with `A()`, `A.foo()`,
//! `A.bar()`: the oracle emits `could_not_infer` at all three —
//! the alias's recursive bound `G<C<X>>` makes `X`'s inferred
//! `C<Object?> Function(C<Never>)` violate it. We emit only the
//! bare `A()` row; the two factory spellings (`A.foo()` — a
//! factory body through the alias; `A.bar()` — a redirecting
//! factory `= C`) reach no inference path at all. Owner: the
//! `invocation.rs` lane (B4) — the constructor-through-alias
//! send-close must run the same could_not_infer verdict the
//! bare-call arm already does.
//!
//! **S4 — int-to-double coercion never applies under a pattern's
//! required type: 2 extra rows in `int_to_double_test`.**
//! `var (double d) = 123;` and `(d) = 234;` push `double` as the
//! matched-value context onto the `int` literal — the literal's
//! static type becomes `double` upstream (the spec's int-to-
//! double conversion inside a pattern context; `type_analyzer.
//! dart`'s `isSubtypeOf` then sees `double <: double`). Our
//! matched value stays `int`, so the irrefutable-context check
//! (`scopes_bodies.rs`:4280) reports
//! `pattern_type_mismatch_in_irrefutable_context` twice. Owner:
//! the `scopes_bodies.rs` lane (W) — the required-type/context
//! donation must apply the int-literal coercion before the
//! subtype ask.
//!
//! Offsets are UTF-16; `at` shifts by [`CORE_PRELUDE`] because the
//! rig carries the fixture behind it on one line. All fixtures are
//! ASCII.

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

/// Byte offset of `needle` in the fixture, shifted past the core
/// prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— controls (green today; guard the faces the routed work
// —— touches) ——

/// A literal `Future<int>` receiver flattens the written head:
/// `await f` types `int`, so the disagreeing declaration reports
/// the inferred face. The superinterface walk must keep this arm.
#[test]
fn control_future_head_flattens_written_arg() {
    let source = r#"
void f(Future<int> f) async {
  int a = await f;
  String bad = a;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'int'") && rows[0].2.contains("'String'"),
        "{}",
        rows[0].2
    );
}

/// A bare `int` literal keeps `int` where no pattern context
/// donates `double`: `int i = 123` still binds `int`, and the
/// disagreeing `String` declaration reports the pair. The coercion
/// arm must stay context-gated, not type-the-literal `double`
/// unconditionally.
#[test]
fn control_int_literal_stays_int_without_double_context() {
    let source = r#"
void f() {
  int i = 123;
  String s = i;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'int'") && rows[0].2.contains("'String'"),
        "{}",
        rows[0].2
    );
}

// —— routed acceptance pins (fail-before, pass-after;
// —— `#[ignore]`-gated until the owning lane lands) ——

/// S1a acceptance: `class SubFuture implements Future<int>` —
/// `await x` resolves through the interface-derived future type
/// `Future<int>` (upstream `T.asInstanceOf(futureElement)`), so
/// `int a = await x` is silent and `String bad = a` reports the
/// `int` face. Landed by the #3897 `Engine::flatten` derived-future arm
/// (`future_supertype_arg`); it failed before by keeping `SubFuture` and
/// reporting `invalid_assignment 'SubFuture'→'int'`.
#[test]
#[ignore = "held: flatten_ty (engine_core.rs:13472) lacks the T.asInstanceOf(futureElement) superinterface walk (upstream type_system.dart:456-468, _futureTypeOfBounded :1843-1875); re-verified 2026-11-05 vs oracle 3.13.7: still fails — `await x` keeps `SubFuture` and emits invalid_assignment 'SubFuture'->'int' (the implements_non_class extra on the implements arm is fixed, #4001); owner engine_core.rs lane V"]
fn await_uses_interface_derived_future_type() {
    let source = r#"
class SubFuture implements Future<int> {
  SubFuture();
}
void f(SubFuture x) async {
  int a = await x;
  String bad = a;
}
"#;
    let report = report_ops(source);
    let assign = rows_of(&report, "invalid_assignment");
    assert_eq!(assign.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        assign[0].2.contains("'int'") && assign[0].2.contains("'String'"),
        "{}",
        assign[0].2
    );
}

/// S1b acceptance: `x is X1` where `X1 extends Future<Never>` —
/// the promoted bound's own future type is `Future<Never>`
/// (upstream `_futureTypeOfBounded` on the promoted bound), so
/// `await x` types `Never` and the receiver read below reports
/// `receiver_of_type_never` plus the `dead_code` tail. Fails
/// today: `await x` keeps `X`/`Unknown` and the member read
/// reports `undefined_method`.
#[test]
#[ignore = "held: flatten_param (engine_core.rs:9074) lacks the _futureTypeOfBounded promoted-bound arm (upstream type_system.dart:1843-1875, rule 4.1 walks X1's bound to Future<Never>); re-verified 2026-11-05 vs oracle 3.13.7: still fails — `await x` types `Unknown` and the member read reports undefined_method 'abs' instead of receiver_of_type_never + dead_code; owner engine_core.rs lane V"]
fn await_promoted_bound_derives_never() {
    let source = r#"
void f<X, X1 extends Future<Never>>(X x) async {
  if (x is X1) {
    var a = await x;
    a.abs();
  }
}
"#;
    let report = report_ops(source);
    let never_rows = rows_of(&report, "receiver_of_type_never");
    let dead_rows = rows_of(&report, "dead_code");
    assert_eq!(never_rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(dead_rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(never_rows[0].0, at(source, "a.abs"), "{:#?}", never_rows);
}

/// S1c acceptance: `x is X1?` — the NULLABLE promoted bound
/// short-circuits the future walk: upstream
/// `_futureTypeOfBounded(X1?)` consults `X1`'s ELEMENT bound
/// (`Future<Never>` is non-nullable and not `FutureOr` → null),
/// so `flatten(X & X1?)` falls to `flatten(X)` = `X`
/// (type_system.dart:406-422). `Never? a = await x` then reports
/// `invalid_assignment 'X'→'Never?'`. Fails today: our
/// `flatten_param` strips `?` and reads the bound directly,
/// answering `Never?` — the row stays silent.
#[test]
#[ignore = "held: flatten_param (engine_core.rs:9074) strips `?` and reads X1's bound directly instead of the upstream _futureTypeOfBounded(X1?) two-arm split (element bound then promoted bound, type_system.dart:406-422) — flatten(X & X1?) should fall through to flatten(X)=X; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `await x` keeps X so the invalid_assignment 'X'->'Never?' row stays silent; owner engine_core.rs lane V"]
fn await_nullable_promoted_bound_keeps_parameter() {
    let source = r#"
void f<X, X1 extends Future<Never>>(X x) async {
  if (x is X1?) {
    Never? a = await x;
  }
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'X'") && rows[0].2.contains("'Never?'"),
        "{}",
        rows[0].2
    );
}

/// S2 acceptance: `<A<dynamic>, String>{const A(): 1}` key-checks
/// `const A()` under the map pattern's key context `A<dynamic>` —
/// the recursive bound `T extends A<T>` makes the solved
/// `A<Object?>` violate it, so the oracle emits `could_not_infer`
/// and `type_argument_not_matching_bounds` at the key, plus
/// `constant_pattern_never_matches_value_type` on the `String`-
/// typed value `1` (matched `String` vs constant `int`). Fails
/// today: zero rows at every site.
#[test]
#[ignore = "held: the map-pattern key's `const A()` never reaches the generic-send inference path — could_not_infer/type_argument_not_matching_bounds live on invocation.rs:7249/:7583/:8875 and constant_pattern_never_matches_value_type on engine_core.rs:12435/diagnose.rs:2502; re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero rows at the key; owners invocation.rs lane B4 + engine_core.rs/diagnose.rs pattern faces"]
fn map_pattern_key_runs_generic_inference() {
    let source = r#"
class A<T extends A<T>> {
  const A();
}
void test1() {
  switch ({} as dynamic) {
    case <A<dynamic>, String>{const A(): 1}:
      break;
  }
}
"#;
    let report = report_ops(source);
    let cni = rows_of(&report, "could_not_infer");
    let bounds = rows_of(&report, "type_argument_not_matching_bounds");
    let never = rows_of(&report, "constant_pattern_never_matches_value_type");
    assert_eq!(cni.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(bounds.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(never.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(cni[0].0, at(source, "const A()"), "{:#?}", cni);
}

/// S3 acceptance: `A.foo()`/`A.bar()` through
/// `typedef A<X extends G<C<X>>> = C<X>` run the same
/// could_not_infer verdict the bare `A()` already does — the
/// factory spellings reach the constructor-through-alias
/// inference face upstream. Fails today: only the bare call
/// emits.
#[test]
#[ignore = "held: the constructor-through-alias send-close never runs the could_not_infer verdict the bare-call arm already does (upstream pkg/analyzer); re-verified 2026-11-05 vs oracle 3.13.7: still fails — zero could_not_infer rows on `A.foo()`/`A.bar()` factory spellings; owner invocation.rs lane B4"]
fn factory_through_alias_runs_could_not_infer() {
    let source = r#"
class C<X> {
  C();
  factory C.foo() => C();
  factory C.bar() = C;
}
typedef G<X> = X Function(X);
typedef A<X extends G<C<X>>> = C<X>;
void f() {
  A.foo();
  A.bar();
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "could_not_infer");
    assert_eq!(rows.len(), 2, "{:#?}", report.diagnostics);
    assert!(rows[0].2.contains("'X'"), "{}", rows[0].2);
    assert!(rows[1].2.contains("'X'"), "{}", rows[1].2);
}

/// S4 acceptance: the pattern context `double` coerces the `int`
/// literal — `var (double d) = 123;` types `d` as `double` (the
/// literal's context applies the int-to-double conversion
/// upstream), so no `pattern_type_mismatch_in_irrefutable_
/// context` row fires. Fails today: the matched value stays
/// `int` and the verdict reports `'int'→'double'` extras.
#[test]
#[ignore = "held: the pattern context's required-type donation never applies the int-to-double coercion before the subtype ask (upstream type_analyzer.dart:353-365 isSubtypeOf sees double<:double after the conversion); re-verified 2026-11-05 vs oracle 3.13.7: still fails — pattern_type_mismatch_in_irrefutable_context 'int'->'double' fires on `var (double d) = 123` and `(x) = 234`; owner scopes_bodies.rs:4280 lane W"]
fn pattern_context_coerces_int_literal_to_double() {
    let source = r#"
void f() {
  var (double d) = 123;
  double x = 0;
  (x) = 234;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "pattern_type_mismatch_in_irrefutable_context");
    assert_eq!(rows.len(), 0, "{:#?}", report.diagnostics);
}
