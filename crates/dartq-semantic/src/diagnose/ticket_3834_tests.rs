//! #3834 pins — the alias invoke face asks the `Type` instance's own
//! member face before it reports.
//!
//! An ungrounded type-alias receiver (`Cb` over `typedef Cb = void
//! Function()`) denotes the `Type` INSTANCE, so its selector resolves with
//! the ordinary property/invocation machinery — `Type`'s interface (the
//! `Object` face) first, then every applicable extension of the import
//! closure — and only a clean miss reports `undefined_method` naming
//! `Type`. `report_alias_selector_face` used to emit that row for every
//! selector, so `Cb.toString()` — which answers `Object.toString` — was a
//! spurious `undefined_method` (`undefined_method` gap: 16 fabricated rows
//! across the Constructor-tear-offs `summary_A03_*` trio and
//! `built_in_type_annotation_test_typedef`).
//!
//! The oracle's (probe 3.13.7, `dart analyze --format=machine` over a
//! single-file package) exact rows for the fixtures below:
//!
//! ```text
//! // typedef Cb = void Function(); Cb.toString(); Cb.hashCode; Cb.runtimeType;
//! (no rows)
//! // extension EOnObject on Object { int cm() => 1; } Cb.cm();
//! (no rows)
//! // Cb.sm(); extension EOnString on String { int sm() => 1; }
//! ERROR|COMPILE_TIME_ERROR|UNDEFINED_METHOD|lib/test.dart|6|6|2|The method
//!   'sm' isn't defined for the type 'Type'.
//! // Cb.named();
//! ERROR|COMPILE_TIME_ERROR|UNDEFINED_METHOD|lib/test.dart|7|6|5|The method
//!   'named' isn't defined for the type 'Type'.
//! // Cb.foo;
//! ERROR|COMPILE_TIME_ERROR|UNDEFINED_GETTER|lib/test.dart|8|14|3|The getter
//!   'foo' isn't defined for the type 'Type'.
//! ```
//!
//! The miss direction is pinned as hard as the silence: the same predicate
//! must not swallow a genuine `Type` miss, and a concrete `on` head
//! (`on String`) declares the member for ITS receivers only, so it never
//! suppresses the literal's row.

use super::FileReport;
use super::test_rig::report_ops;

/// `typedef Cb = void Function();` — the ungrounded alias head every
/// fixture uses.
const CB_ALIAS: &str = "typedef Cb = void Function();\n";

/// Every diagnostic's `(code name, message)` in report order.
fn rows(report: &FileReport) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .map(|row| (row.code.name.to_string(), row.message.to_string()))
        .collect()
}

/// Messages of one code, in report order.
fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .map(|row| row.message.to_string())
        .collect()
}

/// `Cb.toString()` answers `Object.toString` through the `Type` instance —
/// no row at all. The corpus twin is
/// `co19/LanguageFeatures/Constructor-tear-offs/summary_A03_t01.dart:90`.
#[test]
fn alias_literal_object_method_invoke_stays_silent() {
    let report = report_ops(&format!("{CB_ALIAS}void f() {{\n  Cb.toString();\n}}\n"));
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// The `Object` getters resolve the same way: `Cb.hashCode` and
/// `Cb.runtimeType` are property reads of `Type`, not misses.
#[test]
fn alias_literal_object_getter_reads_stay_silent() {
    let report = report_ops(&format!(
        "{CB_ALIAS}int f(int x) {{\n  Cb.hashCode;\n  Cb.runtimeType;\n  return x;\n}}\n"
    ));
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// An extension on `Object` is applicable to `Type`, so `Cb.cm()` resolves
/// and stays silent (probe: no rows).
#[test]
fn alias_literal_object_extension_invoke_stays_silent() {
    let report = report_ops(&format!(
        "{CB_ALIAS}extension EOnObject on Object {{\n  int cm() => 1;\n}}\n\
         void f() {{\n  Cb.cm();\n}}\n"
    ));
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// A genuine miss still reports once, naming `Type` — the fix must not
/// silence `Cb.named()`.
#[test]
fn alias_literal_invoke_miss_still_names_type() {
    let report = report_ops(&format!("{CB_ALIAS}void f() {{\n  Cb.named();\n}}\n"));
    assert_eq!(
        messages_named(&report, "undefined_method"),
        ["The method 'named' isn't defined for the type 'Type'."]
    );
}

/// The read twin is untouched: `Cb.foo` reports `undefined_getter` naming
/// `Type`.
#[test]
fn alias_literal_getter_miss_still_names_type() {
    let report = report_ops(&format!(
        "{CB_ALIAS}int f(int x) {{\n  Cb.foo;\n  return x;\n}}\n"
    ));
    assert_eq!(
        messages_named(&report, "undefined_getter"),
        ["The getter 'foo' isn't defined for the type 'Type'."]
    );
}

/// A concrete `on` head declares the member for ITS receivers only:
/// `on String` never answers a `Type` receiver, so `Cb.sm()` keeps its row
/// (probe: one `undefined_method` naming `Type`).
#[test]
fn alias_literal_concrete_on_extension_keeps_type_row() {
    let report = report_ops(&format!(
        "{CB_ALIAS}extension EOnString on String {{\n  int sm() => 1;\n}}\n\
         void f() {{\n  Cb.sm();\n}}\n"
    ));
    assert_eq!(
        messages_named(&report, "undefined_method"),
        ["The method 'sm' isn't defined for the type 'Type'."]
    );
}
