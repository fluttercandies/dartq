//! #3428 pins — a live-prefix member miss (`p.C` with `p` a real import
//! prefix and `C` absent) routes through the resolver's context chain
//! exactly as a bare miss does.
//!
//! # The wrong fact
//!
//! `push_type_use`'s continuation arm answered every live-prefix member miss
//! with the `TypeVerdict::Prefixed` verdict, whose pass-2 arm reports
//! `undefined_class` unconditionally — it never asked the
//! `is`/`as`-operand window (`non_type_pos`/`emit_operand_type_miss`), the
//! type-argument face (`type_arg_miss_rows`), or the catch-`on` row
//! (`catch_on_miss_rows`). Upstream `reportNullOrNonTypeElement`
//! (named_type_resolver.dart:720-770) runs the same context arms for a
//! qualified `p.C` it runs for a bare `C`, so the oracle reports
//! `type_test_with_undefined_name`, `cast_to_non_type`,
//! `non_type_as_type_argument`, or `non_type_in_catch_clause` at the whole
//! written `p.C` span (`_getErrorRange`'s prefix-through-name default) —
//! `malformed_test.dart` reports the dedicated codes on every
//! `prefix.Unresolved` position.
//!
//! The fix pushes a `TypeVerdict::Missing` row over the qualified `p.C`
//! span for context positions only — the verdict the context machinery
//! already routes — and keeps `Prefixed` in the plain positions (the
//! local-head silence and the broken-import guard ride its own fields).

use crate::diagnose::test_rig::{RigImport, assert_silent, code_names, report_ops_imports};

/// `import 'package:p/p.dart' as p` — a live prefix edge.
const P: RigImport = RigImport {
    uri: "package:p/p.dart",
    prefix: Some("p"),
    source: "class C {}",
};

/// The codes of one fixture's report, in source order.
fn names(source: &str) -> Vec<&'static str> {
    let report = report_ops_imports(source, &[P]);
    code_names(&report)
}

/// An `is` operand whose qualified target misses the prefix member reports
/// `type_test_with_undefined_name` — never `undefined_class`
/// (`visitIsExpression`'s arm fires for `p.C` exactly as for `C`).
#[test]
fn t3428_is_prefixed_miss_reports_the_operand_code() {
    assert_eq!(
        names("void f(Object? v) { v is p.Missing; }"),
        ["type_test_with_undefined_name"]
    );
    assert_eq!(
        names("void f(Object? v) { v is p.Missing<int>; }"),
        ["type_test_with_undefined_name"]
    );
}

/// An `as` operand whose qualified target misses reports `cast_to_non_type`
/// — the same consumption the bare arm already pinned.
#[test]
fn t3428_as_prefixed_miss_reports_cast_to_non_type() {
    assert_eq!(
        names("void f(Object? v) { v as p.Missing; }"),
        ["cast_to_non_type"]
    );
    assert_eq!(
        names("void f(Object? v) { v as p.Missing<int>; }"),
        ["cast_to_non_type"]
    );
}

/// A qualified miss inside a type-argument group reports
/// `non_type_as_type_argument` — in a declaration and inside an `is`/`as`
/// operand alike (the argument face outranks the operand face).
#[test]
fn t3428_prefixed_miss_in_type_arguments_reports_the_argument_code() {
    assert_eq!(
        names("List<p.Missing>? x;"),
        ["non_type_as_type_argument"]
    );
    assert_eq!(
        names("void f(Object? v) { v is List<p.Missing>; }"),
        ["non_type_as_type_argument"]
    );
    assert_eq!(
        names("void f(Object? v) { v as List<p.Missing<int>>; }"),
        ["non_type_as_type_argument"]
    );
}

/// A catch `on` type's qualified miss reports `non_type_in_catch_clause` —
/// the parser's recovered on-type face (`malformed_test.dart`'s
/// `on prefix.Unresolved` arms).
#[test]
fn t3428_prefixed_miss_in_catch_on_reports_the_catch_code() {
    assert_eq!(
        names("void f() { try {} on p.Missing catch (e) {} }"),
        ["non_type_in_catch_clause", "unused_catch_clause"]
    );
    assert_eq!(
        names("void f() { try {} on p.Missing<int> catch (e) {} }"),
        ["non_type_in_catch_clause", "unused_catch_clause"]
    );
}

/// The report anchors at the whole written `p.Missing` span
/// (`_getErrorRange`'s prefix-through-name default), never the leaf's
/// offset.
#[test]
fn t3428_prefixed_miss_anchors_the_qualified_span() {
    let source = "void f(Object? v) { v is p.Missing; }";
    let report = report_ops_imports(source, &[P]);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_test_with_undefined_name")
        .expect("the operand code reports");
    // The analyzed unit is `{CORE_PRELUDE}{source}` — the qualified span
    // opens at the `p` token, not the leaf `Missing`.
    let at = crate::diagnose::test_rig::CORE_PRELUDE.len() as u32
        + source.find("p.Missing").unwrap() as u32;
    assert_eq!(row.range.offset, at);
    assert_eq!(row.range.length, "p.Missing".len() as u32);
    assert_eq!(
        row.message,
        "The name 'Missing' isn't defined, so it can't be used in an 'is' expression."
    );
}

/// A plain-position `p.C` miss keeps the `Prefixed` verdict's
/// `undefined_class` — the unchanged face the context conversion must not
/// touch.
#[test]
fn t3428_prefixed_miss_in_a_plain_position_keeps_undefined_class() {
    assert_eq!(
        names("typedef M = p.Missing;"),
        ["undefined_class"]
    );
    assert_eq!(
        names("p.Missing f() => throw 0;"),
        ["undefined_class"]
    );
}

/// Regression guards: a RESOLVED `p.C` operand produces neither row, a
/// resolved bare `is` operand stays silent, and `x is Future<Missing>`
/// keeps its parameterized-argument behaviour (the argument's bare miss
/// reports `non_type_as_type_argument`).
#[test]
fn t3428_resolved_and_bare_operands_unchanged() {
    let resolved = report_ops_imports(
        "typedef G = p.C; void f(Object? v) { v is p.C; v as p.C; }",
        &[P],
    );
    assert!(
        resolved.diagnostics.is_empty(),
        "expected silence, got {:?}",
        resolved
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert_silent(&report_ops_imports(
        "void f(Object? v) { v is int; v as int; }",
        &[P],
    ));
    assert_eq!(
        names("void f(Object? v) { v is Future<p.Missing>; }"),
        ["non_type_as_type_argument"]
    );
}
