//! The `super?.` initializer left-paren recovery (#2800, handed over from
//! #2790): the official `parseSuperInitializerExpression` `?.` branch is
//! sequential, not an alternative — after it steps over `?.` and the
//! (possibly synthetic) member name, the `=`/`(` checks re-run on the
//! follower (upstream Dart SDK source, parser_impl.dart:4934-4953), so
//! `super?. <follower>` with no `(` reports `expectedAfterButGot('(')` at
//! the follower and wraps the member name in empty synthetic parens
//! (`insertParens(includeIdentifier: false)`). The `invalid_operator_
//! questionmark_period_for_super` row still rides the expression re-read
//! (`parseSuperExpression`, parser_impl.dart:7698+). Every pinned span is
//! the frozen oracle row (conformance/expected.tsv,
//! ConstructorTest/test_constructor_super_qdot_*).

use dartq_scanner::ScanConfig;

use super::collector::{Collector, latest_features};
use crate::ParserFeatures;
use crate::parser::Parser;

/// Parses `source` and returns the scanner+parser rows as
/// `(code, start, end)` sorted by offset (gate_bi_name_faces harness shape).
fn rows_with(features: ParserFeatures, source: &'static str) -> Vec<(String, u32, u32)> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let mut rows: Vec<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows.sort_by(|a, b| (a.1, a.2, &a.0).cmp(&(b.1, b.2, &b.0)));
    rows
}

fn rows(source: &'static str) -> Vec<(String, u32, u32)> {
    rows_with(latest_features(), source)
}

/// The parser-tier face set for `source` equals `expected` exactly: the
/// oracle's parser faces, in offset order. (Semantic-tier faces —
/// `undefined_constructor_in_initializer`, `const_instance_field`,
/// `undefined_annotation`, … — are outside this crate and stay out of the
/// pins.) `qdot` is the shared `super?.` head prefix, 20 code units long
/// (`super?.` ends at offset 22).
fn assert_faces(source: &'static str, expected: &[(&str, u32, u32)]) {
    let rows = rows(source);
    let expected: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows, expected, "faces for {source:?}");
}

/// `super?.}` — synthetic member name before `}`, then the re-run `(` check
/// reports at `}` and wraps the name in empty parens; the body check
/// anchors on the same `}` (test_constructor_super_qdot_eof).
#[test]
fn super_qdot_eof_reports_paren_at_the_follower() {
    assert_faces(
        "class C {C() : super?.}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 22, 23), // '(' at `}`
            ("missing_function_body", 22, 23),
        ],
    );
}

/// `super?. var f;` — the reserved `var` is not a member name: a synthetic
/// name splices in silently, `(` is expected at `var`, and the `var f;`
/// field parses clean behind the body row (test_constructor_super_qdot_field).
#[test]
fn super_qdot_field_reports_paren_at_var() {
    assert_faces(
        "class C {C() : super?. var f;}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 23, 26), // '(' at `var`
            ("missing_function_body", 23, 26),
        ],
    );
}

/// `super?. const f = 0;` — same synthetic-name + `(`-expect pair; the
/// `const_instance_field` rider is semantic-tier and stays out
/// (test_constructor_super_qdot_fieldConst).
#[test]
fn super_qdot_field_const_reports_paren_at_const() {
    assert_faces(
        "class C {C() : super?. const f = 0;}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 23, 28), // '(' at `const`
            ("missing_function_body", 23, 28),
        ],
    );
}

/// `super?. int get a => 0;` — `int` is identifier-kind, so it IS the
/// member name and the re-run `(` check reports at the `get` follower
/// (test_constructor_super_qdot_getter).
#[test]
fn super_qdot_getter_reports_paren_after_the_real_name() {
    assert_faces(
        "class C {C() : super?. int get a => 0;}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 27, 30), // '(' at `get`
            ("missing_function_body", 27, 30),
        ],
    );
}

/// `super?. set a(b) {}` — built-in `set` is the member name; the `(` is
/// expected at `a`, and the trailing `a(b) {}` parses as a member
/// (test_constructor_super_qdot_setter).
#[test]
fn super_qdot_setter_reports_paren_at_the_method_name() {
    assert_faces(
        "class C {C() : super?. set a(b) {}}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 27, 28), // '(' at `a`
            ("missing_function_body", 27, 28),
        ],
    );
}

/// `super?. @annotation var f;` — the `@` follower takes the synthetic name
/// and the `(`-expect pair at `@`; the annotated field parses behind the
/// body row (test_constructor_super_qdot_annotation, the handed-over
/// annotated-parameter face).
#[test]
fn super_qdot_annotation_reports_paren_at_the_annotation() {
    assert_faces(
        "class C {C() : super?. @annotation var f;}",
        &[
            ("invalid_operator_questionmark_period_for_super", 20, 22),
            ("expected_token", 23, 24), // '(' at `@`
            ("missing_function_body", 23, 24),
        ],
    );
}

/// `super?.m();` — a well-formed null-aware super invocation reports only
/// the `?.`-on-super row: the re-run `(` check sees the real `(` and stays
/// silent (the official `?.` branch guard, :4951 `!next.isA(OPEN_PAREN)`).
#[test]
fn super_qdot_with_real_paren_reports_only_the_operator_row() {
    assert_faces(
        "class C {C() : super?.m();}",
        &[("invalid_operator_questionmark_period_for_super", 20, 22)],
    );
}
