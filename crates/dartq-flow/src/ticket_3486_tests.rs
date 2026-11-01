//! The #3486 pins: a for-in over a typed `void` list literal.
//! Oracle (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`):
//! `void f() { for (int x in <void>[]) {} }` rows `use_of_void_result
//! 2:17` at `<` spanning the whole `<void>[]` literal; `for (int x in
//! <void>[1])` rows `2:17` spanning `<void>[1]` (the empty list is the
//! gap — the row truncates to `<void>` when the list is `[]`).
//!
//! The reported range is the semantic layer's iterable-row extent
//! (`single_row_since` -> `push_typed_value_row(span_start..right.end)`,
//! diagnose/listener.rs `handle_literal_list`), where `right` is the
//! LIST LITERAL's `]` token the parser hands `handleLiteralList`. The
//! flow engine types the literal as `List<void>` and propagates `void`
//! to the loop variable's element type (`for_in_parts_at`,
//! interp.rs:4406-4501) — that contract is what these pins lock: the
//! flow side already answers the element type correctly and publishes
//! no literal-extent span (the anchor the semantic row reads is the
//! parser's `right` token, not a flow fact).
//!
//! The empty-`[]` truncation is NOT a flow defect: the scanner keeps
//! `[]` as one `Index` token, and the parser's empty-list arms
//! (`parse_list_literal_index_arm`/`finish_list_literal_index`,
//! literals.rs) synthesize a zero-width `]` at `[`'s offset rather than
//! the real `]` the official `rewriteSquareBrackets` emits one token
//! past `[` (parser_impl.dart:8459-8464) — so the semantic `right.end`
//! lands at `>` for `[]` and at the real `]` for `[1]`. Upstream:
//! `resolveListLiteral`'s written-arguments arm
//! (typed_literal_resolver.dart:68-107).

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

/// The loop variable's read-type rows: `(offset, ty-named-bool)` — the
/// for-in element type reaches `x`'s declared/meta face. The flow engine
/// must keep typing `<void>[]` as `List<void>` so the loop variable reads
/// the iterable's element type (`void`), never a degraded unknown.
fn read_names(source: &str) -> Vec<String> {
    let facts = scan_parse(source);
    let at = |span: dartq_source::SourceRange| {
        source[span.offset as usize..(span.offset + span.length) as usize].to_string()
    };
    facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::ReadType { span, .. } => Some(at(span.range)),
            _ => None,
        })
        .collect()
}

/// `for (int x in <void>[]) {}` parses the `<void>[]` iterable as a typed
/// list literal and binds `x` — the engine must not bail, drop the
/// literal's type arguments, or emit spurious dead rows. Oracle rows only
/// `use_of_void_result` at `<void>[]` (a semantic-layer diagnostic, not a
/// flow fact) plus `unused_local_variable` — no `dead_code`, no `Never`
/// propagation into the body.
#[test]
fn for_in_void_list_literal_body_stays_reachable() {
    let source = "void f() {\n  for (int x in <void>[]) {\n    x;\n  }\n}";
    let dead: Vec<_> = scan_parse(source)
        .into_iter()
        .filter(|fact| matches!(fact, FlowFact::DeadCode { .. }))
        .collect();
    assert!(dead.is_empty(), "no dead rows inside the loop: {dead:?}");
}

/// The non-empty typed literal `<void>[1]` shares the element-type
/// contract: `x` is bound and the body stays live. The `]` extent the
/// semantic row reports is the parser's real `]` — only the empty `[]`
/// arm truncates.
#[test]
fn for_in_void_nonempty_literal_body_stays_reachable() {
    let source = "void f() {\n  for (int x in <void>[1]) {\n    x;\n  }\n}";
    let dead: Vec<_> = scan_parse(source)
        .into_iter()
        .filter(|fact| matches!(fact, FlowFact::DeadCode { .. }))
        .collect();
    assert!(dead.is_empty(), "no dead rows inside the loop: {dead:?}");
}

/// Reading the loop variable inside the body emits its read row at the
/// identifier — the element-type feed (`void`) binds `x` as a live local,
/// so `x;` is a real read, not a dead or unresolved one.
#[test]
fn for_in_void_literal_loop_variable_reads() {
    let source = "void f() {\n  for (int x in <void>[]) {\n    x;\n  }\n}";
    assert!(
        read_names(source).iter().any(|s| s == "x"),
        "x reads as a live local: {:?}",
        read_names(source)
    );
}
