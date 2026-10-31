//! The #3225 pins: two dead-region span shapes the F5-residual census
//! left anchored one token too early / one region too wide.
//!
//! * A C-style `for` whose INITIALIZER is a pattern variable assignment
//!   (`for (var (i) = throw 0; true; 1)`) must mark the same header tail
//!   as its non-pattern siblings: the dead condition is the official's
//!   first dead node and, its parent being the `ForParts`, the row trims
//!   to the last updater's end (`dead_code_verifier.dart:324-326`) —
//!   `true; 1`, never the `) {}` tail the covering close would fold.
//!   The init/cond boundary the trim keys on is the `ForInit` marker, so
//!   every `handle_for_initializer_*` hook must record it.
//! * A Never-operand `as` cast inside a conditional arm
//!   (`c ? x : ((throw 0) as int)`) anchors its tail row at the cast
//!   TYPE, not the `as` keyword: the official's first dead node is the
//!   type annotation node (`flowEnd(arm)`, type_analyzer.dart:2588/2616),
//!   so the row reads `int)`, never `as int)`.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The `dead_code` row texts of `source`, in source order.
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

/// The confirmed pattern-initializer shape: a dead `for` initializer
/// leaves the condition the first dead node, so the header-tail trim
/// (`for_conditionEnd`'s `parent is ForParts` arm) ends the row at the
/// updater — `true; 1`, not the `) {}` fold.
#[test]
fn for_init_pattern_dead_region_trims_at_the_updater() {
    let source = r#"
void main() {
  for (var (i) = throw 0; true; 1) {}
  use(0);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), ["true; 1"]);
}

/// The non-pattern sibling of the same shape: identical row, which is
/// the parity the pattern-init hook must reach (it already records the
/// `ForInit` boundary marker).
#[test]
fn for_init_declaration_dead_region_trims_at_the_updater() {
    let source = r#"
void main() {
  for (var i = throw 0; true; 1) {}
  use(0);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), ["true; 1"]);
}

/// A live pattern-initialized loop keeps every statement after it live —
/// the marker is a partition, not a deadness signal.
#[test]
fn live_pattern_for_stays_silent() {
    let source = r#"
void main() {
  for (var (int i) = 0; i < 3; i++) {}
  use(1);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// The cast-tail shape (upstream `DeadCodeTest/test_asExpression_type`):
/// `doNotReturn() as int;` in an expression body — the Never-typed
/// operand leaves the cast TYPE the first dead node, so the row starts at
/// the annotation (`int;`), never at the `as` keyword (`as int;`).
#[test]
fn cast_tail_statement_anchors_at_the_type_annotation() {
    let source = r#"
Never doNotReturn() => throw 0;
test() => doNotReturn() as int;
"#;
    assert_eq!(dead_of(source), ["int;"]);
}

/// The corpus arm face (upstream `least_upper_bound_greatest_closure_1`:
/// `condition` is a TOP-LEVEL `var`, so neither arm folds): the Never
/// `throw` inside the arm leaves the cast type the first dead node and
/// the arm's own `flowEnd` row runs from it to the arm's end.
#[test]
fn conditional_arm_cast_tail_anchors_at_the_type_annotation() {
    let source = r#"
var condition = true;
class C2 {}
void main() {
  var z1 = condition ? 1 : ((throw 0) as int);
  use(z1);
  var z4 = condition ? ((throw 0) as C2) : 1;
  use(z4);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), ["int)", "C2)"]);
}

/// The then-arm position alone (no preceding arm row): the same
/// type-anchored tail.
#[test]
fn conditional_then_arm_cast_tail_anchors_at_the_type_annotation() {
    let source = r#"
var condition = true;
class C2 {}
void main() {
  var z4 = condition ? ((throw 0) as C2) : 1;
  use(z4);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), ["C2)"]);
}

/// A live cast in the same arm position reports nothing — the trim is a
/// dead-row anchor, not a cast face.
#[test]
fn live_conditional_arm_cast_stays_silent() {
    let source = r#"
void main(int x) {
  var z = x > 0 ? 1 : (2 as int);
  use(z);
}
void use(Object? x) {}
"#;
    assert_eq!(dead_of(source), Vec::<String>::new());
}
