//! Ticket #3938 — parser anchor tails routed out of the
//! `partial_code/annotation` census family (Lane AX).
//!
//! 1. `missing_method_parameters` anchor for a synthetic `(` group:
//!    `class C { @a( int get a => 0; }` — the class-member loop reparses the
//!    `=>` head as a member, splices a synthetic name in front of it, and
//!    reaches `parse_getter_or_formal_parameters`'s missing-`(` arm. The
//!    official `findNonZeroLengthToken` walk (parser/util.dart:56-61) anchors
//!    the row on the first non-zero token AFTER the synthetic name — the
//!    `=>` at (24,26). The previous implementation skipped `look_span_at(0)`
//!    and anchored on `0` at (27,28); members.rs:2763 now reads the peek
//!    span first so the walk matches the official token-stream order.
//!
//! 2. `operator>()` reparse tail: `@p.A.ctor<B>() class C {}` — after the
//!    `top_level_operator` row at `>`, the official
//!    `parseTopLevelDeclarationImpl` (parser_impl.dart:652-661) splices a
//!    synthetic identifier after the operator and re-enters
//!    `parseTopLevelMemberImpl`, so the `()` group parses as the parameter
//!    list and the follower reports `missing_function_body` at `class`
//!    (1:16 len 5). The previous implementation minted `expected_executable`
//!    on the `(` and `)` instead; declarations.rs:165 now consumes the
//!    operator, drains a synthetic name, and dispatches
//!    `parse_top_level_method` — the `()` parameter list is consumed and
//!    `class` carries the missing-body row.

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

struct Out {
    errors: Vec<(String, u32, u32)>,
}

fn parse(source: &'static str) -> Out {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
    }
}

// ------------------------------------------------------------------
// 1. `missing_method_parameters` anchor — synthetic `(` group
// ------------------------------------------------------------------

/// `class C { @a( int get a => 0; }`: the `int`/`get`/`a` identifiers join
/// the argument group through the `,`-splice arm, then `=>` hits
/// `ensureCloseParen` and the class-member loop reparses `=> 0; }`. The
/// member walk's `MethodDeclaration` keep-set keeps `=>` current, the
/// synthetic name splices in front of it, and the missing-`(` arm anchors
/// `missing_method_parameters` on the `=>` — the first non-zero follower
/// of the synthetic name (`findNonZeroLengthToken`).
#[test]
fn annotation_getter_reparse_anchors_parameters_on_arrow() {
    // Oracle SYNTACTIC rows:
    //   `expected_token ','`          1:19 len 3 (before `get`)
    //   `expected_token ','`          1:23 len 1 (before `a`)
    //   `missing_identifier`          1:25 len 2 (the `=>`)
    //   `expected_token ')'`          1:25 len 1 (the `=>`)
    //   `missing_method_parameters`   1:25 len 2 (the `=>`, 24..26)
    let out = parse("class C { @a( int get a => 0; }\n");
    assert!(
        out.errors
            .iter()
            .any(|(c, s, e)| c == "missing_method_parameters" && *s == 24 && *e == 26),
        "missing_method_parameters must anchor on the `=>` (24,26): {:?}",
        out.errors
    );
}

/// Full row set for the same face: the anchor fix must not disturb the
/// `,`-splice or close-paren rows the preceding arms already match.
#[test]
fn annotation_getter_reparse_full_row_set() {
    let out = parse("class C { @a( int get a => 0; }\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_token".to_owned(), 18, 21),
            ("expected_token".to_owned(), 22, 23),
            ("expected_token".to_owned(), 24, 25),
            ("missing_identifier".to_owned(), 24, 26),
            ("missing_method_parameters".to_owned(), 24, 26),
        ],
        "parser rows for `class C {{ @a( int get a => 0; }}`: {:?}",
        out.errors
    );
}

// ------------------------------------------------------------------
// 2. `operator>()` reparse tail — declarations.rs
// ------------------------------------------------------------------

/// `@p.A.ctor<B>() class C {}`: the `<B>` reparses as binary `<`/`>`
/// operators (`type_args_after_third_segment_reparse_as_binary` in
/// ticket_3930_tests.rs pins the head four rows). After the
/// `top_level_operator` row at `>`, the `()` group reparses as an
/// `operator>()` declaration — oracle reports `missing_function_body` at
/// `class` (1:16 len 5).
#[test]
fn operator_gt_reparse_reports_missing_function_body() {
    // Oracle SYNTACTIC rows after the first four (which ticket_3930 pins):
    //   `missing_function_body`  1:16 len 5 (the `class` keyword)
    let out = parse("@p.A.ctor<B>() class C {}\n");
    assert!(
        out.errors
            .iter()
            .any(|(c, s, e)| c == "missing_function_body" && *s == 15 && *e == 20),
        "missing_function_body must anchor on `class` (15,20): {:?}",
        out.errors
    );
}

/// Full row set for the `>()` reparse face: the `()` group must not mint
/// `expected_executable` on the parens — the parameter list is consumed
/// by the synthetic-named method parse and `class` reports the missing
/// body.
#[test]
fn operator_gt_reparse_full_row_set() {
    let out = parse("@p.A.ctor<B>() class C {}\n");
    assert_eq!(
        out.errors,
        vec![
            ("expected_executable".to_owned(), 9, 10),
            ("missing_const_final_var_or_type".to_owned(), 10, 11),
            ("expected_token".to_owned(), 10, 11),
            ("top_level_operator".to_owned(), 11, 12),
            ("missing_function_body".to_owned(), 15, 20),
        ],
        "parser rows for `@p.A.ctor<B>() class C {{}}`: {:?}",
        out.errors
    );
}
