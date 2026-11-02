//! The #3602 pins: `UP(Null, T) = T?` in [`Interp::name_lub`] — a `?.`
//! selector over a `Null | T?` conditional receiver keeps its access path
//! live.
//!
//! Census: ~65 rows across `tests/language/constructor/
//! explicit_instantiation_syntax_test_*.dart` reported a spurious
//! `WARNING|dead_code` at the `?.toString()` call of
//! `oneArg((boolVar ? null : C<int, int>) ?. toString());`. The receiver's
//! static type is `C<int,int>?` — `LUB(Null, C<int,int>)` made nullable —
//! so the official `_nullAwareAccess_rightBegin` lands on
//! `TypeClassification.potentiallyNullable` and keeps BOTH the access path
//! and the skip path reachable (flow_analysis.dart:7700-7702). The
//! name-layer `name_lub` answered `Null`: the `Base::Unknown` early-drop
//! (`LUB(InvalidType, X) = X`) ran before the `UP(Null, T)` arms, so
//! `LUB(Null, Unknown?)` returned the spelled `Null` side, the `?.`
//! classified its receiver `nullOrEquivalent`, and the member poll went
//! dead.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

fn facts_of(source: &str) -> Vec<FlowFact> {
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
    flow.0.borrow_mut().take_facts()
}

/// Offsets of dead-code facts, rendered as the dead source text for
/// readable assertions.
fn dead_snippets(source: &str) -> Vec<String> {
    facts_of(source)
        .into_iter()
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

/// The census shape: a literal-folded `bool` condition over `null` and an
/// unspellable `T` (a type-literal arm feeds the opaque face), then a `?.`
/// selector on the joined value. The oracle keeps the member call live.
#[test]
fn null_arm_conditional_null_aware_call_stays_live() {
    let source = "bool boolVar = true;\n\
                  class C<X, Y> {}\n\
                  void oneArg(_) {}\n\
                  void f() {\n\
                    oneArg((boolVar ? null : C<int, int>) ?. toString());\n\
                  }\n";
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// The same join feeding a non-literal condition — the `?.` over `Null | T?`
/// stays live however the conditional split falls.
#[test]
fn opaque_conditional_null_aware_call_stays_live() {
    let source = "class C<X, Y> {}\n\
                  void oneArg(_) {}\n\
                  void f(bool c) {\n\
                    oneArg((c ? null : C<int, int>) ?. toString());\n\
                  }\n";
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}

/// A `??` over the same joined value: `LUB(Null, T?)` is `T?`, so the
/// right operand is not pre-deaded by a `Null` receiver either — the
/// null-aware chain reads the nullable join.
#[test]
fn null_arm_conditional_then_if_null_stays_live() {
    let source = "class C<X, Y> {}\n\
                  void oneArg(_) {}\n\
                  void f(bool c) {\n\
                    oneArg((c ? null : C<int, int>) ?? 0);\n\
                  }\n";
    assert_eq!(dead_snippets(source), Vec::<String>::new());
}
