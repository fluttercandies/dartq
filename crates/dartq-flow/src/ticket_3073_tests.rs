//! #3073 — dead-code pins for the constructor-initializer-then-body row
//! (`dead_code` MISS) plus regression locks for the census-verified
//! const-condition and sealed-switch silences.
//!
//! Root cause (the MISS face): a function BODY block carried no dead poll.
//! The walk enters a constructor's body dead after a throwing initializer,
//! and the first anchored node was the body's first statement (`x;`) — the
//! official anchors at the BODY node itself
//! (`visitBlockFunctionBody`'s `checkUnreachableNode`,
//! upstream Dart SDK source, pkg/analyzer/lib/src/generated/resolver.dart),
//! so the row spans the body block's full extent, `}` included: the
//! covering `flowEnd(constructor)` (dead_code_verifier.dart:3072) applies
//! no last-statement trim to a constructor. The fix polls the body's
//! `BlockBodyBegin` event (the `{` token) and widens a body-anchored row
//! to the `}` at `BlockBodyEnd`; an EMPTY body stays silent
//! (dead_code_verifier.dart:252-256 exempts an unreachable empty
//! constructor body, and the `;`-form `EmptyFunctionBody` has no block).
//!
//! The regression locks pin the census-verified 3.13.7 silences this
//! crate already gets right (probed via the package-context A/B rig over
//! the dead_code corpus): a plain `const bool` reference is NOT folded
//! into branch deadness (the only constant face is the boolean literal,
//! resolver.dart `visitBooleanLiteral` — no const-variable resolution
//! feeds `getExpressionInfo`), and a subtype-pattern arm chain leaves the
//! later switch arms reachable (`switchStatement_beginAlternative`
//! branches from the chained unmatched state, flow_analysis.dart:6606).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
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
            FlowFact::DeadCode { span } => {
                Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
            }
            _ => None,
        })
        .collect()
}

/// Asserts `source` reports no dead rows at all.
fn no_dead(source: &str) {
    assert_eq!(dead_of(source), Vec::<String>::new());
}

// —— constructor initializer throw, then a body ——————————————————————

#[test]
fn ctor_init_throw_body_row_spans_the_whole_block() {
    // Oracle (3.13.7, package-context A/B): one row over the body block's
    // full extent — `{`, statements, closing `}` included.
    let source = "class A {\n  int x;\n  A() : x = throw 0 {\n    x;\n  }\n}\n";
    let dead = dead_of(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].starts_with('{'), "{dead:?}");
    assert!(dead[0].ends_with('}'), "{dead:?}");
    assert!(dead[0].contains("x;"), "{dead:?}");
}

#[test]
fn ctor_init_throw_empty_block_body_stays_silent() {
    // dead_code_verifier.dart:252-256 — an unreachable empty constructor
    // body is exempt.
    no_dead("class A {\n  int x;\n  A() : x = throw 0 {}\n}\n");
}

#[test]
fn ctor_init_throw_semicolon_body_stays_silent() {
    // The `;` form has no body node to poll.
    no_dead("class A {\n  int x;\n  A() : x = throw 0;\n}\n");
}

#[test]
fn live_member_body_keeps_statement_anchored_tail() {
    // The poll must stay a no-op for a body entered live: the tail row
    // anchors at the first dead statement, not the body's `{`.
    let dead = dead_of("void f() {\n  throw 0;\n  g();\n}\n");
    assert_eq!(dead, vec!["g();".to_string()], "{dead:?}");
}

#[test]
fn dead_nested_statement_block_row_spans_the_block() {
    // Same extent rule one level down (the statement-block poll): the row
    // is the inner block's `{ ... }`, closed by the covering exec trim.
    let dead = dead_of("void f() {\n  throw 0;\n  {\n    g();\n  }\n}\n");
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].starts_with('{'), "{dead:?}");
    assert!(dead[0].ends_with('}'), "{dead:?}");
    assert!(dead[0].contains("g();"), "{dead:?}");
}

// —— const-condition silences (census lock: no const-variable folding) —

#[test]
fn const_bool_conditional_stays_silent() {
    no_dead("const bool DEBUG = true;\nint g() => DEBUG ? 1 : 2;\n");
}

#[test]
fn const_bool_if_else_stays_silent() {
    no_dead(
        "const bool DEBUG = true;\nvoid g() {\n  if (DEBUG) {\n    1;\n  } else {\n    2;\n  }\n}\n",
    );
}

#[test]
fn const_bool_while_stays_silent() {
    no_dead("const bool DEBUG = true;\nvoid g() {\n  while (DEBUG) {\n    1;\n  }\n  2;\n}\n");
}

#[test]
fn const_bool_short_circuit_stays_silent() {
    no_dead("const bool DEBUG = true;\nbool g() => DEBUG || true;\n");
    no_dead("const bool DEBUG = true;\nbool g() => DEBUG && false;\n");
}

#[test]
fn literal_true_conditional_still_folds() {
    // The literal face survives: only `true`/`false` literals split.
    let dead = dead_of("int h() => true ? 1 : 2;\n");
    assert_eq!(dead, vec!["2".to_string()], "{dead:?}");
}

#[test]
fn literal_true_if_else_reports_the_else_block() {
    let dead = dead_of("void h() {\n  if (true) {\n    1;\n  } else {\n    2;\n  }\n}\n");
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].starts_with('{'), "{dead:?}");
    assert!(dead[0].ends_with('}'), "{dead:?}");
    assert!(dead[0].contains("2;"), "{dead:?}");
}

#[test]
fn literal_or_reports_operator_to_operand() {
    let dead = dead_of("bool h() => true || false;\n");
    assert_eq!(dead, vec!["|| false".to_string()], "{dead:?}");
}

// —— sealed-subtype switch arms stay reachable (census lock) ——————————

#[test]
fn sealed_subtype_switch_expression_arms_stay_live() {
    no_dead(
        "sealed class Face {}\nclass Jack extends Face {}\n\
         class Queen extends Face {}\nclass King extends Face {}\n\
         String f(Face face) => switch (face) {\n      Jack _ => 'J',\n      Queen _ => 'Q',\n      King _ => 'K'\n    };\n",
    );
}

#[test]
fn sealed_subtype_switch_statement_arms_stay_live() {
    no_dead(
        "sealed class Face {}\nclass Jack extends Face {}\n\
         class Queen extends Face {}\nclass King extends Face {}\n\
         String f(Face face) {\n  switch (face) {\n    case Jack _:\n      return 'J';\n    case Queen _:\n      return 'Q';\n    case King _:\n      return 'K';\n  }\n}\n",
    );
}
