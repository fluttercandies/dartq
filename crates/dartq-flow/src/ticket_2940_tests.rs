//! The #2940 pins: the declaration-initializer's type-of-interest
//! promotion (`boolean_conditional_evaluation_A02_t02`,
//! `Object? o = true; x = o` — the oracle's `invalid_assignment` spells
//! the RHS's promoted `Object`, dartq spelled `Object?`). Two deltas at
//! the initializer's `apply_write`:
//!
//! * The boolean literal carries its real static type `bool` (official
//!   `literalBool`'s recorded type — the `matchedType` the write
//!   promotes through, `_initialize` → `PromotionModel.write` →
//!   `_tryPromoteToTypeOfInterest`, upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
//!   7546-7595, 3837-3903, 3982-4098): `bool <: Object` makes `Object`
//!   the write's promotion step over the `Object?` declared face.
//!   Without it the opaque unknown written type failed every
//!   `written <: candidate` question and the read kept `Object?`.
//! * The initializer's `promoteToTypeOfInterest` is the official's
//!   `!isImplicitlyTyped && !isFinal` (flow_analysis.dart:7583), not a
//!   blanket `true`: a `final`/`const` local's initializer never
//!   promotes (`final Object? o = true` stays `Object?` upstream), and
//!   an implicitly typed declarator's slot is already the inferred type.
//!   `rhs.literal` also rides in, so a collection-literal initializer
//!   takes the same written-face bridge the plain-assign write has
//!   (`Object? o = <num>[]` promotes `o` to `Object`).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// A name-level ops with one hierarchy fact the spelling alone cannot
/// carry: `bool <: Object` (the resolved seam's answer — the oracle's
/// subtype oracle is the real element hierarchy). Every other question
/// delegates to [`crate::ty::NameTypeOps`].
struct BoolToObjectOps {
    bool_id: std::cell::Cell<u32>,
    object_id: std::cell::Cell<u32>,
}

impl BoolToObjectOps {
    /// The ids come from the run's own interner — seeded after the
    /// recorder interns `bool`/`Object`, before `set_ops` mounts this.
    fn seed(&self, bool_id: u32, object_id: u32) {
        self.bool_id.set(bool_id);
        self.object_id.set(object_id);
    }
}

impl TypeOps for BoolToObjectOps {
    fn is_subtype(&self, left: crate::ty::Ty, right: crate::ty::Ty) -> bool {
        if left.base == Base::Name(self.bool_id.get())
            && right.base == Base::Name(self.object_id.get())
        {
            return true;
        }
        crate::ty::NameTypeOps.is_subtype(left, right)
    }

    fn factor(&self, from: crate::ty::Ty, what: crate::ty::Ty) -> crate::ty::Ty {
        crate::ty::NameTypeOps.factor(from, what)
    }

    fn try_promote_to(&self, to: crate::ty::Ty, from: crate::ty::Ty) -> Option<crate::ty::Ty> {
        crate::ty::NameTypeOps.try_promote_to(to, from)
    }

    fn promote_to_non_null(&self, t: crate::ty::Ty) -> crate::ty::Ty {
        crate::ty::NameTypeOps.promote_to_non_null(t)
    }

    fn classify(&self, t: crate::ty::Ty) -> crate::ty::Classification {
        crate::ty::NameTypeOps.classify(t)
    }

    fn is_bottom(&self, t: crate::ty::Ty) -> bool {
        crate::ty::NameTypeOps.is_bottom(t)
    }
}

/// Scans and parses `source` with the recorder attached. `intern` names
/// are interned on the recorder's own table before parsing; `ops`, when
/// given, is seeded with `intern`'s first two ids (`bool`, `Object`)
/// and mounted as the replay's back-end (None = the name-level default).
/// Returns the interned ids and the taken facts.
fn analyze<'t>(
    source: &'static str,
    intern: &[&str],
    ops: Option<&'t BoolToObjectOps>,
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
            ops.seed(ids[0], ids[1]);
            recorder.set_ops(ops);
        }
        ids
    };
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    (ids, flow.0.borrow_mut().take_facts())
}

/// `(snippet, ty)` of every `ReadType`/`PromotedRead` row.
fn reads<'a>(
    source: &'a str,
    facts: &[FlowFact],
) -> (Vec<(&'a str, crate::ty::Ty)>, Vec<(&'a str, crate::ty::Ty)>) {
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

/// The headline repro (`boolean_conditional_evaluation_A02_t02`):
/// `Object? o = true; x = o` — the initializer write's `matchedType` is
/// `bool`, and `bool <: Object` makes `Object` a type of interest the
/// write promotes to (official `_tryPromoteToTypeOfInterest`,
/// flow_analysis.dart:4039-4049: the declared non-null form is the first
/// candidate). The `o` read at `x = o` therefore carries `Object` — the
/// oracle's `invalid_assignment` source-type spelling.
#[test]
fn bool_literal_initializer_promotes_to_declared_non_null() {
    let source = "void f() {\n  Object? o = true;\n  bool x = false;\n  x = o;\n}";
    let ops = BoolToObjectOps {
        bool_id: std::cell::Cell::new(0),
        object_id: std::cell::Cell::new(0),
    };
    let (ids, facts) = analyze(source, &["bool", "Object"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let o_read = read
        .iter()
        .rev()
        .find(|(snippet, _)| *snippet == "o")
        .expect("the `x = o` read carries a row");
    assert_eq!(
        o_read.1,
        crate::ty::Ty {
            base: Base::Name(ids[1]),
            nullable: false,
        },
        "the `x = o` read carries the promoted `Object`: {read:?}"
    );
    assert!(
        promoted
            .iter()
            .any(|(snippet, ty)| *snippet == "o" && ty.base == Base::Name(ids[1]) && !ty.nullable),
        "the promotion row stands on `o`: {promoted:?}"
    );
}

/// A plain `=` write of `true` promotes the same way — the official
/// write's `promoteToTypeOfInterest` is unconditional for assignments
/// (`flow.write`, flow_analysis.dart:4802+): `Object? o; o = true` still
/// promotes `o` to `Object`.
#[test]
fn bool_literal_assignment_promotes_to_declared_non_null() {
    let source = "void f() {\n  Object? o;\n  o = true;\n  bool x = false;\n  x = o;\n}";
    let ops = BoolToObjectOps {
        bool_id: std::cell::Cell::new(0),
        object_id: std::cell::Cell::new(0),
    };
    let (ids, facts) = analyze(source, &["bool", "Object"], Some(&ops));
    let (read, _) = reads(source, &facts);
    let o_read = read
        .iter()
        .rev()
        .find(|(snippet, _)| *snippet == "o")
        .expect("the `x = o` read carries a row");
    assert!(
        !o_read.1.nullable && o_read.1.base == Base::Name(ids[1]),
        "the `x = o` read carries `Object`: {read:?}"
    );
}

/// The `isFinal` arm of the official gate (`promoteToTypeOfInterest:
/// !isImplicitlyTyped && !isFinal`, flow_analysis.dart:7583): a `final`
/// local's initializer never promotes — `final Object? o = true` stays
/// `Object?`, and the `x = o` read keeps the nullable declared face.
/// Without the gate the bool literal's real type would over-promote
/// finals the oracle leaves alone.
#[test]
fn final_local_initializer_never_promotes() {
    let source = "void f() {\n  final Object? o = true;\n  bool x = false;\n  x = o;\n}";
    let ops = BoolToObjectOps {
        bool_id: std::cell::Cell::new(0),
        object_id: std::cell::Cell::new(0),
    };
    let (ids, facts) = analyze(source, &["bool", "Object"], Some(&ops));
    let (read, promoted) = reads(source, &facts);
    let o_read = read
        .iter()
        .rev()
        .find(|(snippet, _)| *snippet == "o")
        .expect("the `x = o` read carries a row");
    assert!(
        o_read.1.nullable && o_read.1.base == Base::Name(ids[1]),
        "the final local's read keeps `Object?`: {read:?}"
    );
    assert!(
        promoted.iter().all(|(snippet, _)| *snippet != "o"),
        "no promotion row stands on `o`: {promoted:?}"
    );
}

/// The same-type face at name level: `bool? b = true` promotes `b` to
/// `bool` — the declared non-null candidate's `type == writtenType` arm
/// (flow_analysis.dart:4022-4025), decidable without a hierarchy.
#[test]
fn bool_literal_initializer_promotes_nullable_bool() {
    let source = "void f() {\n  bool? b = true;\n  b;\n}";
    let (_, facts) = analyze(source, &[], None);
    let (_, promoted) = reads(source, &facts);
    assert!(
        promoted.iter().any(|(snippet, _)| *snippet == "b"),
        "`bool? b = true` promotes `b` to `bool`: {promoted:?}"
    );
}

/// The collection-literal initializer rides the same `literal` bridge
/// the plain-assign write has (`rhs.literal` at `InitializerEnd`):
/// `Object? o = <num>[]` promotes `o` to `Object` — the written value IS
/// a `List<num>` (a proper subtype of the declared non-null form), which
/// the opaque unknown face admits through the literal arm. Name-level
/// decides this one — no hierarchy needed.
#[test]
fn collection_literal_initializer_promotes_to_declared_non_null() {
    let source = "void f() {\n  Object? o = <num>[];\n  bool x = false;\n  x = o;\n}";
    let (ids, facts) = analyze(source, &["Object"], None);
    let (read, promoted) = reads(source, &facts);
    let o_read = read
        .iter()
        .rev()
        .find(|(snippet, _)| *snippet == "o")
        .expect("the `x = o` read carries a row");
    assert_eq!(
        o_read.1,
        crate::ty::Ty {
            base: Base::Name(ids[0]),
            nullable: false,
        },
        "the `x = o` read carries the promoted `Object`: {read:?}"
    );
    assert!(
        promoted
            .iter()
            .any(|(snippet, ty)| *snippet == "o" && !ty.nullable),
        "the promotion row stands on `o`: {promoted:?}"
    );
}

/// An implicitly typed declarator never takes the type-of-interest arm
/// (`isImplicitlyTyped`, flow_analysis.dart:7583): `var o = true`'s
/// declared slot is the inferred type itself — the write records no
/// promotion row (the slot the fold reads was the adoption's answer,
/// not a promotion). Under the bare pipeline the unknown slot stays the
/// conservative face; the semantic layer's staged inference resolves it.
#[test]
fn implicit_declarator_takes_no_interest_promotion() {
    let source = "void f() {\n  var o = true;\n  o;\n}";
    let (_, facts) = analyze(source, &[], None);
    let (read, promoted) = reads(source, &facts);
    let o_read = read
        .iter()
        .rev()
        .find(|(snippet, _)| *snippet == "o")
        .expect("the `o` read carries a row");
    // The slot stays the unknown face — non-nullable now that the RHS
    // spells `bool` (the literal can never be null, and the conservative
    // face no longer pretends it can be).
    assert_eq!(o_read.1.base, Base::Unknown, "{read:?}");
    assert!(!o_read.1.nullable, "{read:?}");
    assert!(
        promoted.iter().all(|(snippet, _)| *snippet != "o"),
        "no promotion row stands on `o`: {promoted:?}"
    );
}
