//! #3074 pins: the member-face cascade bound — a recovered type-argument
//! group's re-parse may never swallow the member name (upstream pin
//! the pinned Dart SDK 3.13.7,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`).
//!
//! For `List<void Function<T 5 6>()>? x;` the type scan's face ends before
//! the junk `Function` (the recovered group's `skipEnd` is the synthetic `>`
//! at the junk head, `syntheticGt`, util.dart:270 + type_info_impl.dart:1421,
//! so `skipType` (:738-740) lands the member-name position there,
//! parser_impl.dart:4245). `parseMethod` then asserts its return-type parse
//! lands exactly before that name (`assert(token.next == name)`,
//! parser_impl.dart:5875). dartq's return-type re-parse could overshoot the
//! landing by absorbing `Function <T 5 6> ()` into the type, which re-anchored
//! the whole member cascade on `?` (9 rows vs the oracle's 5). The fix reads
//! the scan-face landing after the re-parse: on overshoot the
//! `missing_method_parameters` row re-anchors on the scan-designated name
//! (official `parseGetterOrFormalParameters` recovery reports on the name,
//! parser_impl.dart:1684, with the `findNonZeroLengthToken` anchor walk,
//! util.dart:56), and the body face reports `missing_function_body` on the
//! follower of the absorbed `?` — official `ensureBlock` reports from the
//! synthetic `{` spliced at the body position and `reportRecoverableError`
//! re-anchors a synthetic token on the first real follower
//! (parser_impl.dart:4939 + util.dart:56).

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

/// Oracle 3.13.1 row set for the nested member face, reproduced exactly:
/// the group-recovery row anchors at `void` (17,21), the formals-recovery
/// row re-anchors on the scan-designated name `Function` (22,30), the
/// nameless position `?` (40,41) carries `missing_identifier`, and the body
/// pair lands on the next member head `x` (42,43).
#[test]
fn nested_type_args_overshoot_cascade_matches_oracle() {
    assert_eq!(
        sorted_rows("class A {\n  List<void Function<T 5 6>()>? x;\n}\n"),
        vec![
            code("expected_token", 17, 21),
            code("missing_const_final_var_or_type", 42, 43),
            code("missing_function_body", 42, 43),
            code("missing_identifier", 40, 41),
            code("missing_method_parameters", 22, 30),
        ],
        "oracle 5-row cascade for `List<void Function<T 5 6>()>? x;`"
    );
}

/// The direct GFT member face (`void Function<T 5 6>()? f;`, pinned by
/// `ticket_3072.rs`) has NO overshoot — the scan face is plain `void`, the
/// name lands on `Function`, and the method-type-variable group reports at
/// `T` (26,27) — the overshoot bound must not touch it.
#[test]
fn direct_gft_face_has_no_overshoot() {
    let rows = sorted_rows("class A {\n  void Function<T 5 6>()? f;\n}\n");
    assert!(
        rows.contains(&code("expected_token", 26, 27)),
        "type-variable group row must stay anchored at `T`: {rows:?}"
    );
    assert!(
        !rows.contains(&code("missing_method_parameters", 22, 30)),
        "the direct GFT face must not gain the overshoot re-anchor: {rows:?}"
    );
}

/// A plain nullable compound field must stay silent (the overshoot bound
/// only arms when the return-type re-parse runs past the scan-face landing).
#[test]
fn clean_nullable_compound_field_stays_silent() {
    assert!(sorted_rows("class A {\n  List<void Function<T>()>? x;\n}\n").is_empty());
    assert!(sorted_rows("class A {\n  Map<String, int>? x;\n}\n").is_empty());
}

/// Aligned by #3077 (was a frozen 4-row residual at #3074 close): the
/// Map-face member cascade. The oracle treats the member head as a
/// type-variable list that BREAKS at `B<C>` (`looksLikeTypeParamOrArg`,
/// type_info_impl.dart:512-522, never continues over `<`), reports the `>`
/// row on the variable head `B` via `parseUnexpectedEnd` (:1658-1690), and
/// the split compound closer leaves the member before its formals — the
/// nameless-method body face lands on `?`, the operator cluster
/// (`missing_keyword_operator` + `invalid_operator` via
/// `parseInvalidOperatorDeclaration`/`parseOperatorName`,
/// parser_impl.dart:11204/:6288) re-enters there, and the `x` cascade
/// closes.
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
