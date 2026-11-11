//! Ticket #3935 — the unnamed `<...>(...)` formal: an inline function type
//! with type parameters and a parameter list, but neither a return type nor a
//! name (`Function(<S>())`, `f(<T>())`, `Function(<>($`).
//!
//! Upstream (`parseFormalParameter`, parser_impl.dart):
//! * `:2208-2211` consumes a tentative name when the follower is an identifier;
//! * `:2214-2239` is the UNNAMED arm — `next.isA(LT)` computes the group at
//!   the token BEFORE the `<`, and a closer followed by `(` makes the whole
//!   shape the parameter's type (`beforeInlineFunctionType = token`), skipping
//!   to the parameter list's own `)`;
//! * `:2298-2328` re-walks the type, the type-variable group and the parameter
//!   list, and reports `invalidInlineFunctionType` at
//!   `beforeInlineFunctionType.next!` inside a generalized function type;
//! * `:2335-2352` runs the name slot with `token` left at `beforeType` — the
//!   token BEFORE the inline function type — so `ensureIdentifier` anchors
//!   there and its follower (the `<`) carries `missing_identifier`; the parse
//!   then resumes at the inline function type's end.
//!
//! `formal_params.rs`'s probe previously only had the NAME-LED `<`/`(`
//! follower (the `tentative_name` gate), so an unnamed `<` fell through to the
//! ordinary name/type lane and cascaded. Every row below is measured against
//! `dart analyze --format=machine` on the same fixture (oracle SDK 3.13.7).

use crate::parser::Parser;
use crate::tests::collector::{Collector, latest_features, scan_ok};

fn rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let listener: &'static mut Collector = Box::leak(Box::new(Collector::default()));
    let mut parser = Parser::new(text, table, latest_features(), listener);
    parser.parse_unit();
    parser.listener.errors.clone()
}

#[test]
fn unnamed_type_param_formal_in_function_type() {
    // `typedef T=Function(<S>());` — oracle row set:
    //   MISSING_IDENTIFIER           1:20 len 1  (the `<`, offset 19)
    //   INVALID_INLINE_FUNCTION_TYPE 1:20 len 1  (same span)
    // The parser produces both rows itself (the inline-function-type report
    // rides the re-walk, parser_impl.dart:2323-2328); the spans match the
    // oracle exactly.
    assert_eq!(
        rows("typedef T=Function(<S>());\n"),
        vec![
            ("invalid_inline_function_type".to_owned(), 19, 20),
            ("missing_identifier".to_owned(), 19, 20),
        ]
    );
}

#[test]
fn unnamed_type_param_formal_in_a_method() {
    // `class C {\n  f(<T>());\n}\n` — not a generalized function type, so no
    // `invalid_inline_function_type`; the single oracle parser row is
    // `missing_identifier` at the `<` (2:5, offset 14). The
    // `concrete_class_with_abstract_member` row the oracle also reports is
    // semantic-tier and stays out of the parser rows.
    assert_eq!(
        rows("class C {\n  f(<T>());\n}\n"),
        vec![("missing_identifier".to_owned(), 14, 15)]
    );
}

#[test]
fn unnamed_type_param_formal_with_empty_group_cascades_like_the_oracle() {
    // `typedef K=Function(<>($` — oracle row set (every span matched):
    //   MISSING_IDENTIFIER 1:20 len 1  (the `<`, offset 19)
    //   INVALID_INLINE_FUNCTION_TYPE 1:20 len 1
    //   MISSING_IDENTIFIER 1:21 len 1  (the `>` — the empty group's name slot)
    //   EXPECTED_TOKEN ';' 1:23 len 1  (the `$`, offset 22)
    //   EXPECTED_TOKEN ')' 2:1 len 1   (offset 24)
    // Emission order differs from the oracle's severity-sorted output; the row
    // SET is what the census compares.
    assert_eq!(
        rows("typedef K=Function(<>($\n"),
        vec![
            ("missing_identifier".to_owned(), 20, 21),
            ("invalid_inline_function_type".to_owned(), 19, 20),
            ("missing_identifier".to_owned(), 19, 20),
            ("expected_token".to_owned(), 22, 23),
            ("expected_token".to_owned(), 24, 25),
        ]
    );
}

#[test]
fn named_inline_function_typed_formal_is_unchanged() {
    // Control: the NAME-LED shape keeps its own path — `void f(g(int x)) {}`
    // reports nothing (oracle: zero rows).
    assert!(rows("void f(g(int x)) {}\n").is_empty());
}
