//! Long-tail parser-recovery pins (#2200): real-corpus faces from the
//! drift/dio/xml/petitparser packages where the parser derailed while the
//! oracle parsed cleanly. The full-package files the fixes were measured on
//! now live in the pub cache (see conformance/realworld.toml); the pins here
//! are the minimal reductions extracted from them, so the recovery decisions
//! stay pinned without vendoring package source.
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
