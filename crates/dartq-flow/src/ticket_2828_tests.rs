//! The #2828 E4 pins: a C-style `for` whose initializer is a pattern
//! variable declaration (`var (int i) = 1`) and/or whose updaters are
//! pattern assignments (`(i) = (i + 1)`) executes like a traditional loop
//! — the loop exits through its condition's false arm, and everything
//! after it stays live. The census face (report.tsv post-wave-3): both
//! shapes were modeled as never exiting, killing every following
//! statement/element (21 extra `dead_code` rows across the co19
//! `Patterns/execution_pattern_for_*` family; the oracle rows for
//! `execution_pattern_for_element_A01_t01` are empty).
//!
//! Upstream: for_resolver.dart resolves the pattern initializer/updater
//! through the ordinary assignment machinery (the pattern head inside
//! `parseExpressionStatementOrDeclarationAfterModifiers`, and
//! `patternAssignment_end`'s writes), so the loop head's conservative
//! join sees the updater's write and the condition keeps both arms.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

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

/// The collection-element form (co19
/// `Patterns/execution_pattern_for_element_A01_t01`): a pattern-declared
/// loop variable with a pattern-assignment updater; the element after the
/// loop and every later statement stay live.
#[test]
fn pattern_for_element_terminates() {
    let source = r#"
main() {
  var list1 = [
    0,
    for (var (int i) = 1; i <= 3; (i) = (i + 1)) i,
    4
  ];
  use(list1);
  int j = -1;
  var list2 = [
    0,
    for ((j) = 1; j < 4; (j) = (j + 1)) j,
    4
  ];
  use(list2);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// The statement form (co19
/// `Patterns/execution_pattern_for_statement_A01_t01`): both loops run and
/// the statements after each stay live.
#[test]
fn pattern_for_statement_terminates() {
    let source = r#"
main() {
  String log = "";
  for (var (int i) = 0; i < 3; (i) = (i + 1)) {
    log += "$i;";
  }
  use(log);
  log = "";
  var j = -1;
  for ((j) = 0; j < 3; (j) = (j + 1)) {
    log += "$j;";
  }
  use(log);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), Vec::<String>::new());
}
