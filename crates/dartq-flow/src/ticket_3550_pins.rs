//! The #3550 pins: a case pattern spelling an extension type's
//! representation type covers the extension-type scrutinee.
//!
//! Oracle (`dart analyze`, the extension-type exhaustiveness family):
//!
//! ```dart
//! class B {}
//! extension type BET1(B it) {}
//! String test1(BET1 b) {
//!   switch (b) {
//!     case B _: return 'B'; // covers: BET1's erasure IS B, B <: B.
//!   }
//! }
//! ```
//! stays silent — no `body_might_complete_normally`. `promoteForPattern`
//! answers its cover question over the ERASED faces
//! (`coversMatchedType = isSubtypeOf(extensionTypeErasure(matchedType),
//! extensionTypeErasure(knownType))`, flow_analysis.dart:6394-6397): the
//! case cannot fail, the unmatched chain keeps its dead seed, the
//! switch's implicit fall-through arm never joins the break model, and
//! the body's end stays unreachable. Before the erasure the name-level
//! verdict answered `BET1 <: B` false, the failure arm rejoined the
//! chain, and every such body over-reported the completion row (~20
//! census rows). An extension type is a subtype of its representation
//! (subtype.dart's declared-interfaces arm, :107-114); the erasure
//! applies the type's own nullability on top of the rep (`E?` over `R`
//! erases to `R?`, type_system.dart:42-63), so a nullable scrutinee
//! stays uncovered by a non-nullable head.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

fn scan_parse(source: &str) -> Vec<FlowFact> {
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
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// The `body_might_complete_normally` family's pure reachability face:
/// offsets of every [`FlowFact::BodyMayCompleteNormally`] row.
fn completion_offsets(source: &str) -> Vec<u32> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::BodyMayCompleteNormally { span } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// The dead-code family: offsets of every [`FlowFact::DeadCode`] row.
fn dead_offsets(source: &str) -> Vec<u32> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// The mission fixture: `case B _` covers a `BET1` scrutinee whose
/// representation is `B`, so the returning case leaves no live
/// fall-through arm and neither flow family reports.
///
/// Fails if the cover question misses the erasure (the failure arm
/// rejoins the unmatched chain, the phantom fall-through joins the
/// break model, and the body's end reports completion).
#[test]
fn representation_typed_case_covers_extension_type_scrutinee() {
    let source = concat!(
        "class B {} ",
        "extension type BET1(B it) {} ",
        "String test1(BET1 b) { switch (b) { case B _: return 'B'; } }"
    );
    assert!(completion_offsets(source).is_empty());
    assert!(dead_offsets(source).is_empty());
}

/// The declared-variable face of the same cover (`case B bb`): the same
/// erased verdict, the same silence.
#[test]
fn representation_typed_declared_variable_covers_the_scrutinee() {
    let source = concat!(
        "class B {} ",
        "extension type BET1(B it) {} ",
        "int test6(BET1 b) { switch (b) { case B bb: return 0; } }"
    );
    assert!(completion_offsets(source).is_empty());
    assert!(dead_offsets(source).is_empty());
}

/// The object-pattern face of the same cover (`case B(:var h)`): the
/// head's required type erases the scrutinee to `B` and covers.
#[test]
fn representation_typed_object_head_covers_the_scrutinee() {
    let source = concat!(
        "class B {} ",
        "extension type BET1(B it) {} ",
        "int test7(BET1 b) { switch (b) { case B(:var h): return 0; } }"
    );
    assert!(completion_offsets(source).is_empty());
    assert!(dead_offsets(source).is_empty());
}

/// The chain: an extension type repped by another extension type erases
/// through both reps (`BET3` → `BET1` → `B`), so `case B _` covers a
/// `BET3` scrutinee too.
#[test]
fn chained_extension_types_erase_through_the_rep_chain() {
    let source = concat!(
        "class B {} ",
        "extension type BET1(B it) {} ",
        "extension type BET3(BET1 it) {} ",
        "String test4(BET3 b) { switch (b) { case B _: return 'B'; } }"
    );
    assert!(completion_offsets(source).is_empty());
    assert!(dead_offsets(source).is_empty());
}

/// The non-extension control, unchanged: an exact-type case over a plain
/// class scrutinee covers at the name level today and still does — the
/// erasure is a no-op for heads the unit captured nothing for.
#[test]
fn plain_class_exact_type_cover_is_unchanged() {
    let source = concat!(
        "class B2 {} ",
        "int test3(B2 b) { switch (b) { case B2 _: return 0; } }"
    );
    assert!(completion_offsets(source).is_empty());
    assert!(dead_offsets(source).is_empty());
}

/// The over-claim guard: a representation that is a SUPERTYPE of the
/// case's type does not cover (`BET2`'s rep is the abstract `A`, and `A
/// <: B` is false) — the case can fail on the `C` half, the failure arm
/// rejoins, and the completion row still fires exactly as before the
/// erasure existed.
#[test]
fn rep_supertype_case_still_reports_completion() {
    let source = concat!(
        "abstract class A {} class B extends A {} class C extends A {} ",
        "extension type BET2(A it) {} ",
        "String test2(BET2 b) { switch (b) { case B _: return 'B'; } }"
    );
    assert_eq!(completion_offsets(source).len(), 1);
    assert!(dead_offsets(source).is_empty());
}

/// Nullability rides the erasure: `BET1?` erases to `B?`, and `B? <: B`
/// is false — a non-nullable head does not cover a nullable extension
/// scrutinee, so the row stays.
#[test]
fn nullable_scrutinee_stays_uncovered_by_non_null_head() {
    let source = concat!(
        "class B {} ",
        "extension type BET1(B it) {} ",
        "String test5(BET1? b) { switch (b) { case B _: return 'B'; } }"
    );
    assert_eq!(completion_offsets(source).len(), 1);
}

/// A record-typed representation spells no named head — the field rows
/// ride under the record guard and the record's own row bypasses the
/// head latch — so nothing is captured and `case int _` over `R` keeps
/// the may-fail join (upstream's erasure lands on the record type, which
/// subtypes no named head either).
#[test]
fn record_typed_representation_is_not_captured() {
    let source = concat!(
        "extension type R((int, int) pair) {} ",
        "int test8(R r) { switch (r) { case int _: return 0; } }"
    );
    assert_eq!(completion_offsets(source).len(), 1);
}

/// A function-typed representation's return row fires BEFORE
/// `begin_function_type`, so capturing it would type the rep as the bare
/// return type and over-claim (`case int _` would cover). The token walk
/// past the chain head (`int Function(int)`) disarms the capture; the
/// may-fail join stays, exactly like upstream's erased
/// `int Function(int)` face against the `int` head.
#[test]
fn function_typed_representation_is_not_captured() {
    let source = concat!(
        "extension type F1(int Function(int) f) {} ",
        "int test9(F1 f) { switch (f) { case int _: return 0; } }"
    );
    assert_eq!(completion_offsets(source).len(), 1);
}
