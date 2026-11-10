//! #3096 — the in-scope `dead_code` residual verdicts on the statement
//! replay. Every expectation is a `dart analyze` 3.13.1 machine-format A/B
//! over `/tmp/f3096/{f1,f3,w2,w3}` fixtures; the upstream basis is
//! `pkg/analyzer/lib/src/error/dead_code_verifier.dart`.
//!
//! Face 1 — post-`finally{return}` reachability (verified CORRECT, pinned
//! here): a `finally` block that always exits makes the following statement
//! the verifier's first dead node (`_visitNode`, :183-205), which rows its
//! own extent, trailing `;` included, at every enclosing-construct nesting
//! (plain body, switch member, catch body, catch+switch). The
//! `test_switchCase_final_rethrow` corpus shape differs only in the dead
//! statement being a `rethrow;`, whose row needs the `rethrow` keyword
//! offset the recorder does not carry (`record.rs`'s
//! `end_rethrow_statement` records `Rethrow` + `StmtEnd` and no
//! `ExprStmt` wrapper) — a recorder-side residual, not a reachability one.
//!
//! Face 3 — `DeadCodeVerifier.visitVariableDeclaration` (:119-135): a
//! `late` LOCAL wildcard declaration with an initializer reports
//! `dead_code_late_wildcard_variable_initializer` at the INITIALIZER,
//! independently of reachability (the initializer is never evaluated — the
//! wildcard can never be read). Oracle: `2:16|1` = `0`.
//!
//! Face 2 (`default` keyword row after an exhausting head) is NOT pinned
//! here: the chain-kill verdict that decides it lives in the pattern walk
//! (`patterns.rs`'s `cannot_fail`, falsified-only at :1053/:1086 and read
//! by `apply_pattern_exit` :1542-1562), outside this lane's scope — see the
//! residual routing in the ticket report.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as the exact source text they cover.
pub(crate) fn dead_of(source: &str) -> Vec<String> {
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

// —— face 1: post-`finally{return}` deadness, one pin per nesting ——

/// Oracle `6:3|2`: the plain-body nesting.
#[test]
fn statement_after_plain_finally_return_is_dead() {
    let source = "\
void f() {
  try {
  } finally {
    return;
  }
  2;
}
";
    assert_eq!(dead_of(source), vec!["2;".to_string()]);
}

/// Oracle `7:7|2`: the switch-member-body nesting.
#[test]
fn statement_after_finally_return_in_switch_case_is_dead() {
    let source = "\
void f(int a) {
  switch (a) {
    case 0:
      try {} finally {
        return;
      }
      2;
  }
}
";
    assert_eq!(dead_of(source), vec!["2;".to_string()]);
}

/// Oracle `7:5|2`: the catch-body nesting.
#[test]
fn statement_after_finally_return_in_catch_is_dead() {
    let source = "\
void f(int a) {
  try {
  } on int {
    try {} finally {
      return;
    }
    2;
  }
}
";
    assert_eq!(dead_of(source), vec!["2;".to_string()]);
}

/// Oracle `9:9|2`: the catch-body + switch-member nesting.
#[test]
fn statement_after_finally_return_in_catch_switch_is_dead() {
    let source = "\
void f(int a) {
  try {
  } on int {
    switch (a) {
      case 0:
        try {} finally {
          return;
        }
        2;
    }
  }
}
";
    assert_eq!(dead_of(source), vec!["2;".to_string()]);
}

// —— face 3: the `late` wildcard initializer ——

/// Oracle `2:16|1`: the `late` wildcard declaration's initializer is dead
/// (never evaluated) — reachability-independent, so it rows even though the
/// enclosing body is live.
#[test]
fn late_wildcard_initializer_is_dead() {
    let source = "\
f() {
  late var _ = 0;
}
";
    assert_eq!(dead_of(source), vec!["0".to_string()]);
}

/// Oracle: no rows — a named `late` local's initializer stays live.
#[test]
fn late_named_initializer_stays_live() {
    let source = "\
f() {
  late var x = 0;
}
";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// Oracle: no rows — a non-`late` wildcard is an ordinary variable whose
/// initializer is evaluated.
#[test]
fn non_late_wildcard_initializer_stays_live() {
    let source = "\
f() {
  var _ = 0;
}
";
    assert_eq!(dead_of(source), Vec::<String>::new());
}
