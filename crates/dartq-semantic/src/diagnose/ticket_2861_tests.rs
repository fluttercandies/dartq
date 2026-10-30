//! The #2861 case-body dead-row anchor pins: a switch member whose body
//! is preceded by a comment (`case null: // Unreachable`) anchors the
//! dead row at the body's FIRST statement, not the comment's span — the
//! official row trims to the member's statement extent
//! (`dead_code_verifier.dart`'s `flowEnd` member trim; corpus
//! `bool_switch_test`/`enum_switch_test`: expected `145:7|27` over
//! `print('null');` + `break;`, the pre-fix row rode the trailing
//! `// Unreachable` comment at `144:16|48`). The fix snaps
//! `trimmed_span`'s leading edge to the first token in the span
//! (comments are outside the token table), so comment-free bodies are
//! byte-identical.

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

/// Pin 1 — the corpus shape: a trailing comment on the case head must not
/// absorb the anchor. The row opens at the body's first statement
/// (`print('null')`) and runs through `break;` — exactly
/// `print('null');\n      break;` (27 UTF-16 units), never the comment.
#[test]
fn comment_led_body_anchors_at_first_statement() {
    let source = "\
void f(bool b) {
  switch (b) {
    case true:
      break;
    case null: // Unreachable
      print('null');
      break;
  }
}
";
    let report = report_ops(source);
    let start = at(source, "print('null')");
    assert_eq!(
        dead_rows(&report),
        vec![format!("{start}:27")],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        &analyzed(source)[start..start + 27],
        "print('null');\n      break;"
    );
}

/// Pin 2 — regression: the comment-free body keeps its precise row (the
/// first-statement anchor the fix makes unconditional).
#[test]
fn comment_free_body_keeps_first_statement_anchor() {
    let source = "\
void f(bool b) {
  switch (b) {
    case true:
      break;
    case null:
      print('null');
      break;
  }
}
";
    let report = report_ops(source);
    let start = at(source, "print('null')");
    assert_eq!(
        dead_rows(&report),
        vec![format!("{start}:27")],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        &analyzed(source)[start..start + 27],
        "print('null');\n      break;"
    );
}

/// Pin 3 — several statements in the body: the anchor is still the FIRST
/// statement, the row running through the member's last one.
#[test]
fn multiple_statements_anchor_at_the_first() {
    let source = "\
void f(bool b) {
  switch (b) {
    case true:
      break;
    case null: // Unreachable
      print('first');
      print('second');
      break;
  }
}
";
    let report = report_ops(source);
    let start = at(source, "print('first')");
    // `print('first');` + blank-joined `print('second');` + `break;`
    // (15 + 7 + 16 + 7 + 6).
    assert_eq!(
        dead_rows(&report),
        vec![format!("{start}:51")],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        &analyzed(source)[start..start + 51],
        "print('first');\n      print('second');\n      break;"
    );
}
