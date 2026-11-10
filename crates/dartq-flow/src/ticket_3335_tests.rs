//! The #3335 pins: the empty-branch poll and the initializer-position
//! `??` row.
//!
//! Two faces the #3299 lane's recon found:
//!
//! * A statement block is a polled node (official `visitBlock`'s
//!   `checkUnreachableNode`) with no empty-block exemption, so an if
//!   branch the flow model reached dead reports its braces even when no
//!   inner statement polls. A short-circuit condition (`if (x && false) {}`)
//!   kills the then arm through the `&&`/`||` split, which carries no
//!   constant — the block poll used to wait for a fold marker and never
//!   opened the window.
//! * An implicitly typed local's slot stays the opaque unknown face (the
//!   #2243 guard), so the guaranteed-non-null verdict behind a `??` right
//!   operand never saw `final x = 0;` as non-null: `final y = x ?? 1;`
//!   reported nothing where the oracle rows `?? 1`.

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
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

// —— the empty dead branch (H1) ————————————————————————————————————————

#[test]
fn short_circuit_false_lhs_reports_the_operand_and_the_braces() {
    // `false && x` — oracle rows `&& x` (the dead right operand, anchored
    // at the operator by the binary's own `flowEnd`) and `{}` (the then
    // arm is unreachable; the block is the next first dead node after the
    // binary's close reset it).
    let src = "void f(bool x) {\n  if (false && x) {}\n}";
    assert_eq!(
        dead_of(src),
        vec!["&& x".to_string(), "{}".to_string()],
        "{:?}",
        dead_of(src)
    );
}

#[test]
fn short_circuit_false_rhs_reports_the_braces() {
    // `x && false` — the left operand runs live (no operand row), the
    // whole condition is provably false, so the then arm is the dead one
    // and its braces report (oracle 1 row).
    let src = "void f(bool x) {\n  if (x && false) {}\n}";
    assert_eq!(dead_of(src), vec!["{}".to_string()], "{:?}", dead_of(src));
}

#[test]
fn non_empty_short_circuit_dead_branch_keeps_its_single_row() {
    // Regression surface: a non-empty dead branch polled its block before
    // this change and still reports exactly one row, the braces included
    // (`g();` folds into the block-anchored interval).
    let src = "void f(bool x) {\n  if (x && false) {\n    g();\n  }\n}";
    let rows = dead_of(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].starts_with('{'), "{rows:?}");
    assert!(rows[0].contains("g();"), "{rows:?}");
    assert!(rows[0].ends_with('}'), "{rows:?}");
}

#[test]
fn live_short_circuit_branch_stays_silent() {
    // The poll's own reachability gate: `x && y` leaves both arms live, so
    // neither the empty block nor the operands report.
    let src = "void f(bool x, bool y) {\n  if (x && y) {}\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

// —— the `??` right operand over an inferred non-nullable local (H2) ————

#[test]
fn if_null_rhs_dies_over_an_inferred_non_nullable_initializer() {
    // `final x = 0;` — the inferred slot is non-nullable int, so the
    // right operand of `x ?? 1` can never run (official
    // `ifNullExpression_rightBegin`'s guaranteed-non-null kill).
    let src = "void f() {\n  final x = 0;\n  final y = x ?? 1;\n}";
    assert_eq!(dead_of(src), vec!["?? 1".to_string()], "{:?}", dead_of(src));
}

#[test]
fn if_null_rhs_over_a_written_annotation_is_unchanged() {
    // Regression surface: a written non-nullable annotation already
    // killed the operand from the slot's own classification.
    let src = "void f(int v) {\n  var z = v ?? 1;\n}";
    assert_eq!(dead_of(src), vec!["?? 1".to_string()], "{:?}", dead_of(src));
}

#[test]
fn if_null_rhs_stays_live_over_a_nullable_local() {
    // The flag must not over-reach: a nullable initializer keeps the
    // right operand live.
    let src = "void f(int? n) {\n  final x = n;\n  final y = x ?? 1;\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn if_null_rhs_stays_live_after_a_write_to_the_local() {
    // A write ends the by-construction face: the value read below may be
    // nullable again.
    let src = "void f(int? n) {\n  var x = 0;\n  x = n;\n  final y = x ?? 1;\n}";
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}
