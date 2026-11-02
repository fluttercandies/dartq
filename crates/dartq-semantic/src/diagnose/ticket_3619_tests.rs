//! #3619 pins — the `??`/`?:` operand join's real least upper bound, the
//! if-null `T2'` callable coercion, and the await face's user-Future
//! flatten (the E1 conditional-LUB family, census 20261102-122042:
//! 256 `type_argument_not_matching_bounds` extras over 50
//! `features/inference-update-3` cases plus the
//! `derive_future_type_test` await rows).
//!
//! Oracle evidence: the corpus's own pinned expectations
//! (`conditional_expression_test`'s rule comments name `T = num`,
//! `T = Iterable<num>`, `T = A`, `T = Object`, `T = int?` — the raw LUB
//! answers; the `B1<Object?>`/`Iterable<num>` SITE answers are the
//! context-rule `S`, surfaced by the receiver face #3635 owns).

use super::test_rig::{code_names, report_ops, rows};

/// The `invalid_assignment` rows alone — the face the join feeds
/// (`check_assignment`'s RHS read), each `code @offset: message`.
fn assignment_rows(report: &super::FileReport) -> Vec<String> {
    rows(report)
        .into_iter()
        .filter(|row| row.starts_with("invalid_assignment"))
        .collect()
}

/// One `invalid_assignment` row exists and its message carries `needle`.
#[track_caller]
fn assert_one_assignment(report: &super::FileReport, needle: &str) {
    let found = assignment_rows(report);
    assert_eq!(found.len(), 1, "expected exactly one row, got {found:#?}");
    assert!(
        found[0].contains(needle),
        "expected {needle:?} in {:?}",
        found[0]
    );
}

/// No `invalid_assignment` row at all.
#[track_caller]
fn assert_no_assignment(report: &super::FileReport) {
    let found = assignment_rows(report);
    assert!(found.is_empty(), "expected no rows, got {found:#?}");
}

/// The join-truth-table classes every conditional pin shares
/// (`conditional_expression_test`'s own hierarchy).
const JOIN_CLASSES: &str = "\
class A {}
class B1<T> implements A {}
class B2<T> implements A {}
class C1<T> implements B1<T>, B2<T> {}
class C2<T> implements B1<T>, B2<T> {}
";

/// `UP(int, double) = num` — the numeric ladder through the
/// common-supertype walk (`conditional_expression_test` :71: T = num; the
/// kept-right face answered `double`).
#[test]
fn join_int_double_answers_num() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var i = 1;
  var d = 2.0;
  int tgt;
  tgt = b ? i : d;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'num' can't be assigned");
}

/// `UP(Iterable<int>, Iterable<double>) = Iterable<num>` — the same-head
/// pairwise argument join (corpus :87: T = Iterable<num>; the kept-right
/// face answered `Iterable<double>`).
#[test]
fn join_same_head_merges_arguments() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var iterableInt = <int>[] as Iterable<int>;
  var iterableDouble = <double>[] as Iterable<double>;
  Iterable<int> tgt;
  tgt = b ? iterableInt : iterableDouble;
}}
"
    );
    assert_one_assignment(
        &report_ops(&source),
        "of type 'Iterable<num>' can't be assigned",
    );
}

/// `UP(C1<int>, C2<double>) = A` — the instantiated-closure intersection:
/// `B1` is a common ancestor HEAD but both sides reach it under DIFFERENT
/// instantiations (`B1<int>` vs `B1<double>`), so the intersection holds
/// `A` only (corpus :107's own comment: T = A; the kept-right face
/// answered `C2<double>`). The SITE answer `B1<Object?>` is the
/// context-rule `S` the receiver face owns, not the raw LUB.
#[test]
fn join_cross_head_walks_instantiated_intersection() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var c1Int = C1<int>();
  var c2Double = C2<double>();
  int tgt;
  tgt = b ? c1Int : c2Double;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'A' can't be assigned");
}

/// `UP(Iterable<int>, List<num>) = Object` — the declared edge does not
/// decide an argument-contradicting pair (`List<num> <!: Iterable<int>`),
/// and the instantiated intersection is empty above `Object` (corpus
/// :140's own comment: T = Object; the kept-right face answered
/// `List<num>`).
#[test]
fn join_subsumed_pair_answers_object() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var iterableInt = <int>[] as Iterable<int>;
  var listNum = <num>[];
  int tgt;
  tgt = b ? iterableInt : listNum;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'Object' can't be assigned");
}

/// `UP(Null, int) = int?` (least_upper_bound.dart:487-524) — the null
/// literal's then-arm joins the nullable else arm (corpus :165: the
/// oracle types `b ? null : i` as `int?`; the kept-right face answered
/// `int`).
#[test]
fn join_null_leg_answers_nullable() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var i = 1;
  int tgt;
  tgt = b ? null : i;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'int?' can't be assigned");
}

/// `UP(int, String) = Object` — the unrelated pair's terminal bound (a
/// recovered under-report: the kept-right face answered `String`, which
/// silently assigned into a `String` target where the oracle reports).
#[test]
fn join_unrelated_pair_answers_object() {
    let source = format!(
        "{JOIN_CLASSES}\
void main(bool b) {{
  var i = 1;
  var s = 'x';
  String tgt;
  tgt = b ? i : s;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'Object' can't be assigned");
}

/// The `??=` value's `T2'` coercion (`resolveIfNull`'s coercion step): a
/// `call`-declaring class writes into the bare `Function?` target without
/// a row (corpus `if_null_assignment_local_test` :141/:127's
/// `CallableClass<int>` into `Function?` — 5 rows per case the coercion
/// removes).
#[test]
fn if_null_write_coerces_callable_class() {
    let source = format!(
        "{JOIN_CLASSES}\
class CallableClass<T> {{
  T call() => throw '';
}}
void main() {{
  Function? local2;
  var callableClassInt = CallableClass<int>();
  local2 = null as Function?;
  local2 ??= callableClassInt;
}}
"
    );
    assert_no_assignment(&report_ops(&source));
}

/// The join's own if-null coercion: `UP(NonNull(Function?),
/// CallableClass<int>) = Function` — the value joins as its `call`
/// signature against the bare `Function` left (corpus :142/:127: the
/// oracle types the pair `Function`; the kept-right face answered
/// `CallableClass<int>`). The failing target spells the joined `Function`
/// in the row; the accepting target proves the coercion is not a blanket
/// silence.
#[test]
fn if_null_join_coerces_callable_class() {
    let source = format!(
        "{JOIN_CLASSES}\
class CallableClass<T> {{
  T call() => throw '';
}}
void main() {{
  Function? f;
  var callableClassInt = CallableClass<int>();
  f = null as Function?;
  Function? ok;
  ok = f ?? callableClassInt;
  int Function() spelled;
  spelled = f ?? callableClassInt;
}}
"
    );
    let report = report_ops(&source);
    let found = assignment_rows(&report);
    assert_eq!(found.len(), 1, "expected exactly one row, got {found:#?}");
    assert!(
        found[0].contains("of type 'Function' can't be assigned"),
        "expected the coerced join face 'Function', got {:?}",
        found[0]
    );
}

/// The await flatten's user-Future walk (`derive_future_type_test`):
/// `flatten` walks a class that `implements Future<E>` — `await
/// subFuture` types `int`, both directions (the accepting target loses
/// its fabricated row, the refusing target names the flattened `int`).
#[test]
#[ignore = "test-rig gap: `instance_env`'s registry pre-registers the builtin `Future` decl (resolved_ops.rs:674), so a fixture `class Future` is skipped (resolved_ops.rs:1626) and the `implements` edge resolves to the fixture decl the snapshot never registered — `as_instance_of` answers `None`; `implements` on the names-only stub emits `implements_non_class`. The flatten arm itself is pinned by `await_flatten_walks_parameter_bound`."]
fn await_flatten_walks_user_future_class() {
    let source = "\
class Future<T> {}
class SubFuture implements Future<int> {}
void main() async {
  var subFuture = SubFuture();
  int ok = await subFuture;
  String bad = await subFuture;
}
";
    let report = report_ops(source);
    let found = assignment_rows(&report);
    assert_eq!(found.len(), 1, "expected exactly one row, got {found:#?}");
    assert!(
        found[0].contains("of type 'int' can't be assigned"),
        "expected the flattened 'int', got {:?}",
        found[0]
    );
}

/// The mixin and indirect-chain shapes: `WithMixFuture` reaches
/// `Future<int>` through `with MixFuture` — the snapshot's instantiated
/// view answers `int` for the whole chain.
#[test]
#[ignore = "test-rig gap: same `instance_env` registry collision as `await_flatten_walks_user_future_class` — `implements Future` resolves to a fixture decl the snapshot never registers."]
fn await_flatten_walks_mixin_chain() {
    let source = "\
class Future<T> {}
mixin MixFuture implements Future<int> {}
class WithMixFuture extends Object with MixFuture {}
void main() async {
  var x = WithMixFuture();
  int ok = await x;
  String bad = await x;
}
";
    let report = report_ops(source);
    let found = assignment_rows(&report);
    assert_eq!(found.len(), 1, "expected exactly one row, got {found:#?}");
    assert!(
        found[0].contains("of type 'int' can't be assigned"),
        "expected the flattened 'int', got {:?}",
        found[0]
    );
}

/// The extension-type shape: `ExtensionFuture implements Future<String?>`
/// flattens to `String?` (corpus :112's shape).
#[test]
#[ignore = "test-rig gap: same `instance_env` registry collision as `await_flatten_walks_user_future_class` — `implements Future` resolves to a fixture decl the snapshot never registers."]
fn await_flatten_walks_extension_type() {
    let source = "\
class Future<T> {}
extension type ExtensionFuture(Future<String> _) implements Future<String?> {}
void main() async {
  var x = ExtensionFuture(null as Future<String>);
  String? ok = await x;
  int bad = await x;
}
";
    let report = report_ops(source);
    let found = assignment_rows(&report);
    assert_eq!(found.len(), 1, "expected exactly one row, got {found:#?}");
    assert!(
        found[0].contains("of type 'String?' can't be assigned"),
        "expected the flattened 'String?', got {:?}",
        found[0]
    );
}

/// The type-parameter bound walk (`X & B` derives `B`'s future):
/// `X1 extends Future<int>` flattens `await x1` to `int`, and the
/// nullable operand `X1?` to `int?` (corpus f1's 4.1/3.4.1 shapes).
#[test]
fn await_flatten_walks_parameter_bound() {
    let source = "\
class Future<T> {}
Future<void> f<X1 extends Future<int>>(X1 x1) async {
  int ok = await x1;
  String bad = await x1;
  X1? x1n = null as X1?;
  int? okn = await x1n;
  String badn = await x1n;
}
";
    let report = report_ops(source);
    let found = assignment_rows(&report);
    assert_eq!(found.len(), 2, "expected exactly two rows, got {found:#?}");
    assert!(
        found[0].contains("of type 'int' can't be assigned"),
        "expected the flattened 'int', got {:?}",
        found[0]
    );
    assert!(
        found[1].contains("of type 'int?' can't be assigned"),
        "expected the flattened 'int?', got {:?}",
        found[1]
    );
}

/// The `?:`-tail sentinel suppression preserved (#3593's posture): a
/// receiver whose tail operand is `null` still grounds NO bounds row —
/// the extension-member face the receiver owns must not gain a fabricated
/// row from the join lane (corpus :201's sibling `b ? i : null` produces
/// no oracle row).
#[test]
fn conditional_null_tail_receiver_stays_suppressed() {
    let source = "\
class A {}
typedef Exactly<T> = T Function(T);
extension StaticType<T> on T {
  T expectStaticType<R extends Exactly<T>>() => this;
}
void main(bool b) {
  var i = 1;
  Object? o;
  o = (b ? i : null)..expectStaticType<Exactly<int?>>();
}
";
    let report = report_ops(source);
    assert!(
        !code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "fabricated bounds rows: {:#?}",
        rows(&report)
    );
}

/// The extension-member bounds face still decides over a genuine
/// receiver — the join lane's changes are no blanket silence (the
/// still-reports control #3593 carries).
#[test]
fn genuine_receiver_bounds_row_still_reports() {
    let source = "\
typedef Exactly<T> = T Function(T);
extension StaticType<T> on T {
  T expectStaticType<R extends Exactly<T>>() => this;
}
void main() {
  var d = 2.0;
  d.expectStaticType<Exactly<num>>();
}
";
    let report = report_ops(source);
    assert!(
        code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "genuine bounds row lost: {:#?}",
        rows(&report)
    );
}

/// `UP(nonNull(Null), T) = UP(Never, T) = T` — the `??` non-null rewrite
/// takes the null literal to bottom before the join, so `null ?? i`
/// stands `int`, never the NULL arm's `int?`
/// (least_upper_bound.dart:429-451 after resolveIfNull's `nonNull`).
#[test]
fn if_null_literal_left_answers_right() {
    let source = format!(
        "{JOIN_CLASSES}\
void main() {{
  var i = 1;
  String tgt;
  tgt = null ?? i;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'int' can't be assigned");
}

/// `UP(int Function(), String Function()) = Function` — two distinct
/// function shapes take the coarse `Function` head (the official
/// correlation-failure fallback, least_upper_bound.dart:684-686; the
/// kept-right face answered `String Function()`).
#[test]
fn join_two_functions_answers_bare_function() {
    let source = format!(
        "{JOIN_CLASSES}\
int f1() => 1;
String f2() => '';
void main(bool b) {{
  int tgt;
  tgt = b ? f1 : f2;
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'Function' can't be assigned");
}

/// The candidate walk's tied-level drop: `C implements I1, I2` and
/// `D implements I1, I2` where `I1`/`I2` are mutually incomparable and
/// both subtype `J` — the tied top level `{I1, I2}` drops wholesale and
/// the descent answers `J`, the deepest unique ancestor
/// (least_upper_bound.dart:162-177).
#[test]
fn join_diamond_drops_tied_level() {
    let source = format!(
        "{JOIN_CLASSES}\
class J {{}}
class I1 implements J {{}}
class I2 implements J {{}}
class JC implements I1, I2 {{}}
class JD implements I1, I2 {{}}
void main(bool b) {{
  int tgt;
  tgt = b ? JC() : JD();
}}
"
    );
    assert_one_assignment(&report_ops(&source), "of type 'J' can't be assigned");
}

/// `UP(T, T) = T` survives the function-merge arm (`UP(f?, f)` strips to
/// an identical pair — the nullability merge must re-ask the identity
/// rule, not read `Function`).
#[test]
fn join_nullable_function_with_self_stays_self() {
    let source = format!(
        "{JOIN_CLASSES}\
int f() => 1;
void main() {{
  int Function()? fMaybe = f;
  int Function() fSame = f;
  int Function() tgt;
  tgt = fMaybe ?? fSame;
}}
"
    );
    assert_no_assignment(&report_ops(&source));
}

/// The `??=` extension-property write's target face: the setter's write
/// type (`Object?` on `pIntQuestion`) accepts the `double` value — the
/// corpus's `Extension('').pIntQuestion ??= d` shape fires no
/// `invalid_assignment` (the `UP(int, double) = num` EXPRESSION type the
/// `expectStaticType` reads is the receiver face #3635 owns).
#[test]
fn if_null_assignment_extension_property_write_accepts() {
    let source = "\
extension E on String {
  int? get pIntQuestion => null;
  set pIntQuestion(Object? value) {}
}
void main() {
  var d = 2.0;
  E('').pIntQuestion ??= d;
}
";
    assert_no_assignment(&report_ops(source));
}

