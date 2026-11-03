//! The #3778 pins: a local function declared to return `Never` whose
//! block body falls through must surface the same `BodyMayCompleteNormally`
//! reachability fact every other executable kind emits — the consumer's
//! `body_might_complete_normally` verdict reports the ERROR twin at the
//! declaring name because `Never` is potentially non-nullable
//! (`typeSystem.isPotentiallyNonNullable(returnType)` picks the
//! COMPILE_TIME_ERROR arm, resolver.dart:587-590).
//!
//! Oracle (`dart analyze` 3.13.7, `--format=machine`), measured on the
//! ticket fixture `int f() { Never g() {} return g(); }`:
//!
//! ```text
//! ERROR|COMPILE_TIME_ERROR|BODY_MIGHT_COMPLETE_NORMALLY|…|1|17|1|
//! The body might complete normally, causing 'null' to be returned, but the
//! return type, 'Never', is a potentially non-nullable type.
//! ```
//!
//! — exactly one row, at `g`'s name (the consumer's staged anchor; the flow
//! fact itself anchors at the body's `}`). The enclosing `f` reports
//! nothing: `return g();` exits (`handleReturn`), so its end is
//! unreachable either way.
//!
//! Upstream rule (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:538-612,
//! `checkForBodyMayCompleteNormally`): the FIRST decision is the pure
//! reachability gate `if (!flowAnalysis.flow!.isReachable) return;`
//! (:547-550) — the declared return type is only read afterwards
//! (:552-603). A `Never` annotation is therefore a consumer-side
//! verdict input, never a flow-side suppression: the body's own events
//! decide whether the end is reachable, and the flow layer owes the
//! fact whenever it is. The defect this ticket closes: the local
//! function's `Never` return annotation was sinking into the replay's
//! `pending_ty` slot (the signature's `Ev::Type` fires inside the
//! executable's own buffer), where a later `VarDeclBegin`/`Formal`/
//! `InitializerEnd` consumer could mis-adopt it as a declaration type —
//! and the leak let the bottom type reach faces that answered the
//! body's reachability wrong. The signature's annotation is not a
//! declaration type at all; the `pending_ty` sink must not claim it.
//!
//! Negative controls (the same fixture, oracle-verified silent): a
//! `Never` local function whose body always diverges (`throw`, an
//! infinite loop) emits no completion fact, because its end really is
//! unreachable — the fix must not start over-reporting the opposite
//! direction. A `Never?` (null-or-equivalent) local function's
//! fall-through body still emits the fact — the reachability face does
//! not change with nullability (the consumer's twin split owns that
//! verdict; upstream reports the `_nullable` WARNING twin for `Never?`,
//! whose `FutureOr` base is `Null` — silent upstream, but the flow
//! fact's emission contract is unaffected).

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

/// Offsets of every [`FlowFact::BodyMayCompleteNormally`] row — the pure
/// reachability face of the official `checkForBodyMayCompleteNormally`
/// gate.
fn completion_offsets(source: &str) -> Vec<u32> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::BodyMayCompleteNormally { span } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// The ticket fixture: `Never g() {}` falls off its `}` — the body's end
/// is reachable and the flow layer owes exactly one completion fact,
/// anchored at that `}` (offset 21). The enclosing `f` ends in `return`,
/// so no second row stands. Fails while the `Never` annotation's
/// `pending_ty` leak makes the local function's completion face
/// disappear.
#[test]
fn never_local_function_fall_through_emits_completion() {
    assert_eq!(
        completion_offsets("int f() { Never g() {} return g(); }"),
        [21]
    );
}

/// A `Never` local function whose body diverges emits nothing — the end
/// really is unreachable, so the fix cannot regress into over-reporting.
/// Oracle-verified silent for both the `throw` and the `for (;;)` shape.
#[test]
fn never_local_function_diverging_body_stays_silent() {
    assert!(completion_offsets("int f() { Never g() { throw 'x'; } return g(); }").is_empty());
    assert!(completion_offsets("int f() { Never g() { for (;;) {} } return 0; }").is_empty());
}

/// The same row for the sibling declaration flavors the oracle reports
/// identically (measured: top-level `Never g() {}` and method
/// `class C { Never m() {} }` each report the ERROR twin at the
/// declaring name) — the reachability fact stands at each body's `}`.
#[test]
fn never_top_level_and_method_fall_through_emit_completion() {
    assert_eq!(completion_offsets("Never g() {}"), [11]);
    assert_eq!(completion_offsets("class C { Never m() {} }"), [21]);
}

/// Nullability does not change the reachability face: `Never?` is the
/// null-or-equivalent spelling, but the body's `}` is still reachable —
/// the fact stands and the consumer's twin split owns the verdict.
#[test]
fn nullable_never_local_function_still_emits_the_fact() {
    assert_eq!(
        completion_offsets("int f() { Never? g() {} return 0; }"),
        [22]
    );
}

/// The typed-declaration sink guard: the signature's `Never` annotation
/// must not be adopted as the declared type of a local declared at the
/// top of the body (the `pending_ty` leak's other victim). `x` is `int`,
/// never `Never` — a `Never`-typed `x` would exit the body's flow at its
/// own read and wrongly kill the completion fact.
#[test]
fn signature_never_annotation_does_not_retype_body_locals() {
    let facts = scan_parse("int f() { Never g() { int x = 0; x; } return 0; }");
    let x_tys: Vec<crate::ty::Base> = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } if span.range.offset == 33 => Some(ty.base),
            _ => None,
        })
        .collect();
    assert_eq!(x_tys.len(), 1, "{facts:?}");
    assert!(
        matches!(x_tys[0], crate::ty::Base::Name(_)),
        "x must read its written `int`, not the leaked `Never`: {facts:?}"
    );
}

/// The enclosing-direction of the same sink scope: an immediately-invoked
/// closure's untyped formal must NOT inherit the enclosing signature's
/// `int` return annotation (the official's formal is `dynamic` —
/// `function_expression_resolver.dart:106` overwrites only a still-
/// `DynamicType` element). A leaked `int` would read `a` as `int` where
/// the oracle reads `dynamic`.
#[test]
fn enclosing_return_annotation_does_not_type_closure_formals() {
    let facts = scan_parse("int f() { (a) { a; }(); return 0; }");
    let a_tys: Vec<crate::ty::Base> = facts
        .iter()
        .filter_map(|fact| match fact {
            // `a`'s read inside the closure at offset 16.
            FlowFact::ReadType { span, ty, .. } if span.range.offset == 16 => Some(ty.base),
            _ => None,
        })
        .collect();
    assert_eq!(a_tys.len(), 1, "{facts:?}");
    assert!(
        !matches!(a_tys[0], crate::ty::Base::Name(_)),
        "untyped formal `a` must not read the enclosing `int`: {facts:?}"
    );
}

/// The outward leak through the boundary itself: a local `Never`
/// function's return annotation must not survive its `exec_end` into the
/// enclosing body's next untyped formal — `(a) { a; }` after
/// `Never g() {}` reads `a` as implicit `dynamic`, never `Never`. A
/// `Never`-typed `a` would exit at the read and open a dead-code row the
/// oracle does not produce (oracle output for the fixture: only the
/// `body_might_complete_normally` ERROR at `g`).
#[test]
fn local_never_signature_does_not_leak_to_sibling_formals() {
    let facts = scan_parse("int f() { Never g() {} var h = (a) { a; }; return 0; }");
    let a_tys: Vec<crate::ty::Base> = facts
        .iter()
        .filter_map(|fact| match fact {
            // `a`'s read inside the closure at offset 37.
            FlowFact::ReadType { span, ty, .. } if span.range.offset == 37 => Some(ty.base),
            _ => None,
        })
        .collect();
    assert_eq!(a_tys.len(), 1, "{facts:?}");
    assert!(
        !matches!(a_tys[0], crate::ty::Base::Never),
        "untyped formal `a` must not read the leaked `Never`: {facts:?}"
    );
}
