//! #3052 pins: Lane-E — index-expression junk recovery and the
//! nested-generic type-argument `>` boundary.
//!
//! Roots (upstream pin `.third/dart-sdk-3.13.7`,
//! `pkg/_fe_analyzer_shared/lib/src/parser/`):
//!
//! * Index junk (`parser_impl.dart:8017-8032`,
//!   `parseArgumentOrIndexStar`): the recovery row
//!   `expectedButGot(']')` fires at the follower UNCONDITIONALLY — the
//!   `endGroup.isSynthetic` check only chooses HOW the closer is adopted.
//!   A scanner-synthetic `]` is relocated onto the expression end
//!   (`rewriter.moveSynthetic` :8025-8027) so the junk run stays current
//!   for the enclosing recovery layer; a real `]` absorbs the junk run
//!   (skip-to-endGroup :8028-8030). dartq previously skipped to the parked
//!   synthetic silently (no row, junk swallowed to EOF, spurious
//!   post-parse unmatched row at the wrong anchor).
//! * Nested-generic boundaries: `computeTypeParamOrArg`
//!   (`type_info.dart:420`) validates a group through
//!   `ComplexTypeParamOrArgInfo.compute`, and `computeType` threads the
//!   caller's `inDeclaration` into the nested group after an identifier
//!   (type_info.dart:264-268) while `computeRest` hardcodes `true` for a
//!   GFT's own type-variable group (type_info_impl.dart:1095-1098). A
//!   rejected group leaves the type as the bare identifier
//!   (type_info.dart:380-384) and a broken `Function [<...>` chain without
//!   the `(` never updates `end` (`computeNoTypeGFT` :1061-1071 /
//!   `computeVoidGFT` :935-944 fall back to the bare head). dartq's
//!   `skip_type_len` used a raw `<>`-balance count (`gt_group_len`), which
//!   over-extends across content the validated scan rejects
//!   (`<C()>`: `C` followed by `(`; `Function<T 5 6>`), misplacing every
//!   downstream group boundary.

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted —
/// the parity contract is the row SET (the conformance harness compares
/// (severity, code, file) sets), so the pins fix anchors, not emission order.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    sorted(rows_of(source))
}

fn rows_of(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

fn sorted(mut rows: Vec<(String, u32, u32)>) -> Vec<(String, u32, u32)> {
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

// ---------------------------------------------------------------------------
// Family A — index-expression junk recovery (expressions.rs)
// ---------------------------------------------------------------------------

/// `a[<junk>;` (oracle 3.13.1: two `expected_token` rows, both at the `;`):
/// the `<junk>` parses as a type-inferred list literal missing its `[`
/// (`parseLiteralListSetMapOrFunction` :8355-8365 row), and the index
/// recovery's `]` row anchors on the same `;` with the scanner-synthetic
/// closer relocated next to it — never an EOF row, never a swallowed `; }`.
#[test]
fn index_lt_junk_lands_both_rows_on_semicolon() {
    let rows = sorted_rows("void f() {\n  a[<junk>;\n}\n");
    assert_eq!(
        rows,
        vec![code("expected_token", 21, 22), code("expected_token", 21, 22)],
        "`a[<junk>;` must close the index at the `;`"
    );
}

/// `a[f(1);` (oracle: exactly one row, the `]` at the `;`): the scanner
/// parks BOTH closers synthetically; the relocated `]` closes the index on
/// the expression end and the unmatched-group pass must stay silent.
#[test]
fn index_unclosed_operand_row_at_follower_only() {
    let rows = sorted_rows("void f() {\n  a[f(1);\n}\n");
    assert_eq!(
        rows,
        vec![code("expected_token", 19, 20)],
        "`a[f(1);` must emit only the relocated `]` row"
    );
}

/// `a[1 }` (oracle: `expected_token ';'` at the `1`, `expected_token ']'` at
/// the `}`, `expected_executable` at the enclosing `}`): the junk `}` stays
/// current after the relocated `]` so the statement's semicolon walk-back
/// and the member-level tail keep their oracle anchors.
#[test]
fn index_brace_junk_keeps_statement_anchors() {
    let rows = sorted_rows("void f() {\n  a[1 }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_executable", 19, 20),
            code("expected_token", 15, 16),
            code("expected_token", 17, 18),
        ],
        "`a[1` + brace junk anchors must survive the moveSynthetic relocation"
    );
}

/// Real-junk control (`a[1 2]`): a real `]` absorbs the junk run — unchanged
/// skip-to-endGroup posture, one row at the junk token.
#[test]
fn index_real_closer_absorbs_junk() {
    let rows = sorted_rows("void f() {\n  a[1 2];\n}\n");
    assert_eq!(rows, vec![code("expected_token", 17, 18)]);
}

/// Clean forms stay silent: plain index, null-aware index, and the empty
/// `a[]` operand face (oracle issue_42267: one `missing_identifier`, the
/// closer follows).
#[test]
fn index_clean_forms_stay_silent() {
    assert!(rows_of("void f() {\n  a[1];\n}\n").is_empty());
    assert!(rows_of("void f() {\n  a?[1];\n}\n").is_empty());
    assert_eq!(
        sorted_rows("void f() {\n  a[];\n}\n"),
        vec![code("missing_identifier", 15, 16)],
        "`a[]` keeps its single missing_identifier"
    );
}

// ---------------------------------------------------------------------------
// Family B — nested-generic type-argument boundaries (type_args.rs)
// ---------------------------------------------------------------------------

/// `A<B<C()>>` as a tear-off receiver (oracle: the group is NOT type
/// arguments — `<C()>` fails the validated scan after `C`, so the whole
/// `<B<C()>>` devolves to relational expressions: two comparison rows at
/// the inner `<` and a missing identifier at the `;`).
#[test]
fn tearoff_nested_paren_group_rejects_to_expression() {
    let rows = sorted_rows("class A {\n  void h() {\n    var f = A<B<C()>>;\n  }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("equality_cannot_be_equality_operand", 38, 39),
            code("missing_identifier", 44, 45),
        ],
        "`A<B<C()>>;` must fall back to relational parsing"
    );
}

/// The same shape inside an argument list.
#[test]
fn argument_nested_paren_group_rejects_to_expression() {
    let rows = sorted_rows("class A {\n  void h() {\n    f(A<B<C()>>);\n  }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("equality_cannot_be_equality_operand", 32, 33),
            code("missing_identifier", 38, 39),
        ],
        "`f(A<B<C()>>)` must fall back to relational parsing"
    );
}

/// Junk identifier inside the nested group: the oracle devolves to
/// comparisons and reports the expected-token row on the junk run.
#[test]
fn argument_nested_junk_identifier_rejects_to_expression() {
    let rows = sorted_rows("class A {\n  void h() {\n    f(A<B<C junk>>);\n  }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("equality_cannot_be_equality_operand", 32, 33),
            code("expected_token", 35, 39),
            code("missing_identifier", 41, 42),
        ],
        "`f(A<B<C junk>>)` face"
    );
}

/// Declaration context (`List<B<C()> >? x;`): the nested `<C()>` group
/// validates RECOVERED with `inDeclaration: true` (upstream
/// `looksLikeTypeParamOrArg` + the `(`-step recovery tail), and the inner
/// group's parse reports its single row at `C`.
#[test]
fn declaration_nested_paren_group_recovers_at_inner_closer() {
    let rows = sorted_rows("class A {\n  List<B<C()> >? x;\n}\n");
    assert_eq!(
        rows,
        vec![code("expected_token", 19, 20)],
        "`List<B<C()> >? x;` face"
    );
}

/// Shared-compound boundary (`Map<String, B<C()>>? x;`): the inner group's
/// end shares the `>>` with the outer group — the type ends ON the compound
/// and the outer group closes on its last `>`; one row at `C`.
#[test]
fn declaration_nested_group_shares_compound_closer() {
    let rows = sorted_rows("class A {\n  Map<String, B<C()>>? x;\n}\n");
    assert_eq!(
        rows,
        vec![code("expected_token", 26, 27)],
        "`Map<String, B<C()>>? x;` face"
    );
}

/// `skip_type_len_in` at `offset 0` — the scan-path surface of the
/// validated boundaries above, in tokens.
fn skip_len(source: &'static str, in_declaration: bool) -> usize {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.skip_type_len_in(0, in_declaration, 0)
}

/// Broken GFT chain (`void Function<T 5 6>()? f;`): the type-variable group
/// ends at the synthetic `>` in front of the `5` (upstream `syntheticGt`),
/// and the `(` never joins so `computeVoidGFT` falls back to plain `void`
/// (type_info_impl.dart:949-959 via the `computeRest` break :1102-1103).
///
/// In-scope face: the group recovery row anchors at the last type variable
/// `T` — `(expected_token, 26, 27)`, which is in the oracle 3.13.1 set — and
/// the skip surface falls back to the bare `void`.
///
/// Residual (out of Lane-E scope, reported): the oracle cascade continues
/// with 6 member-lane rows (`missing_function_body`/`missing_method_parameters`
/// at the `?`, `missing_keyword_operator`/`invalid_operator`, then
/// `missing_function_body`/`missing_const_final_var_or_type` at `f`) because
/// the member-level `computeRest` must break when the token after the
/// validated type-variable group is not `(` (type_info_impl.dart:1094-1103,
/// the validated probe `computeTypeParamOrArg(token, true).skip(token)`
/// :1097-1100).
/// dartq's member-level probe (`type_parse.rs` `compute_rest`) still tests
/// the `<...>` head with a raw `<>`-balance pair (`table_matching_end`), so
/// `<T 5 6>` counts as the group and the chain is consumed as a valid
/// nullable GFT field. Fix: probe with the validated scan (this lane's
/// `group_skip_len`) and break on a non-`(` successor.
#[test]
fn broken_void_gft_chain_group_row_anchors_at_last_type_var() {
    let rows = sorted_rows("class A {\n  void Function<T 5 6>()? f;\n}\n");
    assert!(
        rows.contains(&code("expected_token", 26, 27)),
        "type-variable group recovery row must anchor at `T`: {rows:?}"
    );
    // The scan surface: without the `(`-suffix the chain is discarded and
    // the type is the bare `void` (token count 1); the complete chain skips
    // the whole `void Function<T>()?` (8 tokens).
    assert_eq!(
        skip_len("void Function<T 5 6>()? f;", false),
        1,
        "broken `Function <...>` suffix is discarded: bare `void` fallback"
    );
    assert_eq!(
        skip_len("void Function<T>()? f;", false),
        8,
        "complete `void Function<T>()?` skips the whole GFT"
    );
}

/// The same broken chain nested in a type-argument group
/// (`List<void Function<T 5 6>()>? x;`): the Function suffix is discarded
/// from the skip so the outer boundary follows the validated inner end —
/// the outer group ends on the synthetic `>` in front of `Function`
/// (encoded as the group's last token being `void`), never across the
/// `Function <T 5 6> (...)` tail.
///
/// Residual (out of Lane-E scope, reported): the oracle 3.13.1 row set for
/// the declaration face is 5 rows — `expected_token` at `void` (17,21),
/// `missing_method_parameters` at `Function` (22,30),
/// `missing_identifier` (40,41), `missing_function_body` +
/// `missing_const_final_var_or_type` at `;` (42,43). dartq's divergence is
/// the same member-level raw probe documented on
/// [`broken_void_gft_chain_group_row_anchors_at_last_type_var`]: with the
/// chain raw-accepted the leftover tail re-cascades with different anchors.
#[test]
fn broken_gft_chain_skip_discards_incomplete_function_suffix() {
    // Outside a declaration the junk `Function` rejects the group entirely
    // (`looksLikeTypeParamOrArg` break → `noTypeParamOrArg`): bare `List`.
    assert_eq!(
        skip_len("List<void Function<T 5 6>()>? x;", false),
        1,
        "non-declaration face: the group is not type arguments at all"
    );
    // In a declaration the group recovers: it ends before the `Function`
    // junk (`List < void` + synthetic `>` — 3 tokens), never across it.
    assert_eq!(
        skip_len("List<void Function<T 5 6>()>? x;", true),
        3,
        "declaration face: outer boundary follows the validated inner end"
    );
}

/// Clean nested forms stay silent: validated nesting, shared `>>`, GFT type
/// variables, prefixed tear-off receiver, and the chained-void-GFT control.
#[test]
fn nested_generic_clean_forms_stay_silent() {
    assert!(rows_of("class A {\n  List<B<C>>? x;\n}\n").is_empty());
    assert!(rows_of("class A {\n  Map<String, List<int>>? m;\n}\n").is_empty());
    assert!(rows_of("class A {\n  void Function<T>()? f;\n}\n").is_empty());
    assert!(
        rows_of("class A {\n  void h() {\n    var f = A<B<C>>.new;\n  }\n}\n").is_empty(),
        "clean constructor tear-off with nested type args"
    );
    assert!(
        rows_of("var x = <void Function() Function()?>[];\n").is_empty(),
        "chained void-return GFT type arg (gate_acceptance control)"
    );
}
