//! #3149 pins — the `unchecked_use_of_nullable_value` receiver faces: the
//! `?..` cascade target's read re-spells to the promoted face, and the
//! `!` operand keeps its declared face.
//!
//! Upstream faces pinned here.
//!
//! 1. `x?..m()..n()` — upstream `cascadeExpression_afterTarget` opens one
//!    `_nullAwareAccess_rightBegin` for the whole cascade
//!    (flow_analysis.dart:6561-6603), so every section's receiver verdict
//!    re-derives through the target's promoted read: the `ReadType` row
//!    at `x`'s token re-spells `A`, never `A?`, for the whole cascade
//!    extent — `a..m()..n()` on a nullable `a` reports at every section,
//!    `a?..m()..n()` reports none. The `?..`'s own
//!    `unnecessary_null_aware` check reads the promoted-face channel
//!    (`PromotedRead`), which the re-spell does not touch — only the
//!    flow-type row moves.
//! 2. `x!` — the operand's own `ReadType` row keeps the DECLARED face:
//!    upstream resolves the `x` identifier before the `!` applies
//!    (`nonNullAssert_end`'s `tryMarkNonNullable` marks the reference for
//!    SUBSEQUENT reads, flow_analysis.dart:7384-7404 — the operand's own
//!    `staticType` stays `T?`). The for-in/spread/yield* operand windows
//!    already skip a `!`-postfixed operand (the read's span ends before
//!    the `!` token), so no consumer needs the row re-spelled — and a
//!    `PromotedRead` mirror misreports the very `!` as
//!    `unnecessary_non_null_assertion` on a genuinely-nullable operand.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// All facts of `source`, parsed against a shared flow recorder.
fn facts_of(source: &str) -> Vec<FlowFact> {
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
    flow.0.borrow_mut().take_facts()
}

/// `(offset, ty)` pairs of `ReadType` facts.
fn read_types(source: &str) -> Vec<(u32, crate::ty::Ty)> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect()
}

/// Offsets of `PromotedRead` facts.
fn promoted_read_offsets(source: &str) -> Vec<u32> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, .. } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// A `?..` cascade head over a nullable local: the target's `ReadType`
/// row re-spells the promoted face for the whole cascade extent, so each
/// section's receiver verdict sees a non-nullable `A` — and no section
/// reports the oracle's `unchecked_use_of_nullable_value`.
#[test]
fn null_aware_cascade_target_respells_promoted() {
    let source = "class A { void m() {} void n() {} } void f(A? a) { a?..m()..n(); }";
    let reads = read_types(source);
    let a = source.find("a?..").unwrap() as u32;
    let (_, ty) = reads
        .iter()
        .find(|(offset, _)| *offset == a)
        .unwrap_or_else(|| panic!("no ReadType at the `a?..` target: {reads:?}"));
    assert!(
        !ty.nullable,
        "the `a?..` target's ReadType must drop the `?`: {reads:?}"
    );
}

/// The `?..` target keeps its declared face on the promotion-mirror side:
/// a `PromotedRead` at the target would misreport the `?..` itself as
/// unnecessary (upstream `_checkForUnnecessaryNullAware` reads the
/// declared face, which stays `A?` outside the short).
#[test]
fn null_aware_cascade_target_emits_no_promoted_read() {
    let source = "class A { void m() {} void n() {} } void f(A? a) { a?..m()..n(); }";
    let reads = promoted_read_offsets(source);
    let a = source.find("a?..").unwrap() as u32;
    assert!(
        !reads.contains(&a),
        "the `a?..` target must carry no PromotedRead: {reads:?}"
    );
}

/// A plain `..` cascade over a nullable local re-spells nothing: every
/// section's receiver reads the declared `A?`, so the member verdicts
/// report the oracle's `unchecked_use_of_nullable_value` at each section.
#[test]
fn plain_cascade_target_stays_nullable() {
    let source = "class A { void m() {} void n() {} } void f(A? a) { a..m()..n(); }";
    let reads = read_types(source);
    let a = source.find("a..").unwrap() as u32;
    let (_, ty) = reads
        .iter()
        .find(|(offset, _)| *offset == a)
        .unwrap_or_else(|| panic!("no ReadType at the `a..` target: {reads:?}"));
    assert!(
        ty.nullable,
        "the `a..` target's ReadType must keep the `?`: {reads:?}"
    );
}

/// The `!` operand's own `ReadType` row keeps the declared face: upstream
/// resolves the operand identifier before the `!` applies, so `x!`'s `x`
/// reads `int?` — the asserted `int` belongs to the postfix expression's
/// type and to the operand's SUBSEQUENT reads, never to its own row.
#[test]
fn bang_operand_read_keeps_declared_face() {
    let source = "void f(int? x) { x!.abs(); }";
    let reads = read_types(source);
    let x = source.find("x!").unwrap() as u32;
    let (_, ty) = reads
        .iter()
        .find(|(offset, _)| *offset == x)
        .unwrap_or_else(|| panic!("no ReadType at the `x!` operand: {reads:?}"));
    assert!(
        ty.nullable,
        "the `x!` operand's own ReadType must keep the `?`: {reads:?}"
    );
}

/// The `!` never emits a `PromotedRead` at its operand: upstream's
/// `unnecessary_non_null_assertion` reads the operand's declared face,
/// which stays nullable — the assertion is not a standing promotion the
/// verifier re-reads. A `PromotedRead` row here reports the very `!` as
/// unnecessary on a genuinely-nullable operand.
#[test]
fn bang_operand_emits_no_promoted_read() {
    let source = "void f(int? x) { x!.abs(); }";
    let reads = promoted_read_offsets(source);
    let x = source.find("x!").unwrap() as u32;
    assert!(
        !reads.contains(&x),
        "the `x!` operand must carry no PromotedRead: {reads:?}"
    );
}

/// The `!`'s promotion still stands for SUBSEQUENT reads: `x!.abs();
/// x.isEven` promotes `x`, so the later `x` read carries `int` — the
/// oracle's own `tryMarkNonNullable` face.
#[test]
fn bang_promotes_subsequent_reads() {
    let source = "void f(int? x) { x!.abs(); x.isEven; }";
    let reads = read_types(source);
    let last_x = source.rfind("x.").unwrap() as u32;
    let (_, ty) = reads
        .iter()
        .find(|(offset, _)| *offset == last_x)
        .unwrap_or_else(|| panic!("no ReadType at the later `x` read: {reads:?}"));
    assert!(
        !ty.nullable,
        "the `x!` promotion must stand for later reads: {reads:?}"
    );
}
