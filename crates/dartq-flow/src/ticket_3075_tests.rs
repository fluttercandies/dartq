//! #3075 — the declared/written distinction (official `isImplicitlyTyped`,
//! `flow_analysis.dart` `declare`/`_initialize`): a local declared WITH a
//! written annotation keeps that annotation as its slot face — the
//! initializer's inferred/staged type must never replace it — while an
//! implicitly typed (`var`) declarator adopts the inferred face exactly
//! like before.
//!
//! Two defects pinned:
//!
//! * A written-nullable local (`int? q = f()`) whose read row answered
//!   from the initializer instead of the annotation — every
//!   `Analysis::type_of` consumer read the wrong nullability face.
//! * The stale-annotation leak: an annotation written inside an
//!   initializer (a local function's `int?` return type) waited in
//!   `pending_ty` and was adopted by the NEXT `VarDeclBegin`, so
//!   `var q = (int? g() => 0)(); var r = 1;` declared `r` as `int?` and
//!   gated its inferred-face adoption off — `InitializerEnd` now drops a
//!   pending type that survived its own construct.
//!
//! Oracle anchors: `declare`'s `isImplicitlyTyped` arm and
//! `_initialize`'s `promoteToTypeOfInterest: !isImplicitlyTyped && !isFinal`
//! (flow_analysis.dart:7561-7595, :9052-9088).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

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
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// `(snippet, ty)` of every `ReadType`/`PromotedRead` row, in fact order.
fn reads<'a>(source: &'a str, facts: &[FlowFact]) -> (Vec<(&'a str, Ty)>, Vec<(&'a str, Ty)>) {
    let at = |span: dartq_source::SourceRange| {
        &source[span.offset as usize..(span.offset + span.length) as usize]
    };
    let mut read = Vec::new();
    let mut promoted = Vec::new();
    for fact in facts {
        match fact {
            FlowFact::ReadType { span, ty, .. } => read.push((at(span.range), *ty)),
            FlowFact::PromotedRead { span, ty, .. } => promoted.push((at(span.range), *ty)),
            _ => {}
        }
    }
    (read, promoted)
}

/// The headline: `int? q = f()` keeps its written `?` on the read row —
/// an unresolvable-call initializer contributes no promotion and no
/// staged face, so the row must carry the declared `int?` verbatim.
/// Pre-fix this shape could read non-nullable (the slot typed from the
/// initializer rather than the annotation).
#[test]
fn written_nullable_local_keeps_its_annotation_on_reads() {
    let source = "void f() { int? q = g(); q; }";
    let (read, _) = reads(source, &facts_of(source));
    let q = read
        .iter()
        .find(|(snippet, _)| *snippet == "q")
        .expect("a read row for q");
    let Ty { base, nullable } = q.1;
    assert!(
        nullable && matches!(base, Base::Name(_)),
        "int? q = g() must keep the written nullable face on reads: {read:?}"
    );
}

/// The promotion that SHOULD fire still fires: `int? q = 0` is the
/// official's `promoteToTypeOfInterest` case (`!isImplicitlyTyped &&
/// !isFinal`, flow_analysis.dart:7583) — `int <: int?` promotes `q` to
/// `int` at the declaration's own initialization write. The declared slot
/// stays `int?`; the read row answers the promoted `int`.
#[test]
fn written_nullable_local_still_promotes_from_a_non_null_initializer() {
    let source = "void f() { int? q = 0; q; }";
    let (read, promoted) = reads(source, &facts_of(source));
    let q = read
        .iter()
        .find(|(snippet, _)| *snippet == "q")
        .expect("a read row for q");
    assert!(
        !q.1.nullable && matches!(q.1.base, Base::Name(_)),
        "the promoted read of q answers int: {read:?}"
    );
    assert!(
        promoted.iter().any(|(snippet, _)| *snippet == "q"),
        "a PromotedRead row stands for the initializer promotion: {promoted:?}"
    );
}

/// `final` never promotes at its initializer (official
/// `_initialize`'s `!isFinal` arm): `final int? q = 0` reads `int?`
/// — the written annotation AND its nullability both survive.
#[test]
fn final_written_nullable_local_never_promotes() {
    let source = "void f() { final int? q = 0; q; }";
    let (read, promoted) = reads(source, &facts_of(source));
    let q = read
        .iter()
        .find(|(snippet, _)| *snippet == "q")
        .expect("a read row for q");
    assert!(
        q.1.nullable && matches!(q.1.base, Base::Name(_)),
        "final int? q = 0 stays int?: {read:?}"
    );
    assert!(
        promoted.is_empty(),
        "a final local's initializer promotes nothing: {promoted:?}"
    );
}

/// The stale-annotation leak: `var q = (int? g() => 0)();` — the inner
/// `int?` return annotation must not leak into the NEXT declaration.
/// `var r = 1` is implicitly typed: its slot may adopt the inferred face
/// but must never adopt the leaked `int?` as its DECLARED face. Pre-fix
/// `r` read `int?`; post-fix it reads the unknown inferred face.
#[test]
fn an_initializer_internal_annotation_does_not_leak_into_the_next_decl() {
    let source = "void f() { var q = (int? g() => 0)(); var r = 1; r; }";
    let (read, _) = reads(source, &facts_of(source));
    let r = read
        .iter()
        .find(|(snippet, _)| *snippet == "r")
        .expect("a read row for r");
    assert_eq!(
        r.1,
        Ty::UNKNOWN,
        "var r must not inherit the initializer's inner int? annotation: {read:?}"
    );
}

/// A null check on a written-nullable local still promotes to its
/// non-null form — proving the declared `?` stood in the slot (only a
/// nullable declared type can non-null-promote).
#[test]
fn written_nullable_local_keeps_null_check_promotion() {
    let source = "void f() { num? n = g(); if (n == null) {} else { n; } }";
    let (read, _) = reads(source, &facts_of(source));
    let arm = read
        .iter()
        .filter(|(snippet, _)| *snippet == "n")
        .map(|(_, ty)| *ty)
        .collect::<Vec<_>>();
    assert!(
        arm.iter()
            .any(|ty| !ty.nullable && matches!(ty.base, Base::Name(_))),
        "the else arm reads n non-nullable (promoted off num?): {read:?}"
    );
}

/// An implicitly typed local still adopts nothing it was never told: with
/// no staged face and an untyped-`var` initializer, `var q = 0` keeps the
/// unknown slot — the write's `promote_to_interest` gate stays off for an
/// implicit declarator (official `!isImplicitlyTyped`).
#[test]
fn implicit_local_never_type_of_interest_promotes() {
    let source = "void f() { var q = 0; q; }";
    let (read, promoted) = reads(source, &facts_of(source));
    let q = read
        .iter()
        .find(|(snippet, _)| *snippet == "q")
        .expect("a read row for q");
    assert_eq!(
        q.1,
        Ty::UNKNOWN,
        "var q = 0 keeps the unknown declared face in the unit harness: {read:?}"
    );
    assert!(
        promoted.is_empty(),
        "an implicit declarator promotes nothing at its initializer: {promoted:?}"
    );
}
