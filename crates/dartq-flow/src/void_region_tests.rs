//! The dead-region interval pins behind the `void_type_usage` extras: a
//! statement that BEGAN dead owns one interval that runs across the
//! following statements — an inner null-short must not trim it to its
//! chain end (upstream `_containsFirstDeadNode`: the statement is the
//! first dead node, the chain does not contain it), and the next dead
//! statement must not open a second row.

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

/// A dead region that starts on a null-short statement over a receiver
/// with no read span (`(true ? 499 : x)?.toString()`, `recv_offset` 0)
/// keeps ONE interval across the following dead cascade and print
/// statement. Oracle 3.13.7 on this fixture: one row `4:3|73` covering
/// the whole tail.
#[test]
fn dead_region_short_then_void_cascade_is_one_row() {
    let source = "void f() {\n  void x;\n  throw 1;\n  (true ? 499 : x)?.toString();\n  (true ? 499 : x)..toString();\n  print(1);\n}\n";
    assert_eq!(
        dead_of(source),
        vec![
            "(true ? 499 : x)?.toString();\n  (true ? 499 : x)..toString();\n  print(1);"
                .to_string()
        ]
    );
}

/// The counter-case: a genuinely dead tail after a `void`-member statement
/// stays dead — one interval, never suppressed.
#[test]
fn dead_tail_after_void_member_still_dead() {
    let source = "class A { void x; }\nvoid f(A a) {\n  throw 1;\n  a.x;\n  print(1);\n}\n";
    assert_eq!(dead_of(source), vec!["a.x;\n  print(1);".to_string()]);
}

/// The positive direction: a `void`-member cascade in LIVE code reports no
/// dead interval (the `use_of_void_result` face is the semantic layer's).
#[test]
fn void_cascade_in_live_region_stays_live() {
    let source = "class A { void x; }\nvoid f(A a) {\n  a.x..toString();\n  print(1);\n}\n";
    assert_eq!(dead_of(source), Vec::<String>::new());
}
