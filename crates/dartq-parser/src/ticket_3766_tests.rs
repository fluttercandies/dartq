//! Ticket #3766 — `expected_token` parity pins. Each case is frozen to the
//! oracle 3.13.7 `(code, start, end)` row set produced by
//! `dart analyze --format=machine` on the pinned source. Clusters covered:
//!
//! * `await for (a }` / `for (a }` `in`-anchor — the `in` report and the
//!   synthetic `in` splice anchor on the FIRST non-zero-length lookahead
//!   (upstream `findNonZeroLengthToken`, util.dart:56), not the queued
//!   synthetic `)` the scanner parked at the recovery point.
//! * `catch (e, {}` / forin `missing_identifier` length — a zero-length
//!   synthetic `)` in the `identifier` slot borrows the first non-zero
//!   lookahead's length (`report_missing_identifier`), matching the
//!   upstream synthetic-lexeme-length report so `admit_row` dedups.
//! * `typedef F<@m T> = int Function<@m X>(@m int)` — metadata inside the
//!   GFT type-variable group no longer strands the alias body; the
//!   `degraded_gft_return_type_end` probe skips `@...(...)` before reading
//!   the type-variable name and the `(` follower.
//! * `Foo(bar: == () => 0)` inside a switch case — object/map/list/paren
//!   pattern field expressions re-enable `may_parse_function_expressions`
//!   (upstream `parseObjectPatternRest` :12227, `parseMapPatternSuffix`
//!   :12063, `parseParenthesizedPatternOrRecordPattern` :12149,
//!   `parseListPatternSuffix` :11983), and the pattern-primary `(` arm
//!   routes through the function-literal probe
//!   (`parseParenthesizedExpressionFunctionLiteralOrRecordLiteral`,
//!   :8188-8192) instead of the plain record literal.
//! * `f(x < y, (o as int) > (p as int))` — a parenthesized expression is
//!   not a type-argument entry; the `(` arm of `skip_type_len_in` validates
//!   the record-type interior (upstream
//!   `_checkIfRecordTypeParenthesisAreRecovered`,
//!   type_info_impl.dart:882-945) so `x<y, (o as int)>` stays a comparison.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features};
use dartq_scanner::ScanConfig;

/// Parses `source` and returns the scanner+parser rows as a set of
/// `(code, start, end)` (the oracle rows compare position-sorted).
fn rows(source: &'static str) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut rows: BTreeSet<(String, u32, u32)> = result
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
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn assert_rows(source: &'static str, expected: &[(&str, u32, u32)]) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows(source), expected, "rows for {source:?}");
}

#[test]
fn await_for_in_anchor_walks_past_synthetic_closer() {
    // Oracle `await for (a }`: `;` at 29-30 (`}` before `a`... offset table
    // below is the oracle byte range), `missing_identifier` + `)` + `in` all
    // at 31-32 (the real `}` the recovery parked the synthetic `)` on), and
    // `expected_executable` at 33-34. The `in` report must borrow the real
    // `}` length (1), not the queued synthetic `)`'s zero width.
    assert_rows(
        "void f() async* { await for (a } }\n",
        &[
            ("expected_token", 29, 30),
            ("missing_identifier", 31, 32),
            ("expected_token", 31, 32),
            ("expected_token", 31, 32),
            ("expected_executable", 33, 34),
        ],
    );
}

#[test]
fn await_for_in_anchor_var() {
    assert_rows(
        "void f() async* { await for (var a } }\n",
        &[
            ("expected_token", 33, 34),
            ("missing_identifier", 35, 36),
            ("expected_token", 35, 36),
            ("expected_token", 35, 36),
            ("expected_executable", 37, 38),
        ],
    );
}

#[test]
fn for_in_anchor_non_await() {
    // `for (a }` — non-await `(`-recovery splices `( ; ; )`, the `;` report
    // anchors at 16-17, `missing_identifier` + `)` at 18-19, and
    // `expected_executable` at 20-21.
    assert_rows(
        "void f() { for (a } }\n",
        &[
            ("expected_token", 16, 17),
            ("missing_identifier", 18, 19),
            ("expected_token", 18, 19),
            ("expected_executable", 20, 21),
        ],
    );
}

#[test]
fn for_in_clean_forms_silent() {
    assert_rows("void f() async* { await for (a in b) {} }\n", &[]);
    assert_rows("void f() { for (var a in b) {} }\n", &[]);
}

#[test]
fn typedef_gft_metadata_group() {
    // `typedef F<@m T> = int Function<@m X>(@m int);` — metadata inside the
    // GFT type-variable group parses clean (oracle: no parser rows). The
    // `@m` before `X`/the parameter no longer forces the degraded-alias arm
    // that stranded `';'` + `missing_function_body`.
    assert_rows("typedef F<@m T> = int Function<@m X>(@m int);\n", &[]);
}

#[test]
fn pattern_function_literal_operand() {
    // `Foo(bar: == () => 0)` — the `==` operand `() => 0` parses as a
    // function literal (flag re-enabled inside the object pattern); oracle
    // emits only the semantic `non_constant_relational_pattern_expression`
    // face, which the parser does not surface here.
    assert_rows("f(x) => switch(x) { Foo(bar: == () => 0) => 0 };\n", &[]);
}

#[test]
fn paren_expression_not_type_argument() {
    // `f(x < y, (o as int) > (p as int))` — `(o as int)` is a parenthesized
    // expression, not a record-type entry, so `x<y, (...)>` stays a
    // comparison and no `')'` is expected before `(p`.
    assert_rows(
        "g(int x, int y, Object o, Object p) { f(x < y, (o as int) > (p as int)); }\n",
        &[],
    );
}
