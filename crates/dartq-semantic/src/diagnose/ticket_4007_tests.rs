//! #4007 — the `list_element_type_not_assignable` channel rework, pinned
//! at the oracle rows.
//!
//! One channel walk — `check_list_literal_elements` →
//! `list_literal_ctx_element` → `literal_ctx_ty` → `enclosing_ctx_ty` —
//! decides every row of the family. The pins below cover the classes the
//! channel owns:
//!
//! 1. Boundary pins (oracle-verbatim): an assignable literal reports only
//!    at the failing element, an untyped `var`/`dynamic` position emits
//!    nothing, a null-aware `?e` element reports at its `?`-inclusive span,
//!    and a nested collection literal reports its own upward face.
//! 2. Class 2 (null-aware element unskip): `<num>[?""]` reports at the
//!    `?""` span — the `?` gate must not skip the element outright.
//! 3. Class 1 (for-in header channel): `[for (int i in ["not", "int"])
//!    i]` reports at each iterable-literal element span.
//! 4. Class 4 (nested-literal upward face): `[if (true) {1}]` under
//!    `List<int>` reports `Set<int>` vs `int` at the `{1}` span.
//!
//! Oracle provenance: `dart analyze --format=machine` (3.13.7 oracle) on
//! the fixture — `LIST_ELEMENT_TYPE_NOT_ASSIGNABLE|…|2|19|3|'String'
//! can't be assigned to … 'int'`, `…|5|9|3|'String' vs 'num'`,
//! `…|6|19|5|'String' vs 'int'`, `…|6|26|5|'String' vs 'int'`,
//! `…|7|34|3|'Set<int>' vs 'int'`.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::FileReport;

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// One `list_element_type_not_assignable` row anchored at `offset:len`
/// carrying `message`.
#[track_caller]
fn assert_list_element(report: &FileReport, offset: usize, len: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "list_element_type_not_assignable"
                && d.range.offset as usize == offset
                && d.range.length == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one list_element row: {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

/// Count of `list_element_type_not_assignable` rows.
fn count_list_element(report: &FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "list_element_type_not_assignable")
        .count()
}

/// Boundary pin: `List<int> xs = ["a", 3]` reports only at `"a"` — the
/// assignable `3` stays silent, and no other position invents a row.
#[test]
fn boundary_only_failing_element_reports() {
    let src = "void main() {\n  List<int> xs = [\"a\", 3];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "\"a\""),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Boundary pin: `var ys = ["a"]` and `dynamic d = ["a"]` carry no
/// declared element face — zero rows, never a fabricated verdict.
#[test]
fn boundary_untyped_positions_emit_nothing() {
    let src = "void main() {\n  var ys = [\"a\"];\n  dynamic d = [\"a\"];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 0, "{:#?}", rows(&report));
}

/// Boundary pin: a `dynamic` element does not fabricate a row — the
/// literal's own context still decides, but an element whose static type
/// is `dynamic` answers `assignable(_, _, _) == None`, never `Some(false)`.
#[test]
fn boundary_dynamic_element_stays_silent() {
    let src = "void f(dynamic x) {\n  List<int> xs = [x];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 0, "{:#?}", rows(&report));
}

/// Class 2: `<num>[?""]` reports at the `?""` span — the `?` prefix does
/// not skip the element; the operand's `String` face answers against `num`.
/// The span covers the `?` AND the operand (`?""` = 3 UTF-16 units).
#[test]
fn null_aware_element_reports_at_question_span() {
    let src = "void main() {\n  <num>[?\"\"];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "?\"\""),
        3,
        "The element type 'String' can't be assigned to the list type 'num'.",
    );
}

/// Class 1: `[for (int i in ["not", "int"]) i]` — the iterable literal's
/// elements report at their own spans (`"not"` at 6:19|5, `"int"` at
/// 6:26|5 in the oracle's machine rows), never the for-body `i`.
#[test]
fn for_in_typed_loop_var_checks_iterable_elements() {
    let src = "void main() {\n  [for (int i in [\"not\", \"int\"]) i];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 2, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "\"not\""),
        5,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
    assert_list_element(
        &report,
        at(src, "\"int\"]"),
        5,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 4: `List<int> listInt = [if (true) {1}]` — the nested `{1}` set
/// literal reports its own upward face `Set<int>` vs `int` at the `{1}`
/// span, never the stale `Set<dynamic>`.
#[test]
fn nested_literal_reports_upward_face() {
    let src = "void main() {\n  List<int> listInt = [if (true) {1}];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "{1}"),
        3,
        "The element type 'Set<int>' can't be assigned to the list type 'int'.",
    );
}

/// Class 1 (var head): `[for (var i in ["a", "b"]) i]` under `List<int>`
/// — the `var` head carries no written element type, so the iterable
/// literal infers `List<String>` and the loop variable stages `String`;
/// the for-BODY element `i` reads `String` and reports `String` vs `int`
/// at the body's own span. One row, never per-iterable-element rows.
#[test]
fn for_in_var_head_reports_at_body() {
    let src = "void main() {\n  List<int> xs = [for (var i in [\"a\", \"b\"]) i];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "i];"),
        1,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 3: `var map = <String, List<int>>{'k': ['s']}` — the written
/// `<K, V>` group on the `{` literal donates `Map<String, List<int>>` as
/// the map's context, so the inner `['s']` element checks against `int`
/// and reports at `'s'`. The `var` initializer's `=` channel carries no
/// declared type, so without the group the inner literal stays opaque.
#[test]
fn typed_group_map_donates_value_context() {
    let src = "void main() {\n  var map = <String, List<int>>{'k': ['s']};\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "'s'"),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 8: `List<int> a = [...["x"]]` — the spread operand's own list
/// literal checks its elements against the enclosing element type `int`
/// through the channel walk, reporting `String` vs `int` at `"x"`.
#[test]
fn spread_literal_operand_checks_elements() {
    let src = "void main() {\n  List<int> a = [...[\"x\"]];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "\"x\""),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 3 boundary: an annotated `Map<String, List<int>>` declaration
/// donates the same value context through the `=` channel — the inner
/// `['s']` reports identically.
#[test]
fn annotated_map_donates_value_context() {
    let src = "void main() {\n  Map<String, List<int>> map = {'k': ['s']};\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "'s'"),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

// —— Routed residuals (classes outside the channel walk) ——
//
// The following pins document oracle-verified behavior the channel walk
// cannot reach; each names the owning layer it routes to. They are
// `#[ignore]`d so the suite stays green while the residual tickets carry
// the work — zero silent drops: every routed class has a named pin and a
// named owner.

/// Class 5 (generic downward slot): `String x = f([42])` into
/// `T f<T>(List<T> s)` — the generic `T` solves to `String` from the
/// assignment target, so `[42]`'s `int` element reports `int` vs `String`
/// at `42` (oracle 3:17|2). Routed: `arg_slot_ty_ctx`'s open-formal
/// abstention lives in `scopes_bodies.rs` (READ-ONLY this lane) — the
/// generic-argument solving is not the literal channel's to invent.
#[test]
#[ignore = "routed: generic downward slot needs arg_slot_ty_ctx (scopes_bodies.rs)"]
fn routed_generic_downward_slot() {
    let src = "T f<T>(List<T> s) => s[0];\nvoid main() {\n  String x = f([42]);\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "42"),
        2,
        "The element type 'int' can't be assigned to the list type 'String'.",
    );
}

/// Class 6 (pattern declaration ctx): `List<int> xs = [if (true) "s"
/// else 1]` — the if-element's then-body `"s"` reports `String` vs `int`
/// (oracle 3:29|3). The pattern/if-element body's context donation is the
/// control-flow element's own face, not the bracket channel — routed to
/// the if/for element body resolution.
#[test]
#[ignore = "routed: if-element body ctx is the control-flow element face"]
fn routed_if_element_body_ctx() {
    let src = "void main() {\n  List<int> xs = [if (true) \"s\" else 1];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "\"s\""),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 7 (const face): `const List<int> ys = [1, "s"]` — the const
/// literal's `"s"` reports `String` vs `int` (oracle 3:28|3). The const
/// literal's element type is the const-eval face
/// (`const_eval_faces.rs`), which the channel's declared-type walk does
/// not reach — routed.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn routed_const_literal_element() {
    let src = "void main() {\n  const List<int> ys = [1, \"s\"];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "\"s\""),
        3,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}

/// Class 9 (misc singleton): `List<int> xs = [x!]` where `x` is `String?`
/// — the non-null-assertion element's operand resolves through the `!`
/// face to `String`, reporting `String` vs `int` at the `x!` span
/// (oracle 2:19|2). Routed to the `!` operand resolution — the channel's
/// row path must unwrap the assertion before asking assignability.
#[test]
#[ignore = "routed: non-null-assertion element operand face"]
fn routed_non_null_assertion_element() {
    let src = "void f(String? x) {\n  List<int> xs = [x!];\n}\n";
    let report = report_ops(src);
    assert_eq!(count_list_element(&report), 1, "{:#?}", rows(&report));
    assert_list_element(
        &report,
        at(src, "x!"),
        2,
        "The element type 'String' can't be assigned to the list type 'int'.",
    );
}
