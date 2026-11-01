//! The #3491 dead-anchor pins: a `Never`-typed argument's sibling row.
//! Oracle (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`):
//! `f(int a, Never b, int c) { a.clamp(b, c); }` rows `dead_code 2:14
//! len3` (`c);`), `void g(int x, int y) {}` + `f(Never b, int c) { g(b,
//! c); }` rows `3:8 len3` (`c);`), `f(int a, Never b) { a.clamp(b, 3); }`
//! rows `2:14 len3` (`3);`). The flow interval ends at the argument
//! list's `)` — `flowEnd`'s `length = node.end - offset`
//! (dead_code_verifier.dart:321) reports through the flow-ending node's
//! end; the trailing `;` rides at the semantic layer's statement-tail
//! face, so the flow pin anchors at `c)`/`3)`. Upstream: the
//! bottom-typed read's own exit (`recordStaticType` -> `handleExit` on
//! `type.isBottom`, ast.dart:11146-11153) ends the flow at `b`; the NEXT
//! argument node is the first dead node and anchors
//! (dead_code_verifier.dart's plain `node` arm at `checkUnreachableNode`
//! — the `AssignmentExpression`/`BinaryExpression` re-anchors do not
//! apply, so the operand's own span IS the official extent).

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

/// Dead-code rows rendered as `(offset, length)` pairs.
fn dead_ranges(source: &str) -> Vec<(u32, u32)> {
    scan_parse(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

/// A `Never` argument's NEXT sibling anchors — `a.clamp(b, c)` rows
/// `c);` at `c`=40 through the statement's `;`=42 (the interval runs to
/// the flow-ending node's end, `flowEnd`'s `length = node.end - offset`,
/// dead_code_verifier.dart:321). Oracle: `dead_code 2:14 len3` = `c);`.
#[test]
fn never_argument_sibling_anchors_at_next_argument() {
    let source = "f(int a, Never b, int c) {\n  a.clamp(b, c);\n}";
    assert_eq!(dead_ranges(source), [(40, 3)]);
}

/// A bare call's `Never` argument dies the same way — `g(b, c)` rows
/// `c);` (`c`=51 through `;`=53). Oracle: `dead_code 3:8 len3` = `c);`.
#[test]
fn never_argument_sibling_anchors_in_bare_call() {
    let source = "void g(int x, int y) {}\nf(Never b, int c) {\n  g(b, c);\n}";
    assert_eq!(dead_ranges(source), [(51, 3)]);
}

/// A literal sibling anchors identically — `a.clamp(b, 3)` rows `3);`
/// (`3`=33 through `;`=35). Oracle: `dead_code 2:14 len3` = `3);`.
#[test]
fn never_argument_sibling_anchors_at_literal() {
    let source = "f(int a, Never b) {\n  a.clamp(b, 3);\n}";
    assert_eq!(dead_ranges(source), [(33, 3)]);
}

/// A `Never` LAST argument is the list's own death — the argument was
/// live when the official's pre-order poll ran, so the next node
/// anchors, not the argument itself: `a.clamp(c, b)` kills the flow at
/// `b`, and the following `a;` statement rows as the dead region.
#[test]
fn never_argument_last_anchors_beyond_list() {
    let source = "f(int a, Never b, int c) {\n  a.clamp(c, b);\n  a;\n}";
    assert_eq!(dead_ranges(source), [(46, 2)]);
}
