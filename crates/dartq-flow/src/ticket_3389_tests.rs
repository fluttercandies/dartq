//! The #3389 pins: `cannotMatch` over a SWITCH kind's pattern head.
//!
//! #3377 mounted the verdict for standalone contexts only (`if (0 case
//! Null v)`, `if (null case Null())`) because a statement switch's chain
//! and post-switch reachability are consumed one level up
//! (`interp_stmt.rs`'s exhaustive-verdict join): the verdict must kill the
//! matched arm WITHOUT folding the code after a non-exhaustive switch.
//!
//! The join holds because the two official writes are independent — the
//! verdict sets `_current` unreachable (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:7755-7765)
//! while `matchFailsIfWrongType && !coversMatchedType` joins the failed arm
//! into `_unmatched` beside it (:7756-7762). A cannot-match pair is never a
//! cover, so the clause always feeds the next alternative and the
//! post-switch state; only an exhaustive switch's own join
//! (`switch_end`'s `breakState ??= previous.setUnreachable()`, :7990-8000)
//! deads the code past it.
//!
//! The switch kinds read their scrutinee's static type off the switch
//! frame — the same face the reference path already used — so the verdict
//! answers for `switch (0)`, `switch (null)` and `switch (fn())` exactly
//! like for `switch (x)`.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;

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
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

// —— the arm kill, the post-switch survival ————————————————————————————————

/// `switch (0) { case Null(): … }` — the matched value is `int`, the
/// required type `Null`: `cannotMatch` kills the case body. The clause's
/// failed arm still joins the chain, so the switch's unmatched state stays
/// live and the code after this NON-exhaustive switch does too.
#[test]
fn non_exhaustive_switch_cannot_match_deads_the_body_only() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (0) {\n",
        "    case Null():\n",
        "      notRun();\n",
        "  }\n",
        "  g();\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("notRun"), "{rows:?}");
}

/// The `matchFailsIfWrongType` join still feeds the next alternative: the
/// cannot-match head contributes its failed arm, so the following clause
/// matches and its body stays live.
#[test]
fn surviving_clause_after_a_cannot_match_head_stays_live() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (0) {\n",
        "    case Null():\n",
        "      notRun();\n",
        "    case int():\n",
        "      g();\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("notRun"), "{rows:?}");
}

/// A switch over an always-exhaustive scrutinee (`null` is `Null`) whose
/// only clause cannot match: the case body dies, and `switch_end` finds no
/// break model and no unmatched chain to join, so the code after it is
/// dead too.
#[test]
fn exhaustive_all_cannot_match_switch_deads_the_post_switch() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (null) {\n",
        "    case int():\n",
        "      g();\n",
        "  }\n",
        "  notReached();\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert!(
        rows.iter().any(|row| row.contains("notReached")),
        "{rows:?}"
    );
}

/// The typed declared-variable head (`case Null v:`) answers the same
/// question through its own non-reference arm: the case body dies, the
/// post-switch state survives the non-exhaustive switch.
#[test]
fn typed_switch_head_cannot_match_deads_the_body_only() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (0) {\n",
        "    case Null v:\n",
        "      notRun();\n",
        "  }\n",
        "  g();\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("notRun"), "{rows:?}");
}

/// The boundary: `Null` against `Null` is no disjoint pair, so the clause
/// matches and its body stays live. `Null` is an always-exhaustive
/// scrutinee, yet the post-switch state survives too — the body's live
/// fall-through is the break model, which is exactly what the exhaustive
/// verdict's `breakState ??= previous.setUnreachable()` fallback never
/// reaches. A wrongly-fired `cannotMatch` would dead the body and show a
/// row here.
#[test]
fn null_scrutinee_against_null_head_keeps_the_body_live() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (null) {\n",
        "    case Null():\n",
        "      g();\n",
        "  }\n",
        "  notReached();\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert!(rows.is_empty(), "{rows:?}");
}
