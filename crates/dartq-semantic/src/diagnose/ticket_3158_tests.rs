//! #3158 — the `undefined_getter` display and compound-write read faces.
//!
//! The getter face keeps the full `getDisplayString` render — type
//! arguments stay (`B<int> b; b.gone;` names `B<int>`) — but the
//! NULLABILITY is stripped on a null-aware access: the oracle resolves
//! `b?.gone` on the shorted non-null face and renders it without the `?`
//! (probe 3.13.7: `B<int>? b; b?.gone;` reports "The getter 'gone' isn't
//! defined for the type 'B<int>'." — upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/lookup_failure_reporter.dart's
//! `InstanceLookupDomain`, which reports `getDisplayString` of the
//! RESOLVED receiver type). A compound write resolves the getter BEFORE
//! the setter: a write satisfied through the setter spelling alone still
//! reports the read face's own miss (`undefined_getter`), on the
//! instance face and the static-container face alike.

use super::test_rig::report_ops;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    rows_named(report, name)
        .into_iter()
        .map(|d| d.message.to_string())
        .collect()
}

// —— The getter face's display ————————————————————————————————————

/// A null-aware read strips the receiver type's nullability while keeping
/// the written arguments (`B<int>? b; b?.gone;` names `B<int>`).
#[test]
fn null_aware_getter_face_strips_nullability_keeps_arguments() {
    let report = report_ops(
        "class B<T> {}\n\
         void f(B<int>? b) {\n\
         \x20 b?.gone;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'B<int>'."),
        "null-aware getter names the shorted face: {getters:?}"
    );
}

/// A plain read on a non-nullable generic receiver keeps the full render
/// with its arguments (`B<int> b; b.gone;` names `B<int>` — the strip is
/// the null-aware face's alone).
#[test]
fn plain_getter_face_keeps_the_full_display() {
    let report = report_ops(
        "class B<T> {}\n\
         void f(B<int> b) {\n\
         \x20 b.gone;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'B<int>'."),
        "plain getter keeps the arguments: {getters:?}"
    );
}

// —— The compound write's read face ———————————————————————————————

/// A compound write over a SETTER-only class reports the getter half's
/// miss and nothing else (`c.foo += 1` names `C`; the satisfied setter
/// face stays silent).
#[test]
fn compound_write_over_setter_only_reports_the_getter() {
    let report = report_ops(
        "class C {\n\
         \x20 set foo(int v) {}\n\
         }\n\
         void f(C c) {\n\
         \x20 c.foo += 1;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'C'."),
        "the read face names the class: {getters:?}"
    );
    assert!(
        rows_named(&report, "undefined_setter").is_empty(),
        "the satisfied write stays silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The same split on the static-container face (`C.foo += 1` over a
/// `static set foo` reports `undefined_getter` naming the container).
#[test]
fn static_compound_write_over_setter_only_reports_the_getter() {
    let report = report_ops(
        "class C {\n\
         \x20 static set foo(int v) {}\n\
         }\n\
         void f() {\n\
         \x20 C.foo += 1;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'C'."),
        "the static read face names the container: {getters:?}"
    );
    assert!(
        rows_named(&report, "undefined_setter").is_empty(),
        "the satisfied write stays silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A getter-backed compound write keeps its read face silent — only the
/// setter half reports (`c.foo += 1` over a getter-only `C` is
/// `undefined_setter` alone).
#[test]
fn compound_write_over_getter_only_reports_the_setter() {
    let report = report_ops(
        "class C {\n\
         \x20 int get foo => 0;\n\
         }\n\
         void f(C c) {\n\
         \x20 c.foo += 1;\n\
         }\n",
    );
    assert!(
        rows_named(&report, "undefined_getter").is_empty(),
        "the satisfied read stays silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    let setters = messages_named(&report, "undefined_setter");
    assert_eq!(setters.len(), 1, "one setter miss: {setters:?}");
}
