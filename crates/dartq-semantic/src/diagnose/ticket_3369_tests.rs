//! #3369 — the extension override's `void` receiver row and the anchor of
//! its not-assignable sibling.
//!
//! Upstream `resolveOverride` reads the override's single argument as the
//! receiver and reports both verdicts at the WHOLE receiver expression
//! (extension_member_resolver.dart:288-313): `if (receiverType is
//! VoidType)` reports `use_of_void_result`, the `else if` reports
//! `extension_override_argument_not_assignable` — both
//! `.at(receiverExpression)`, so an invoked receiver anchors at `f()`
//! (the oracle's `test_extensionOverride_argument_error#0` → `9:5|3`),
//! never at the callee row's `f`.

use super::*;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// `(offset, length)` per row of `code`, in report order.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The UTF-16 offset of the receiver inside a `(receiver)` spelling, in
/// the text [`report_ops`] actually analyzes (the core prelude rides ahead
/// of the fixture). The pins spell each use as `(expr)` so a same-named
/// declaration's `f()` cannot match first.
fn receiver_offset(source: &str, needle: &str) -> u32 {
    let full = format!("{CORE_PRELUDE}{source}");
    let at = full.find(needle).expect("needle present") + 1;
    full[..at].chars().map(char::len_utf16).sum::<usize>() as u32
}

/// `E(f()).g` over a `void f()` reports one `use_of_void_result` anchored
/// at the whole invoked receiver `f()` — the callee row arrives as `f`, so
/// the invocation's closing `)` is what the anchor must reach.
#[test]
fn void_override_invocation_anchors_at_the_call() {
    let source = "extension E on int {\n\
         \x20 int get g => this;\n\
         }\n\
         void f() {}\n\
         void g() { E(f()).g; }\n";
    let report = report_ops(source);
    let expected = receiver_offset(source, "(f())");
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(expected, 3)],
        "E(f()) over a void f reports once at f(): {:?}",
        names(&report)
    );
    assert!(
        rows(&report, "extension_override_argument_not_assignable").is_empty(),
        "the void arm keeps the not-assignable row off the same argument: {:?}",
        names(&report)
    );
}

/// A plain identifier receiver has no invocation and no member read to
/// widen to — `E(v).g` over a `void v` parameter keeps `v`'s own span.
#[test]
fn void_override_plain_identifier_keeps_its_own_span() {
    let source = "extension E on int {\n\
         \x20 int get g => this;\n\
         }\n\
         void h(void v) { E(v).g; }\n";
    let report = report_ops(source);
    assert_eq!(
        rows(&report, "use_of_void_result"),
        vec![(receiver_offset(source, "(v)"), 1)],
        "E(v) over a void parameter reports at v: {:?}",
        names(&report)
    );
}

/// A non-`void` receiver takes the assignability arm only: `E(f()).g` over
/// `int f()` reports nothing at all.
#[test]
fn non_void_override_invocation_reports_no_void_row() {
    let source = "extension E on int {\n\
         \x20 int get g => this;\n\
         }\n\
         int f() => 0;\n\
         void g() { E(f()).g; }\n";
    let report = report_ops(source);
    assert!(
        rows(&report, "use_of_void_result").is_empty(),
        "an int receiver consumes no void value: {:?}",
        names(&report)
    );
    assert!(
        rows(&report, "extension_override_argument_not_assignable").is_empty(),
        "an int receiver is assignable to `on int`: {:?}",
        names(&report)
    );
}

/// The not-assignable sibling carries the same whole-expression anchor:
/// `E(f()).g` over `String f()` reports at `f()`, not at the callee `f`.
#[test]
fn not_assignable_override_invocation_anchors_at_the_call() {
    let source = "extension E on int {\n\
         \x20 int get g => this;\n\
         }\n\
         String f() => '';\n\
         void g() { E(f()).g; }\n";
    let report = report_ops(source);
    let expected = receiver_offset(source, "(f())");
    assert_eq!(
        rows(&report, "extension_override_argument_not_assignable"),
        vec![(expected, 3)],
        "E(f()) over a String f reports at f(): {:?}",
        names(&report)
    );
    assert!(
        rows(&report, "use_of_void_result").is_empty(),
        "a String receiver consumes no void value: {:?}",
        names(&report)
    );
}

/// A non-invoked member receiver keeps the property-access span: `E(c.bad)`
/// anchors the not-assignable row at the whole read `c.bad`, never at the
/// selector `bad` alone (the invocation widening must not disturb the
/// property-access fallback).
#[test]
fn not_assignable_override_property_access_anchors_at_the_read() {
    let source = "extension E on String {\n\
         \x20 int get g => 0;\n\
         }\n\
         class C {\n\
         \x20 int bad = 0;\n\
         }\n\
         void h(C c) { E(c.bad).g; }\n";
    let report = report_ops(source);
    assert_eq!(
        rows(&report, "extension_override_argument_not_assignable"),
        vec![(receiver_offset(source, "(c.bad)"), 5)],
        "E(c.bad) over an int getter reports at the read c.bad: {:?}",
        names(&report)
    );
}
