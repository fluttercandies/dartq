//! #3009 — dead-code pins for the C-style `for` header's updater/condition
//! regions, mirroring the official verifier's two mechanisms
//! (`upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart`):
//!
//! * `for_updaterBegin` (dead_code_verifier.dart:357-371): when the loop
//!   body ends unreachable and NO dead interval was open at condition
//!   end, the updaters region reports its own row from the first
//!   updater's begin token through the last updater's `node.end` — ONE
//!   row spanning the whole `updaters` list (`for (;; 1, 2)` → `1, 2`),
//!   independent of the interval walk (the official emits it directly,
//!   `_firstDeadNode` untouched).
//! * The `parent is ForParts` trim (dead_code_verifier.dart:309-310): a
//!   dead interval whose first dead node IS a header node (the condition
//!   or an updater) ends at the last updater's `node.end` — the dead
//!   body's interval never folds the header row into its own.
//!
//! Oracle: `dart analyze` 3.13.7 over
//! `dead_code_test.dart`'s `test_flowEnd_forParts_*` /
//! `test_flowEnd_forElementParts_*` families.

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

// —— dead updaters: the whole-region row ————————————————————————————————

#[test]
fn updaters_binary_reports_whole_region() {
    // `for (var i = 0;; i + 1) { return; }` — the body's `return` ends
    // the back edge dead; the updaters row covers `i + 1` whole
    // (test_flowEnd_forParts_updaters_binaryExpression).
    let src = "void f() {\n  for (var i = 0;; i + 1) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["i + 1".to_string()]);
}

#[test]
fn updaters_multiple_reports_whole_region() {
    // `for (;; 1, 2)` — one row `1, 2`, begin of first through end of
    // last (test_flowEnd_block_forStatement_updaters_multiple).
    let src = "void f() {\n  for (;; 1, 2) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["1, 2".to_string()]);
}

#[test]
fn updaters_assignment_reports_whole_region() {
    let src = "void f() {\n  for (var i = 0;; i = i + 1) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["i = i + 1".to_string()]);
}

#[test]
fn updaters_conditional_reports_whole_region() {
    // A conditional updater reports its whole extent — not per-arm
    // fragments (test_flowEnd_forParts_updaters_conditionalExpression).
    let src = "void f() {\n  for (var i = 0;; i > 1 ? i : i) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["i > 1 ? i : i".to_string()]);
}

#[test]
fn updaters_prefix_reports_whole_region() {
    // `++i` — the row begins at the operator (the updater's beginToken).
    let src = "void f() {\n  for (var i = 0;; ++i) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["++i".to_string()]);
}

#[test]
fn updaters_postfix_reports_whole_region() {
    let src = "void f() {\n  for (var i = 0;; i++) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["i++".to_string()]);
}

#[test]
fn updaters_method_invocation_reports_whole_region() {
    let src = "void f() {\n  for (var i = 0;; i.toString()) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["i.toString()".to_string()]);
}

#[test]
fn updaters_prefixed_identifier_reports_whole_region() {
    let src =
        "import 'dart:math' as m;\n\nvoid f() {\n  for (;; m.Point) {\n    return;\n  }\n}";
    assert_eq!(dead_of(src), vec!["m.Point".to_string()]);
}

#[test]
fn updaters_dead_then_body_dead_stmt_reports_own_row() {
    // `for (;; 1) { return; 2; }` — the updaters row `1` and the body's
    // dead statement `2;` are TWO independent rows
    // (test_flowEnd_block_forStatement_updaters).
    let src = "void f() {\n  for (;; 1) {\n    return;\n    2;\n  }\n}";
    assert_eq!(dead_of(src), vec!["1".to_string(), "2;".to_string()]);
}

#[test]
fn updaters_dead_nested_body_stmt_reports_own_row() {
    // The nested block's dead statement folds into the covering close's
    // trim — oracle `6:7|8` is `2;` through the inner block's `}`
    // (test_flowEnd_nestedBlock_forStatement_updaters).
    let src = "void f() {\n  for (;; 1) {\n    {\n      return;\n      2;\n    }\n  }\n}";
    assert_eq!(dead_of(src), vec!["1".to_string(), "2;\n    }".to_string()]);
}

#[test]
fn updaters_live_loop_is_silent() {
    // The loop body CAN complete — the updaters are live.
    let src = "void f() {\n  for (var i = 0; i < 3; i + 1) {\n    i = i + 1;\n  }\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

// —— dead interval anchored in the header ———————————————————————————————

#[test]
fn dead_after_throw_condition_reports_updater_and_body() {
    // `for (; throw 0; 1) {}` — the condition's `throw` kills the walk:
    // `1` is the first dead node (an updater — `parent is ForParts`,
    // dead_code_verifier.dart:309-310 → `node = updaters.last` = `1`),
    // and the body's own dead poll reports `{}` separately
    // (test_flowEnd_forParts_condition_exists: rows `1` and `{}`).
    let src = "void f() {\n  for (; throw 0; 1) {}\n}";
    assert_eq!(dead_of(src), vec!["1".to_string(), "{}".to_string()]);
}

#[test]
fn updater_dies_mid_list_reports_last_updater_only() {
    // `for (;; 0, throw 1, 2) {}` — updaters began live; `throw 1` kills
    // the walk mid-list, so `2` is the first dead node — `parent is
    // ForParts` ends the row at `updaters.last` = `2` itself
    // (dead_code_verifier.dart:309-310; test_flowEnd_forParts_updaters_
    // throw).
    let src = "void f() {\n  for (;; 0, throw 1, 2) {}\n}";
    assert_eq!(dead_of(src), vec!["2".to_string()]);
}

// —— collection for-elements share the face —————————————————————————————

#[test]
fn for_element_updaters_reports_whole_region() {
    // `[for (;; 1, 2) ...[throw '']]` — same updater row for the element
    // form (test_flowEnd_list_forElement_updaters_multiple).
    let src = "f() => [for (;; 1, 2) ...[throw '']];";
    assert_eq!(dead_of(src), vec!["1, 2".to_string()]);
}

// Residual (#3009): `[for (;; 1) ...[throw '', 2]]` also reports the dead
// `2` element through the covering close (`2]];`) — the poll needs an
// element-boundary event the record stream does not carry yet; a leaf
// literal poll over-fires inside dead for-element BODIES (upstream polls
// `checkUnreachableNode` only through `analyzeExpression`, which element
// bodies do not run).
