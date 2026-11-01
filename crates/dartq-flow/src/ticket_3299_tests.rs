//! The #3299 pins: a callable-typed local/parameter invocation adopts the
//! semantically staged return type.
//!
//! [`Base::Function`] spells the head only — the signature is the semantic
//! layer's answer — so a bare `nf()` over a `Null Function()` parameter used
//! to keep the function type as the send's static type. The equality-with-
//! `null` constant fold reads that type, which inverted the verdict
//! (`null == nf()` folded `false`, reporting the then branch where the
//! oracle reports the else) or dropped it entirely (`v == nf()` over a
//! non-nullable `v` folded nothing). The semantic layer resolves the shape's
//! `return_ty` and stages it; the flow gate must therefore accept the staged
//! row for the signature-unknown function face exactly as it does for the
//! unknown-read callees.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Parses `source` through a flow listener, optionally staging a `Null`
/// send result at `stage_return_at` (the callee identifier's offset).
fn facts_of(source: &str, stage_return_at: Option<usize>) -> Vec<FlowFact> {
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
        recorder.set_sound_flow(features.sound_flow_analysis);
        if let Some(offset) = stage_return_at {
            recorder.stage_call_result(offset as u32, Ty::NULL, Vec::new());
        }
    }
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// The `dead_code` row texts of `source`.
fn dead_of(source: &str, facts: &[FlowFact]) -> Vec<String> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source[span.range.offset as usize
                    ..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// The read-type fact recorded at `offset`.
fn read_ty_at(facts: &[FlowFact], offset: usize) -> Option<Ty> {
    facts.iter().find_map(|fact| match fact {
        FlowFact::ReadType { span, ty, .. } if span.range.offset as usize == offset => Some(*ty),
        _ => None,
    })
}

const NE_NULL: &str = "void f(Null Function() nf) {\n  if (null == nf()) {\n    print(1);\n  } else {\n    print(2);\n  }\n}\n";
const EQ_NULL: &str = "void f(int v, Null Function() nf) {\n  if (v == nf()) {\n    print(1);\n  } else {\n    print(2);\n  }\n}\n";

#[test]
fn callable_parameter_reads_as_the_signature_unknown_function_face() {
    let at = NE_NULL.find("nf()").expect("callee offset");
    let facts = facts_of(NE_NULL, None);
    assert_eq!(
        read_ty_at(&facts, at),
        Some(Ty {
            base: Base::Function,
            nullable: false,
        }),
        "the callee read is the function face the gate keys on"
    );
}

#[test]
fn null_eq_call_over_a_null_return_folds_true_with_the_staged_return() {
    // The condition is always true (`null == null`), so the ELSE branch is
    // the dead one.
    let at = NE_NULL.find("nf()").expect("callee offset");
    let staged = dead_of(NE_NULL, &facts_of(NE_NULL, Some(at)));
    assert_eq!(staged.len(), 1, "{staged:?}");
    assert!(staged[0].contains("print(2)"), "{staged:?}");
}

#[test]
fn null_eq_call_without_the_staged_return_keeps_the_pre_fix_fold() {
    // Control: the function-type read folds the comparison `false`, so the
    // gate (not the lattice) is what the staged adoption changes.
    let unstaged = dead_of(NE_NULL, &facts_of(NE_NULL, None));
    assert_eq!(unstaged.len(), 1, "{unstaged:?}");
    assert!(unstaged[0].contains("print(1)"), "{unstaged:?}");
}

#[test]
fn non_nullable_eq_call_folds_false_with_the_staged_return() {
    // `int == Null` is always false: the THEN branch is the dead one, and
    // without the staged return no row is produced at all (the function
    // type is non-nullable too, so the fold has nothing to decide).
    let at = EQ_NULL.find("nf()").expect("callee offset");
    assert!(dead_of(EQ_NULL, &facts_of(EQ_NULL, None)).is_empty());
    let staged = dead_of(EQ_NULL, &facts_of(EQ_NULL, Some(at)));
    assert_eq!(staged.len(), 1, "{staged:?}");
    assert!(staged[0].contains("print(1)"), "{staged:?}");
}
