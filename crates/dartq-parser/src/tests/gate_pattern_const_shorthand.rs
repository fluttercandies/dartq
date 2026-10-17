//! The `const .a` dot-shorthand gate's pattern-vs-expression face split
//! (#934 review follow-up): below dot-shorthands' 3.10 the gate anchors
//! follow the official decision order (`parser_impl.dart:7042-7053`) —
//!
//! - **Pattern faces** (`case const .a:`, `if (o case const .a)`, list/map
//!   pattern elements, `const .new`): the oracle reports exactly ONE
//!   `experiment_not_enabled` row at the dot. `parsePrimaryPattern`'s const
//!   arm enters the expression engine with `const` already the current
//!   token, so the `_isDotShorthand` entry check takes the plain
//!   (non-const) branch and `endConstDotShorthand`'s const-keyword anchor
//!   never fires. dartq mirrors the split by consuming `const` in the
//!   pattern dispatcher (`parse_primary_pattern`) before the expression
//!   engine runs.
//! - **Expression faces** (`var x = const .a;`, arguments, assignments,
//!   returns) **and the expression-engine faces inside patterns** where the
//!   operand's `const` is still ahead of the cursor — duplicate `const
//!   const .a`, the negated `-const .a`, relational operands
//!   (`> const .a`), and map-key expressions (`{const .a: 0}`) — the
//!   oracle reports BOTH rows (dot + `const` keyword): `_isDotShorthand`'s
//!   const branch runs and `endConstDotShorthand` fires. dartq reaches
//!   these through `parse_pattern_primary`'s `Const` arm →
//!   `parse_const_expression`'s const-dot branch, whose const-keyword
//!   anchor (`expr_const.rs`, `report_dot_shorthands_gate(const_keyword)`)
//!   is load-bearing for exactly these faces — removing it to chase the
//!   pattern faces would diverge here.
//!
//! Oracle ground truth: dart 3.13.1, package pinned `sdk: ^3.9.0`,
//! `dart analyze --format=machine` `EXPERIMENT_NOT_ENABLED` rows per face.

use super::collector::{Collector, scan_ok};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

/// Parses `source` at language version `major.minor` and returns the error
/// rows in emission order as `(code, start, end)` UTF-16 tuples.
fn rows_at(source: &str, major: u32, minor: u32) -> Vec<(String, u32, u32)> {
    let source: &'static str = Box::leak(source.to_string().into_boxed_str());
    let (table, text) = scan_ok(source);
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(major, minor),
    ));
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// The offset of the `.a`/`.new` shorthand head's dot.
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn dot_of(source: &str) -> u32 {
    let at = source
        .rfind(".a")
        .or_else(|| source.rfind(".new"))
        .expect("shorthand dot");
    at as u32
}

fn gate(code: &str, at: u32, len: u32) -> (String, u32, u32) {
    (code.to_string(), at, at + len)
}

/// Every plain pattern face reports the dot and nothing else at 3.9 — the
/// const-keyword anchor never fires when the pattern dispatcher consumed
/// `const` before the expression engine (oracle: one row per face, probed
/// shapes at the cited lines). The selector chain's last link is the one
/// exception: `.b` parses as a plain const-context send whose `:` follower
/// cannot complete a const constructor invocation, so the official
/// explicit-const gate (`parseSend` :9413-9423) adds its
/// `invalidConstantPatternConstPrefix` row on the link's last consumed
/// token — exactly one gate row, then the context is reset (parseSend-local).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn pattern_const_shorthand_reports_the_dot_only_below_3_10() {
    let cases: [(&str, &str, Option<&str>); 8] = [
        (
            "switch case",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .a:\n      break;\n  }\n}\n",
            None,
        ),
        (
            "if-case",
            "enum E { a }\nvoid f(Object o) {\n  if (o case const .a) {}\n}\n",
            None,
        ),
        (
            "list element",
            "enum E { a }\nvoid f(Object o) {\n  switch (0) {\n    case [const .a]:\n      break;\n  }\n}\n",
            None,
        ),
        (
            "map value pattern",
            "enum E { a }\nvoid f(Object o) {\n  switch (0) {\n    case {0: const .a}:\n      break;\n  }\n}\n",
            None,
        ),
        (
            "new head",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .new:\n      break;\n  }\n}\n",
            None,
        ),
        (
            "invocation",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .a():\n      break;\n  }\n}\n",
            None,
        ),
        (
            "selector chain",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .a.b:\n      break;\n  }\n}\n",
            Some(".b"),
        ),
        (
            "null assert suffix",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .a!:\n      break;\n  }\n}\n",
            None,
        ),
    ];
    for (label, source, chain_link) in cases {
        let dot = dot_of(source);
        let mut expected = vec![gate("experiment_not_enabled", dot, 1)];
        if let Some(link) = chain_link {
            let at = source.rfind(link).expect("chain link") as u32 + 1;
            expected.push(gate("invalid_constant_const_prefix", at, 1));
        }
        assert_eq!(
            rows_at(source, 3, 9),
            expected,
            "{label}: the 3.9 face rows diverged"
        );
    }
}

/// Expression faces report both anchors at 3.9: the deduped head+context
/// row at the dot plus the `endConstDotShorthand` row at the `const`
/// keyword (oracle rows cited in `gate_m0_wave10`'s const pin; re-probed for
/// each face here).
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn expression_const_shorthand_reports_dot_and_const_below_3_10() {
    let cases: [(&str, &str); 5] = [
        ("declaration", "enum E { a }\nfinal x = const .a;\n"),
        (
            "argument",
            "enum E { a }\nvoid g(Object x) {}\nvoid f() {\n  g(const .a);\n}\n",
        ),
        (
            "assignment",
            "enum E { a }\nvoid f(Object x) {\n  x = const .a;\n}\n",
        ),
        (
            "return",
            "enum E { a }\nObject f() {\n  return const .a;\n}\n",
        ),
        (
            "collection element",
            "enum E { a }\nvoid f() {\n  var x = [const .a];\n}\n",
        ),
    ];
    for (label, source) in cases {
        let dot = dot_of(source);
        let const_kw = source.find("const").expect("const keyword") as u32;
        assert_eq!(
            rows_at(source, 3, 9),
            vec![
                gate("experiment_not_enabled", dot, 1),
                gate("experiment_not_enabled", const_kw, 5),
            ],
            "{label}: dot head anchor + const keyword anchor"
        );
    }
}

/// The expression-engine faces inside patterns — where the operand's
/// `const` is still ahead of the expression engine's cursor — take the
/// const-dot branch and report BOTH anchors, exactly like the oracle:
/// duplicate `const const .a` (plus the duplicate-`const` report), the
/// negated operand, the relational operand, and the map-key expression.
/// These faces are why the const-keyword anchor in
/// `parse_const_expression` must stay.
#[test]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn pattern_operand_expression_faces_report_both_anchors_below_3_10() {
    // `const const .a`: duplicate-const report on the second keyword, then
    // the dot and second-keyword gate anchors.
    let source = "enum E { a }\nvoid f() {\n  switch (0) {\n    case const const .a:\n      break;\n  }\n}\n";
    let second_const = source.rfind("const").expect("second const") as u32;
    let dot = dot_of(source);
    assert_eq!(
        rows_at(source, 3, 9),
        vec![
            gate("invalid_constant_pattern_duplicate_const", second_const, 5),
            gate("experiment_not_enabled", dot, 1),
            gate("experiment_not_enabled", second_const, 5),
        ],
        "duplicate const + both gate anchors"
    );

    for (label, source) in [
        (
            "negated operand",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case -const .a:\n      break;\n  }\n}\n",
        ),
        (
            "relational operand",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case > const .a:\n      break;\n  }\n}\n",
        ),
        (
            "map key expression",
            "enum E { a }\nvoid f() {\n  switch (0) {\n    case {const .a: 0}:\n      break;\n  }\n}\n",
        ),
    ] {
        let dot = dot_of(source);
        let const_kw = source.find("const").expect("const keyword") as u32;
        assert_eq!(
            rows_at(source, 3, 9),
            vec![
                gate("experiment_not_enabled", dot, 1),
                gate("experiment_not_enabled", const_kw, 5),
            ],
            "{label}: both gate anchors"
        );
    }
}

/// The default (latest) face stays silent on every pinned shape: the gate
/// only arms below 3.10.
#[test]
fn const_shorthand_gate_is_silent_at_latest_version() {
    let pattern =
        "enum E { a }\nvoid f() {\n  switch (0) {\n    case const .a:\n      break;\n  }\n}\n";
    let expression = "enum E { a }\nfinal x = const .a;\n";
    assert_eq!(rows_at(pattern, 3, 15), []);
    assert_eq!(rows_at(expression, 3, 15), []);
}
