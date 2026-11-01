//! The #3452 pins: a completed invocation also drops the member chain's
//! `ref_chain` flag — not just the `reference`/`prop` slots #3435 cleared.
//!
//! `x.f`'s own DotAccess correctly sets `ref_chain` (the official
//! `propertyGet` hands a property reference to a read rooted at a flow
//! reference, so `x.f is C` stays open on an invalid face). The `Ev::Send`
//! close pops that expression as the callee, and #3435 dropped its
//! `reference`/`prop`/`non_null_literal` — but the flag survived onto the
//! send's result, so `x.f() is C` and `x.f().g is C` re-rooted at the
//! callee's variable and took the reference chain's LENIENT arm on an
//! invalid face, where the oracle's non-reference folds dead the else
//! (`x.foo()`, `f().bar` — the no-property-slot group, oracle-probed).
//!
//! The invalid face in these pins is a receiver whose written annotation
//! resolves nowhere (`Nowhere x`) under a mounted
//! [`InvalidFaceOps`] seam: the invalid-receiver member read inherits the
//! face and the succeed fold's vacuous answer deads exactly the else arm.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{ArgTy, Base, Classification, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Scans and parses `source` with `ops` mounted when given; returns the
/// taken facts.
fn analyze<'t>(source: &'static str, ops: Option<&'t dyn TypeOps>) -> Vec<FlowFact> {
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
    {
        let mut recorder = flow.0.borrow_mut();
        if let Some(ops) = ops {
            recorder.set_ops(ops);
        }
        recorder.set_sound_flow(features.sound_flow_analysis);
    }
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// The `dead_code` row texts of `source`, in source order.
fn dead_of(source: &str, facts: &[FlowFact]) -> Vec<String> {
    facts
        .iter()
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

/// The sharpest shape: an invalid receiver's method send tests `C`. The
/// call's result carries the invalid face (a method call on an invalid
/// receiver resolves without a target), and as a NON-reference operand the
/// succeed fold's vacuous answer deads exactly the else. The leaked
/// `ref_chain` used to root the result at `x` and keep both arms live.
#[test]
fn member_send_result_is_deads_the_else_arm() {
    let source = "class C {}\nvoid g() {\n  Nowhere x;\n  if (x.f() is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    let dead = dead_of(source, &facts);
    assert_eq!(
        dead.len(),
        1,
        "the call result is no reference operand — exactly the else arm \
         dies, got {dead:?}"
    );
    assert!(dead[0].contains('2'), "the dead arm is the else, got {dead:?}");
}

/// The chain INTERRUPT propagates: a member read over the call result
/// (`x.f().g`) inherits the invalid face through the send's result, and
/// with the flag dropped it re-roots as non-reference — the oracle's
/// `f().bar` group, dead else.
#[test]
fn member_of_call_result_deads_the_else_arm() {
    let source = "class C {}\nvoid g() {\n  Nowhere x;\n  if (x.f().g is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    let dead = dead_of(source, &facts);
    assert_eq!(
        dead.len(),
        1,
        "`x.f().g` is a member read off a call result — non-reference, \
         exactly the else arm dies, got {dead:?}"
    );
    assert!(dead[0].contains('2'), "the dead arm is the else, got {dead:?}");
}

/// The member READ on the same reference chain keeps its lenient arm: no
/// invocation, no clear — `x.f is C` stays open on the invalid face (the
/// official's property reference over the resolver's `dynamic`
/// unpromoted type, which neither folds nor factors to bottom).
#[test]
fn member_read_on_reference_chain_stays_open() {
    let source = "class C {}\nvoid g() {\n  Nowhere x;\n  if (x.f is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "the unresolved property read on a reference chain stays open — \
         both arms live"
    );
}

/// A deeper plain-read chain propagates the same way: `x.f.g is C` stays
/// open — the chain's lenient arm is what the fix must not touch.
#[test]
fn deep_chain_over_plain_reads_stays_open() {
    let source = "class C {}\nvoid g() {\n  Nowhere x;\n  if (x.f.g is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "`x.f.g` keeps the reference-rooted chain's lenient arm"
    );
}

/// The null-shorted member read is unchanged: `x?.f is C` runs on the
/// short's own branch machinery (the bare-leaf send carries no argument
/// list, so the close never fires) and keeps both arms live on the
/// invalid face.
#[test]
fn null_shorted_member_read_is_unchanged() {
    let source = "class C {}\nvoid g() {\n  Nowhere x;\n  if (x?.f is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "`x?.f is C` folds nothing — the short's join covers both arms"
    );
}

/// The #3435 contract stands beside the new clear: an is-test on a DIRECT
/// unresolved call still takes the non-reference arm and deads exactly the
/// else (the bare head never carried a chain — the flag clear is a no-op
/// there).
#[test]
fn direct_call_result_stays_non_reference() {
    let source = "class C {}\nvoid g() {\n  if (nowheresfn() is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    let dead = dead_of(source, &facts);
    assert_eq!(dead.len(), 1, "exactly the else arm dies, got {dead:?}");
    assert!(dead[0].contains('2'), "the dead arm is the else, got {dead:?}");
}

/// The oracle-cited `f().bar` face: a member read off an unresolved DIRECT
/// call inherits the call's invalid face with no chain to re-root (a bare
/// head never had one) — dead else, unchanged by the fix.
#[test]
fn member_of_direct_call_result_deads_the_else_arm() {
    let source = "class C {}\nvoid g() {\n  if (nowheresfn().g is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &InvalidFaceOps;
    let facts = analyze(source, Some(ops));
    let dead = dead_of(source, &facts);
    assert_eq!(dead.len(), 1, "exactly the else arm dies, got {dead:?}");
    assert!(dead[0].contains('2'), "the dead arm is the else, got {dead:?}");
}

/// The healthy face is untouched: a resolved receiver's method send whose
/// result reads a member folds nothing (an unknown/unpromoted operand
/// face is never a dead-branch producer) — the clear introduces no new
/// fold on a live face.
#[test]
fn healthy_member_of_call_result_keeps_both_arms() {
    let source = "class C { int m() => 0; }\nvoid g(C c) {\n  if (c.m().g is int) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "a healthy operand's is-test folds nothing at name level"
    );
}

/// The seam ops for the invalid faces: the written annotation `Nowhere`
/// resolves nowhere (the receiver-side `InvalidType` face) and the head
/// `nowheresfn` resolves nowhere (the callee-side face); any relation
/// with either spelling is vacuously true — the `subtype.dart` 0c arm the
/// succeed fold reads.
struct InvalidFaceOps;

impl InvalidFaceOps {
    fn is_invalid(&self, ty: Ty, names: &[Box<str>]) -> bool {
        match ty.base {
            Base::Name(id) => names.get(id as usize).is_some_and(|name| {
                name.as_ref() == "Nowhere" || name.as_ref() == "nowheresfn"
            }),
            _ => false,
        }
    }
}

impl TypeOps for InvalidFaceOps {
    fn is_resolved_seam(&self) -> bool {
        true
    }

    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        crate::ty::NameTypeOps.is_subtype(left, right)
    }

    fn is_subtype_typed(
        &self,
        left: Ty,
        left_args: u32,
        right: Ty,
        right_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        // The 0c arm: any relation with the invalid head is vacuous.
        self.is_invalid(left, names)
            || self.is_invalid(right, names)
            || crate::ty::NameTypeOps.is_subtype_typed(
                left, left_args, right, right_args, arena, names,
            )
    }

    fn is_invalid_typed(
        &self,
        ty: Ty,
        _args: u32,
        _arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        self.is_invalid(ty, names)
    }

    fn call_head_unresolved(&self, name: u32, names: &[Box<str>]) -> bool {
        names
            .get(name as usize)
            .is_some_and(|name| name.as_ref() == "nowheresfn")
    }

    fn factor(&self, from: Ty, what: Ty) -> Ty {
        crate::ty::NameTypeOps.factor(from, what)
    }

    fn try_promote_to(&self, to: Ty, from: Ty) -> Option<Ty> {
        crate::ty::NameTypeOps.try_promote_to(to, from)
    }

    fn promote_to_non_null(&self, t: Ty) -> Ty {
        crate::ty::NameTypeOps.promote_to_non_null(t)
    }

    fn classify(&self, t: Ty) -> Classification {
        crate::ty::NameTypeOps.classify(t)
    }

    fn is_bottom(&self, t: Ty) -> bool {
        crate::ty::NameTypeOps.is_bottom(t)
    }
}
