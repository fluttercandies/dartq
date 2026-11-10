//! #3707 pins (the `listener.rs` half) — `end_record_literal`'s
//! `use_of_void_result` anchor for a named field NOT last in source order.
//!
//! The oracle anchors `use_of_void_result` on the FIELD node — a
//! `RecordLiteralNamedField` covers `name: value` (upstream Dart SDK
//! source, `pkg/analyzer/lib/src/dart/resolver/record_literal_resolver
//! .dart:159` `.at(field)`). The anchor loop gated `named_field_span` on
//! `index >= first_named` — right only when every named field trails the
//! positional ones; a named field in first position kept the value-only
//! anchor and a trailing positional field burned a span call that fell
//! back anyway. `named_field_span` self-detects (no adjoining `:` →
//! `None`), so the gate is gone and every candidate goes through it.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

/// The `use_of_void_result` rows' `(offset, length)` pairs, ascending.
fn void_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "use_of_void_result")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    rows.sort_unstable();
    rows
}

/// `needle`'s absolute offset inside `fixture` as the rig analyzes it (the
/// prelude shifts every fixture-local offset).
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// `(a: v(), 1)` — a `void` send as a named field NOT last in source
/// order: the anchor covers `a: v()` — the label included.
#[test]
fn named_field_not_last_anchors_at_label() {
    let fixture = "void v() {}\nvoid main() {\n  var r = (a: v(), 1);\n  print(r);\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "a: v()");
    let len = "a: v()".chars().map(char::len_utf16).sum::<usize>() as u32;
    assert_eq!(
        void_rows(&report),
        vec![(start, len)],
        "{:#?}",
        rows(&report)
    );
}

/// `(a: v())` — the trailing named field (the common shape the gate was
/// written for): unchanged — the anchor still covers `a: v()`.
#[test]
fn trailing_named_field_anchor_unchanged() {
    let fixture = "void v() {}\nvoid main() {\n  var r = (a: v());\n  print(r);\n}\n";
    let report = report_ops(fixture);
    let start = at(fixture, "a: v()");
    let len = "a: v()".chars().map(char::len_utf16).sum::<usize>() as u32;
    assert_eq!(
        void_rows(&report),
        vec![(start, len)],
        "{:#?}",
        rows(&report)
    );
}

/// `(v(), a: 1)` — a positional `void` field before a named one: its
/// anchor stays the bare value send `v()` — no label adjoins it.
#[test]
fn positional_field_before_named_keeps_value_anchor() {
    let fixture = "void v() {}\nvoid main() {\n  var r = (v(), a: 1);\n  print(r);\n}\n";
    let report = report_ops(fixture);
    // `find` would hit the `v()` inside `void v() {}` — anchor at the call
    // inside the record literal instead.
    let start = at(fixture, "(v(), a: 1)") + 1;
    let len = "v()".chars().map(char::len_utf16).sum::<usize>() as u32;
    assert_eq!(
        void_rows(&report),
        vec![(start, len)],
        "{:#?}",
        rows(&report)
    );
}

/// `(a: v(), b: v())` — two named `void` fields, both anchored at their
/// labels in source order.
#[test]
fn two_named_void_fields_anchor_at_their_labels() {
    let fixture = "void v() {}\nvoid main() {\n  var r = (a: v(), b: v());\n  print(r);\n}\n";
    let report = report_ops(fixture);
    let len = "a: v()".chars().map(char::len_utf16).sum::<usize>() as u32;
    assert_eq!(
        void_rows(&report),
        vec![(at(fixture, "a: v()"), len), (at(fixture, "b: v()"), len),],
        "{:#?}",
        rows(&report)
    );
}
