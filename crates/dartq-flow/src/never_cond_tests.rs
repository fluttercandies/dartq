//! The `Never`-typed condition pins (boolean-conversion family): the
//! official `_ExpressionInfo` capture timing makes an identifier-rooted
//! bottom read's condition arms enter from the PRE-exit state, while a
//! top-level getter read or an invocation result records its type after
//! `handleExit` and keeps its arms dead.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text, in fact order.
fn dead_of(source: &str) -> Vec<String> {
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
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
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

/// A `Never`-typed **parameter** read as an `if` condition keeps both arms
/// live: oracle 3.13.7 reports nothing for
/// `void f(Never v) { if (v) { print(1); } print(2); }`.
#[test]
fn never_parameter_if_condition_stays_live() {
    let source = "void f(Never v) {\n  if (v) { print(1); }\n  print(2);\n}\n";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// The `while` twin of the parameter face — oracle silent.
#[test]
fn never_parameter_while_condition_stays_live() {
    let source = "void f(Never v) {\n  while (v) { print(1); }\n  print(2);\n}\n";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// An identifier-rooted `Never` getter condition (`a.np`, `this.np`, the
/// bare implicit-`this` read) is the same face — oracle silent
/// (`if (a.np) { print(1); } print(2);`).
#[test]
fn never_identifier_receiver_getter_condition_stays_live() {
    let source = "class A {\n\
                  \x20 Never get np => throw 0;\n\
                  \x20 void m(A a) {\n\
                  \x20   if (a.np) { print(1); }\n\
                  \x20   print(2);\n\
                  \x20   if (np) { print(3); }\n\
                  \x20   print(4);\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// A top-level `Never` getter condition keeps its dead arms: oracle rows
/// `{ print(1); }` and `print(2);`.
#[test]
fn never_top_level_getter_condition_stays_dead() {
    let source = "Never get gv => throw 0;\nvoid f() {\n  if (gv) { print(1); }\n  print(2);\n}\n";
    assert_eq!(
        dead_of(source),
        vec!["{ print(1); }".to_string(), "print(2);".to_string()]
    );
}

/// A `Never`-returning **call** condition keeps its dead arms — oracle rows
/// `{ print(1); }` and `print(2);`.
#[test]
fn never_call_condition_stays_dead() {
    let source =
        "Never never() => throw 0;\nvoid f() {\n  if (never()) { print(1); }\n  print(2);\n}\n";
    assert_eq!(
        dead_of(source),
        vec!["{ print(1); }".to_string(), "print(2);".to_string()]
    );
}

/// The statement-position counter-case: a bare `Never` read still kills the
/// following statement — this is why the rule cannot be "bottom reads do
/// not exit". The flow layer's fact anchors at the next statement
/// (`print(1);`); the product-level row starts at the read itself
/// (oracle `3:3|9`), the semantic layer's re-anchor.
#[test]
fn never_parameter_statement_still_kills() {
    let source = "void f(Never v) {\n  v;\n  print(1);\n}\n";
    assert_eq!(dead_of(source), vec!["print(1);".to_string()]);
}
