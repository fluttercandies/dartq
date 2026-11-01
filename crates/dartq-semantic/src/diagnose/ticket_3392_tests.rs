//! #3392 — the operand-static-type faces of `==`/`!=`/`is` over
//! top-level bare reads and completed sends.
//!
//! The verdicts themselves already resolve those operands
//! ([`Engine::type_check_operand_ty`], [`Engine::null_comparison`]): a
//! top-level variable read answers its declared type and a completed
//! send answers the callee's declared return. What did not hold is the
//! *anchor*: the invariant-null-comparison row opens at the operator and
//! runs through the `null` operand, while a send operand's row ends at
//! the callee identifier — the trimmed gap over its argument list
//! widened the span to `() == null` where the oracle reports `== null`.
//!
//! The pins below carry the oracle's own answer for the three operand
//! shapes and the empty answer for a genuinely unknown one: a `dynamic`
//! operand types no constant and must stay silent (staging a guess would
//! over-report).

use super::*;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// The unit's source as the rig analyzed it (the prelude prefix included,
/// so diagnostic offsets index this string).
fn unit_source(body: &str) -> String {
    format!("{CORE_PRELUDE}{body}")
}

/// The UTF-16 slice of `[offset, offset + length)` over `source`.
fn utf16_slice(source: &str, offset: u32, length: u32) -> String {
    let end = offset + length;
    let mut out = String::new();
    let mut at = 0u32;
    for ch in source.chars() {
        let width = ch.len_utf16() as u32;
        if at >= end {
            break;
        }
        if at >= offset {
            out.push(ch);
        }
        at += width;
    }
    out
}

/// The source text the first `code` row of `report` covers, sliced over
/// the rig's prelude-prefixed unit.
fn row_text(report: &FileReport, source: &str, code: &str) -> Option<String> {
    let row = report.diagnostics.iter().find(|d| d.code.name == code)?;
    Some(utf16_slice(source, row.range.offset, row.range.length))
}

// —— the operand shapes this ticket's seam feeds ——

/// A completed send as the compared operand: `int intFn() => 0` types
/// the invocation `int`, so `intFn() == null` is constant-false. The row
/// anchors at the operator through `null` — the callee row's own end sits
/// before the argument list and must not widen it.
#[test]
fn call_result_equality_anchors_at_the_operator() {
    let body = "int intFn() => 0;\nvoid f() { intFn() == null; }\n";
    let source = unit_source(body);
    let report = report_ops(body);
    assert_eq!(
        row_text(&report, &source, "unnecessary_null_comparison").as_deref(),
        Some("== null"),
        "{:?}",
        report.diagnostics
    );
}

/// The `null`-on-the-left polarity of the same arm: `null == intFn()`
/// reports `null ==` (the oracle's extent runs from the null operand
/// through the operator).
#[test]
fn call_result_equality_null_first_anchors_at_the_operator() {
    let body = "int intFn() => 0;\nvoid f() { null == intFn(); }\n";
    let source = unit_source(body);
    let report = report_ops(body);
    assert_eq!(
        row_text(&report, &source, "unnecessary_null_comparison").as_deref(),
        Some("null =="),
        "{:?}",
        report.diagnostics
    );
}

/// A top-level bare read as the compared operand: `int topLevelInt` is
/// strictly non-nullable, so `topLevelInt == null` is constant-false and
/// the row anchors at the operator through `null`.
#[test]
fn top_level_read_equality_anchors_at_the_operator() {
    let body = "int topLevelInt = 0;\nvoid f() { topLevelInt == null; }\n";
    let source = unit_source(body);
    let report = report_ops(body);
    assert_eq!(
        row_text(&report, &source, "unnecessary_null_comparison").as_deref(),
        Some("== null"),
        "{:?}",
        report.diagnostics
    );
}

/// A completed send as the `is` operand: `intFn()` types `int`, so
/// `intFn() is int` is constant-true (`_checkAllTypeChecks`'s provable
/// subtype arm) and the row covers the whole check.
#[test]
fn call_result_type_check_reports() {
    let body = "int intFn() => 0;\nvoid f() { intFn() is int; }\n";
    let source = unit_source(body);
    let report = report_ops(body);
    assert_eq!(
        row_text(&report, &source, "unnecessary_type_check").as_deref(),
        Some("intFn() is int"),
        "{:?}",
        report.diagnostics
    );
}

// —— the unknown-operand guard: no fabricated constant ——

/// A genuinely unknown operand types nothing, so no verdict may fire: a
/// `dynamic`-returning send, a `dynamic` formal, and an undeclared name
/// all stay silent (`dynamic` is not strictly non-nullable, and no type
/// check against it is provable; the unresolved read has no static type
/// at all — staging a guess would over-report).
#[test]
fn unknown_operand_stays_silent() {
    let body = "dynamic dynFn() => 0;\n\
                void f() { dynFn() == null; dynFn() is int; }\n\
                void g(dynamic d) { d == null; d is int; }\n\
                void h() { missingName == null; }\n";
    let report = report_ops(body);
    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .filter(|name| {
            matches!(*name, "unnecessary_null_comparison" | "unnecessary_type_check")
        })
        .collect();
    assert!(codes.is_empty(), "{codes:?}");
}
