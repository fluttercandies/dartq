//! #2507 — the map/where chain's element-nullability contract at the flow
//! layer, pinned for the analyzer-corpus
//! `function_expression_resolver.dart` rows (`parameterFragments.nonNulls`
//! then `.map((fragment) => fragment.element).where(...)` at 136:39/153:39).
//!
//! The element face a chain's higher-order closures see is the semantic
//! layer's inference answer (the `Iterable.map` context schema), staged for
//! each closure formal at its name token exactly the way the stager does
//! here. What this crate owns is the carriage: the formal adopts the staged
//! face at the closure entry and every read inside the closure body keeps
//! it — the chain's member/send sequence and the nested executables may
//! neither drop a `?` (the oracle's `unchecked_use_of_nullable_value` feed
//! dies and a real row goes missing) nor keep one the getter stripped
//! (a spurious unchecked row at the post-`nonNulls` sites, where
//! `dart analyze` reports none — the 3.13.7 tree carries the identical
//! code warning-clean and the corpus oracle key has no row at either
//! anchor).
//!
//! Two faces pinned:
//!
//! * A nullable element (`Iterable<T?>`'s `T?`) stays nullable at the
//!   closure-body read through the `.map(...).where(...)` chain.
//! * The `nonNulls`-stripped element stays non-null — the corpus posture
//!   that keeps 136:39/153:39 silent.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Read-type facts of `source` with `staged` inferred-local faces pre-seeded
/// — `(name-token offset, head lexeme, nullable)` per closure formal — as
/// `(read offset, flow type)` pairs beside the interned ids of every
/// `wanted` lexeme (the name-level lattice compares bases by id, so the
/// asserts read ids from the same table the replay interned).
fn read_types_of(
    source: &str,
    staged: &[(u32, &str, bool)],
    wanted: &[&str],
) -> (Vec<(u32, Ty)>, Vec<u32>) {
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
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut ids = Vec::with_capacity(wanted.len());
    for &(offset, lexeme, nullable) in staged {
        let base = flow.0.borrow_mut().intern_name(lexeme);
        flow.0.borrow_mut().stage_inferred_local(
            offset,
            Ty {
                base: Base::Name(base),
                nullable,
            },
            Vec::new(),
        );
    }
    for lexeme in wanted {
        ids.push(flow.0.borrow_mut().intern_name(lexeme));
    }
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    let rows = flow
        .0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect();
    (rows, ids)
}

/// The read-type row at `needle`'s offset (ASCII sources: byte offset ==
/// UTF-16 offset).
fn read_ty_at(
    source: &str,
    staged: &[(u32, &str, bool)],
    wanted: &[&str],
    needle: &str,
) -> (Ty, Vec<u32>) {
    let at = source.find(needle).expect("needle present") as u32;
    let (rows, ids) = read_types_of(source, staged, wanted);
    (
        rows.into_iter()
            .find(|(offset, _)| *offset == at)
            .map(|(_, ty)| ty)
            .unwrap_or_else(|| panic!("no ReadType row at {needle:?} in {source:?}")),
        ids,
    )
}

#[test]
fn map_where_chain_keeps_the_nullable_element() {
    let source = "void f(Iterable<L?> p) {\n  p.map((e) => e.field).where((x) => x != null);\n}\n";
    // The semantic element inference staged at each formal's name token:
    // `e` is `Iterable<L?>`'s element (`L?`), `x` is the map result's
    // element (`M`, non-null — the closure body's own return face).
    let e_formal = source.find("(e) =>").unwrap() as u32 + 1;
    let x_formal = source.find("(x) =>").unwrap() as u32 + 1;
    let staged = vec![(e_formal, "L", true), (x_formal, "M", false)];
    let (e_read, ids) = read_ty_at(source, &staged, &["L"], "e.field");
    assert_eq!(
        e_read.base,
        Base::Name(ids[0]),
        "the chain keeps the element face at the closure-body read: {e_read:?}"
    );
    assert!(
        e_read.nullable,
        "the chain must keep the element's `?` — dropping it kills the oracle's unchecked-use feed: {e_read:?}"
    );
    let (x_read, _) = read_ty_at(source, &staged, &[], "x != null");
    assert!(
        !x_read.nullable,
        "the second link's own staged element stays non-null: {x_read:?}"
    );
}

#[test]
fn nonnulls_chain_keeps_the_stripped_element() {
    // The function_expression_resolver.dart 136:39/153:39 posture: after
    // `NullableIterableExtensions.nonNulls` the element is `L` (the `?` is
    // stripped), so the closure-body read stays non-null and no
    // unchecked-access row is correct there.
    let source =
        "void g(List<L?> ls) {\n  ls.nonNulls.map((e) => e.field).where((x) => x != null);\n}\n";
    let e_formal = source.find("(e) =>").unwrap() as u32 + 1;
    let x_formal = source.find("(x) =>").unwrap() as u32 + 1;
    let staged = vec![(e_formal, "L", false), (x_formal, "M", false)];
    let (e_read, ids) = read_ty_at(source, &staged, &["L"], "e.field");
    assert_eq!(
        e_read.base,
        Base::Name(ids[0]),
        "the post-nonNulls element face survives the chain: {e_read:?}"
    );
    assert!(
        !e_read.nullable,
        "the nonNulls-stripped element must stay non-null through the chain (the corpus rows stay silent): {e_read:?}"
    );
    let (x_read, _) = read_ty_at(source, &staged, &[], "x != null");
    assert!(!x_read.nullable, "{x_read:?}");
}
