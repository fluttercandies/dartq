//! The #3435 pins: a call's result is never a reference operand — the
//! `Ev::Send` close now drops the callee identifier's `reference` slot on
//! the expression it produces.
//!
//! Upstream a call expression carries no `_ExpressionReference`: its
//! static type is the invoked element's RETURN type, so `Ev::IsEnd` on a
//! call takes the non-reference arm — the same contract the file's own
//! comment already states ("a call carries no reference"), which used to
//! hold only for the unresolved-invalid arm. The callee flowed through as
//! the send's result row with its variable slot standing, so `nf() is int`
//! read `nf`'s DECLARED callable type, `if (nf())` emitted a
//! [`FlowFact::ConditionType`] row on `nf`, `switch (nf())` latched `nf`
//! as the matched-value promotion key, and a `dynamic` callee's
//! `nf() is int` promoted `nf` itself in the then arm.
//!
//! The clear is gated on the argument list (a real `Ev::Send` directly
//! after `Ev::ArgumentsEnd`) exactly like the typing faces beside it —
//! in this event stream every `Send` that is an invocation carries an
//! argument list (a `f<T>` tear-off is a type-argument application and
//! fires no `Send` at all), so the gate's edge IS every invocation.
//! `super(...)`/`this(...)`
//! constructor sends and implicit-`this` callees carry no variable
//! reference to begin with — the receiver/`this` slot and the property
//! slot live in other fields — so the only rows the clear touches are
//! bare callable-value heads: `nf()`, `(nf)()`, and the inner result of
//! `a()()`, which is already reference-less when the outer send pops it.

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

/// The flow type at the `ReadType` row covering the read starting at byte
/// offset `at` (an `nf;`/`a;` inside a branch, located by `source.find`
/// from the caller).
fn read_ty_at(facts: &[FlowFact], at: usize) -> Option<Ty> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => {
                (span.range.offset as usize == at).then_some(*ty)
            }
            _ => None,
        })
        .next()
}

/// Every `ConditionType` row's covered text.
fn condition_rows(source: &str, facts: &[FlowFact]) -> Vec<String> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ConditionType { span, .. } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// Every `MatchedType` row's covered anchor text.
fn matched_rows(source: &str, facts: &[FlowFact]) -> Vec<String> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::MatchedType { span, .. } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// The ticket's shape: `nf() is int` on a `Null Function()` parameter is a
/// NON-reference test. `nf` is untouched by the test — its read inside
/// the then arm keeps the declared `Function` face — and neither branch
/// dies at name level: the call keeps the signature-less `Function` head
/// while the semantic layer stages no return row (#3436's half), and that
/// head is disjoint from nothing.
#[test]
fn call_result_is_test_takes_the_non_reference_arm() {
    let source = "void g(Null Function() nf) {\n  if (nf() is int) {\n    nf;\n  } else {\n    nf;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "`nf() is int` folds nothing at name level — the reference arm's \
         declared-type read is gone"
    );
    let then_read = source.find("nf;").expect("then read");
    assert_eq!(
        read_ty_at(&facts, then_read),
        Some(Ty {
            base: Base::Function,
            nullable: false,
        }),
        "`nf` inside the then arm keeps its declared callable type — the \
         test ran on the call's result, not on `nf`"
    );
}

/// The `is!` mirror: the same non-reference posture, no dead arm, `nf`
/// untouched in either branch.
#[test]
fn call_result_is_not_test_mirrors_the_non_reference_arm() {
    let source = "void g(Null Function() nf) {\n  if (nf() is! int) {\n    nf;\n  } else {\n    nf;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(dead_of(source, &facts), Vec::<String>::new());
    let else_read = source.rfind("nf;").expect("else read");
    assert_eq!(
        read_ty_at(&facts, else_read),
        Some(Ty {
            base: Base::Function,
            nullable: false,
        }),
    );
}

/// The sharpest split between the arms: a `dynamic` callee's `nf() is int`
/// must leave `nf` alone. With the reference slot standing, `Ev::IsEnd`
/// took the reference arm and promoted `nf` itself to `int` inside the
/// then arm — `nf;` there read `int`. As a non-reference operand the call
/// (`dynamic` static type) folds nothing and `nf` keeps `dynamic`.
#[test]
fn dynamic_callee_is_test_does_not_promote_the_callee() {
    let source = "void g(dynamic nf) {\n  if (nf() is int) {\n    nf;\n  }\n}\n";
    let facts = analyze(source, None);
    let then_read = source.rfind("nf;").expect("then read");
    assert_eq!(
        read_ty_at(&facts, then_read),
        Some(Ty::DYNAMIC),
        "`nf` must stay `dynamic` — the test reads the call's result, not \
         the callee variable"
    );
    assert_eq!(dead_of(source, &facts), Vec::<String>::new());
}

/// A call is no condition operand either: `if (nf())` emits no
/// `ConditionType` row — the official's condition-operand query covers
/// promotable reference reads only, and a call result is not one. The
/// standing slot used to pair `nf`'s read span with its variable and
/// emit a row naming `nf`.
#[test]
fn call_condition_emits_no_condition_type_row() {
    let source = "void g(bool Function() nf) {\n  if (nf()) {\n    1;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(
        condition_rows(source, &facts),
        Vec::<String>::new(),
        "a call carries no flow-visible operand type — `nf` must not \
         appear as the condition's reference"
    );
}

/// The same latch for a switch scrutinee: `switch (nf())` matches on the
/// call's result, so `nf` is no promotion key — no `MatchedType` rows, and
/// a `case int` arm leaves `nf` at `dynamic` instead of promoting the
/// callee to the matched type.
#[test]
fn call_scrutinee_latches_no_promotion_key() {
    let source = "void g(dynamic nf) {\n  switch (nf()) {\n    case int v:\n      nf;\n    default:\n      break;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(
        matched_rows(source, &facts),
        Vec::<String>::new(),
        "a call scrutinee carries no matched-value promotion key"
    );
    let case_read = source.rfind("nf;").expect("case body read");
    assert_eq!(
        read_ty_at(&facts, case_read),
        Some(Ty::DYNAMIC),
        "`nf` inside `case int` keeps `dynamic` — the match tested the \
         call's result, not `nf`"
    );
}

/// A chained send: `a()()` clears on BOTH closes — the inner result
/// already stands reference-less when the outer `Send` pops it, and the
/// outer's own result carries nothing either. `a()() is int` leaves `a`
/// at `dynamic` in the then arm (pre-fix the inner send handed `a`'s
/// slot forward, so the outer result still read `a`'s variable).
#[test]
fn chained_send_result_keeps_no_reference() {
    let source = "void g(dynamic a) {\n  if (a()() is int) {\n    a;\n  }\n}\n";
    let facts = analyze(source, None);
    let then_read = source.rfind("a;").expect("then read");
    assert_eq!(
        read_ty_at(&facts, then_read),
        Some(Ty::DYNAMIC),
        "`a` stays `dynamic` — neither send's result is `a`"
    );
    assert_eq!(dead_of(source, &facts), Vec::<String>::new());
}

/// A parenthesized callee: `(nf)()` is the same invocation shape — the
/// paren forwards the read's row, so the clear must not depend on the
/// callee arriving straight off `Ev::Ident`.
#[test]
fn parenthesized_callee_result_keeps_no_reference() {
    let source = "void g(dynamic nf) {\n  if ((nf)() is int) {\n    nf;\n  }\n}\n";
    let facts = analyze(source, None);
    let then_read = source.rfind("nf;").expect("then read");
    assert_eq!(read_ty_at(&facts, then_read), Some(Ty::DYNAMIC));
}

/// The null-shorted send keeps its own machinery: `nf?.call()` splits on
/// `nf` at the `?.` (the receiver snapshot at `NullAwareMemberBegin` — a
/// different slot's ride, untouched by the send close) and the call's
/// result tests nothing at name level. Both arms stay live and the chain
/// joins at the condition's own consumer.
#[test]
fn null_shorted_send_keeps_its_short_machinery() {
    let source = "void g(int? Function()? nf) {\n  if (nf?.call() is int) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(
        dead_of(source, &facts),
        Vec::<String>::new(),
        "the null-shorted call's `is` folds nothing — and must not crash \
         or starve the short's join"
    );
}

/// A method call never carried the callee shape this fix touches — the
/// member's own expression has no variable slot — so `c.m() is int` keeps
/// its pre-fix posture: both arms live and `c` keeps its declared named
/// face inside the then arm.
#[test]
fn method_call_result_unchanged() {
    let source =
        "class C { int m() => 0; }\nvoid g(C c) {\n  if (c.m() is int) {\n    c;\n  }\n}\n";
    let facts = analyze(source, None);
    assert_eq!(dead_of(source, &facts), Vec::<String>::new());
    let then_read = source.rfind("c;").expect("then read");
    match read_ty_at(&facts, then_read) {
        Some(Ty {
            base: Base::Name(_),
            nullable: false,
        }) => {}
        other => panic!("`c` must read its declared named type, got {other:?}"),
    }
}

/// The unresolved-invalid arm is unchanged: `nowheresfn()` resolves
/// nowhere under the mounted seam, so its result carries the
/// `non_reference_invalid` face and the succeed fold's vacuous answer
/// still deads the `is` else — the pre-existing posture the ticket's
/// gate does not touch (the flag was never the leaked slot).
#[test]
fn unresolved_call_result_keeps_the_invalid_face() {
    let source =
        "class C {}\nvoid g() {\n  if (nowheresfn() is C) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    let ops: &dyn TypeOps = &UnresolvedHeadOps;
    let facts = analyze(source, Some(ops));
    let dead = dead_of(source, &facts);
    assert_eq!(
        dead.len(),
        1,
        "the unresolved call's `is` still deads exactly the else arm, \
         got {dead:?}"
    );
    assert!(
        dead[0].contains('2'),
        "the dead arm is the else, got {dead:?}"
    );
}

/// The gate's boundary, pinned on its exact edge: a `g<int>` type-
/// argument application is NO invocation — no `Ev::Send` fires at all
/// (the group's own `handle_type_argument_application` carries no flow
/// event), so `g`'s read row reaches the `==` untouched and the
/// null-comparison marks `g` non-null in the else arm exactly like a bare
/// `g == null` would. Had the application been mistaken for a send, the
/// mark would have nothing to land on.
#[test]
fn type_argument_application_keeps_the_reference() {
    let source =
        "void g(Object? g) {\n  if (g<int> == null) {\n    1;\n  } else {\n    g;\n  }\n}\n";
    let facts = analyze(source, None);
    let else_read = source.rfind("g;").expect("else read");
    match read_ty_at(&facts, else_read) {
        Some(Ty {
            base: Base::Name(_),
            nullable: false,
        }) => {}
        other => panic!(
            "`g<int>`'s leaf still carries `g`'s reference — the `!=` arm \
             marks it non-null, got {other:?}"
        ),
    }
}

/// The resolved-seam ops for [`unresolved_call_result_keeps_the_invalid_
/// face`]: `nowheresfn` resolves nowhere (the callee-side `InvalidType`
/// face), and any relation with the invalid spelling is vacuously true —
/// the `subtype.dart` 0c arm the succeed fold reads.
struct UnresolvedHeadOps;

impl UnresolvedHeadOps {
    fn is_invalid(&self, ty: Ty, names: &[Box<str>]) -> bool {
        match ty.base {
            Base::Name(id) => names
                .get(id as usize)
                .is_some_and(|name| name.as_ref() == "nowheresfn"),
            _ => false,
        }
    }
}

impl TypeOps for UnresolvedHeadOps {
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
            || crate::ty::NameTypeOps
                .is_subtype_typed(left, left_args, right, right_args, arena, names)
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
