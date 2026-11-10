//! #3076 — dead-code pins for the constructor-initializer entry row and
//! the C-style `for` updater-region extents, transcribed from
//! `dart analyze` 3.13.1 package-context A/B over /tmp/dc3076 fixtures
//! (the upstream expectations are the same shapes:
//! `upstream Dart SDK source, pkg/analyzer/test/src/diagnostics/
//! dead_code_test.dart` `test_flowEnd_forParts_*` /
//! `test_flowEnd_forElementParts_*`, plus
//! `pkg/analyzer/lib/src/error/dead_code_verifier.dart`):
//!
//! * The ctor-initializer entry row (dead_code_verifier.dart:284-291,
//!   `parent is ConstructorInitializer`): a dead interval anchored inside
//!   an initializer entry reports the ENTRY's own extent and the
//!   covering-close tail is suppressed — `A() : x = throw 0, y = 1 { x; }`
//!   reports `y = 1`, never `y = 1 { ... }`. An `assert(...)` entry rows
//!   its whole `assert(y)` call. An entry AFTER the dead one stays silent
//!   (the official's one-interval fold), and a throw in the LAST entry
//!   keeps #3073's body-block row.
//! * The updater-region extents (dead_code_verifier.dart:357-378's
//!   `updaters.beginToken`/`endToken`, :309-310's `parent is ForParts`
//!   trim): the dead row spans the FULL updater list through its trailing
//!   closers — `values[0]` includes `]`, `i.toString()` includes `()`,
//!   `C()` includes the implicit-creation parens, `(i).sign` BEGINS at
//!   the grouping `(` — and a header-anchored interval (the
//!   initializer-throw shapes) trims at the last updater's end, the
//!   covering `) {}` never folding in. Statement and collection forms
//!   share the face.

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

// —— constructor-initializer entry rows ——————————————————————————————————

#[test]
fn ctor_multi_init_row_covers_trailing_initializer() {
    // Oracle 4:22|5 — the row is `y = 1`, the entry after the throwing
    // one; the body never folds in.
    let src = "class A {\n  int x;\n  int y;\n  A() : x = throw 0, y = 1 {\n    x;\n  }\n}\n";
    assert_eq!(dead_of(src), vec!["y = 1".to_string()],);
}

#[test]
fn ctor_three_inits_row_is_first_after_throw() {
    // Oracle 5:22|5 — `y = 1`; `z = 2` continues the one dead interval.
    let src = "class A {\n  int x;\n  int y;\n  int z;\n  A() : x = throw 0, y = 1, z = 2 {}\n}\n";
    assert_eq!(dead_of(src), vec!["y = 1".to_string()]);
}

#[test]
fn ctor_assert_entry_rows_whole_call() {
    // Oracle 4:22|9 — the assert entry's own extent `assert(y)`;
    // `z = 2` folds.
    let src = "class A {\n  int x;\n  bool y;\n  A() : x = throw 0, assert(y), z = 2 {}\n}\n";
    assert_eq!(dead_of(src), vec!["assert(y)".to_string()]);
}

#[test]
fn ctor_throw_mid_list_next_entry_owns_row() {
    // Oracle 4:33|5 — `z = throw 0` itself is live; `w = 3` is the first
    // dead node and its entry the row.
    let src =
        "class A {\n  bool y;\n  int z;\n  int w;\n  A() : assert(y), z = throw 0, w = 3 {}\n}\n";
    assert_eq!(dead_of(src), vec!["w = 3".to_string()]);
}

#[test]
fn ctor_throw_in_last_entry_keeps_body_block_row() {
    // Oracle 4:28|12 — no entry after the throw: the body block anchors
    // (#3073's face) and rows its full extent, `}` included.
    let src = "class A {\n  int x;\n  int y;\n  A() : y = 1, x = throw 0 {\n    x;\n  }\n}\n";
    let dead = dead_of(src);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].starts_with('{'), "{dead:?}");
    assert!(dead[0].ends_with('}'), "{dead:?}");
}

// —— updater-region extents ——————————————————————————————————————————————

#[test]
fn element_postfix_updater_row_spans_operator() {
    // Oracle 1:26|3 — `i++`, the operator included.
    let src = "f() => [for (var i = 0;; i++) throw ''];";
    assert_eq!(dead_of(src), vec!["i++".to_string()]);
}

#[test]
fn element_prefix_updater_row_spans_operator() {
    // Oracle 1:26|3 — `++i`.
    let src = "f() => [for (var i = 0;; ++i) throw ''];";
    assert_eq!(dead_of(src), vec!["++i".to_string()]);
}

#[test]
fn element_method_updater_row_spans_call_parens() {
    // Oracle 1:26|12 — `i.toString()`; the trailing `()` rides the
    // updaters' end token.
    let src = "f() => [for (var i = 0;; i.toString()) throw ''];";
    assert_eq!(dead_of(src), vec!["i.toString()".to_string()]);
}

#[test]
fn element_index_updater_row_spans_bracket() {
    // Oracle 1:33|9 — `values[0]`; the `]` closer rides the updaters'
    // end token.
    let src = "f(List<int> values) => [for (;; values[0]) throw ''];";
    assert_eq!(dead_of(src), vec!["values[0]".to_string()]);
}

#[test]
fn element_implicit_creation_row_spans_parens() {
    // Oracle 2:17|3 — `C()`; the implicit-creation argument list rides
    // the updaters' end token.
    let src = "class C {}\nf() => [for (;; C()) throw ''];";
    assert_eq!(dead_of(src), vec!["C()".to_string()]);
}

#[test]
fn element_paren_updater_row_begins_at_grouping() {
    // Oracle 1:26|8 — the row BEGINS at the grouping `(` (the updaters'
    // beginToken, dead_code_verifier.dart:361-375).
    let src = "f() => [for (var i = 0;; (i).sign) throw ''];";
    assert_eq!(dead_of(src), vec!["(i).sign".to_string()]);
}

#[test]
fn element_updater_throw_mid_list_reports_last_updater() {
    // Oracle 1:29|1 — the element twin of the statement-form trim
    // (test_flowEnd_forElementParts_updaters_throw).
    let src = "f() => [for (;; 0, throw 1, 2) 0];";
    assert_eq!(dead_of(src), vec!["2".to_string()]);
}

#[test]
fn initializer_throw_trims_at_last_updater() {
    // Oracle 2:17|7 — `true; 1`: the condition is the first dead node
    // (`parent is ForParts`), the row ends at the last updater's end and
    // the covering `) {}` never folds in
    // (test_flowEnd_forParts_initializer_expression_throw).
    let src = "void f() {\n  for (throw 0; true; 1) {}\n}";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

#[test]
fn element_initializer_throw_trims_at_last_updater() {
    // Oracle 1:23|7 — the element twin
    // (test_flowEnd_forElementParts_initializer_expression_throw).
    let src = "f() => [for (throw 0; true; 1) 0];";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

#[test]
fn declaration_throw_trims_at_last_updater() {
    // Oracle 2:25|7 (test_flowEnd_forParts_initializer_declaration_throw).
    let src = "void f() {\n  for (var i = throw 0; true; 1) {}\n}";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

#[test]
fn element_declaration_throw_trims_at_last_updater() {
    // Oracle 1:31|7
    // (test_flowEnd_forElementParts_initializer_declaration_throw).
    let src = "f() => [for (var i = throw 0; true; 1) 0];";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

#[test]
fn element_pattern_initializer_throw_trims_at_last_updater() {
    // Oracle 1:33|7
    // (test_flowEnd_forElementParts_initializer_pattern_throw).
    let src = "f() => [for (var (a) = throw 0; true; 1) 0];";
    assert_eq!(dead_of(src), vec!["true; 1".to_string()]);
}

#[test]
fn dead_condition_argument_keeps_covering_fold() {
    // Oracle 2:21|9 — `1); 2) {}`: the first dead node is the throw's
    // sibling ARGUMENT (its parent is an ArgumentList — the ForParts trim
    // misses), so the row keeps the covering close's full fold
    // (test_flowEnd_forParts_condition_throw).
    let src = "void f(bool Function(Object?, Object?) g) {\n  for (; g(throw 0, 1); 2) {}\n}";
    assert_eq!(dead_of(src), vec!["1); 2) {}".to_string()]);
}

#[test]
fn element_dead_condition_argument_keeps_covering_fold() {
    // Oracle 1:60|10 — the element twin
    // (test_flowEnd_forElementParts_condition_throw).
    let src = "f(bool Function(Object?, Object?) g) => [for (; g(throw 0, 1); 2) 0];";
    assert_eq!(dead_of(src), vec!["1); 2) 0];".to_string()]);
}
