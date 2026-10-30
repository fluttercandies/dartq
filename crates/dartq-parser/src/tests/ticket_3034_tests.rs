//! #3034 pins: the expression-tail residual — infix `~` parses as a binary
//! expression with zero parser rows, matching the oracle.
//!
//! Root (upstream pin `.third/dart-sdk-3.13.7`
//! `pkg/_fe_analyzer_shared/lib/src/`):
//!
//! * `_computePrecedence` (`parser_impl.dart:7707-7744`): the default arm
//!   returns `type.precedence`, and `TILDE` carries `PREFIX_PRECEDENCE` in
//!   the token table (`token.dart:2276-2283`) — the only token reaching the
//!   precedence loop at the prefix tier (`!` shares the tier upstream but is
//!   remapped to POSTFIX/SELECTOR at :7709-7719 before it can arrive). In the
//!   loop no arm matches level 15, so infix `~` lands in the plain-binary
//!   else arm (:7340-7364): `beginBinaryExpression(~)` fires, the RHS parses
//!   at level 16, and the whole `5 ~ 3` is an ordinary BinaryExpression —
//!   the `not_binary_operator` row is the analyzer's *semantic* face
//!   (`binary_expression_resolver.dart:67-74`), never a parser row. dartq's
//!   `precedence_of` returned `None` for `~`, so `var a = 5 ~ 3;` broke out
//!   of the expression loop at `~` and the declaration layer minted the
//!   `expected_token ';'` + `expected_executable` ×2 + `unexpected_token ';'`
//!   cascade the recon buckets as B8 `semi:other` / B14 `unexpected:';'`.

use super::collector::{Collector, latest_features, scan_ok};
use crate::ParserFeatures;
use dartq_language::{Feature, FeatureSet};

fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

fn anon_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(
        FeatureSet::latest().with_experiment(Feature::AnonymousMethods),
    )
}

/// `var a = 5 ~ 3;` (oracle `diagnostics/not_binary_operator/doc_0`,
/// `NonBinaryOperatorTest/test_unaryTilde`): the oracle's parser emits ZERO
/// rows — `~` reaches the binary else arm at PREFIX_PRECEDENCE and the whole
/// `5 ~ 3` is a BinaryExpression; the single oracle row is the semantic
/// `not_binary_operator`, not a parser face.
#[test]
fn unary_tilde_in_infix_parses_as_binary() {
    let rows = rows_with("var a = 5 ~ 3;\n", latest_features());
    assert!(
        rows.is_empty(),
        "infix `~` must parse as binary with zero parser rows, got {rows:?}"
    );
}

/// The same in-statement and chained: `1 ~ 2 ~ 3` left-associates through the
/// else arm (level stays 15), still zero rows.
#[test]
fn unary_tilde_infix_chain_parses_clean() {
    let rows = rows_with("void f() {\n  var b = 1 ~ 2 ~ 3;\n}\n", latest_features());
    assert!(
        rows.is_empty(),
        "chained infix `~` must parse clean, got {rows:?}"
    );
}

/// `~x ~ ~y`: prefix and infix `~` interleave — the infix `~` continues the
/// binary walk while each operand's head `~` keeps the prefix arm.
#[test]
fn prefix_and_infix_tilde_mix_parses_clean() {
    let rows = rows_with("var a = ~x ~ ~y;\n", latest_features());
    assert!(
        rows.is_empty(),
        "prefix+infix `~` mix must parse clean, got {rows:?}"
    );
}

/// `x ? a ~ b : c` — infix `~` inside a conditional then-arm terminates at
/// `:` (no precedence), so the conditional still parses clean.
#[test]
fn infix_tilde_inside_conditional_arm_parses_clean() {
    let rows = rows_with("void f() {\n  var z = c ? a ~ b : d;\n}\n", latest_features());
    assert!(
        rows.is_empty(),
        "infix `~` inside a conditional arm must parse clean, got {rows:?}"
    );
}

/// `x = 5 ~` (missing RHS): the binary arm's RHS parse recovers through
/// `parse_primary` → `ensure_identifier` — exactly one `missing_identifier`
/// at the `;`, the oracle's face for a dangling operator.
#[test]
fn infix_tilde_missing_rhs_reports_once() {
    let rows = rows_with("var a = 5 ~ ;\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 12, 13)),
        "one missing_identifier at the `;` for `5 ~ ;`, got {rows:?}"
    );
    assert_eq!(rows.len(), 1, "no extra rows, got {rows:?}");
}

/// Flag-on `return foo..{ return foo; }` (oracle
/// `anonymous_methods/block/cascaded_test`): the cascade opens an anonymous
/// method and the inner `return` is its body — zero parser rows.
#[test]
fn flag_on_return_inside_anonymous_method_body_parses_clean() {
    let rows = rows_with(
        "f() {\n  return foo..{\n    return foo;\n  };\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on `..{{ return ...; }}` must parse clean, got {rows:?}"
    );
}

/// Flag-off `return foo..{ return foo; }` — the oracle's ordinary-selector
/// noise face: `expected_token ';'` at the `..`, `missing_identifier` at
/// the `{`, nothing else (the `return` inside the braces parses as a
/// statement inside the recovered set-literal body).
#[test]
fn flag_off_return_cascade_matches_oracle_noise() {
    let rows = rows_with(
        "f() {\n  return foo..{\n    return foo;\n  };\n}\n",
        latest_features(),
    );
    assert_eq!(
        rows,
        vec![
            ("missing_identifier".to_string(), 20, 21),
            ("expected_token".to_string(), 18, 20),
        ],
        "flag-off cascade noise face"
    );
}

/// `return []..method();` — a cascade on a literal receiver: the cascade
/// section opens after ANY expression (upstream `parseCascadeExpression`
/// :7091-7119), including a list literal — zero parser rows.
#[test]
fn return_list_cascade_parses_clean() {
    let rows = rows_with("f() {\n  return []..method();\n}\n", latest_features());
    assert!(
        rows.is_empty(),
        "return `[]..m()` must parse clean, got {rows:?}"
    );
}
