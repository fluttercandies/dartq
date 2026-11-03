//! #3077 pins: the Map-face member cascade — a member-head type-variable
//! list breaks at a variable followed by `<` (upstream pin
//! the pinned Dart SDK 3.13.7,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`,
//! `type_info_impl.dart`).
//!
//! For `class A { Map<String, B<C>>()? x; }` the member dispatch classifies
//! the head as `noType` (the type-argument group ends on the shared `>>`,
//! whose successor is never a name — `computeSimpleWithTypeArguments`,
//! type_info_impl.dart:1048-1061), so the head parses as a generic method
//! named `Map` and `<String, B<C>>` walks as its type-variable list. The
//! official list BREAKS at `B<C>`: `looksLikeTypeParamOrArg`
//! (type_info_impl.dart:512-522) requires the current token to be an
//! identifier, and `<` never continues. The tail then runs
//! `parseUnexpectedEnd(token3 = B, isArguments: false)` (:1583-1585), whose
//! nested type-argument arm (:1658-1690) reports
//! `expected_after_but_got('>')` on the variable head `B` and re-parses
//! `<C>>` silently; the split compound closer keeps the remainder chained,
//! so the group's own closer consumes it and the member continues at its
//! formals `()`. The body face (`parseFunctionBody` → `expectedBody`) lands
//! on `?`, the class-body loop re-enters there with the operator face
//! (`parseInvalidOperatorDeclaration` → `missingOperatorKeyword`,
//! parser_impl.dart:11204; `parseOperatorName` → `invalidOperator`,
//! :6288; formals recovery → `missing_method_parameters`, :1684), and the
//! `x` head closes with `missing_function_body` +
//! `missing_const_final_var_or_type`.

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

/// Oracle 3.13.1 row set, reproduced exactly (probed with
/// `fvm dart analyze --format machine`): `expected_token` ("Expected to
/// find '>'") on `B` (24,25); `missing_function_body` +
/// `missing_method_parameters` + `missing_keyword_operator` +
/// `invalid_operator` on `?` (31,32); `missing_function_body` +
/// `missing_const_final_var_or_type` on `x` (33,34).
#[test]
fn map_face_cascade_matches_oracle() {
    assert_eq!(
        sorted_rows("class A {\n  Map<String, B<C>>()? x;\n}\n"),
        vec![
            code("expected_token", 24, 25),
            code("invalid_operator", 31, 32),
            code("missing_const_final_var_or_type", 33, 34),
            code("missing_function_body", 31, 32),
            code("missing_function_body", 33, 34),
            code("missing_keyword_operator", 31, 32),
            code("missing_method_parameters", 31, 32),
        ],
        "oracle 7-row cascade for `Map<String, B<C>>()? x;`"
    );
}

/// The non-compound siblings share the same nameless-member cascade and
/// already matched the oracle before #3077 (`Map<String, B>()? x;` closes
/// its type-variable list at the plain `>`; `B<C>()? x;` takes the
/// Simple-1 fast shape) — the break-posture route must not touch them.
#[test]
fn non_compound_nameless_faces_stay_oracle_exact() {
    let six_rows = vec![
        code("invalid_operator", 28, 29),
        code("missing_const_final_var_or_type", 30, 31),
        code("missing_function_body", 28, 29),
        code("missing_function_body", 30, 31),
        code("missing_keyword_operator", 28, 29),
        code("missing_method_parameters", 28, 29),
    ];
    assert_eq!(
        sorted_rows("class A {\n  Map<String, B>()? x;\n}\n"),
        six_rows,
        "`Map<String, B>()? x;` keeps its clean-list operator cascade"
    );
    assert_eq!(
        sorted_rows("class A {\n  B<C>()? x;\n}\n"),
        vec![
            code("invalid_operator", 18, 19),
            code("missing_const_final_var_or_type", 20, 21),
            code("missing_function_body", 18, 19),
            code("missing_function_body", 20, 21),
            code("missing_keyword_operator", 18, 19),
            code("missing_method_parameters", 18, 19),
        ],
        "`B<C>()? x;` keeps its Simple-1 operator cascade"
    );
}

/// The clean nullable compound field never enters the member method lane
/// (`computeType` classifies it; `parse_fields` consumes it) — the
/// break-posture route cannot reach it.
#[test]
fn clean_nested_compound_field_stays_silent() {
    assert!(sorted_rows("class A {\n  Map<String, B<C>>? x;\n}\n").is_empty());
}
