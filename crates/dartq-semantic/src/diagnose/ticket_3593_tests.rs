//! #3593 pins — the `type_argument_not_matching_bounds` fabricated-row
//! family the 20261102-080528 census isolated (920 extra rows): the
//! extension-member bounds face over an UNGROUNDED receiver face.

use super::test_rig::{code_names, report_ops, rows};

/// The shared `expectStaticType` scaffolding: the census cluster's
/// receiver-typing shapes all ride `extension StaticType<T> on T` with
/// `R extends Exactly<T>`, where the bound `Exactly<T>` is only as good
/// as the receiver face the engine grounds.
const EXTENSION_SCAFFOLD: &str = "\
class A {}
class B1<T> implements A {}
class C1<T> implements B1<T> {}
class C2<T> implements B1<T> {}
class CallableClass<T> { T call() => throw ''; }
typedef Exactly<T> = T Function(T);
extension StaticType<T> on T {
  T expectStaticType<R extends Exactly<T>>() => this;
}
";

/// Oracle evidence (2026-11-02, `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart
/// analyze --format=machine`, fixture `/tmp/3593fx/fx1/lib/test2.dart`
/// mirroring the census shapes): NO `type_argument_not_matching_bounds`
/// rows — the receiver `(local2 ??= callableClassC2Int)` types as the
/// coerced join `B1<int> Function()`/`A Function()`
/// (`UP(NonNull(T1), T2')` with the callable-object coercion), never the
/// RHS's `CallableClass<C2<int>>`.
#[test]
fn if_null_assignment_receiver_abstains() {
    let source = format!(
        "{EXTENSION_SCAFFOLD}\
void main() {{
  Function? local2;
  var callableClassC2Int = CallableClass<C2<int>>();
  local2 = null as Function?;
  Object? o;
  o = (() => C1<int>()) as Object?;
  if (local2 is C1<int> Function()? && o is C1<int> Function()) {{
    o = (local2 ??= callableClassC2Int)
      ..expectStaticType<Exactly<A Function()>>();
  }}
}}
"
    );
    let report = report_ops(&source);
    assert!(
        !code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "fabricated rows: {:#?}",
        rows(&report)
    );
}

/// The `??` twin: an unjoined LUB (`int ?? double`, oracle type `num`)
/// abstains exactly like the `??=` form — the frozen face would be the
/// RHS `double`. A grounded join (same-head operands) is the carve-out
/// that still decides, but the engine already under-reports it today
/// (the staged join never reaches the frozen cascade receiver face), so
/// no fabricated row exists to lose either way; the genuine-receiver
/// control below carries the still-decides proof. Oracle evidence for
/// the ungrounded shape: the 2026-11-02 probe on
/// /tmp/3593fx/fx1/lib/test2.dart emits no bounds row (receiver types
/// `num`, matching `Exactly<num>`).
#[test]
fn if_null_receiver_lub_abstains() {
    let ungrounded = format!(
        "{EXTENSION_SCAFFOLD}\
void main() {{
  Object? local1;
  var d = 2.0;
  Object? o;
  local1 = null as Object?;
  o = 0 as Object?;
  if (local1 is int? && o is int?) {{
    o = (local1 ?? d)..expectStaticType<Exactly<num>>();
  }}
}}
"
    );
    let report = report_ops(&ungrounded);
    assert!(
        !code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "fabricated rows: {:#?}",
        rows(&report)
    );
}

/// The plain-receiver control: a receiver whose face is genuine (the
/// `double` literal's own type) still reports — the abstain is scoped
/// to if-null extents, never a blanket silence. Oracle evidence: the
/// 2026-11-02 probe on `/tmp/3593fx/fx1/lib/test.dart` line 32 emits
/// exactly this row.
#[test]
fn genuine_receiver_still_reports() {
    let source = format!(
        "{EXTENSION_SCAFFOLD}\
void main() {{
  var d = 2.0;
  d.expectStaticType<Exactly<num>>();
}}
"
    );
    let report = report_ops(&source);
    let names = code_names(&report);
    assert!(
        names.contains(&"type_argument_not_matching_bounds"),
        "genuine receiver row lost: {:#?}",
        rows(&report)
    );
    assert_eq!(names.len(), 1, "unexpected extra rows: {:#?}", rows(&report));
}

/// The super-bounded acceptance family (`super_bounded_types_test` /
/// `super_bounded_types_error2_test`, 83 census extras): a bottom-bounded
/// type parameter (`N extends Never`) satisfies a covariant cyclic bound
/// (`FcovCyclicBound<A<N>>` with `X extends A<X>` — `A<N> <: A<A<N>>`
/// through class-parameter covariance and N's bottom bound). The engine's
/// lazy-scope ids carry no declared bound, so the `N <: A<N>` leaf
/// answered a fabricated mismatch; the gate now suppresses any NESTED
/// mention of a bounded in-scope parameter. Oracle evidence (2026-11-02,
/// fixture /tmp/3593fx/fx2/lib/test.dart): NO
/// `type_argument_not_matching_bounds` rows — only `unused_local_variable`
/// warnings.
#[test]
fn super_bounded_nested_bottom_param_accepts() {
    let source = "\
class A<X> {}
typedef FcovCyclicBound<X extends A<X>> = X Function();
typedef Fcov<X> = X Function();
class CFcov<X extends Fcov<X>> {}
void testCovariantSuperbound<N extends Never>() {
  FcovCyclicBound<Never> x3;
  FcovCyclicBound<N> x4;
  FcovCyclicBound<A<Never>> x5;
  FcovCyclicBound<A<N>> x6;
  CFcov<Never> x13;
  CFcov<N> x14;
  CFcov<Fcov<N>> x16;
}
";
    let report = report_ops(&source);
    let bounds_rows: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.contains("type_argument_not_matching_bounds"))
        .collect();
    assert!(
        bounds_rows.is_empty(),
        "fabricated rows: {bounds_rows:#?}"
    );
}

/// The composite-receiver family (`number_operator_typing_test`, 55
/// census extras): a parenthesized arithmetic composite over a `dynamic`
/// operand reads an operator result the oracle refines
/// (`refineBinaryExpressionType`: `int - dynamic` is the declared `num`),
/// where the engine's face followed the operand. The face now abstains
/// when the operand row is still on the stack; a composite over concrete
/// operands (`(i - 1)`, result `int`) keeps its genuine row.
///
/// Reach note: the `int + Never` shape of the same family is NOT covered
/// — the binary fold collapses its operands into one `Value` result row
/// (refined to `double` through a subtype test where upstream asks
/// identity), so no operand face survives to the bounds site; that fix
/// belongs to the fold's refinement.
///
/// Oracle evidence (2026-11-02, fixture /tmp/3593fx/fx5/lib/test.dart):
/// exactly one row, line 16 — `'Exactly<num>' doesn't conform to the bound
/// 'Exactly<int>' of the type parameter 'R'.`; the `dyn1`/`never` operand
/// lines are silent.
#[test]
fn composite_receiver_dynamic_operand_abstains() {
    let source = format!(
        "{EXTENSION_SCAFFOLD}\
final dynamic dyn1 = 1;
void main() {{
  int i = 1;
  (i - dyn1).expectStaticType<Exactly<num>>();
  (i - 1).expectStaticType<Exactly<num>>();
}}
"
    );
    let report = report_ops(&source);
    let bounds_rows: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.contains("type_argument_not_matching_bounds"))
        .collect();
    // Count discriminates the abstain (2 rows if it fails); the bound text
    // discriminates the anchor — the fabricated row would read the operand
    // face `dynamic` in the bound, only the concrete composite reads `int`.
    assert_eq!(bounds_rows.len(), 1, "rows: {bounds_rows:#?}");
    assert!(
        bounds_rows[0].contains("'Exactly<num>' doesn't conform to the bound 'Exactly<int>'"),
        "the concrete-operand control drifted: {}",
        bounds_rows[0]
    );
}

/// The dynamic-receiver family (`function_bounds_test`, 5 census
/// extras): `(g as dynamic)<double, int>(...)` is a dynamic invocation —
/// the oracle resolves no element, so `_checkInvocationTypeArguments`
/// never runs over it. The engine's nameless-literal arm borrowed the
/// LAST closed type-variables window (`g`'s own `<T extends U, U extends
/// num>`) and judged the written arguments against it. The DYNAMIC
/// callee face now abstains; the resolved `f<String>('x')` control keeps
/// its genuine row. Oracle evidence (2026-11-02, fixture
/// /tmp/3593fx/fx4/lib/test.dart): exactly one row, line 6 —
/// `'String' doesn't conform to the bound 'num' of the type parameter 'T'.`
#[test]
fn dynamic_callee_type_args_abstain_resolved_callee_reports() {
    let source = "\
void main() {
  f<T extends num>(T x) => x;
  g<T extends U, U extends num>(T x, U y) => x;
  (f as dynamic)<Object>(42);
  (g as dynamic)<double, int>(42.0, 100);
  f<String>('x');
}
";
    let report = report_ops(source);
    let bounds_rows: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.contains("type_argument_not_matching_bounds"))
        .collect();
    assert_eq!(bounds_rows.len(), 1, "rows: {bounds_rows:#?}");
    assert!(
        bounds_rows[0].contains("'String' doesn't conform to the bound 'num'"),
        "the resolved control drifted: {}",
        bounds_rows[0]
    );
}
/// The genuine-violation control of the same family: concrete arguments
/// that truly miss the cyclic bound still report, with the oracle's own
/// row text. Oracle evidence (2026-11-02, fixture
/// /tmp/3593fx/fx2/lib/test4.dart lines 4-5):
/// `'int' doesn't conform to the bound 'A<int>'` and
/// `'A<int>' doesn't conform to the bound 'A<A<int>>'`.
#[test]
fn super_bounded_concrete_violation_still_reports() {
    let source = "\
class A<X> {}
typedef FcovCyclicBound<X extends A<X>> = X Function();
void f() {
  FcovCyclicBound<int> bad1;
  FcovCyclicBound<A<int>> bad2;
}
";
    let report = report_ops(&source);
    let bounds_rows: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.contains("type_argument_not_matching_bounds"))
        .collect();
    assert_eq!(bounds_rows.len(), 2, "rows: {bounds_rows:#?}");
    assert!(
        bounds_rows
            .iter()
            .any(|row| row.contains("'int' doesn't conform to the bound 'A<int>'")),
        "first genuine row missing: {bounds_rows:#?}"
    );
    assert!(
        bounds_rows
            .iter()
            .any(|row| row.contains("'A<int>' doesn't conform to the bound 'A<A<int>>'")),
        "second genuine row missing: {bounds_rows:#?}"
    );
}

/// The argument-position shape stays silent in BOTH directions: the
/// census `context<…>((x ??= y)..expectStaticType<…>())` forms never
/// reached the bounds face before #3593 (the argument row's receiver is
/// unresolved), and the abstain must not fabricate the reverse — no row
/// before, no row after. Oracle evidence: the 2026-11-02 probe on
/// /tmp/3593fx/fx1/lib/test2.dart emits no bounds row for any of its
/// three context shapes.
#[test]
fn argument_position_if_null_stays_silent() {
    let source = format!(
        "{EXTENSION_SCAFFOLD}\
Object? context<T>(T x) => x;
void main() {{
  Function? local2;
  var callableClassC2Int = CallableClass<C2<int>>();
  local2 = null as Function?;
  if (local2 is C1<int> Function()?) {{
    context<Function>(
      (local2 ??= callableClassC2Int)..expectStaticType<Exactly<Function>>(),
    );
  }}
}}
"
    );
    let report = report_ops(&source);
    assert!(
        !code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "fabricated rows: {:#?}",
        rows(&report)
    );
}
