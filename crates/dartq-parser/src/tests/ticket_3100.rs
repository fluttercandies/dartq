//! #3100 pins: the typedef-header anchor faces (oracle 3.13.1). A
//! new-style alias whose function-type RHS carries a recovered
//! type-variable group (`Function<extends int>()`) never validates as a
//! generalized function type: the alias degrades to the bare return type,
//! `ensureSemicolon` anchors `expectedToken(';')` on it
//! (parser_impl.dart:5048), and the `Function<...>(...)` tail is re-read
//! as a top-level member whose `;` reports `missingFunctionBody`
//! (`parseFunctionBody`, `diag.expectedBody`).

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// The degraded alias face: the anchor row set is the anchor on the bare
/// return type plus the member-tail rows (`missing_identifier` on the keep
/// head, `missing_function_body` on the trailing `;`). The return type's
/// own extent is the anchor (`void` (12,16), `int` (12,15)).
#[test]
fn degraded_alias_anchors_match_oracle() {
    assert_eq!(
        sorted_rows("typedef F = void Function<extends int>();\n"),
        vec![
            code("expected_token", 12, 16),
            code("missing_function_body", 40, 41),
            code("missing_identifier", 26, 33),
        ],
        "oracle 3.13.1: the `;` anchor sits on the return type, the tail \
         member reports the keep head and the body row"
    );
    assert_eq!(
        sorted_rows("typedef F = int Function<extends int>();\n"),
        vec![
            code("expected_token", 12, 15),
            code("missing_function_body", 39, 40),
            code("missing_identifier", 25, 32),
        ],
        "oracle: the identifier return type degrades the same way — the \
         anchor moves with the head's extent"
    );
}

/// The close-anchor control face: a non-function alias RHS leaves the
/// trailing junk to `ensureSemicolon`, which anchors on the RHS's last
/// token and hands the tail to the declaration loop
/// (`typedef F = List<int> x;` — the member reparse reports the variable
/// keyword row on the `;`).
#[test]
fn alias_close_anchor_matches_oracle() {
    assert_eq!(
        sorted_rows("typedef F = List<int> x;\n"),
        vec![
            code("expected_token", 20, 21),
            code("missing_const_final_var_or_type", 22, 23),
        ],
        "oracle: `expected_token` on the `>` (the RHS's last token), the \
         variable-keyword row on the `;`"
    );
}

/// Group shapes whose classification validates keep the plain keep-face
/// rows: an empty group, an identifier name (with or without a bound), a
/// `required`-class built-in name, and an immediate-closer reserved or
/// member-keyword head never degrade the alias.
#[test]
fn validating_group_shapes_stay_silent() {
    assert_eq!(
        sorted_rows("typedef F = void Function<>();\n"),
        vec![code("missing_identifier", 26, 27)],
        "oracle: the empty group's missing-name row only"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function<int>();\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: a plain type argument is a valid alias"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function<T extends int>();\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: an identifier name with a bound validates"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function<required int>();\n"),
        vec![code("missing_identifier", 26, 34)],
        "oracle: the `required`-class built-in keeps the keep face without \
         degrading the alias"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function<super>();\n"),
        vec![code("missing_identifier", 26, 31)],
        "oracle: an immediate-closer reserved head keeps the keep face"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function<extends>();\n"),
        vec![
            code("expected_type_name", 33, 34),
            code("missing_identifier", 26, 33),
        ],
        "oracle: the immediate-closer head degrades to the keep face plus \
         the bound row — no alias anchors"
    );
}
