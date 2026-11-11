//! Ticket #4133 pins: the two span-face fixes this lane adjudicated.
//!
//! # Face ① — `duplicate_field_name`
//!
//! Oracle `dart analyze --format=machine` (3.13.7 SDK) on the record-literal
//! and record-type fixtures these pins mirror:
//!
//! - `(x: 1, y: 'a', x: 2)` → `DUPLICATE_FIELD_NAME …|2|26|1` (the LATER
//!   `x` label token, length = the name's utf16 length).
//! - `({int x, int x})` → `DUPLICATE_FIELD_NAME …|1|21|1` (same shape on
//!   the type-annotation side).
//!
//! The literal side's walk lives in `record_field_tys`
//! (diagnose/resolution.rs:5063-5120) — every named field after the first
//! occurrence reports at its own label span, keeping the FIRST occurrence
//! as the `The first ` context anchor. The type-annotation side's walk
//! lives in `check_record_type_field_names`
//! (diagnose/engine_core.rs:3228) — same span contract through
//! `check_record_field_name`'s sibling path — **ROUTED to #4129**
//! (engine_core.rs is that ticket's reserved file; this lane's
//! editable-side contribution is the `SemanticCode::DuplicateFieldName`
//! variant + message/correction arms in codes.rs).
//!
//! # Face ② — `dead_code` after an always-throwing cast pattern
//!
//! Oracle on the `case Foo() as Bar:` over a `Foo` scrutinee fixture
//! (`class Foo {} class Bar {} test(Foo x) { switch (x) { case Foo() as
//! Bar: break; case Foo(): break; case Bar(): break; } }`):
//!
//! - `UNREACHABLE_SWITCH_CASE …|7|5|4` on `case Foo():` (covered by the
//!   cast's subpattern space).
//! - `DEAD_CODE …|9|5|4` on `case Bar():` (the `case` keyword).
//! - `DEAD_CODE …|10|7|6` on `break;` (the dead body statement).
//!
//! With a never-matching typed pattern appended (`case int():`), the
//! oracle additionally emits `PATTERN_NEVER_MATCHES_VALUE_TYPE …|11|10|3`
//! on `int` plus `DEAD_CODE` rows on the case head and body.
//!
//! The flow's reachability marks the case body and every subsequent case
//! dead after the always-throwing cast exhausts the switch — `dead_code`
//! rides the same emission face every other dead-statement row uses.
//! **TREE-FIXED**: the mechanism already lands these rows; the pins below
//! guard the oracle-verified spans.

use super::FileReport;
use super::test_rig::{report_ops_bare, rows};

/// The absolute fixture offset of `needle` (no prelude — `report_ops_bare`).
fn at(fixture: &str, needle: &str) -> usize {
    fixture.find(needle).unwrap()
}

/// Exactly one `code` row anchored at `offset` with `length`, carrying
/// `message` — the oracle's `(code, offset, length, message)` contract.
#[track_caller]
fn assert_row(report: &FileReport, code: &str, offset: usize, length: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == code && d.range.offset as usize == offset && d.range.length == length
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {code} row at {offset}+{length}: {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

const DUP_FIELD: &str = "duplicate_field_name";
const DEAD_CODE: &str = "dead_code";
const NEVER_MATCHES: &str = "pattern_never_matches_value_type";

// —— Face ①: `duplicate_field_name` ————————————————————————————————————

/// `(x: 1, y: 'a', x: 2)` — the LATER `x` label reports at its own span.
/// Oracle `DUPLICATE_FIELD_NAME …|2|26|1`. The literal-side walk in
/// `record_field_tys` emits through `SemanticCode::DuplicateFieldName`.
/// The message's `{name}` slot is #4119's fill (resolution.rs is foreign);
/// the parity gate is `(severity, code, file)` so the span+code is the
/// contract pinned here.
#[test]
fn record_literal_duplicate_named_field_reports_later_label() {
    let src = "void f() {\n  var r = (x: 1, y: 'a', x: 2);\n}\n";
    let report = report_ops_bare(src);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == DUP_FIELD
                && d.range.offset as usize == at(src, "x: 2")
                && d.range.length == 1
        })
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}

/// `(a, b: 1, c, d: 2, b: 3)` — a mixed positional+named literal where the
/// duplicate `b` label sits past positional fields. Oracle reports at the
/// later `b` label (`…|2|30|1` on the probe fixture). The literal-side
/// walk's `positional_count` derivation must not misclassify the named
/// windows.
#[test]
fn record_literal_mixed_positional_named_duplicate_reports() {
    let src = "void f() {\n  var r = (a, b: 1, c, d: 2, b: 3);\n}\n";
    let report = report_ops_bare(src);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == DUP_FIELD
                && d.range.offset as usize == at(src, "b: 3")
                && d.range.length == 1
        })
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}

/// `(x: 1, y: 'a', x: 2, x: 3)` — every occurrence after the first reports.
/// Oracle emits TWO `DUPLICATE_FIELD_NAME` rows at the second and third `x`
/// labels. The `used` map keeps the FIRST occurrence as the context anchor.
#[test]
fn record_literal_triple_duplicate_reports_each_later_occurrence() {
    let src = "void f() {\n  var r = (x: 1, y: 'a', x: 2, x: 3);\n}\n";
    let report = report_ops_bare(src);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == DUP_FIELD)
        .collect();
    assert_eq!(hits.len(), 2, "{:#?}", rows(&report));
    assert_eq!(hits[0].range.offset as usize, at(src, "x: 2"));
    assert_eq!(hits[0].range.length, 1);
    assert_eq!(hits[1].range.offset as usize, at(src, "x: 3"));
    assert_eq!(hits[1].range.length, 1);
}

/// `({int x, int x})` — the type-annotation side's twin. Oracle
/// `DUPLICATE_FIELD_NAME …|1|21|1`. **ROUTED**: the type-side walk's
/// natural home is `check_record_type_field_names`
/// (diagnose/engine_core.rs:3228) — #4129's reserved file — so the
/// emission itself is not this lane's to land; the `SemanticCode`
/// variant + message/correction arms in codes.rs are the editable-side
/// companion this lane owns.
#[test]
fn record_type_duplicate_named_field_is_routed_to_engine_core() {
    let src = "void g(({int x, int x}) r) {}\n";
    let report = report_ops_bare(src);
    // The literal-side walk does not fire on type annotations; the
    // type-side emission is #4129's domain. This pin documents the
    // routing boundary — it asserts the CURRENT (pre-#4129) silence so
    // a future lane can flip it when the emission lands.
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == DUP_FIELD)
        .collect();
    assert!(hits.is_empty(), "{:#?}", rows(&report));
}

// —— Face ②: `dead_code` after always-throwing cast pattern —————————————

/// `case Foo() as Bar:` over a `Foo` scrutinee — the cast's required type
/// `Bar` can never match `Foo`, so the case body and every subsequent
/// case are dead. Oracle rows on the probe fixture
/// (`class Foo {} class Bar {} test(Foo x) { switch (x) { case Foo() as
/// Bar: break; case Foo(): break; case Bar(): break; } }`):
/// `UNREACHABLE_SWITCH_CASE …|7|5|4`, `DEAD_CODE …|9|5|4`,
/// `DEAD_CODE …|10|7|6`.
#[test]
fn dead_code_after_always_throwing_cast_pattern() {
    let src = "class Foo {}\nclass Bar {}\n\
               void f(Foo x) {\n  switch (x) {\n    case Foo() as Bar:\n      break;\n    case Foo():\n      break;\n    case Bar():\n      break;\n  }\n}\n";
    let report = report_ops_bare(src);
    // `case` keyword of `case Bar():` — the dead case head.
    assert_row(&report, DEAD_CODE, at(src, "case Bar():"), 4, "Dead code.");
    // `break;` body of `case Bar():` — the dead body statement.
    let bar_case = src.find("case Bar():").unwrap();
    let bar_break = src[bar_case..].find("break;").unwrap() + bar_case;
    assert_row(&report, DEAD_CODE, bar_break, 6, "Dead code.");
}

/// `case Foo() as Bar:` + `case Foo():` + `case Bar():` + `case int():` —
/// the same always-throwing cast with a never-matching typed pattern
/// after it. Oracle rows: `UNREACHABLE_SWITCH_CASE …|7|5|4`,
/// `DEAD_CODE …|9|5|4`, `DEAD_CODE …|10|7|6`, `DEAD_CODE …|11|5|4`,
/// `PATTERN_NEVER_MATCHES_VALUE_TYPE …|11|10|3`, `DEAD_CODE …|12|7|6`.
#[test]
fn dead_code_after_always_throwing_cast_pattern_with_never_match() {
    let src = "class Foo {}\nclass Bar {}\n\
               void f(Foo x) {\n  switch (x) {\n    case Foo() as Bar:\n      break;\n    case Foo():\n      break;\n    case Bar():\n      break;\n    case int():\n      break;\n  }\n}\n";
    let report = report_ops_bare(src);
    // `case` keyword of `case Bar():` — the dead case head.
    assert_row(&report, DEAD_CODE, at(src, "case Bar():"), 4, "Dead code.");
    // `break;` body of `case Bar():` — the dead body statement.
    let bar_case = src.find("case Bar():").unwrap();
    let bar_break = src[bar_case..].find("break;").unwrap() + bar_case;
    assert_row(&report, DEAD_CODE, bar_break, 6, "Dead code.");
    // `case` keyword of `case int():` — the dead case head.
    assert_row(&report, DEAD_CODE, at(src, "case int():"), 4, "Dead code.");
    // `int` type in `case int():` — the never-match verdict.
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "int():"),
        3,
        "The matched value type 'Foo' can never match the required type 'int'.",
    );
    // `break;` body of `case int():` — the dead body statement.
    let int_case = src.find("case int():").unwrap();
    let int_break = src[int_case..].find("break;").unwrap() + int_case;
    assert_row(&report, DEAD_CODE, int_break, 6, "Dead code.");
}
