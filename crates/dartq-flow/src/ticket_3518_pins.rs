//! The #3518 face-1 pins: a `when` guard is a branch split, never a
//! chain terminator.
//!
//! Oracle (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`):
//!
//! ```dart
//! int e(bool b) => switch (b) { true when !false => 0, _ => throw 'x' };
//! ```
//! reports nothing. The constant-true guard keeps the matched arm live and
//! joins an unreachable guard-false arm, so the chain stays exactly where
//! the PATTERN left it — `_` is reachable, and its `throw` body is a live
//! `Never` value, not dead code.
//!
//! ```dart
//! int h(int i) {
//!   switch (i) {
//!     case 1 when false:
//!       return 0;          // <- dead_code `14:7`
//!     default:
//!       return 1;
//!   }
//! }
//! ```
//! reports one `dead_code` row at the guarded member's body and NOTHING at
//! `default`: the constant-false guard kills the clause's own matched arm
//! while its guard-false arm keeps the chain live for every later case
//! (`_popPattern`'s `unmatched = _join(unmatched, guardInfo.ifFalse)`,
//! upstream `pkg/_fe_analyzer_shared/lib/src/flow_analysis/
//! flow_analysis.dart:7785-7793`).
//!
//! The pair is deliberately two-sided: the first pin fails if a guard is
//! allowed to terminate the chain, the second fails if the guard split is
//! dropped altogether (the pre-#3518 body reported the `_ => throw` arm
//! dead and never reported the `when false` body).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

fn scan_parse(source: &str) -> Vec<FlowFact> {
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
    flow.0.borrow_mut().take_facts()
}

/// Dead-code rows as `(offset, length)` pairs.
fn dead_ranges(source: &str) -> Vec<(u32, u32)> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

/// A constant-true guard (`when !false`) keeps the chain live: the wildcard
/// arm and its `throw` body carry no `dead_code` row.
///
/// Oracle: `switch (b) { true when !false => 0, _ => throw 'x' }` — no
/// issues.
#[test]
fn constant_true_guard_keeps_the_later_arm_live() {
    let source = "int e(bool b) => switch (b) {\n  true when !false => 0,\n  _ => throw 'x',\n};";
    assert_eq!(dead_ranges(source), []);
}

/// A constant-false guard (`when false`) kills only the clause's own body:
/// one row at that body, and the later `default` stays live.
///
/// Oracle: `case 1 when false: return 0;` — one `dead_code` row at
/// `return 0;` (`14:7` in the probe package), nothing at `default`.
#[test]
fn constant_false_guard_deads_the_member_body_only() {
    let source = "int h(int i) {\n  switch (i) {\n    case 1 when false:\n      return 0;\n    default:\n      return 1;\n  }\n}";
    let body = source.find("return 0;").unwrap() as u32;
    let rows: Vec<u32> = dead_ranges(source).into_iter().map(|(off, _)| off).collect();
    assert_eq!(rows, [body]);
}

/// The same guard on a constant-true body: nothing dies, and the `default`
/// stays live.
///
/// Oracle: `case 1 when !false: return 0;` — no issues.
#[test]
fn constant_true_guard_in_a_statement_switch_is_silent() {
    let source = "int k(int i) {\n  switch (i) {\n    case 1 when !false:\n      return 0;\n    default:\n      return 1;\n  }\n}";
    assert_eq!(dead_ranges(source), []);
}
