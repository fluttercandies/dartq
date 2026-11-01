//! #3288 pins — the `dead_code` emit layer's two remaining shape families.
//!
//! The face's dominant residual root is upstream of this file (the flow
//! layer's facts carry a token span, while the oracle's `flowEnd` derives
//! the reported length from the flow-ending NODE, `length = node.end -
//! offset`, dead_code_verifier.dart:321). What the emit layer owns is the
//! tail shaping and the dead-region containment, and both are pinned here:
//!
//! * the `for` header's `)` never rides a dead run's tail — the `ForParts`
//!   arm clamps the tail to the last updater (`node = parent.updaters.last`,
//!   :309-310), so `for (;; 0, throw 1, 2)` rows `2`, never `2)`;
//! * a run that ends a collection element carries the literal's closers and
//!   the statement's `;` (`[for (;; 1) ...[throw '', 2]];` rows `2]];`),
//!   while a `)` beyond the absorbed closers belongs to an enclosing call
//!   the dead node does not cover;
//! * a dead catch clause subsumes the dead code inside it — the official
//!   verifier keeps `_deadCatchClauseRanges` and returns before latching a
//!   nested dead node (:449-455), so a dead clause's `if (false) {}` yields
//!   no second row.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// Every `dead_code` row's SOURCE TEXT, in report order.
///
/// [`report_ops`] reports offsets into `CORE_PRELUDE + source`, so the
/// assertions read the reported span back out of that same text — the
/// shape the oracle compares, independent of prelude length.
fn dead_text(report: &FileReport, source: &str) -> Vec<String> {
    let text = format!("{CORE_PRELUDE}{source}");
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            let start = d.range.offset as usize;
            let end = start + d.range.length as usize;
            text.get(start..end)
                .expect("row span is inside the reported text")
                .to_owned()
        })
        .collect()
}

/// Every row of one code, in report order.
fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

/// The dominant tail shape: a dead run that ends at the last `for` updater
/// stops at that updater — the header's `)` closes a live statement head
/// and is never part of the reported node.
#[test]
fn for_header_updater_tail_excludes_the_header_paren() {
    let source = "void f() {\n  for (;; 0, throw 1, 2) {}\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["2".to_owned()]);
}

/// The same header inside a collection `for` element: the tail still stops
/// at the updater (the `)` here closes the element's `for`, not a literal).
#[test]
fn for_element_header_updater_tail_excludes_the_header_paren() {
    let source = "f() => [for (;; 0, throw 1, 2) 0];\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["2".to_owned()]);
}

/// A run that ends a collection element carries the literal's closers and
/// the statement terminator: `[for (;; 1) ...[throw '', 2]];` rows `2]];`.
#[test]
fn collection_tail_absorbs_closers_and_terminator() {
    let source = "f() => [for (;; 1) ...[throw '', 2]];\n";
    let report = report_ops(source);
    assert!(
        dead_text(&report, source).iter().any(|t| t == "2]];"),
        "{:?}",
        dead_text(&report, source)
    );
}

/// A clause the catch-chain family reported dead subsumes its body: the
/// nested `if (false) {}` adds no second row.
#[test]
fn dead_catch_clause_suppresses_nested_dead_code() {
    let source = "f() {\n  try {} catch (e) {} catch (e) { if (false) {} }\n}\n";
    let report = report_ops(source);
    assert!(dead_text(&report, source).is_empty());
    assert_eq!(count(&report, "dead_code_catch_following_catch"), 1);
}

/// The `on`-clause face is the same region: a subtype-shadowed clause's
/// nested dead block rides the clause row only.
#[test]
fn dead_on_catch_clause_suppresses_nested_dead_code() {
    let source = "class A {}\nclass B extends A {}\n\
                  f() {\n  try {} on A catch (e) {} on B catch (e) { if (false) {} }\n}\n";
    let report = report_ops(source);
    assert!(dead_text(&report, source).is_empty());
    assert_eq!(count(&report, "dead_code_on_catch_subtype"), 1);
}

/// The silent-negative half: a LIVE catch clause keeps its nested row — the
/// containment gate keys on the clause-level families, never on liveness
/// guesses of its own.
#[test]
fn live_catch_clause_keeps_nested_dead_code() {
    let source = "f() {\n  try {} catch (e) { if (false) {} }\n}\n";
    let report = report_ops(source);
    assert_eq!(dead_text(&report, source), vec!["{}".to_owned()]);
    assert_eq!(count(&report, "dead_code_catch_following_catch"), 0);
}

/// Regression: the plain dead-block spans the tail rules must leave alone —
/// an `if`/`else` block entered dead, and a statement after `return`.
#[test]
fn plain_dead_block_and_after_return_spans_are_unchanged() {
    let source = "f() {\n  if (true) {} else {}\n  return;\n  print(1);\n}\n";
    let report = report_ops(source);
    assert_eq!(
        dead_text(&report, source),
        vec!["{}".to_owned(), "print(1);".to_owned()]
    );
}
