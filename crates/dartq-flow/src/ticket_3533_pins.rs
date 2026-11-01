//! The #3533 pins: an `is` test inside call arguments is consumed as a
//! plain value — its split rejoins at the argument boundary, and a write
//! whose right-hand face the lattice cannot spell demotes a spelled
//! promotion.
//!
//! Oracle (`dart analyze`, the `inheritance_chain_test` family):
//!
//! ```dart
//! var o = instance;                    // `instance` types `o` to the
//!                                      // common supertype (name level:
//!                                      // untypeable -> `var` local).
//! Expect.isTrue(o is A);               // the `is` narrows `o` only for a
//! Expect.isTrue(o is B);               // CONDITION consumer; consumed as
//! ...                                  // an argument VALUE, the official
//! ```
//! leaves `_current` recombined — every later `o is X` is a live test, not
//! an always-true fold. Before the rejoin, the argument's narrowed arm
//! leaked into the continuation and each successive check reported
//! `unnecessary_type_check`.
//!
//! The write half:
//!
//! ```dart
//! if (o is String) {
//!   o = list[1];     // `List<dynamic>` element read — a typed value the
//!   o.m();           // name level cannot spell (`Unknown`). The official's
//! }                  // demotion answers `dynamic <: String` false.
//! ```
//! A bare `Unknown` write covers only an `Unknown` entry — the
//! InvalidType-recovery write (`x ??= unresolvable()`) keeps its chain
//! through the resolved seam's spelled invalid face, never through the
//! opaque marker.

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

/// The promoted type carried by the `needle`-anchored read, when that read
/// promoted (a read inside a narrowed arm reports `PromotedRead`; the same
/// read after the split rejoined reports nothing).
fn promoted_at(source: &str, needle: &str) -> Option<crate::ty::Ty> {
    let offset = source.find(needle).unwrap() as u32;
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } if span.range.offset == offset => Some(ty),
            _ => None,
        })
        .next()
}

/// Positional argument: the `is` inside `f(o is int)` narrows `o` for a
/// condition consumer only — the argument is a plain VALUE, so the read
/// after the call sees the recombined state and promotes to nothing.
///
/// Fails if [`Ev::ArgumentEnd`] drops the rejoin (the narrowed arm leaks
/// past the list and the `o;` read promotes).
#[test]
fn positional_argument_is_test_does_not_leak_past_the_call() {
    let source = "void f(Object o) {\n  take(o is int);\n  o;\n}";
    assert_eq!(promoted_at(source, "o;"), None);
}

/// Named argument: the same rejoin at a named argument's `ArgumentEnd` —
/// the label does not change the argument's value consumption.
#[test]
fn named_argument_is_test_does_not_leak_past_the_call() {
    let source = "void f(Object o) {\n  take(v: o is int);\n  o;\n}";
    assert_eq!(promoted_at(source, "o;"), None);
}

/// `!(...)` argument: the prefix-`!` result carries the operand's own split
/// (inverted), so the argument boundary rejoins it exactly like the bare
/// test — the read after the call sees no promotion in EITHER direction
/// (the `!` arms are not a standing promotion).
#[test]
fn negated_argument_is_test_does_not_leak_past_the_call() {
    let source = "void f(Object o) {\n  take(!(o is int));\n  o;\n}";
    assert_eq!(promoted_at(source, "o;"), None);
}

/// The legit half: a promotion held BEFORE the call survives it — the
/// rejoin only recomputes the argument's own split, it does not retract a
/// standing promotion the then-arm already carries.
///
/// Fails if the rejoin is allowed to kill a pre-existing promotion (the
/// then-arm read after the call must still report `PromotedRead`).
#[test]
fn pre_call_promotion_survives_a_rejoined_argument() {
    let source = "void f(Object o) {\n  if (o is String) {\n    take(o is int);\n    o;\n  }\n}";
    assert!(promoted_at(source, "o;").is_some());
}

/// The demoted half (D2): a write whose right-hand face is the opaque
/// `Unknown` marker (a `List<dynamic>` element read — `dynamic`, a typed
/// value the name level cannot spell) demotes a spelled promotion, exactly
/// like the official's `isSubtypeOf(writtenType, promoted)` answers over
/// the real `dynamic <: String` verdict.
///
/// Fails if `covers()` keeps resurrecting the `String` entry through its
/// `written.base == Unknown` arm.
#[test]
fn unspellable_write_demotes_the_standing_promotion() {
    let source =
        "void f(Object o, List<dynamic> l) {\n  if (o is String) {\n    o = l[1];\n    o;\n  }\n}";
    assert_eq!(promoted_at(source, "o;"), None);
}
