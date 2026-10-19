//! Long-tail parser-recovery pins (#2200): real-corpus faces from the
//! drift/dio/xml/petitparser packages where the parser derailed while the
//! oracle parsed cleanly.
//!
//! Families and fixes:
//! - conditional expression continuing an `is` test with type arguments —
//!   `compute_type_after_is_or_as` now applies the official per-shape
//!   `asNonNullable` (`type_info_impl.dart:219/319/575`) and `is_nullable`
//!   reads `ComplexType.before_question` (`type_info_impl.dart:598`).
//! - function expressions in constructor initializer expressions —
//!   `parse_parenthesized_expression_function_literal_or_record_literal`
//!   re-enables `mayParseFunctionExpressions` for the parenthesized/record
//!   reading (`parser_impl.dart:8272-8279`), so closures nested inside a
//!   group parse despite the initializer-list gate.
//! - `>>`-anchored `Expected ';'` rows in the same files were re-parse
//!   cascades of the two faces above; they close with them.

use super::collector::{Collector, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    std::mem::take(p.listener)
}

fn codes(c: &Collector) -> Vec<String> {
    c.error_codes()
}

// —— conditional after `is` with type arguments ————————————————————————————

#[test]
fn conditional_after_is_with_generic_type() {
    // xml sequence.dart:45 — `items is List<XPathItem> ? items : items.toList()`;
    // the complex type shape went through the nullable path and swallowed the
    // conditional `?`.
    let c = parse("void f(items) { final list = items is List<int> ? items : items.toList(); }");
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

#[test]
fn assignment_conditional_after_is_with_generic_type() {
    // drift update.dart:144-146 — `columns is Map<String, Variable> ? A : B`.
    let c = parse("void f(columns) { x = columns is Map<String, int> ? a.of(columns) : columns; }");
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

#[test]
fn is_nullable_complex_type_kept_before_hard_follower() {
    // `?` stays part of the type before a hard follower (`;`).
    let c = parse("void f(x) { y = x is List<int>? ; }");
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

// —— function expressions in constructor initializers ——————————————————————

#[test]
fn closure_in_initializer_expression() {
    // dio multipart_file.dart:35 — `: _dataBuilder = (() => stream)`.
    let c = parse("class C { C(int stream) : d = (() => stream); }");
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

#[test]
fn closure_with_params_in_initializer_expression() {
    // drift insert.dart:454 — `: _creator = ((old, _) => update(old))`.
    let c = parse("class C { C(update) : d = ((old, _) => update(old)), e = false; }");
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

#[test]
fn initializer_closures_after_function_typed_params() {
    // drift insert.dart:450-456 — function-typed optional params then
    // closure initializers on the initializer list.
    let c = parse(
        "class C<R, S> {\n  C(Insertable<R> Function(S old) update,\n      {Expression<bool> Function(S old)? where})\n      : creator = ((old, _) => update(old)),\n        where = where == null ? null : ((old, _) => WhereClause(where(old)));\n}",
    );
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

#[test]
fn initializer_closure_after_gtgt_parameter() {
    // dio multipart_file.dart:32-36 — `>>`-closing parameter group precedes
    // the initializer list.
    let c = parse(
        "class C {\n  C(this.filename, M? contentType, Map<String, List<String>>? headers)\n      : d = (() => stream),\n        h = m(headers);\n}",
    );
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

// —— IIFE as assert argument ———————————————————————————————————————————————

#[test]
fn iife_in_assert_initializer() {
    // petitparser path.dart:13 — `assert((() { ... return true; }()))`.
    let c = parse(
        "class P {\n  P(p, i)\n      : assert(p.isNotEmpty, 'x'),\n        assert((() {\n          for (var i = 0; i < 1; i++) {\n            if (p[i] != i) return false;\n          }\n          return true;\n        }()));\n}",
    );
    assert!(c.errors.is_empty(), "{:?}", codes(&c));
}

// —— real-corpus locks —————————————————————————————————————————————————————
// The exact files that carried the 19 divergent rows, pinned so the recovery
// decisions stay aligned with the oracle's clean parse.

const DIO_IO_PROGRESS: &str =
    include_str!("../../../../tests/corpus/real/dio/lib/src/progress_stream/io_progress_stream.dart");
const DIO_MULTIPART: &str =
    include_str!("../../../../tests/corpus/real/dio/lib/src/multipart_file.dart");
const DRIFT_UPDATE: &str =
    include_str!("../../../../tests/corpus/real/drift/lib/src/runtime/query_builder/statements/update.dart");
const DRIFT_INSERT: &str =
    include_str!("../../../../tests/corpus/real/drift/lib/src/runtime/query_builder/statements/insert.dart");
const DRIFT_INSERT3: &str =
    include_str!("../../../../tests/corpus/real/drift/lib/src/drift3_preview/src/query_builder/statements/insert.dart");
const XML_SEQUENCE: &str =
    include_str!("../../../../tests/corpus/real/xml/lib/src/xpath/xdm/sequence.dart");
const PP_PATH: &str =
    include_str!("../../../../tests/corpus/real/petitparser/lib/src/reflection/internal/path.dart");

fn assert_target_codes_absent(name: &str, src: &'static str) {
    let mut p = setup(src);
    p.parse_unit();
    let c = std::mem::take(p.listener);
    let offenders: Vec<_> = c
        .errors
        .iter()
        .filter(|(code, _, _)| {
            matches!(
                code.as_str(),
                "expected_token" | "unexpected_token" | "missing_identifier"
            )
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "{name}: target recovery codes present: {offenders:?} (all: {:?})",
        c.errors
    );
}

#[test]
fn corpus_dio_io_progress_stream_parses_clean() {
    assert_target_codes_absent("dio io_progress_stream", DIO_IO_PROGRESS);
}

#[test]
fn corpus_drift_update_parses_clean() {
    assert_target_codes_absent("drift update", DRIFT_UPDATE);
}

#[test]
fn corpus_drift_insert_parses_clean() {
    assert_target_codes_absent("drift insert", DRIFT_INSERT);
}

#[test]
fn corpus_drift_insert3_preview_parses_clean() {
    assert_target_codes_absent("drift insert3_preview", DRIFT_INSERT3);
}

#[test]
fn corpus_xml_sequence_parses_clean() {
    assert_target_codes_absent("xml sequence", XML_SEQUENCE);
}

#[test]
fn corpus_petitparser_path_parses_clean() {
    assert_target_codes_absent("petitparser path", PP_PATH);
}

#[test]
fn corpus_dio_multipart_file_target_codes_absent() {
    // The oracle also reports four EXTRANEOUS_MODIFIER rows in this file
    // (68/90/137/151); those are pinned elsewhere — only the recovery codes
    // are in scope here.
    let mut p = setup(DIO_MULTIPART);
    p.parse_unit();
    let c = std::mem::take(p.listener);
    let unexpected: Vec<_> = c
        .errors
        .iter()
        .filter(|(code, _, _)| {
            matches!(
                code.as_str(),
                "expected_token" | "unexpected_token" | "missing_identifier"
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "dio multipart_file: target recovery codes present: {unexpected:?} (all: {:?})",
        c.errors
    );
    assert_eq!(
        c.error_codes(),
        vec!["extraneous_modifier"; 4],
        "dio multipart_file: full error surface drifted"
    );
}
