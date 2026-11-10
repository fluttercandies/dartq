//! The #3431 pins: the conditional expression's static type is the
//! official `leastUpperBound(thenType, elseType)` — never the else arm
//! alone.
//!
//! `Ev::CondEnd` used to push `else_expr.ty` verbatim (and `Ty::UNKNOWN`
//! when the else arm was unspellable), discarding the then arm's face at
//! `Ev::CondColon`. The official types `c ? a : b` by
//! `typeOperations.leastUpperBound`
//! (pkg/analyzer/lib/src/generated/static_type_analyzer.dart:87), so a
//! conditional whose arms joined anywhere but the else face mis-typed
//! every consumer: inferred locals adopted the else arm, write promotion
//! promoted toward it, and the staged-type seams read it. The join now
//! runs through [`crate::interp::Interp::name_lub`] — the same
//! subtype-ordered join `??` and the collection-element fold use — over
//! the then arm's `(ty, args)` pair, which stays on the expression stack
//! under the else arm's own pushes until `CondEnd` pairs them.
//!
//! The pins read the conditional's type through the two consumers the
//! fact stream exposes: an implicitly-typed local's adopted declared
//! face ([`FlowFact::ReadType`] on its next read) and the write
//! promotion's type-of-interest step on an annotated target. A pinned
//! subtype hierarchy (the edge-closure back-end of
//! [`crate::ticket_3397_tests`]) answers the joins the name-level
//! spelling alone cannot.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{ArgTy, Base, Classification, EMPTY_ARGS, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The edge-closure back-end ([`crate::ticket_3397_tests::HierarchyOps`]):
/// direct `extends` edges answer the subtype question the bare name
/// spelling refuses; the strict promotion gate mirrors the resolved seam.
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
            queue.extend(
                self.edges
                    .iter()
                    .filter(|(s, _)| self.ids.borrow().get(*s) == Some(&cur))
                    .filter_map(|(_, t)| self.ids.borrow().get(*t).copied()),
            );
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

/// Scans and parses `source` with `ops` mounted; returns the interned ids
/// (positions matching `intern`) and the taken facts.
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

/// The flow type at the LAST `ReadType` row whose covered text is `name`
/// (a declared local's reads all carry its name — the read of interest is
/// the trailing `name;` use).
fn ty_of_last_read(source: &str, facts: &[FlowFact], name: &str) -> Option<Ty> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => {
                let text = &source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize];
                (text == name).then_some(*ty)
            }
            _ => None,
        })
        .last()
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

/// The ticket fixture's core shape: `C1` and `N` are unordered names both
/// under `Object`, so the official LUB is `Object` — a face the name-level
/// lattice spells as the unknown marker (the real hierarchy's LUB is the
/// resolved back-end's question, `name_lub`'s contract). The pre-fix push
/// named `N` — the else arm — so `o = c ? C1() : N()` promoted `o` toward
/// `N`. Joined to the unknown face the write admits nothing: `o` keeps its
/// declared `Object?`.
#[test]
fn unordered_arms_join_to_the_unknown_face_not_the_else_arm() {
    // ids: C1, N, Object — edges C1 <: Object, N <: Object, unordered arms.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 2), (1, 2)],
    };
    let source =
        "void f(bool b) {\n  Object? o;\n  o = b ? C1() : N();\n  o;\n}\nclass C1 {}\nclass N {}\n";
    let (ids, facts) = analyze(source, &["C1", "N", "Object"], Some(&ops));
    let object = ids[2];
    assert_eq!(
        ty_of_last_read(source, &facts, "o"),
        Some(Ty {
            base: Base::Name(object),
            nullable: true,
        }),
        "the LUB names no winner at name level: `o` must keep `Object?`, \
         never promote toward the else arm's `N`"
    );
}

/// The else-subtype-of-then face — the sharpest proof the join is not
/// else-only: `B <: A`, so `LUB(A?, B?)` is `A?` (the THEN arm). The
/// pre-fix push answered `B?` and the inferred local adopted it.
#[test]
fn else_subtype_of_then_yields_the_then_arm() {
    // ids: A, B — edge B <: A.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(1, 0)],
    };
    let source = "void f(bool b, A? a, B? c2) {\n  var x = b ? a : c2;\n  x;\n}\nclass A {}\nclass B extends A {}\n";
    let (ids, facts) = analyze(source, &["A", "B"], Some(&ops));
    let a = ids[0];
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty {
            base: Base::Name(a),
            nullable: true,
        }),
        "`LUB(A?, B?)` is `A?` — the else arm's `B?` alone was the bug"
    );
}

/// The then-subtype-of-else contract face: `LUB(B?, A?)` is `A?` — the
/// ELSE arm, so the join lands on it through the subtype order rather than
/// by position. (The pre-fix code answered the same `A?` by accident.)
#[test]
fn then_subtype_of_else_yields_the_else_arm() {
    // ids: A, B — edge B <: A.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(1, 0)],
    };
    let source = "void f(bool b, A? a, B? c2) {\n  var x = b ? c2 : a;\n  x;\n}\nclass A {}\nclass B extends A {}\n";
    let (ids, facts) = analyze(source, &["A", "B"], Some(&ops));
    let a = ids[0];
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty {
            base: Base::Name(a),
            nullable: true,
        }),
        "`LUB(B?, A?)` is `A?`"
    );
}

/// Two identical arms: `LUB(int?, int?)` is `int?` — the same-type face
/// must keep the spelled type (and its adoption path), not fall to the
/// unknown face.
#[test]
fn identical_arms_yield_the_arm_type() {
    let source = "void f(bool b, int? a, int? c2) {\n  var x = b ? a : c2;\n  x;\n}\n";
    let (ids, facts) = analyze(source, &["int"], None);
    let int = ids[0];
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty {
            base: Base::Name(int),
            nullable: true,
        }),
        "`LUB(int?, int?)` is `int?`"
    );
}

/// The dynamic arm absorbs the join (`LUB(dynamic, String?)` = `dynamic`):
/// the official's top-type rule. At name level the pushed face is
/// `dynamic`, which the inferred-local adoption correctly leaves as the
/// unspelled unknown marker — the pre-fix push adopted the else arm's
/// `String?` outright. `x is Never` deads the then block on either face:
/// the bottom tested type folds before the operand's type is read (#3610).
#[test]
fn dynamic_arm_absorbs_the_join() {
    let source = "void f(bool b, dynamic d, String? s) {\n  var x = b ? d : s;\n  if (x is Never) {\n    x;\n  }\n  x;\n}\n";
    let (_ids, facts) = analyze(source, &["String"], None);
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty::UNKNOWN),
        "`LUB(dynamic, String?)` is `dynamic` — the unspelled unknown \
         marker, never the else arm's `String?`"
    );
    // Upstream folds `is Never` before reading the operand (flow_analysis.dart:5944-5949).
    assert_eq!(
        dead_of(source, &facts),
        vec!["{\n    x;\n  }".to_string()],
        "`x is Never` folds to false on the dynamic face too — the bottom \
         tested type decides before the operand"
    );
}

/// Both-arms-subtypes-of-Object through a nested conditional: the inner
/// `d ? cc : c2` joins `LUB(C?, B?)` = `C?` (B <: C), and the outer join
/// `LUB(A?, C?)` = `C?` (A <: C) — every arm contributes through the
/// lattice, so the adopted face is the inner arm's THEN side `C?`. The
/// pre-fix walk pushed the inner else (`B?`) twice over.
#[test]
fn nested_conditional_joins_per_level() {
    // ids: A, B, C — edges A <: C, B <: C.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 2), (1, 2)],
    };
    let source = "void f(bool b, bool d, A? a, B? c2, C? cc) {\n  var x = b ? a : (d ? cc : c2);\n  x;\n}\nclass A {}\nclass B {}\nclass C {}\n";
    let (ids, facts) = analyze(source, &["A", "B", "C"], Some(&ops));
    let c = ids[2];
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty {
            base: Base::Name(c),
            nullable: true,
        }),
        "inner `LUB(C?, B?)` = `C?`, outer `LUB(A?, C?)` = `C?`"
    );
}

/// The then arm's nullability joins through `with_join`
/// (`least_upper_bound` over the nullability-aware lattice): `LUB(A, A?)`
/// is `A?`, and the adopted face keeps the suffix.
#[test]
fn nullable_arm_widens_the_join() {
    // ids: A — no edges needed beyond reflexivity.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[],
    };
    let source = "void f(bool b, A a, A? c2) {\n  var x = b ? a : c2;\n  x;\n}\nclass A {}\n";
    let (ids, facts) = analyze(source, &["A"], Some(&ops));
    let a = ids[0];
    assert_eq!(
        ty_of_last_read(source, &facts, "x"),
        Some(Ty {
            base: Base::Name(a),
            nullable: true,
        }),
        "`LUB(A, A?)` is `A?`"
    );
}
