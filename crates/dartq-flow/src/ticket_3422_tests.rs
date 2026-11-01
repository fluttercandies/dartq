//! #3422 — dead-interval extents on the sound-flow equality family: the
//! oracle rule for a wholly dead branch, the after-`return` block row, and
//! the callable-typed callee that used to flip the branch.
//!
//! The oracle rule, read off the pinned oracle's own rows for
//! `equality_comparisons_test.dart` (`conformance/expected.tsv`, case
//! `features/sound-flow-analysis/language/sound_flow_analysis/
//! equality_comparisons_test`, rows `365:12|55` and `391:12|55`): a dead
//! branch BLOCK anchors at its own `{` and runs to its own `}`, length 55
//! for the corpus shape `{\n      shouldNotBeDemoted = null; //
//! Unreachable\n    }`. That is the official `flowEnd(ifTrue)` /
//! `flowEnd(ifFalse)` close (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:223-330): the first
//! dead node is the branch block itself — the walk enters it unreachable,
//! so `_firstDeadNode` latches on the block before any child — and the
//! trim to `statements.last` only applies when `node != firstDeadNode`
//! (:262-277), so a whole-dead branch reports the block's full extent with
//! its closing brace.
//!
//! The second numbered face is the plain sequential one: unreachable
//! statements after a `return` fold into ONE interval that closes at the
//! covering block's last statement, its trailing `;` included (the same
//! trim, `node.end` at :321).
//!
//! The third face is the branch-selection guard this ticket fixed: a
//! call's static type is the invoked element's RETURN type, so
//! `null == nf()` over a `Null Function()` parameter must never read as a
//! non-nullable comparison (which folded it `false` and reported the dead
//! THEN branch where the oracle reports the dead ELSE one — the corpus
//! drift `389:33|48` vs the oracle's `391:12|55`). The lattice keeps the
//! callee's signature-less `Function` head as the call's face when the
//! real return is not staged; that head's non-nullability describes the
//! CALLABLE, so the equality fold never reads it and no branch dies.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// The dead-code rows of `source` as `(offset, length, covered text)`.
fn dead_rows(source: &str) -> Vec<(u32, u32, String)> {
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
                let start = span.range.offset as usize;
                let end = start + span.range.length as usize;
                Some((
                    span.range.offset,
                    span.range.length,
                    source[start..end].to_string(),
                ))
            }
            _ => None,
        })
        .collect()
}

/// The dead else-branch block reports the block's FULL extent: anchored on
/// the else block's `{`, running through its `}`, length 55 — the exact
/// numbers the oracle reports for this corpus shape (`365:12|55` /
/// `391:12|55` in `conformance/expected.tsv` for the enabled sound-flow
/// equality case; the dead branch is the `else` arm because
/// `null == null` is known `true`).
#[test]
fn dead_else_branch_block_reports_the_block_extent() {
    let source = "void testNullEqualsNull() {\n  {\n    // null == <expr>\n    \
                  int? shouldNotBeDemoted = 0;\n    int? shouldBePromoted;\n    \
                  if (null == null) {\n      shouldBePromoted = 0; // Reachable\n    \
                  } else {\n      shouldNotBeDemoted = null; // Unreachable\n    }\n  \
                  }\n}\n";
    let else_open = source.find("{\n      shouldNotBeDemoted").expect("else brace");
    assert_eq!(
        dead_rows(source),
        vec![(
            else_open as u32,
            55,
            "{\n      shouldNotBeDemoted = null; // Unreachable\n    }".to_string(),
        )],
    );
}

/// Unreachable statements after a `return` inside a block fold into ONE
/// interval that runs to the last statement's `;` (`1; 2;`, length 5) —
/// the same close, trimmed to the covering block's last statement
/// (dead_code_verifier.dart:262-277 + :321).
#[test]
fn dead_code_after_return_widens_to_the_last_statement() {
    let source = "void f() { return; 1; 2; }";
    let start = source.find("1; 2;").expect("dead tail");
    assert_eq!(
        dead_rows(source),
        vec![(start as u32, 5, "1; 2;".to_string())],
    );
}

/// The negative: reachable code opens no interval — the same block shape
/// as the pin above, with the branch left live.
#[test]
fn reachable_branch_block_reports_no_interval() {
    let source = "void f(bool c) {\n  if (c) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    assert_eq!(dead_rows(source), Vec::new());
}

/// The branch-selection guard: `null == nf()` over a callable-typed
/// parameter must not fold. The call's static type is `nf`'s RETURN, which
/// the lattice cannot spell when the semantic layer stages no row — it
/// keeps the callee's signature-less `Function` head, and that head's
/// non-nullability describes the callable, never the value the call
/// answers. Before #3422 the fold read it as a non-nullable value and
/// reported the dead THEN block (the corpus drift: our row started at the
/// then brace while the oracle's `391:12|55` starts at the else brace).
#[test]
fn callable_typed_callee_does_not_flip_the_branch() {
    let source = "void g(Null Function() nf) {\n  if (null == nf()) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    assert_eq!(dead_rows(source), Vec::new());
}

/// Same guard mirrored: `null != nf()` must not fold either — the flipped
/// polarity would report the dead ELSE block, an extent the oracle pins to
/// the `==` shape only.
#[test]
fn callable_typed_callee_does_not_flip_the_inequality_branch() {
    let source = "void g(Null Function() nf) {\n  if (null != nf()) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    assert_eq!(dead_rows(source), Vec::new());
}

/// Same guard on a non-nullable comparison over the callable's result: the
/// unknown result folds nothing there either (`v == nf()` over a
/// non-nullable `v` reported nothing before and after — a pin so the
/// downgrade cannot start inventing a verdict).
#[test]
fn callable_result_keeps_both_branches_on_a_value_comparison() {
    let source =
        "void g(int v, Null Function() nf) {\n  if (v == nf()) {\n    1;\n  } else {\n    2;\n  }\n}\n";
    assert_eq!(dead_rows(source), Vec::new());
}
