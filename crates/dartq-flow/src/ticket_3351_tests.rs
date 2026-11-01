//! The #3351 pins: the `isExpression_end` reference arm's write-capture
//! bail.
//!
//! Upstream decides a type test in three ordered arms (upstream Dart SDK
//! source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart
//! `isExpression_end`, :7178-7210):
//!
//! 1. a bottom checked type or a nullability-disjoint pair folds to the
//!    `booleanLiteral(isNot)` constant — decided BEFORE the reference
//!    branch, so it covers references;
//! 2. a reference operand takes `tryPromoteForTypeCheck` — whose first
//!    statement bails on a write-captured variable with a TRIVIAL
//!    `ExpressionInfo` (:4136-4138), so neither arm dies and nothing is
//!    promoted;
//! 3. only a NON-reference operand can take the
//!    `_isTypeCheckGuaranteedToSucceed` constant fold (:7201-7205).
//!
//! The defect this file pins: a write-captured reference used to fall
//! through to arm 3, so `int x` write-captured reported a `dead_code` row
//! on the `x is! int` then-arm (and on the `x is num` else-arm) that the
//! oracle does not emit. Oracle evidence (Dart 3.13.1 `dart analyze
//! --format machine`): `int x` write-captured keeps BOTH arms live for
//! `x is! int` and for `x is num`, while the static `unnecessary_type_check`
//! warning still fires; an UNcaptured `int x` deads the then-arm of
//! `x is! int` (arm 2's bottom `factor(int, int)` remainder,
//! `ifFalseIsUnreachable` at :4158-4166), and the arm-1 folds still fire
//! over a write-captured receiver (`x is Null` on `int x`).

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
            FlowFact::DeadCode { span } => Some(
                source[span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// Asserts the single dead row covers a branch block.
fn assert_one_block_row(rows: &[String], what: &str) {
    assert_eq!(rows.len(), 1, "{what}: {rows:?}");
    assert!(rows[0].starts_with('{'), "{what}: {rows:?}");
    assert!(rows[0].ends_with('}'), "{what}: {rows:?}");
}

// —— the write-capture bail (arm 2's `writeCaptured` return) ——————————————

#[test]
fn write_captured_is_not_test_stays_silent() {
    // The ticket's repro shapes: `int x` written from a closure is
    // write-captured, so `tryPromoteForTypeCheck` returns trivial and the
    // then-arm stays live — no `dead_code` row (the static
    // `unnecessary_type_check` is a semantic face, not a flow fact).
    let src = concat!(
        "void f(int x) {\n",
        "  void h() { x = 2; }\n",
        "  if (x is! int) {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn write_captured_succeed_test_stays_silent() {
    // Same bail on the `is` side: a test the declared type already
    // satisfies never folds to the succeed literal over a reference, and
    // the write-captured promotion is trivial — both arms live.
    let src = concat!(
        "void f(int x) {\n",
        "  void h() { x = 2; }\n",
        "  if (x is num) {\n",
        "    g();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn write_captured_empty_block_stays_consistent() {
    // The #3335 empty-branch poll must agree with the non-empty body: a
    // reachable branch reports neither its braces nor an inner statement.
    let src = concat!(
        "void f(int x) {\n",
        "  void h() { x = 2; }\n",
        "  if (x is! int) {}\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn write_captured_fail_fold_still_deads_the_then() {
    // The bail sits INSIDE the reference arm: the fail fold decides
    // before it (:7184-7189), so a nullability-disjoint test over the
    // write-captured receiver still deads its arm.
    let src = concat!(
        "void f(int x) {\n",
        "  void h() { x = 2; }\n",
        "  if (x is Null) {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_one_block_row(&rows, "write-captured `x is Null`");
}

// —— the uncaptured controls (unchanged; the oracle deads these) ——————————

#[test]
fn uncaptured_is_not_test_deads_the_then() {
    // No closure write: the promotion split runs and the bottom
    // `factor(int, int)` remainder marks the unmatched arm unreachable,
    // which `is!` maps onto the then-arm.
    let src = concat!(
        "void f(int x) {\n",
        "  if (x is! int) {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_one_block_row(&rows, "uncaptured `x is! int`");
    assert!(rows[0].contains("g();"), "{rows:?}");
}

#[test]
fn uncaptured_succeed_test_deads_the_else() {
    // The else-deadness rides the FACTOR, never the boolean-literal fold.
    let src = concat!(
        "void f(int x) {\n",
        "  if (x is int) {\n",
        "    g();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_one_block_row(&rows, "uncaptured `x is int`");
}

#[test]
fn read_only_capture_keeps_the_promotion_split() {
    // Only a WRITE capture bails promotion (upstream `writeCaptured`,
    // pkg/_fe_analyzer_shared/lib/src/type_inference/assigned_variables.dart
    // :119-126 splits reads from writes): a read-only captured `int x`
    // still promotes, so `x is! int` still deads its then-arm.
    let src = concat!(
        "void f(int x) {\n",
        "  void h() { g(x); }\n",
        "  if (x is! int) {\n",
        "    g(x);\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_one_block_row(&rows, "read-captured `x is! int`");
}

// —— the ticket's regression shapes ———————————————————————————————————

#[test]
fn object_operand_is_not_int_keeps_both_arms() {
    // `Object` is neither the tested type nor disjoint from it: the
    // promotion split leaves both arms reachable (oracle: no row).
    let src = concat!(
        "void f(Object x) {\n",
        "  if (x is! int) {\n",
        "    g();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn int_operand_is_not_string_keeps_both_arms() {
    // An unrelated class type is not a bottom remainder: `factor(int,
    // String)` is `int`, so neither arm dies (oracle: no row).
    let src = concat!(
        "void f(int x) {\n",
        "  if (x is! String) {\n",
        "    g();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}
