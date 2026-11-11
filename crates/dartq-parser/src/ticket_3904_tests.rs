//! Ticket #3904 — parser residual recovery family (census rows routed from
//! #3898): three shared recovery roots, each a different mechanism.
//!
//! 1. **Type-argument group recovery flag** (`type_args.rs`,
//!    `scan_type_group`). The official group scan folds each member type's
//!    own `recovered` flag into the group's (`recovered = recovered |
//!    typeInfo.recovered`, type_info_impl.dart:1357). Without the fold a
//!    group whose nested argument list recovered (`<e, 1>`) still looked
//!    clean, so `a(b < c, d < e, 1 >> (2))` parsed as a generic invocation
//!    and fabricated `expected_type_name` /
//!    `non_type_as_type_argument` rows; the oracle reads it as relational
//!    expressions (`computeMethodTypeArguments` / `parseSend` take type
//!    arguments only from an unrecovered group, type_info.dart:486-494).
//! 2. **Formal-parameter malformed type reference** (`formal_params.rs`).
//!    `parseFormalParameter` re-computes a `noType` probe in the required
//!    position when its follower is a period — or an identifier directly
//!    followed by one (parser_impl.dart:2142-2149) — so `void f(io., a)`
//!    admits the `id.` prefixed shape and reports at the missing name
//!    instead of `Expected to find ')'` at the period.
//! 3. **Type-reference identifier recovery template** (`recovery.rs` +
//!    `types.rs`). The whole type-reference family carries
//!    `recoveryTemplate: diag.expectedType` (identifier_context_impl.dart:
//!    1486-1508) and keeps the type-argument group delimiters current
//!    (:1533-1552); a `.`-prefixed type with its prefix missing is admitted
//!    in the required position (type_info.dart:193-202). `<.[]` therefore
//!    reports two `expected_type_name` rows (the `.` and the `[]` group)
//!    and `[<y.<z>(){}]` reports `expected_type_name` at the inner `<`,
//!    never `missing_identifier`.

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

/// Parsed rows `(code, start, end)` plus the identifier events.
struct Out {
    errors: Vec<(String, u32, u32)>,
    idents: Vec<(String, u32, u32, usize)>,
}

fn parse(source: &'static str) -> Out {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
        idents: parser.listener.idents.clone(),
    }
}

fn has(out: &Out, code: &str, start: u32, end: u32) -> bool {
    out.errors
        .iter()
        .any(|(c, s, e)| c == code && *s == start && *e == end)
}

// ------------------------------------------------------------------
// 1. type-argument group recovery flag
// ------------------------------------------------------------------

#[test]
fn relational_chain_is_not_a_generic_invocation() {
    // `main() {\n  a(b < c, d < e, 1 >> (2));\n}\n`
    // The inner group `<e, 1>` recovers (`1` is not a type), so the outer
    // group after `b` recovers too and `parseSend` refuses the type
    // arguments: the oracle reports only semantic rows for this input
    // (`undefined_identifier` for b/c/d/e, `undefined_function` for a), so
    // the parser tier must stay silent.
    let out = parse("main() {\n  a(b < c, d < e, 1 >> (2));\n}\n");
    assert!(
        out.errors.is_empty(),
        "relational chain must not report parser rows, got {:?}",
        out.errors
    );
    // No type-reference identifier events either: `b`, `c`, `d`, `e` are
    // ordinary expression operands.
    assert!(
        !out.idents.iter().any(|(ctx, ..)| ctx == "TypeReference"),
        "relational operands must not be type references: {:?}",
        out.idents
    );
}

#[test]
fn generic_invocation_still_takes_type_arguments() {
    // Control for the fold: `a(b<c>(2))` keeps `c` as a type argument
    // (oracle: `non_type_as_type_argument` for `c`, no parser row).
    let out = parse("main() {\n  a(b<c>(2));\n}\n");
    assert!(out.errors.is_empty(), "got {:?}", out.errors);
    // `c` sits at offset 15 in `main() {\n  a(b<c>(2));\n}\n`.
    assert!(
        out.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 15 && *e == 16),
        "expected a TypeReference identifier on `c` (15,16): {:?}",
        out.idents
    );
}

// ------------------------------------------------------------------
// 2. formal-parameter malformed prefixed type
// ------------------------------------------------------------------

#[test]
fn partial_formal_parameter_type_reports_missing_name() {
    // `void f(io.,a) {}` — oracle: `expected_type_name` at the `,` (the
    // prefixed type's missing continuation name) plus `missing_identifier`
    // at the same token (the parameter name), and no `expected_token` row.
    let out = parse("void f(io.,a) {}\n");
    assert!(
        has(&out, "expected_type_name", 10, 11),
        "expected expected_type_name at the `,` (10,11): {:?}",
        out.errors
    );
    assert!(
        has(&out, "missing_identifier", 10, 11),
        "expected missing_identifier at the `,` (10,11): {:?}",
        out.errors
    );
    assert!(
        !out.errors.iter().any(|(c, ..)| c == "expected_token"),
        "the `.` must not surface as formal-parameter junk: {:?}",
        out.errors
    );
}

#[test]
fn prefixed_formal_parameter_type_parses() {
    // `void f(io.x) {}` — `io.x` is the parameter's type and the parameter
    // name is missing: the oracle reports `missing_identifier` at the `)`
    // (plus the semantic `undefined_class` row) and never `expected_token`.
    let out = parse("void f(io.x) {}\n");
    assert!(
        has(&out, "missing_identifier", 11, 12),
        "expected missing_identifier at the `)` (11,12): {:?}",
        out.errors
    );
    assert!(
        !out.errors.iter().any(|(c, ..)| c == "expected_token"),
        "the `.` must not surface as formal-parameter junk: {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 3. type-reference recovery template (`<.` partial-type family)
// ------------------------------------------------------------------

#[test]
fn typed_list_literal_partial_type_reports_type_names() {
    // `var v = <.[];` — oracle: `expected_type_name` at the `.` (9,10) and
    // at the `[]` group (10,12); the group recovers into the list literal
    // suffix, so no `expected_token '['` row exists.
    let out = parse("var v = <.[];\n");
    assert!(
        has(&out, "expected_type_name", 9, 10),
        "expected expected_type_name at the `.` (9,10): {:?}",
        out.errors
    );
    assert!(
        has(&out, "expected_type_name", 10, 12),
        "expected expected_type_name at the `[]` group (10,12): {:?}",
        out.errors
    );
    assert!(
        !out.errors
            .iter()
            .any(|(c, s, _)| c == "missing_identifier" && *s == 10),
        "the type-reference continuation must not report missing_identifier: {:?}",
        out.errors
    );
}

#[test]
fn list_literal_element_partial_type_reports_type_name() {
    // `var v = [<y.<z>(){}];` — the prefixed type `y.` is missing its
    // continuation name, so the inner `<` carries `expected_type_name`
    // (12,13); the oracle's only other row is the `expected_token '>'` of
    // the recovered group, never a `missing_identifier`.
    let out = parse("var v = [<y.<z>(){}];\n");
    assert!(
        has(&out, "expected_type_name", 12, 13),
        "expected expected_type_name at the inner `<` (12,13): {:?}",
        out.errors
    );
    assert!(
        !out.errors.iter().any(|(c, ..)| c == "missing_identifier"),
        "the type-reference continuation must not report missing_identifier: {:?}",
        out.errors
    );
}
