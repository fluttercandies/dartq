//! The C3 pins: top-level named `const`/`final` conditions stay silent.
//!
//! Oracle contract (dead_code_test.dart's `debugConst` family, analyzer
//! 3.13.7): the dead-code verifier's constant-condition fold fires on the
//! literal node alone (`booleanLiteral`, resolver.dart's literal arm) — a
//! named element's `evaluationResult` never reaches flow analysis, so
//! `const bool DEBUG = false; if (DEBUG) {} else {}` reports NOTHING, and
//! `final` top-levels behave identically. This file previously pinned the
//! opposite assumption (a `const_bool` backfill fed named elements'
//! `bool_value`); #2994 removed that feed after the oracle diff proved it
//! fabricated rows the oracle never emits. The remaining face is purely
//! negative: every shape below asserts silence, and the mutable/`late`
//! exclusions pin the boundary the locals face keeps (`VarMeta::const_bool`
//! is a local-only slot).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text (the production channel:
/// `set_sound_flow` reads the settled feature set).
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

/// `const t = true; if (t) {} else {}` — the oracle emits nothing
/// (dead_code_test.dart `test_deadBlock_else_debugConst`).
#[test]
fn const_top_level_else_stays_silent() {
    assert_eq!(
        dead_of("const t = true; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// `const t = false` keeps the then arm live too.
#[test]
fn const_top_level_then_stays_silent() {
    assert_eq!(
        dead_of("const t = false; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A `final` top-level header folds exactly like `const` — that is, not
/// at all.
#[test]
fn final_top_level_stays_silent() {
    assert_eq!(
        dead_of("final t = true; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// Collection if-elements follow the same silence.
#[test]
fn const_top_level_element_stays_silent() {
    assert_eq!(
        dead_of("const t = true; var x = 1; var l = [if (t) x else x + 10];"),
        Vec::<String>::new()
    );
}

/// Conditional-expression arms: `t ? 1 : 2` over a named const reports
/// nothing (dead_code_test.dart `test_deadBlock_conditionalThen_debugConst`).
#[test]
fn const_top_level_conditional_stays_silent() {
    assert_eq!(
        dead_of("const t = true; f() { var x = t ? 1 : 2; }"),
        Vec::<String>::new()
    );
}

/// A mutable `var` header never folds — its value can change between the
/// initializer and any read.
#[test]
fn var_top_level_keeps_both_arms_live() {
    assert_eq!(
        dead_of("var t = true; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A `late final` header is lazily bound — no compile-time constant, no
/// fold.
#[test]
fn late_final_top_level_keeps_both_arms_live() {
    assert_eq!(
        dead_of("late final t = true; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// An initializer the fold cannot value (`g() == 1`) leaves no constant —
/// the read stays a runtime question either way.
#[test]
fn unevaluated_initializer_keeps_both_arms_live() {
    assert_eq!(
        dead_of("int g() => 1; const t = g() == 1; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// Even a trivially foldable initializer (`!false`) never feeds a named
/// top-level's condition reads.
#[test]
fn folded_initializer_still_stays_silent() {
    assert_eq!(
        dead_of("const t = !false; f() { if (t) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A multi-declarator list changes nothing — no name feeds a later
/// condition.
#[test]
fn multi_declarator_list_stays_silent() {
    assert_eq!(
        dead_of("const a = true, b = false; f() { if (a) {} else {} if (b) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A read before the declaration under-reports exactly like the
/// `top_level` contract documents — the capture is forward-only.
#[test]
fn forward_only_capture() {
    assert_eq!(
        dead_of("f() { if (t) {} else {} } const t = true;"),
        Vec::<String>::new()
    );
}
