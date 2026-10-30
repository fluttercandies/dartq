//! #3042 — dead-code pins for the C-style `for` literal-condition family,
//! the dominant `dead_code` MISS-direction residual (missed dead
//! body/updater rows paired with a wrongly-dead loop tail).
//!
//! Root cause: the bare-boolean-literal condition (`for (; false;) …`)
//! left `Ev::Bool` unspanned, so the header partition's offset scan
//! ([`crate::interp::ForBounds::build`], keyed on spanned events) read
//! the condition region empty and the loop folded as `for (;;)` — the
//! body and updaters ran live (missed rows) and the exit took the dead
//! false-arm (extra tail row). The fix mirrors the `Ev::Int { span }`
//! precedent: the literal's own token is the partition anchor, plus the
//! header-part leaf's dead poll the `Ev::Int` arm already carries.
//!
//! The second face: `replay_c_for` refreshed the began-dead snapshot
//! only before the condition fold, so a NON-block body (`for (; false;)
//! 2;`) anchored nothing — its `ExprStmt` wrapper still read the
//! pre-fold (live) snapshot. The post-fold refresh mirrors the
//! `WhileBodyBegin` non-wrapper-arm refresh in `stmt_step` (official
//! `for_bodyBegin`'s statement boundary).
//!
//! Upstream semantics (Dart SDK source, `pkg/_fe_analyzer_shared/lib/
//! src/flow_analysis/flow_analysis.dart`): a literal-`false` condition
//! folds at `for_conditionEnd`'s `conditionEnd` — the true branch is
//! unreachable from the first body node; the false branch stays live, so
//! the loop's exit join (`for_statementEnd`) keeps the tail reachable.
//! Dead updaters fire their own row (`for_updaterBegin`,
//! dead_code_verifier.dart:357-378).
//!
//! Oracle: `dart analyze` 3.13.7 over the /tmp/dq3042 probe faces
//! (machine format): `for (var i = 0; false; i++) { 1; }` → rows at
//! `i++` and `{ 1; }`; `for (; false;) 2;` → row `2;`; collection
//! `[for (; false;) 3]` → a row anchored at `3`; `for (…; false;) {
//! break; }` → row `{ break; }` with a live tail; `for (…; true;) {
//! break; }` → clean.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
fn dead_of(source: &str) -> Vec<String> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => {
                Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
            }
            _ => None,
        })
        .collect()
}

/// Asserts `source` reports exactly one dead row covering `marker`.
fn one_dead_row(source: &str, marker: &str) {
    let dead = dead_of(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains(marker), "{dead:?}");
}

/// Asserts `source` reports no dead rows at all.
fn no_dead(source: &str) {
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// Asserts `needle` is covered by no dead row (a live tail lock).
fn nowhere_dead(dead: &[String], needle: &str) {
    assert!(
        dead.iter().all(|row| !row.contains(needle)),
        "{needle} flagged: {dead:?}"
    );
}

// —— literal-false condition: dead body + dead updaters, live tail ————

#[test]
fn for_false_init_and_updater_reports_both_rows() {
    // Oracle: rows at `i++` (for_updaterBegin) and `{ 1; }` (the body
    // block); `9;` stays live — the exit join keeps the false branch.
    let dead = dead_of("void f() {\n  for (var i = 0; false; i++) { 1; }\n  9;\n}");
    assert_eq!(dead.len(), 2, "{dead:?}");
    assert!(
        dead.iter().any(|row| row.contains("i++")),
        "{dead:?}"
    );
    assert!(
        dead.iter().any(|row| row.contains("{ 1; }")),
        "{dead:?}"
    );
    nowhere_dead(&dead, "9");
}

#[test]
fn for_false_nonblock_body_anchors_after_fold() {
    // The began-dead refresh the fix adds: a non-block body's ExprStmt
    // wrapper reads the post-fold snapshot (before the fix the condition
    // region read empty, the loop folded `for(;;)`, and `2;` ran live).
    one_dead_row("void f() {\n  for (; false;) 2;\n}", "2");
}

#[test]
fn for_false_tail_stays_live() {
    // The extras-direction lock: the exit takes the LIVE false branch,
    // never a dead one (`3;` was wrongly flagged when the empty condition
    // region folded the loop as infinite).
    one_dead_row("void f() {\n  for (; false;) 2;\n  3;\n}", "2");
}

#[test]
fn for_false_collection_element_dead() {
    // Collection-for: the element `3` is the first dead node; the row
    // folds the following dead content into one interval (oracle len 51
    // over the probe), so pin the anchor, not the extent.
    one_dead_row("void f() {\n  var l = [for (; false;) 3];\n  4;\n}", "3");
}

#[test]
fn for_false_dead_body_break_keeps_tail_live() {
    // No updaters: only the body row (`{ break; }`); the exit joins the
    // live false branch with the (dead) breaks — the tail `9;` stays
    // reachable.
    let dead = dead_of("void f() {\n  for (var i = 0; false;) { break; }\n  9;\n}");
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(
        dead[0].contains("{ break; }"),
        "{dead:?}"
    );
    nowhere_dead(&dead, "9");
}

// —— regression locks: the neighboring shapes stay as they were ——————

#[test]
fn for_true_with_break_stays_clean() {
    // `for (…; true;) { break; }`: both branches behave — no rows at all
    // (the break keeps the exit reachable; `true` never deads anything).
    no_dead("void f() {\n  for (var i = 0; true;) { break; }\n  8;\n}");
}

#[test]
fn for_ident_latch_condition_unchanged() {
    // The C3 const-feed path (Ident-latched `final b = false;`) kept
    // working through the span change: the body row still reports.
    one_dead_row(
        "void f() {\n  final b = false;\n  for (; b;) { 3; }\n}",
        "{ 3; }",
    );
}

#[test]
fn for_empty_condition_stays_infinite() {
    // `for (;;)`: the empty condition is `true` — the body runs live and
    // the tail is dead only through the loop itself (here it never
    // exits, so the tail IS dead via the never-exiting join, not a
    // condition fold): pin the tail row, none on the body.
    one_dead_row("void f() {\n  for (;;) { 1; }\n  9;\n}", "9");
}
