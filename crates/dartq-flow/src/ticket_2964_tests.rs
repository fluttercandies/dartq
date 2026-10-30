//! #2964 pins: the `unchecked_use_of_nullable_value` /
//! `invalid_null_aware_operator` consumer contract, as seen from the flow
//! layer.
//!
//! Both verdicts decide against the flow-aware static type of a receiver
//! (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/error/unchecked_use_of_nullable_value_verifier.dart`
//! — the receiver's `staticType` read through the flow-informed resolver;
//! `pkg/analyzer/lib/src/generated/error_verifier.dart`
//! `_checkForUnnecessaryNullAware` — the same flow-augmented type on the
//! `?`-operator's target). The semantic layer resolves the receiver's
//! declared member/return type; the flow layer answers two questions for
//! it:
//!
//! * the **read's flow type** — [`FlowFact::ReadType`] at every identifier
//!   read, [`FlowFact::PromotedRead`] where a promotion stands; and
//! * the **property read's promotion** — [`FlowFact::PromotedPropertyRead`]
//!   on a promotable-shaped member read whose slot is non-null.
//!
//! These pins lock the rows the consumer stages against on the failing
//! conformance sub-shapes (the `funcList[0]()` index-invoke, the `gf()`
//! top-level `Function?` invoke, the `if (_g != null) { _g?.abs(); }`
//! promoted-field receiver, and the `(x?.abs())`-in-a-`+`/`?.` chain
//! result): each asserts the row exists where the consumer needs it and
//! carries the nullability the oracle's verdict reads.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};

fn analyze(source: &str) -> (SharedFlow<'_>, Vec<FlowFact>) {
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
    let facts = flow.0.borrow_mut().take_facts();
    (flow, facts)
}

fn read_types(source: &str) -> (SharedFlow<'_>, Vec<(u32, Ty)>) {
    let (flow, facts) = analyze(source);
    let rows = facts
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect();
    (flow, rows)
}

/// `unchecked_use_of_nullable_value` face: a top-level `Function?` variable
/// read publishes its written nullable-function `ReadType` row — the row
/// the consumer's invocation verdict reads when `gf()` fires
/// (`The function can't be unconditionally invoked`, the `Function` kind
/// arm). A bare `Resolved::Top` read carries no promotion state of its
/// own — the written type IS the flow answer.
#[test]
fn top_level_nullable_function_read_carries_the_nullable_face() {
    let source = "Function? gf;\nvoid f() { gf(); }";
    let (flow, rows) = read_types(source);
    let function_name = flow.0.borrow_mut().intern_name("Function");
    let read = source.find("gf()").unwrap() as u32;
    let ty = rows
        .iter()
        .find(|(offset, _)| *offset == read)
        .map(|(_, ty)| *ty)
        .unwrap_or_else(|| panic!("no read row at {read}: {rows:?}"));
    // The bare `Function` head spells `Name("Function")` — `Base::Function`
    // is the signature-less `T Function(...)` form's face.
    assert_eq!(
        ty,
        Ty {
            base: Base::Name(function_name),
            nullable: true
        },
        "{rows:?}"
    );
}

/// `unchecked_use_of_nullable_value` face: the receiver read inside a
/// `?.` chain stays at its declared `int?` — the `NullAwareMemberBegin`
/// split opens AFTER the `x` read, so the read's `ReadType` row answers
/// the unpromoted declared face (the flow type at that exact offset; the
/// `?.` matched-arm promotion lives on the chain's branch state, not on
/// the receiver's own row). The oracle's `unchecked_use` verdict on
/// `x.abs()` reads exactly this nullable face; `x?.abs()`'s validity is
/// the same answer.
#[test]
fn null_aware_receiver_read_type_row_keeps_the_declared_face() {
    let source = "void f(int? x) { x?.abs(); }";
    let (flow, rows) = read_types(source);
    let int_name = flow.0.borrow_mut().intern_name("int");
    let read = source.find("x?.").unwrap() as u32;
    let ty = rows
        .iter()
        .find(|(offset, _)| *offset == read)
        .map(|(_, ty)| *ty)
        .unwrap_or_else(|| panic!("no read row at {read}: {rows:?}"));
    assert_eq!(
        ty,
        Ty {
            base: Base::Name(int_name),
            nullable: true
        },
        "{rows:?}"
    );
    let (_, facts) = analyze(source);
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            FlowFact::PromotedRead { span, .. } if span.range.offset == read
        )),
        "the `?.` split opens after the receiver read: {facts:?}"
    );
}

/// `invalid_null_aware_operator` face: a local already declared
/// non-nullable reads its declared type — `int x` inside `x?.abs()`
/// carries `ReadType = int` and no `PromotedRead` (no promotion stands;
/// the declared face already answers non-null). The consumer's
/// `_checkForUnnecessaryNullAware` verdict reads this directly.
#[test]
fn non_nullable_local_read_carries_the_declared_non_null_face() {
    let source = "void f(int x) { x?.abs(); }";
    let (flow, facts) = analyze(source);
    let int_name = flow.0.borrow_mut().intern_name("int");
    let read = source.find("x?.").unwrap() as u32;
    let read_ty = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } if span.range.offset == read => Some(*ty),
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no read row at {read}: {facts:?}"));
    assert_eq!(
        read_ty,
        Ty {
            base: Base::Name(int_name),
            nullable: false
        },
        "{facts:?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            FlowFact::PromotedRead { span, .. } if span.range.offset == read
        )),
        "a non-nullable declared read emits no promotion row: {facts:?}"
    );
}

/// `invalid_null_aware_operator` face: a `!= null`-promoted local read
/// publishes `PromotedRead = int` at the receiver token — the staged
/// re-decision the consumer's `null_aware_staged` teardown makes
/// (`promotions.promoted(recv)`). The promotion lives on the then-branch
/// state only.
#[test]
fn null_check_promoted_receiver_read_reports_the_non_null_face() {
    let source = "void f(int? x) { if (x != null) { x?.abs(); } }";
    let (flow, facts) = analyze(source);
    let int_name = flow.0.borrow_mut().intern_name("int");
    let read = source.rfind("x?.").unwrap() as u32;
    let ty = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } if span.range.offset == read => Some(*ty),
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no promoted read at {read}: {facts:?}"));
    assert_eq!(
        ty,
        Ty {
            base: Base::Name(int_name),
            nullable: false
        },
        "{facts:?}"
    );
}

/// `invalid_null_aware_operator` face: a promoted-field `?.` receiver
/// emits `PromotedPropertyRead` at the member token — the property mirror
/// of the local promotion row (official `_handleProperty` promotion,
/// `flow_analysis.dart:9010-9054`). `if (_g != null) { _g?.abs(); }` is
/// the conformance failing shape: the row must stand inside the chain so
/// the consumer can re-decide the `?.` as unnecessary. The anchor is the
/// member identifier itself.
#[test]
fn promoted_field_null_aware_receiver_emits_the_property_row() {
    let source =
        "class C { final int? _g = 0; void m() { if (_g != null) { _g?.abs(); } } }";
    let (_, facts) = analyze(source);
    let member = source.rfind("_g?.").unwrap() as u32;
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            FlowFact::PromotedPropertyRead { span } if span.range.offset == member
        )),
        "the promoted field's `?.` receiver must emit its row: {facts:?}"
    );
}

/// `unchecked_use_of_nullable_value` face: `x ??= nullableCall()` leaves
/// `x` unpromoted — the if-null write's join (`LUB(NonNull(declared),
/// rhs)`; an RHS whose type the name level cannot see — the conformance
/// failing shape family F19's resolved-`Node?` arm is its resolved-seam
/// twin) joins back to the declared face or promotes nothing, so the read
/// after reports `int?`; no `PromotedRead`.
#[test]
fn if_null_write_with_nullable_rhs_keeps_the_read_unpromoted() {
    let source =
        "int? f2() => null;\nvoid f(int? x) { x ??= f2(); x + 1; }";
    let (flow, facts) = analyze(source);
    let int_name = flow.0.borrow_mut().intern_name("int");
    let read = source.rfind("x +").unwrap() as u32;
    let read_ty = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } if span.range.offset == read => Some(*ty),
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no read row at {read}: {facts:?}"));
    assert_eq!(
        read_ty,
        Ty {
            base: Base::Name(int_name),
            nullable: true
        },
        "{facts:?}"
    );
    assert!(
        !facts.iter().any(|fact| matches!(
            fact,
            FlowFact::PromotedRead { span, .. } if span.range.offset == read
        )),
        "the nullable `??=` join must not promote: {facts:?}"
    );
}

/// `unchecked_use_of_nullable_value` face: `x ??= <non-null>` promotes
/// `x` through the write — the non-null RHS joins `NonNull(int?)` to
/// `int`, so the read after reports `PromotedRead = int` (the other half
/// of the `??=` join, family F19's non-null arm; the literal `1` carries
/// its `int` static type through `closed.int_name`, so the join sees the
/// real RHS face at name level).
#[test]
fn if_null_write_with_non_null_rhs_promotes_the_read() {
    let source = "void f(int? x) { x ??= 1; x + 1; }";
    let (flow, facts) = analyze(source);
    let int_name = flow.0.borrow_mut().intern_name("int");
    let read = source.rfind("x +").unwrap() as u32;
    let ty = facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } if span.range.offset == read => Some(*ty),
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no promoted read at {read}: {facts:?}"));
    assert_eq!(
        ty,
        Ty {
            base: Base::Name(int_name),
            nullable: false
        },
        "{facts:?}"
    );
}
