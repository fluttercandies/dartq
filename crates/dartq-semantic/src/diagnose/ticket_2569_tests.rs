//! The #2569 pins: dead-code and switch-exhaustiveness sweep.
//!
//! The first root closed here is the C-style `for` **condition** death: a
//! condition slice that turns the flow unreachable mid-expression opens no
//! dead interval of its own (the literal polls inside a condition are gated
//! off as header parts), so the updaters' own row and the body block's row
//! stood in for the official's single condition-anchored row. The official
//! verifier anchors on the first node it VISITS after the kill
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:183-205) — the
//! argument after the `throw`, not the throwing argument itself — and the
//! enclosing close folds out to the loop's end. Both statement and
//! collection-element forms share the face.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The analyzed text [`report_ops`] diagnoses — the slice domain the row
/// offsets address.
fn analyzed(fixture: &str) -> String {
    format!("{CORE_PRELUDE}{fixture}")
}

/// The `dead_code` rows as `offset:length`, the span convention under pin.
fn dead_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| format!("{}:{}", d.range.offset, d.range.length))
        .collect()
}

/// Pin 1 — the statement form. `g(throw 0, 1)`'s second argument is the
/// first node the walk visits after the `throw` killed the flow, so the row
/// opens there and runs through the loop's tail: oracle `3:21|9` over
/// `1); 2) {}`. The pre-fix rows were the updater (`2`) and the body block
/// (`{}`) — two rows the oracle never emits.
#[test]
fn for_condition_death_anchors_at_the_post_throw_argument() {
    let source = "\
void f(bool Function(Object?, Object?) g) {
  for (; g(throw 0, 1); 2) {}
}
";
    let report = report_ops(source);
    let start = at(source, "1); 2) {}");
    assert_eq!(
        dead_rows(&report),
        vec![format!("{start}:9")],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(&analyzed(source)[start..start + 9], "1); 2) {}");
}

/// Pin 2 — the collection-element form shares the face: the same
/// condition-anchored row, folded out to the arrow body's `;` (oracle
/// `2:60|10` over `1); 2) 0];`).
#[test]
fn for_element_condition_death_shares_the_condition_anchor() {
    let source = "f(bool Function(Object?, Object?) g) => [for (; g(throw 0, 1); 2) 0];\n";
    let report = report_ops(source);
    let start = at(source, "1); 2) 0];");
    assert_eq!(
        dead_rows(&report),
        vec![format!("{start}:10")],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(&analyzed(source)[start..start + 10], "1); 2) 0];");
}
