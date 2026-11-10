//! The #3397 pins: the promotion lattice's answered-at-the-read faces.
//!
//! Three faces the L-D-3 audit tracked, each pinned from the fact stream
//! (`FlowFact::ReadType` / `FlowFact::PromotedRead` /
//! `FlowFact::PromotedPropertyRead`):
//!
//! * **Initializer promotion** — `num? n = 1` promotes `n` to `num` at the
//!   reads that follow (official `_tryPromoteToTypeOfInterest` on the
//!   declaration's own initialize write,
//!   pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:7583,
//!   :3982-4098), and a later write demotes it again
//!   (`_demoteViaAssignment`, :4888-4928).
//! * **`T & B` intersection promotion** — `if (i is! int) throw;` over
//!   `T i` promotes the read to the tested type, the name-level spelling of
//!   the official `T & int` (`tryPromoteToType`'s `TypeParameterType` arm,
//!   pkg/analyzer/lib/src/dart/element/type_system.dart:1754-1768), gated
//!   by the declared bound: `T extends String` gains nothing from `is int`.
//! * **Private-final field promotion** — `if (_foo != null)` promotes the
//!   `(this, _foo)` property slot, so the following read emits the
//!   property-promotion fact (upstream `_equalityCheck`'s
//!   `_EqualityCheckIsNullCheck` → `tryMarkNonNullable`,
//!   flow_analysis.dart:4046-4076) — the flow half of
//!   `field_promotion_test.dart`'s `..._simpleIdentifier_nullability`.
//!
//! The first two need a back-end that answers the real hierarchy question
//! `int <: num` (the name-level default refuses cross-name subtyping) and
//! the strict promotion gate the resolved seam applies
//! (`crate::ty::TypeOps::try_promote_to_typed`), so the pins run a small
//! closure-of-edges stand-in for it.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{ArgTy, Base, Classification, EMPTY_ARGS, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// A name-level back-end with the two hierarchy answers the spelling alone
/// cannot carry: the `sub → super` subtype closure (`int <: num` and the
/// `Object` top) and the strict promotion gate the resolved seam applies.
/// Everything else delegates to [`crate::ty::NameTypeOps`].
struct HierarchyOps {
    /// Interned name ids, seeded after the recorder interns them (before
    /// `set_ops` mounts this): the edge pairs index into this list.
    ids: std::cell::RefCell<Vec<u32>>,
    /// `(sub_index, super_index)` pairs into `ids` — one row per direct
    /// `extends`-style edge; the transitive closure answers the subtype
    /// question. Reflexive by construction.
    edges: &'static [(usize, usize)],
}

impl HierarchyOps {
    fn seed(&self, ids: Vec<u32>) {
        *self.ids.borrow_mut() = ids;
    }

    fn direct_supers(&self, sub: u32) -> Vec<u32> {
        let ids = self.ids.borrow();
        self.edges
            .iter()
            .filter(|(s, _)| ids.get(*s) == Some(&sub))
            .filter_map(|(_, t)| ids.get(*t).copied())
            .collect()
    }

    /// Whether `sub` reaches `sup` along the edge closure (reflexive).
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
            queue.extend(self.direct_supers(cur));
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
            // `Null <: X` only when `X` admits null.
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

    /// The resolved seam's strict-subtype gate
    /// (`isValidPromotionStep`, flow_analysis.dart:5989-6007): `to <: from`
    /// and not the reverse. This is what refuses the intersection step
    /// (`int <: T` is false), leaving the type-parameter arm to answer it.
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

/// Scans and parses `source` with the recorder attached. `intern` names are
/// interned on the recorder's own table before parsing (so the fake
/// back-end can name them); `ops`, when given, is mounted as the replay's
/// back-end (`None` = the name-level default). Returns the interned ids and
/// the taken facts.
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
                &source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize],
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
                &source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize],
                *ty,
            )),
            _ => None,
        })
        .collect()
}

/// The `PromotedPropertyRead` rows of `source` as covered snippets.
fn promoted_property_reads<'a>(source: &'a str, facts: &[FlowFact]) -> Vec<&'a str> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::PromotedPropertyRead { span } => Some(
                &source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize],
            ),
            _ => None,
        })
        .collect()
}

// —— initializer promotion (P1) ————————————————————————————————————————

#[test]
fn initializer_promotion_reaches_the_read() {
    // `num? n = 1; n;` — the declaration's initialize write promotes `n`
    // to the declared non-null form (a type of interest: `int <: num`), so
    // the later read answers `num`, not `num?`.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f() {\n  num? n = 1;\n  n;\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object"], Some(&ops));
    let num = ids[1];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads,
        [(
            "n",
            Ty {
                base: Base::Name(num),
                nullable: false
            }
        )],
        "the read answers the promoted `num`, not `num?`: {reads:?}"
    );
    let promoted = promoted_reads(source, &facts);
    assert_eq!(
        promoted,
        [(
            "n",
            Ty {
                base: Base::Name(num),
                nullable: false
            }
        )],
        "the promotion fact rides the read: {promoted:?}"
    );
}

#[test]
fn later_write_demotes_the_initializer_promotion() {
    // The negative: the null write demotes, so the read falls back to the
    // declared `num?` — no promotion fact stands at it.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f() {\n  num? n = 1;\n  n = null;\n  n;\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object"], Some(&ops));
    let num = ids[1];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads,
        [(
            "n",
            Ty {
                base: Base::Name(num),
                nullable: true
            }
        )],
        "the post-write read answers the declared nullable face: {reads:?}"
    );
    assert!(
        promoted_reads(source, &facts).is_empty(),
        "the demoted read carries no promotion fact"
    );
}

#[test]
fn nullable_without_initializer_stays_declared() {
    // The negative: no initializer, no write — the read answers `num?`.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f() {\n  num? n;\n  n;\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object"], Some(&ops));
    let num = ids[1];
    let reads = read_types(source, &facts);
    assert_eq!(
        reads,
        [(
            "n",
            Ty {
                base: Base::Name(num),
                nullable: true
            }
        )],
        "an unwritten nullable local stays nullable: {reads:?}"
    );
    assert!(promoted_reads(source, &facts).is_empty());
}

// —— `T & B` intersection promotion (P2) ————————————————————————————————

#[test]
fn type_parameter_is_test_promotes_to_the_intersection() {
    // `void f<T>(T i) { if (i is! int) throw 'x'; i; }` — the throw arm
    // leaves only the matched path, and the matched read answers the
    // tested `int` (the spelling of `T & int`).
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2)],
    };
    let source = "void f<T>(T i) {\n  if (i is! int) throw 'x';\n  i;\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "T"], Some(&ops));
    let (int, t) = (ids[0], ids[3]);
    let reads = read_types(source, &facts);
    assert_eq!(
        reads,
        [
            (
                "i",
                Ty {
                    base: Base::Name(t),
                    nullable: false
                }
            ),
            (
                "i",
                Ty {
                    base: Base::Name(int),
                    nullable: false
                }
            ),
        ],
        "the guarded read answers `int`, the `T & int` head: {reads:?}"
    );
    assert_eq!(
        promoted_reads(source, &facts),
        [(
            "i",
            Ty {
                base: Base::Name(int),
                nullable: false
            }
        )]
    );
}

#[test]
fn type_parameter_bound_gates_the_intersection() {
    // The negative: `T extends String` gains nothing from `is int` — the
    // tested type sits outside the declared bound, so no promotion.
    let ops = HierarchyOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1), (1, 2), (3, 2)],
    };
    let source = "void f<T extends String>(T i) {\n  if (i is! int) throw 'x';\n  i;\n}";
    let (ids, facts) = analyze(source, &["int", "num", "Object", "String", "T"], Some(&ops));
    let (int, t) = (ids[0], ids[4]);
    let reads = read_types(source, &facts);
    assert_eq!(
        reads,
        [
            (
                "i",
                Ty {
                    base: Base::Name(t),
                    nullable: false
                }
            ),
            (
                "i",
                Ty {
                    base: Base::Name(t),
                    nullable: false
                }
            ),
        ],
        "an out-of-bound test promotes nothing: {reads:?}"
    );
    assert_ne!(reads[1].1.base, Base::Name(int));
    assert!(promoted_reads(source, &facts).is_empty());
}

// —— private-final field promotion (P3) ——————————————————————————————————

#[test]
fn private_final_field_null_check_promotes_the_property_read() {
    // The upstream repro
    // (`field_promotion_test.dart#..._simpleIdentifier_nullability`): the
    // implicit-`this` read of a private final field promotes its
    // `(this, _foo)` slot through the null check, so the invocation's read
    // carries the property-promotion fact.
    let source = "class C {\n\
                  \x20 final void Function()? _foo;\n\
                  \x20 C(this._foo);\n\
                  }\n\
                  class D extends C {\n\
                  \x20 D(super.value);\n\
                  \x20 void f() {\n\
                  \x20   if (_foo != null) {\n\
                  \x20     _foo();\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    let (_, facts) = analyze(source, &[], None);
    let promoted = promoted_property_reads(source, &facts);
    assert_eq!(
        promoted,
        ["_foo"],
        "the read inside the guard carries the promotion row: {promoted:?}"
    );
}
