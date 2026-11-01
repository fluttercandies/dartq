//! The #3405 pins: the pattern face's `T & B` intersection promotion.
//!
//! [`crate::patterns`]' `pattern_promotion_candidate` reached only
//! [`crate::ty::TypeOps::try_promote_to_typed`]'s strict-subtype gate, so a
//! pattern test against a type-variable scrutinee promoted nothing (`T & S`
//! is `S <: T`-false for every proper `S`). #3397 chained
//! [`crate::interp_expr`]'s `type_parameter_promotion` after that gate on
//! the is-test face (`tryPromoteToType`'s type-parameter arm,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:1754-1768); the
//! official pattern face reaches the *same* gate
//! (`promoteForPattern` → `tryPromoteForTypeCheck`,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:7748),
//! so the same arm chains on the pattern face too.
//!
//! The harness is [`crate::ticket_3397_tests`]' — the same edge-closure
//! back-end, because the strict gate (`to <: from && !(from <: to)`)
//! refuses the intersection step for every proper `S` and the declared
//! bound is element data only the Boundary table carries.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{ArgTy, Base, Classification, Ty, TypeOps, EMPTY_ARGS};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The edge-closure back-end ([`crate::ticket_3397_tests::HierarchyOps`]):
/// direct `extends` edges answer the subtype question; the strict
/// promotion gate refuses every non-subtype step.
struct HierarchyOps {
    ids: std::cell::RefCell<Vec<u32>>,
    edges: &'static [(usize, usize)],
}

impl HierarchyOps {
    fn seed(&self, ids: Vec<u32>) {
        *self.ids.borrow_mut() = ids;
    }

    fn reaches(&self, sub: u32, sup: u32) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut queue = std::collections::VecDeque::from([sub]);
        while let Some(cur) = queue.pop_front() {
            if cur == sup {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            for (s, t) in self.edges {
                if self.ids.borrow().get(*s) == Some(&cur) {
                    queue.push_back(self.ids.borrow()[*t]);
                }
            }
        }
        false
    }
}

impl TypeOps for HierarchyOps {
    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        if left == right || left.base == Base::Never {
            return true;
        }
        match (left.base, right.base) {
            (Base::Null, _) => right.nullable,
            (_, Base::Dynamic | Base::Unknown) => true,
            (Base::Name(l), Base::Name(r)) => {
                (!left.nullable || right.nullable) && self.reaches(l, r)
            }
            _ => crate::ty::NameTypeOps.is_subtype(left, right),
        }
    }

    fn factor(&self, from: Ty, what: Ty) -> Ty {
        crate::ty::NameTypeOps.factor(from, what)
    }

    fn try_promote_to(&self, to: Ty, from: Ty) -> Option<Ty> {
        self.try_promote_to_typed(to, EMPTY_ARGS, from, EMPTY_ARGS, &[], &[])
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

    fn try_promote_to_typed(
        &self,
        to: Ty,
        to_args: u32,
        from: Ty,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<Ty> {
        let down = self.is_subtype_typed(to, to_args, from, from_args, arena, names);
        let up = self.is_subtype_typed(from, from_args, to, to_args, arena, names);
        (down && !up).then_some(to)
    }
}

/// [`crate::ticket_3397_tests::analyze`]: scan, parse, mount `ops`.
fn analyze<'t>(
    source: &'static str,
    intern: &[&str],
    ops: Option<&'t HierarchyOps>,
) -> (Vec<u32>, Vec<FlowFact>) {
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
    let ids = {
        let mut recorder = flow.0.borrow_mut();
        let ids: Vec<u32> = intern
            .iter()
            .map(|lexeme| recorder.intern_name(lexeme))
            .collect();
        if let Some(ops) = ops {
            ops.seed(ids.clone());
            recorder.set_ops(ops);
        }
        ids
    };
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let facts = flow.0.borrow_mut().take_facts();
    (ids, facts)
}

/// The `ReadType` rows of `source`: `(snippet, flow type)` in fact order.
fn read_types<'a>(source: &'a str, facts: &[FlowFact]) -> Vec<(&'a str, Ty)> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((
                &source[span.range.offset as usize
                    ..(span.range.offset + span.range.length) as usize],
                *ty,
            )),
            _ => None,
        })
        .collect()
}

/// The `PromotedRead` rows of `source`: `(snippet, promoted type)`.
fn promoted_reads<'a>(source: &'a str, facts: &[FlowFact]) -> Vec<(&'a str, Ty)> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedRead { span, ty, .. } => Some((
                &source[span.range.offset as usize
                    ..(span.range.offset + span.range.length) as usize],
                *ty,
            )),
            _ => None,
        })
        .collect()
}

// —— `T & B` intersection through the pattern face ———————————————————————

#[test]
fn type_parameter_if_case_promotes_to_the_intersection() {
    // `if (value case int _)` over `T extends num` — the pattern face's
    // `tryPromoteForTypeCheck` refuses the strict step (`int <: T` is
    // false) and the chained type-parameter arm answers `int <: num`,
    // so the guarded read spells the `T & int` head.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f<T extends num>(T value) {\n  if (value case int _) {\n    value;\n  }\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "T"], Some(&ops));
    let int = ids[0];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads.last(),
        Some(&("value", Ty { base: Base::Name(int), nullable: false })),
        "the guarded read answers `int`, the `T & int` head: {reads:?}"
    );
    assert_eq!(
        promoted_reads(source, &facts),
        [("value", Ty { base: Base::Name(int), nullable: false })],
        "the promotion fact rides the guarded read"
    );
}

#[test]
fn switch_case_over_type_parameter_promotes_to_the_intersection() {
    // The switch-statement spelling of the same face: `case int _:` over
    // `T extends num` promotes the scrutinee in the arm body only.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f<T extends num>(T value) {\n  switch (value) {\n    case int _:\n      value;\n  }\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "T"], Some(&ops));
    let (int, t) = (ids[0], ids[3]);
    let reads = read_types(source, &facts);
    assert_eq!(
        reads.last(),
        Some(&("value", Ty { base: Base::Name(int), nullable: false })),
        "the arm-body read spells the `T & int` head: {reads:?}"
    );
    assert!(
        reads.contains(&("value", Ty { base: Base::Name(t), nullable: false })),
        "the scrutinee outside the arm stays `T`: {reads:?}"
    );
    assert_eq!(
        promoted_reads(source, &facts),
        [("value", Ty { base: Base::Name(int), nullable: false })]
    );
}

#[test]
fn null_pattern_over_type_parameter_promotes_nothing() {
    // `case Null _` over `T extends num` — `Null` sits outside the bound
    // (`Null <: num` false), both gates refuse, and no read anywhere
    // claims the tested type: the else arm's read stays `T`.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2), (3, 2)],
    };
    let source = "void f<T extends num>(T value) {\n  if (value case Null _) {\n    value;\n  } else {\n    value;\n  }\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "Null", "T"], Some(&ops));
    let t = ids[4];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads.last(),
        Some(&("value", Ty { base: Base::Name(t), nullable: false })),
        "the else-arm read stays `T`: {reads:?}"
    );
    assert!(
        reads.iter().all(|(s, ty)| *ty != Ty { base: Base::Name(ids[3]), nullable: false }),
        "no read claims `Null` — the arm promoted nothing: {reads:?}"
    );
    assert!(promoted_reads(source, &facts).is_empty());
}

#[test]
fn non_type_parameter_scrutinee_promotes_through_the_plain_gate() {
    // Regression: `case int _` over a plain `num` — the strict gate
    // narrows (`int <: num`, `num <: int` false; no boundary row answers
    // the chained arm), the read spells `int`, exactly the pre-#3405
    // face. The widening direction stays put (`case num _` over `int`
    // promotes nothing — a supertype test is no refinement).
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f(num value) {\n  if (value case int _) {\n    value;\n  }\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object"], Some(&ops));
    let int = ids[0];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads.last(),
        Some(&("value", Ty { base: Base::Name(int), nullable: false })),
        "the plain gate's promotion is unchanged: {reads:?}"
    );
    assert_eq!(
        promoted_reads(source, &facts),
        [("value", Ty { base: Base::Name(int), nullable: false })]
    );
}

#[test]
fn bound_gates_the_pattern_intersection() {
    // The negative: `T extends num` gains nothing from `case String _` —
    // `String <: num` is false, the type-parameter arm refuses, and the
    // reads stay `T`.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2), (3, 2)],
    };
    let source = "void f<T extends num>(T value) {\n  if (value case String _) {\n    value;\n  } else {\n    value;\n  }\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "String", "T"], Some(&ops));
    let t = ids[4];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads.last(),
        Some(&("value", Ty { base: Base::Name(t), nullable: false })),
        "an out-of-bound pattern promotes nothing: {reads:?}"
    );
    assert!(
        reads.iter().all(|(_, ty)| *ty != Ty { base: Base::Name(ids[3]), nullable: false }),
        "no read claims `String`: {reads:?}"
    );
    assert!(promoted_reads(source, &facts).is_empty());
}
