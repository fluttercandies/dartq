//! The #2943 pins: the leaf literals' static types as the initializer
//! write's `matchedType` (the #2940 `Ev::Bool` face extended to `int`,
//! `double`, `String`, and the collection heads `List`/`Set`/`Map`).
//! Upstream `literalInt`/`literalDouble`/`literalString` record the real
//! static type; `resolveListLiteral`/`resolveSetOrMapLiteral`'s
//! written-arguments arm spells `List<E>`/`Set<E>`/`Map<K,V>` (upstream
//! Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.dart:
//! 68-107, 732-758). That type is what `PromotionModel.write`'s
//! `_tryPromoteToTypeOfInterest` consumes (`flow_analysis.dart:
//! 4947-5062`): `int <: num` makes `num` a type of interest on
//! `num? n = 1`, `String <: Object` makes `Object` one on
//! `Object? s = 'x'`, and `List<int>` is the written face the
//! collection-literal initializer promotes through. Without the spelled
//! type the opaque unknown face failed every `written <: candidate`
//! question and the read kept the nullable declared face — the
//! under-report.
//!
//! The ops stand-in below carries the hierarchy facts the spelling alone
//! cannot (`int <: num`, `double <: num`, `String <: Object`) — the
//! resolved seam's answer. Everything else delegates to
//! [`crate::ty::NameTypeOps`].

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// A name-level ops with the three subtype edges the pin shapes need —
/// the real element hierarchy's answers (`int <: num`, `double <: num`,
/// `String <: Object`), keyed on the run's own interned ids. Every other
/// question delegates to [`crate::ty::NameTypeOps`].
struct LiteralOps {
    ids: std::cell::RefCell<Vec<u32>>,
    /// `(sub_index, super_index)` pairs into `ids`.
    edges: &'static [(usize, usize)],
}

impl LiteralOps {
    fn seed(&self, ids: Vec<u32>) {
        *self.ids.borrow_mut() = ids;
    }
}

impl TypeOps for LiteralOps {
    fn is_subtype(&self, left: Ty, right: Ty) -> bool {
        if let (Base::Name(l), Base::Name(r)) = (left.base, right.base) {
            let ids = self.ids.borrow();
            let hit = self
                .edges
                .iter()
                .any(|(sub, sup)| ids.get(*sub) == Some(&l) && ids.get(*sup) == Some(&r));
            if hit {
                return !left.nullable || right.nullable;
            }
        }
        crate::ty::NameTypeOps.is_subtype(left, right)
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

    fn classify(&self, t: Ty) -> crate::ty::Classification {
        crate::ty::NameTypeOps.classify(t)
    }

    fn is_bottom(&self, t: Ty) -> bool {
        crate::ty::NameTypeOps.is_bottom(t)
    }
}

/// Scans and parses `source` with the recorder attached. `intern` names
/// are interned on the recorder's own table before parsing; `ops`, when
/// given, is seeded with the interned ids and mounted as the replay's
/// back-end (None = the name-level default). Returns the interned ids,
/// the flow (for `type_args_at` reads), and the taken facts.
fn analyze<'t>(
    source: &'static str,
    intern: &[&str],
    ops: Option<&'t LiteralOps>,
) -> (Vec<u32>, SharedFlow<'t>, Vec<FlowFact>) {
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
    (ids, flow, facts)
}

/// `(snippet, ty, args)` of every `ReadType`/`PromotedRead` row.
fn reads<'a>(
    source: &'a str,
    facts: &[FlowFact],
) -> (Vec<(&'a str, Ty, u32)>, Vec<(&'a str, Ty, u32)>) {
    let at = |span: dartq_source::SourceRange| {
        &source[span.offset as usize..(span.offset + span.length) as usize]
    };
    let mut read = Vec::new();
    let mut promoted = Vec::new();
    for fact in facts {
        match fact {
            FlowFact::ReadType { span, ty, args } => read.push((at(span.range), *ty, *args)),
            FlowFact::PromotedRead { span, ty, args } => {
                promoted.push((at(span.range), *ty, *args))
            }
            _ => {}
        }
    }
    (read, promoted)
}

/// `num? n = 1` — `literalInt`'s recorded `int` is the `matchedType`;
/// `int <: num` makes the declared non-null `num` a type of interest the
/// write promotes to (official `_tryPromoteToTypeOfInterest`,
/// flow_analysis.dart:4971-4999 — `int <: num`, `num <: num?`, and
/// `num? <: num` fails). Without the spelled `int` the unknown face
/// answered no subtype question and `n` read `num?`.
#[test]
fn int_literal_initializer_promotes_to_declared_non_null() {
    let source = "void f() {\n  num? n = 1;\n  n;\n}";
    let ops = LiteralOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1)], // int <: num
    };
    let (ids, _flow, facts) = analyze(source, &["int", "num"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let n_read = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "n")
        .expect("the `n` read carries a row");
    assert_eq!(
        n_read.1,
        Ty {
            base: Base::Name(ids[1]),
            nullable: false,
        },
        "`num? n = 1` promotes `n` to `num`: {read:?}"
    );
    assert!(
        promoted.iter().any(|(snippet, ty, _)| *snippet == "n"
            && ty.base == Base::Name(ids[1])
            && !ty.nullable),
        "the promotion row stands on `n`: {promoted:?}"
    );
}

/// `num? d = 1.5` — `literalDouble`'s `double` twin of the `int` arm
/// (`double <: num` promotes `d` to `num`).
#[test]
fn double_literal_initializer_promotes_to_declared_non_null() {
    let source = "void f() {\n  num? d = 1.5;\n  d;\n}";
    let ops = LiteralOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1)], // double <: num
    };
    let (ids, _flow, facts) = analyze(source, &["double", "num"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let d_read = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "d")
        .expect("the `d` read carries a row");
    assert_eq!(
        d_read.1,
        Ty {
            base: Base::Name(ids[1]),
            nullable: false,
        },
        "`num? d = 1.5` promotes `d` to `num`: {read:?}"
    );
    assert!(
        promoted.iter().any(|(snippet, ty, _)| *snippet == "d"
            && ty.base == Base::Name(ids[1])
            && !ty.nullable),
        "the promotion row stands on `d`: {promoted:?}"
    );
}

/// `Object? s = 'x'` — `literalString`'s recorded `String` is the
/// `matchedType`; `String <: Object` makes `Object` the write's
/// promotion step (the `Object? o = true` repro's string arm).
#[test]
fn string_literal_initializer_promotes_to_declared_non_null() {
    let source = "void f() {\n  Object? s = 'x';\n  s;\n}";
    let ops = LiteralOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1)], // String <: Object
    };
    let (ids, _flow, facts) = analyze(source, &["String", "Object"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let s_read = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "s")
        .expect("the `s` read carries a row");
    assert_eq!(
        s_read.1,
        Ty {
            base: Base::Name(ids[1]),
            nullable: false,
        },
        "`Object? s = 'x'` promotes `s` to `Object`: {read:?}"
    );
    assert!(
        promoted.iter().any(|(snippet, ty, _)| *snippet == "s"
            && ty.base == Base::Name(ids[1])
            && !ty.nullable),
        "the promotion row stands on `s`: {promoted:?}"
    );
}

/// `List<num>? l = <int>[]` — the literal's written `<int>` instantiates
/// its static type `List<int>` (upstream `resolveListLiteral`'s
/// written-arguments arm, typed_literal_resolver.dart:75-107). That
/// `matchedType` is what the write's `_tryPromoteToTypeOfInterest` gate
/// consumes: `List<int> <: List<num>` is a real-hierarchy subtype
/// relation the name-level lattice cannot decide (covariant-argument
/// name equality fails on `int`/`num`), so the collection literal's
/// bridge admits the declared non-null `List<num>` — the same arm the
/// `Object? o = <num>[]` pin rides. The promoted read therefore carries
/// `List<num>` beside its written `[num]` argument tree: the literal's
/// `List<int>` face is what made the promotion possible — the opaque
/// unknown face could not spell it.
#[test]
fn typed_list_literal_promotes_through_the_instantiated_face() {
    let source = "void f() {\n  List<num>? l = <int>[];\n  l;\n}";
    let (ids, flow, facts) = analyze(source, &["List", "num", "int"], None);
    let (read, promoted) = reads(source, &facts);
    let l_read = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "l")
        .expect("the `l` read carries a row");
    assert_eq!(
        l_read.1,
        Ty {
            base: Base::Name(ids[0]),
            nullable: false,
        },
        "`List<num>? l = <int>[]` promotes `l` to `List<num>`: {read:?}"
    );
    // The promoted row's argument tree is the declared `[num]` list —
    // one `ArgTy` naming `num` (the promoted candidate's own spelling).
    let flow = flow.0.borrow();
    let num_arg = flow
        .type_args_at(l_read.2)
        .iter()
        .any(|entry| entry.base == Base::Name(ids[1]));
    assert!(
        num_arg,
        "the promoted row carries the `List<num>` argument list: {read:?}"
    );
    assert!(
        promoted.iter().any(|(snippet, ty, args)| *snippet == "l"
            && ty.base == Base::Name(ids[0])
            && !ty.nullable
            && flow
                .type_args_at(*args)
                .iter()
                .any(|entry| entry.base == Base::Name(ids[1]))),
        "the `List<num>` promotion row stands on `l`: {promoted:?}"
    );
}

/// A non-literal right-hand side spells no `matchedType` — `num? m =
/// o.x`'s unknown written face keeps `m` at the declared `num?`
/// (upstream `_tryPromoteToTypeOfInterest`'s gate needs a real type;
/// the `literal_rhs` bridge applies only to a literal's own value). A
/// member read carries no flow-visible static type, so the write moves
/// no promotion.
#[test]
fn non_literal_rhs_keeps_the_declared_face() {
    let source = "void f(Object o) {\n  num? m = o.x;\n  m;\n}";
    let ops = LiteralOps {
        ids: std::cell::RefCell::new(Vec::new()),
        edges: &[(0, 1)], // int <: num — even with the edge, no literal
    };
    let (ids, _flow, facts) = analyze(source, &["int", "num"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let m_read = read
        .iter()
        .rev()
        .find(|(snippet, _, _)| *snippet == "m")
        .expect("the `m` read carries a row");
    assert_eq!(
        m_read.1,
        Ty {
            base: Base::Name(ids[1]),
            nullable: true,
        },
        "`num? m = g()` keeps `m` at `num?`: {read:?}"
    );
    assert!(
        promoted.iter().all(|(snippet, _, _)| *snippet != "m"),
        "no promotion row stands on `m`: {promoted:?}"
    );
}
