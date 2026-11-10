//! #3171 — dead-code Lane B pins: the statement poll sites of
//! `interp_stmt.rs` and the switch-member keyword rows, asserted as the
//! exact source text the row covers against the oracle 3.13.7 law.
//!
//! Upstream face pinned here (pkg/analyzer/lib/src/error/dead_code_verifier.dart):
//!
//! - `checkUnreachableNode(node)` polls the STATEMENT node itself, so a dead
//!   statement anchors its own first token and the covering `flowEnd` closes
//!   it at `node.end` (`:183-189`, `:253-279`) — `g();` after `return;` rows
//!   `g();`, not `g`.
//! - `flowEnd(SwitchMember)` with `node == firstDeadNode` reports at
//!   `node.keyword` and resets the interval (`:217-234`), so a dead
//!   `case`/`default` member rows the bare keyword; a member folded into an
//!   outer interval keeps it open (`consume_dead_default`'s deferral).
//!
//! The catch-clause half of the lane (`DeadCodeCatchFollowingCatch` /
//! `DeadCodeOnCatchSubtype`) is a semantic-crate face with no flow fact to
//! pin; its law is recorded at `finish_try_statement` in
//! `dartq-semantic/src/diagnose/scopes_bodies.rs`.

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
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

// —— Gap 1: the statement poll sites ——————————————————————————————————————

#[test]
fn poll_site_dead_statement_after_return_anchors_itself() {
    // `return;` ends the block's path; the next statement is the official
    // first dead node (`visitExpressionStatement`'s `checkUnreachableNode`),
    // covering its own span, `;` included.
    let source = "void f() {\n  return;\n  g();\n}\nvoid g() {}\n";
    assert_eq!(dead_of(source), vec!["g();".to_string()]);
}

#[test]
fn poll_site_dead_statement_after_throw_anchors_itself() {
    // Same face through `handleExit`'s Never drain: the `throw` statement
    // began live and stays silent; `g();` anchors.
    let source = "void f() {\n  throw 0;\n  g();\n}\nvoid g() {}\n";
    assert_eq!(dead_of(source), vec!["g();".to_string()]);
}

#[test]
fn poll_site_dead_branch_block_reports_braces() {
    // `visitBlock`'s own poll (`:2244`): a provably dead empty branch block
    // is its own first dead node and rows the braces.
    let source = "void f() {\n  if (false) {}\n}\n";
    assert_eq!(dead_of(source), vec!["{}".to_string()]);
}

// —— Gap 2: the switch-member keyword rows ————————————————————————————————

#[test]
fn switch_default_first_reports_keyword_then_case() {
    // A `default` first makes every later member dead: the default reports
    // its own keyword at the head-time member check, the case member reports
    // its keyword, and its body rows through the covering close.
    let source = "\
void f(int x) {
  switch (x) {
    default:
      break;
    case 1:
      break;
  }
}";
    assert_eq!(
        dead_of(source),
        vec![
            "default".to_string(),
            "case".to_string(),
            "break;".to_string()
        ]
    );
}

#[test]
fn switch_shared_body_default_defers_to_the_next_case_head() {
    // `case 1:` / `default:` sharing one body: the shared group's case head
    // stays live, the default's keyword row fires at the next case-headed
    // opener (`consume_dead_default`'s case-opener face).
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
    default:
      break;
    case 2:
      break;
  }
}";
    assert_eq!(
        dead_of(source),
        vec![
            "default".to_string(),
            "case".to_string(),
            "break;".to_string()
        ]
    );
}

#[test]
fn switch_second_default_reports_keyword_and_body() {
    // A default followed by another default-headed group stays live: only
    // the second default and its body report.
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
    default:
      break;
  }
}";
    assert_eq!(
        dead_of(source),
        vec!["default".to_string(), "break;".to_string()]
    );
}

#[test]
fn switch_trailing_default_stays_live() {
    // A trailing default is reachable: no member row at all.
    let source = "\
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
  }
}";
    assert!(dead_of(source).is_empty());
}
