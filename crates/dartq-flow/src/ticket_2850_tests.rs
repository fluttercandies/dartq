//! The #2850 pins: switch-member unreachability driven by pattern
//! exhaustiveness (official `switchStatement_endAlternative`'s
//! `_popPattern` chain write — an alternative whose pattern provably
//! always matches leaves the switch's unmatched chain dead, so every
//! later member, arm, and the post-switch state read dead).
//!
//! Row faces covered: the statement member's keyword row, the
//! switch-expression arm's whole-range row (`flowEnd(case_)`), and the
//! logical-or operand's `[operator, right end)` row
//! (`LogicalOrPattern.resolvePattern`). May-fail faces (constants,
//! null-checks, non-covered object heads, non-empty maps, guards) keep
//! the chain alive.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text (the production channel:
/// `fold_unit_with_ops` reads `features.sound_flow_analysis` into
/// `Flow::set_sound_flow`).
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
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn or_operand_row_after_always_matching_left() {
    // `int()` covers `int x`: the right operand parses under the left
    // branch's failure state (unreachable) and its row spans
    // `[||, right end)` (DeadCodeTest.test_deadPattern_switchStatement_logicalOrPattern).
    let src = "void f(int x) {\n  switch (x) {\n    case int() || 0:\n      break;\n  }\n}";
    assert_eq!(dead_of(src), vec!["|| 0".to_string()]);
}

#[test]
fn duplicate_or_alternative_dies() {
    // The second `int()` is a duplicate of the first: same row shape.
    let src = "void f(int x) {\n  switch (x) {\n    case int() || int():\n      break;\n  }\n}";
    assert_eq!(dead_of(src), vec!["|| int()".to_string()]);
}

#[test]
fn may_failing_left_keeps_right_operand() {
    // A constant left operand can fail: the right operand walks live and
    // reports nothing (the constant itself joins the chain — no member
    // rows either).
    let src = "void f(int x) {\n  switch (x) {\n    case 1 || 2:\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn switch_expression_arm_rows_after_cover() {
    // Arm 1's `|| 0` row, then the dead chain kills arms 2 and 3 — each
    // reports its whole-range row (`flowEnd(case_)` spans the arm, trimmed
    // to the separator) (DeadCodeTest.test_deadPattern_switchExpression_logicalOrPattern_nextCases).
    let src = "Object f(int x) {\n  return switch (x) {\n    int() || 0 => 0,\n    int() => 1,\n    _ => 2,\n  };\n}";
    assert_eq!(
        dead_of(src),
        vec![
            "|| 0".to_string(),
            "int() => 1".to_string(),
            "_ => 2".to_string()
        ]
    );
}

#[test]
fn default_after_total_binder_dies_at_keyword() {
    // `case var v2:` always matches: the chain dies and the default
    // member's keyword row fires (null_check_A01_t01's face — the dead
    // body statement rows through the existing member machinery).
    let src = "void test(String? s) {\n  switch (s) {\n    case var v1?:\n      break;\n    case var v2:\n      break;\n    default:\n      break;\n  }\n}";
    let dead = dead_of(src);
    assert!(dead.contains(&"default".to_string()), "{dead:?}");
}

#[test]
fn non_exhaustive_switch_keeps_default_live() {
    // A constant pattern can fail: the chain stays alive.
    let src = "void f(int x) {\n  switch (x) {\n    case 1:\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn non_covered_object_head_keeps_default_live() {
    // `Foo()` over `Object o` may fail on the type test.
    let src = "class Foo {}\nvoid f(Object o) {\n  switch (o) {\n    case Foo():\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn null_check_head_keeps_default_live() {
    // `var v?` over a nullable scrutinee may fail on the null path.
    let src = "void f(String? s) {\n  switch (s) {\n    case var v?:\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn covered_object_head_kills_default() {
    // `Foo()` over `Foo foo` covers: the chain dies and the default's
    // keyword reports.
    let src = "class Foo {}\nvoid f(Foo foo) {\n  switch (foo) {\n    case Foo():\n      break;\n    default:\n      break;\n  }\n}";
    let dead = dead_of(src);
    assert!(dead.contains(&"default".to_string()), "{dead:?}");
}

#[test]
fn empty_map_head_kills_default() {
    // `{}` over a `Map`-typed scrutinee cannot fail under sound flow
    // (`matchMayFailEvenIfCorrectType` latches false at zero entries).
    let src = "void f(Map m) {\n  switch (m) {\n    case {}:\n      break;\n    default:\n      break;\n  }\n}";
    let dead = dead_of(src);
    assert!(dead.contains(&"default".to_string()), "{dead:?}");
}

#[test]
fn non_empty_map_head_keeps_default_live() {
    // Entries can fail to match even when the type test passes.
    let src = "void f(Map m) {\n  switch (m) {\n    case {'k': 1}:\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn guard_keeps_default_live() {
    // A guard's false arm revives the chain (official `_popPattern`'s
    // guardInfo join).
    let src = "void f(int x, bool c) {\n  switch (x) {\n    case var v when c:\n      break;\n    default:\n      break;\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn dead_entry_guard_cannot_revive() {
    // A guard splits the arm's ENTRY state: entering dead, both of its
    // arms are dead, so the guard-false join stays dead and the wildcard
    // arm reads dead too (oracle-checked: dart analyze 3.13.5 reports
    // both whole-arm rows, trimmed to the separator).
    let src = "Object f(int x, bool c) {\n  return switch (x) {\n    int() => 0,\n    int y when c => 1,\n    _ => 2,\n  };\n}";
    assert_eq!(
        dead_of(src),
        vec!["int y when c => 1".to_string(), "_ => 2".to_string()]
    );
}

#[test]
fn dead_offsets_are_reusable() {
    // Keeps `dead_offsets` linked for future oracle triage.
    let src = "void f(int x) {\n  switch (x) {\n    case int() || 0:\n      break;\n  }\n}";
    let offsets = dead_offsets(src);
    assert_eq!(offsets, vec![(46, 4)]);
}

fn dead_offsets(source: &str) -> Vec<(u32, u32)> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
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
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

#[test]
fn or_row_in_if_case_operand() {
    // The same operand face in a standalone if-case context: a covering
    // left wildcard walks `0` dead (the declaration-context rows of
    // record_A04_t02 / list_A05_t01 share the face).
    let src = "void f(int r) {\n  if (r case int _ || 0) {\n    r.toString();\n  }\n}";
    assert_eq!(dead_of(src), vec!["|| 0".to_string()]);
}

// —— S2 supplemental faces (the for-element close, the assigned-bool
// condition variable, the dead branch's empty block) ——————————————————

#[test]
fn dead_for_element_anchors_at_updaters() {
    // A constant-false C-for condition kills the iteration before it
    // begins. The updaters precede the element in source order, so they
    // are the first dead nodes and their own official row spans them
    // (`for_updaterBegin`, dead_code_verifier.dart:357-375). The dead
    // ELEMENT gets its own row too (first dead node after the fold;
    // #3042's spanned-literal fix): its covering close is the enclosing
    // block's last statement end — here the var-decl's own `;`.
    // Oracle-probed on 3.13.7 (machine format): rows at `i++` (3 chars)
    // and `1];`.
    let src = "void f() {\n  var x = [for (var i = 0; false; i++) 1];\n}";
    assert_eq!(dead_of(src), vec!["i++".to_string(), "1];".to_string()]);
}

#[test]
fn live_for_element_stays_silent() {
    // A live condition's element body is unreachable only through the
    // element's own exit: no fold, no poll, no row.
    let src = "void f() {\n  var x = [for (var i = 0; i < 3; i++) 1];\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn assigned_bool_condition_variable_splits_reads() {
    // The write-side capture (language#1274): a plain assignment whose
    // RHS is the null-comparison split binds its arms to the written
    // local exactly like the initializer face, so the replay folds a
    // subsequent `a == null` consequent dead (nnbd
    // local_variable_read_A02/A05's `b = a != null;` face).
    let src = "void f(int? a) {\n  bool b;\n  b = a != null;\n  if (b) {\n    if (a == null) {\n      print('dead');\n    }\n  }\n}";
    assert_eq!(
        dead_of(src),
        vec!["{\n      print('dead');\n    }".to_string()]
    );
}

#[test]
fn plain_rewrite_kills_condition_variable() {
    // A plain non-split write leaves the written variable's new version
    // stateless: the stale entry must not replay through it.
    let src = "void f(int? a, bool c) {\n  bool b = a != null;\n  b = c;\n  if (b) {\n    if (a == null) {\n      print('live');\n    }\n  }\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn condition_variable_chain_copies_through_write() {
    // `c = b` chain-copies b's entry (official `variableRead`'s restore
    // feeding the write's captured state).
    let src = "void f(int? a) {\n  bool b = a != null;\n  bool c;\n  c = b;\n  if (c) {\n    if (a == null) {\n      print('dead');\n    }\n  }\n}";
    assert_eq!(
        dead_of(src),
        vec!["{\n      print('dead');\n    }".to_string()]
    );
}

#[test]
fn dead_if_case_empty_then_block_reports_braces() {
    // `if (x case null) {}` over a non-nullable x: the constant
    // pattern's disjoint fold kills the then branch (an if-case context
    // has no semantic switch tracker behind it), the empty block is
    // itself the polled node (official `visitBlock`), and the branch's
    // own close (`flowEnd(ifTrue)`) ends the row at the braces.
    let src = "void f(int x) {\n  if (x case null) {}\n}";
    assert_eq!(dead_of(src), vec!["{}".to_string()]);
}

#[test]
fn dead_constant_if_empty_then_block_reports_braces() {
    // The same face with a constant-false condition: `if (false) {}`
    // reports the empty then-block (dead_code_test.dart 3:13|2).
    let src = "void f() {\n  if (false) {}\n}";
    assert_eq!(dead_of(src), vec!["{}".to_string()]);
}

#[test]
fn same_type_is_negated_branch_stays_silent() {
    // A same-type `is!` is a promotion no-op, not a fold: the then
    // branch stays live and its empty block reports nothing.
    let src = "void f(int x) {\n  if (x is! int) {}\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}
