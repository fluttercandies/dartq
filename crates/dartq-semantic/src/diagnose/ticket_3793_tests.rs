//! #3793 pins — `invalid_assignment` cluster: a compound assignment's
//! `await` operand reads the operand's own static type (the `Future<T>`
//! capture) then flattens, never the unflattened row.
//!
//! Oracle contract (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart
//! + `type_analyzer.dart` `analyzeAwaitExpression`): `x op= await e` types
//! the `op` invocation by the operand's `flatten(T)` face — `int += await
//! Future<int>` resolves `int.+`'s signature and the write-back `int`
//! coerces clean, while `int += await Future<num>` answers `num.+`'s `num`
//! return against `int` and reports. The engine pins both directions.
//!
//! Probed against `dart analyze` 3.13.7 (the pinned oracle at
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The oracle's `invalidAssignment` message prefix (messages.yaml).
const ASSIGN_MSG_PREFIX: &str = "A value of type '";

/// Every `invalid_assignment` row's `(offset, length)` in report order.
fn assign_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with(ASSIGN_MSG_PREFIX))
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Asserts the `invalid_assignment` rows sit at exactly `expected`
/// `(fixture_offset, length)` pairs (`CORE_PRELUDE` shifted inside).
#[track_caller]
fn assert_assign_rows(report: &FileReport, fixture: &str, expected: &[(usize, u32)]) {
    let mut got = assign_rows(report);
    got.sort_unstable();
    let prelude = CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

/// `int i += await Future<int>.value(v)` is the oracle's clean pair — the
/// operand's `Future<int>` flattens to `int` at the `+=` invocation, the
/// resolved `int.+` returns `int`, and the `int` write-back fits `i`'s
/// slot exactly.
///
/// Probed: silent (0 rows) — `dart analyze` reports only the
/// `unused_local_variable` row on `i`, no `invalid_assignment`.
#[test]
fn await_flattened_operand_accepts_int_compound() {
    let fixture = "test(Stream<int> s) async {\n\
         \x20 int i = 0;\n\
         \x20 await for (int v in s) {\n\
         \x20   i += await Future<int>.value(v);\n\
         \x20 }\n\
         }\n";
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[]);
}

/// The same shape through a member-call operand (`await m.f()` where `m.f`
/// returns `Future<int>`): the operand's resolved `Future<int>` flattens
/// the same way.
///
/// Probed: silent (0 rows).
#[test]
fn await_member_call_operand_accepts_int_compound() {
    let fixture = "class M { Future<int> f() async => 0; }\n\
         test(M m) async {\n\
         \x20 int i = 0;\n\
         \x20 i += await m.f();\n\
         }\n";
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[]);
}

/// `int i += await Future<num>.value(v)` is the oracle's reporting pair —
/// the flattened `num` resolves `num.+`, whose declared `num` return does
/// not fit the `int` slot, so `num -> int` reports at the whole
/// right-hand expression (oracle anchors `.at(rightHandSide)`).
///
/// Probed: 1 row at the `await Future<num>.value(v)` span — `3:8`, length
/// 26 inside the fixture.
#[test]
fn await_flattened_num_reports_int_compound() {
    let fixture = "test(int v) async {\n\
         \x20 int i = 0;\n\
         \x20 i += await Future<num>.value(v);\n\
         }\n";
    // The `await` operand spans `Future<num>.value(v)` — the oracle's
    // anchor is the whole `await` expression, which the row model keys on
    // the operand's row range.
    let offset = fixture.find("await Future<num>.value(v)").expect("needle");
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[(offset, 26)]);
}

/// `int i += await f(v)` where `f` returns `Future<int>`: the bare-send
/// operand reads `Future<int>`, flattens to `int`, and the `int` write
/// back coerces clean — the `rhs_expression_ty` pin's twin over the
/// compound face.
///
/// Probed: silent (0 rows).
#[test]
fn await_send_operand_accepts_int_compound() {
    let fixture = "test(int v) async {\n\
         \x20 int i = 0;\n\
         \x20 i += await f(v);\n\
         }\n\
         Future<int> f(int v) async => v;\n";
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[]);
}

// —— LE-1/LE-2: `list_element_type_not_assignable`'s context channels ——

/// Every `list_element_type_not_assignable` row's `(offset, length)` in
/// report order (the `map_key`/`map_value` faces share the
/// `"The element type '"` prefix — the `list type '` needle keeps only
/// the list side).
fn list_element_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.message.starts_with("The element type '") && d.message.contains("list type '")
        })
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Asserts the `list_element_type_not_assignable` rows sit at exactly
/// `expected` `(fixture_offset, length)` pairs.
#[track_caller]
fn assert_list_element_rows(report: &FileReport, fixture: &str, expected: &[(usize, u32)]) {
    let mut got = list_element_rows(report);
    got.sort_unstable();
    let prelude = CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

/// The declaration-initializer channel (`List<int> l = […]`): the armed
/// annotation's element argument types the leaves — `List<int> l2 =
/// ["hello"]` and `l3`'s `"hello"` report `String` against `int`,
/// `List<dynamic>` (`l4`, the same spelling under no element constraint)
/// and a clean `int` leaf stay silent.
///
/// Probed: `dart analyze` 3.13.7 — 2 rows at `"hello"` (length 7), one in
/// `l2`, one in `l3`; `List<dynamic> l4 = ["hello"]` carries none.
#[test]
fn list_element_decl_init_context_reports() {
    let fixture = "test() {\n\
         \x20 List<int> l0 = [];\n\
         \x20 List<int> l1 = [3];\n\
         \x20 List<int> l2 = [\"hello\"];\n\
         \x20 List<int> l3 = [\"hello\", 3];\n\
         \x20 List<dynamic> l4 = [\"hello\"];\n\
         }\n";
    let report = report_ops(fixture);
    let off1 = fixture.find("\"hello\"]").expect("needle");
    // `l3`'s occurrence — the `List<dynamic>` `l4` one stays silent, so the
    // needle must anchor on the trailing comma rather than taking the last
    // `"hello"` in the fixture.
    let off2 = fixture.find("\"hello\", 3").expect("needle");
    assert_list_element_rows(&report, fixture, &[(off1, 7), (off2, 7)]);
}

/// The assignment channel (`l = […]`): the write target's declared
/// element type drives the leaf verdicts.
///
/// Probed: 1 row at `"hello"`.
#[test]
fn list_element_assignment_context_reports() {
    let fixture = "test() {\n\
         \x20 List<int> l = [];\n\
         \x20 l = [\"hello\"];\n\
         }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("\"hello\"").expect("needle");
    assert_list_element_rows(&report, fixture, &[(offset, 7)]);
}

/// The argument channels (`f([…])`, `f(a: […])`): the open slot's
/// declared formal types the literal — positional and named alike.
///
/// Probed: 1 row each at `"hello"`.
#[test]
fn list_element_argument_context_reports() {
    let fixture = "void f0(List<int> a) {}\n\
         void f1({List<int>? a}) {}\n\
         test() {\n\
         \x20 f0([\"hello\"]);\n\
         \x20 f1(a: [\"hello\"]);\n\
         }\n";
    let report = report_ops(fixture);
    let off1 = fixture.find("\"hello\"").expect("needle");
    let off2 = fixture.rfind("\"hello\"").expect("needle");
    assert_list_element_rows(&report, fixture, &[(off1, 7), (off2, 7)]);
}

/// The nested-literal channel (`f3([[…]])`): the enclosing literal's
/// element context (`Iterable<int>`) flows to the inner `[…]`, whose
/// own leaf then answers `int`.
///
/// Probed: 1 row at the inner `"hello"` (the outer `[…]` adopts its
/// `Iterable<int>` context and stays silent).
#[test]
fn list_element_nested_literal_context_reports() {
    let fixture = "void f3(Iterable<Iterable<int>> a) {}\n\
         test() {\n\
         \x20 f3([[3]]);\n\
         \x20 f3([[\"hello\"]]);\n\
         \x20 f3([[\"hello\"], [3]]);\n\
         }\n";
    let report = report_ops(fixture);
    let off1 = fixture.find("\"hello\"").expect("needle");
    let off2 = fixture.rfind("\"hello\"").expect("needle");
    assert_list_element_rows(&report, fixture, &[(off1, 7), (off2, 7)]);
}

/// The map-value channel (`{3: […]}`): the enclosing `Map<K, V>`'s
/// value side types the `[…]`, whose leaf answers `String`.
///
/// Probed: 1 row at `3` inside `[3]` (the `"hello"` key is the map
/// face's own `map_key_type_not_assignable`, filtered out here).
#[test]
fn list_element_map_value_context_reports() {
    let fixture = "test() {\n\
         \x20 Map<int, List<String>> l3 = {3: [3]};\n\
         \x20 Map<int, List<String>> l4 = {\"hello\": [3]};\n\
         }\n";
    let report = report_ops(fixture);
    let off1 = fixture.find("[3]").expect("needle") + 1;
    let off2 = fixture.rfind("[3]").expect("needle") + 1;
    assert_list_element_rows(&report, fixture, &[(off1, 1), (off2, 1)]);
}

/// A `{…}` element inside `<Map<K, V>>[…]` is a MAP literal — the list
/// face stays silent (the entry face owns the key/value verdicts
/// upstream; dartq's wrong-code emission here was LE-2).
///
/// Probed: no `list_element` row (the oracle emits only
/// `map_value_type_not_assignable` inside the inner map).
#[test]
fn list_element_map_literal_element_stays_silent() {
    let fixture = "class Resource {}\n\
         class Folder extends Resource {}\n\
         Resource getResource(String s) => Resource();\n\
         test() {\n\
         \x20 var a = <Map<String, Folder>>[{'pkgA': getResource('/x')}];\n\
         }\n";
    let report = report_ops(fixture);
    assert_list_element_rows(&report, fixture, &[]);
}

/// A `{…}` element under a non-map context keeps its upward-inferred
/// face — `Map<String, int>` against `int`, anchored at the literal's
/// own `{`-`}` span.
///
/// Probed: 1 row at `{'k': 1}` (length 8).
#[test]
fn list_element_map_literal_upward_ty_reports() {
    let fixture = "test() {\n\
         \x20 List<int> b = [{'k': 1}];\n\
         }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("{'k': 1}").expect("needle");
    assert_list_element_rows(&report, fixture, &[(offset, 8)]);
}

/// A nested `[…]` element under a non-list context keeps the
/// upward-inferred face — `List<String>` against `int`, anchored at the
/// inner `[`..`]` span.
///
/// Probed: 1 row at `["hello"]` (length 9).
#[test]
fn list_element_list_literal_upward_ty_reports() {
    let fixture = "test() {\n\
         \x20 List<int> a = [[\"hello\"]];\n\
         }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("[\"hello\"]").expect("needle");
    assert_list_element_rows(&report, fixture, &[(offset, 9)]);
}

/// The typed `<E>` group's generic argument is the element face whole —
/// `<Map<String, Folder>>` spells `Map<String, Folder>`, never the raw
/// `Map` head (LE-2's args-dropped display).
///
/// Probed: silent — the `{…}` element is checked as a map literal
/// against `Map<String, Folder>` and adopts it.
#[test]
fn list_element_typed_generic_group_stays_silent() {
    let fixture = "test() {\n\
         \x20 var a = <Map<String, Folder>>[{'k': 1}];\n\
         }\n";
    let report = report_ops(fixture);
    assert_list_element_rows(&report, fixture, &[]);
}

/// `Iterable<int>` donates its element argument exactly like `List`
/// (the oracle's `Iterable` head is an element context too).
///
/// Probed: 1 row at `"hello"`.
#[test]
fn list_element_iterable_context_reports() {
    let fixture = "test() {\n\
         \x20 Iterable<int> i2 = [\"hello\"];\n\
         }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("\"hello\"").expect("needle");
    assert_list_element_rows(&report, fixture, &[(offset, 7)]);
}

/// The `await` prefix is transparent to the context channel —
/// `List<int> l0 = await [d]` resolves `List<int>`'s element.
///
/// Probed: 1 row at `"hello"` (the `dynamic` leaf in `l0` coerces).
#[test]
fn list_element_await_prefix_context_reports() {
    let fixture = "test() async {\n\
         \x20 List<int> l0 = await [\"hello\"];\n\
         }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("\"hello\"").expect("needle");
    assert_list_element_rows(&report, fixture, &[(offset, 7)]);
}

/// The corpus shape (`language/async_star/await_for_test.dart`): a
/// static generic factory call with a closure argument —
/// `r += await Future<int>.microtask(() => v)` — inside an `await for`
/// body. The operand row spans the whole `Future<int>.microtask(...)`
/// call, its `Future<int>` flattens to `int`, and the write-back fits.
///
/// Probed: silent (0 rows) — `dart analyze` 3.13.7 reports only the
/// `unused_local_variable` on `r`.
#[test]
fn await_static_factory_closure_operand_accepts_int_compound() {
    let fixture = "test(Stream<int> s) async {\n\
         \x20 int r = 0;\n\
         \x20 await for (int v in s) {\n\
         \x20   r += await Future<int>.microtask(() => v);\n\
         \x20 }\n\
         }\n";
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[]);
}

/// The same shape without the `await for` (plain block): the
/// `Future<int>.value(v)` send's operand flatten decides clean.
///
/// Probed: silent (0 rows).
#[test]
fn await_static_factory_value_operand_accepts_int_compound() {
    let fixture = "test(int v) async {\n\
         \x20 int r = 0;\n\
         \x20 r += await Future<int>.value(v);\n\
         }\n";
    let report = report_ops(fixture);
    assert_assign_rows(&report, fixture, &[]);
}

// —— the await-expression span (`return await e` anchors the whole node) ——

/// Every `return_of_invalid_type` row's `(offset, length)` in report
/// order.
fn return_invalid_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    got.sort_unstable();
    got
}

/// Asserts the `return_of_invalid_type` rows sit at exactly `expected`
/// `(fixture_offset, length)` pairs (`CORE_PRELUDE` shifted inside).
#[track_caller]
fn assert_return_invalid_rows(report: &FileReport, fixture: &str, expected: &[(usize, u32)]) {
    let prelude = CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(
        return_invalid_rows(report),
        want,
        "fixture={fixture:?}\nrows={:?}",
        rows(report)
    );
}

/// `return await g();` reports the whole AwaitExpression — the `await`
/// keyword through the operand's closing `)`, never the operand alone and
/// never one column short of it (`reportTypeError`,
/// return_type_verifier.dart:155-205 anchors `.at(expression)`).
///
/// Probed: `dart analyze` 3.13.7 — `return await g();` at line 3 col 10,
/// length 9, i.e. exactly `await g()`.
#[test]
fn return_await_call_anchors_the_whole_await_expression() {
    let fixture = "Future<String> g() async => '';\n\
                   Future<int> f() async {\n\
                   \x20 return await g();\n\
                   }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("await g()").expect("needle");
    assert_return_invalid_rows(&report, fixture, &[(offset, 9)]);
}

/// The bare-operand twin (`return await s;`): the await node still runs
/// through the operand's last token — `await s`, length 7 — not the
/// six-character keyword-plus-space prefix.
///
/// Probed: `dart analyze` 3.13.7 — line 4 col 10, length 7.
#[test]
fn return_await_bare_operand_anchors_through_the_name() {
    let fixture = "Future<String> g() async => '';\n\
                   Future<int> f() async {\n\
                   \x20 var s = g();\n\
                   \x20 return await s;\n\
                   }\n";
    let report = report_ops(fixture);
    let offset = fixture.find("await s").expect("needle");
    assert_return_invalid_rows(&report, fixture, &[(offset, 7)]);
}
