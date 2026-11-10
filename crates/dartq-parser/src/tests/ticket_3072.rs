//! #3072 pins: `compute_rest`'s GFT type-variable-head gate (upstream pin
//! the pinned Dart SDK 3.13.7,
//! `pkg/_fe_analyzer_shared/lib/src/parser/type_info_impl.dart:1095-1103`).
//!
//! The `Function [<typeVarGroup>] (` chain must probe the `<...>` head with
//! the VALIDATED group skip — `computeTypeParamOrArg(token,
//! /* inDeclaration = */ true).skip(token)` (:1095-1100), dartq's
//! `Parser::group_skip_len` — not the raw scanner `<>` pairing. For the junk
//! head `<T 5 6>` the validated scan ends on the recovery tail's boundary
//! (the synthetic `>` in front of the junk `5`), so the successor `5` is no
//! `(` and the chain breaks (:1102-1103); the member face then cascades
//! exactly like the oracle (`computeVoidGFT` :949-959 falls back to plain
//! `void`). With the old raw pairing `<T 5 6>` counted as the balanced
//! group, the probe landed on the `(`, and the chain consumed the tail as a
//! valid nullable GFT field — silencing the member cascade the oracle keeps.
//!
//! Oracle row sets come from the `ticket_3052.rs` residual report
//! (member faces `void Function<T 5 6>()? f;` = 7 rows and
//! `List<void Function<T 5 6>()>? x;` = 5 rows).

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

/// Sorted error codes only — the parity contract is the (severity, code,
/// file) SET, so the cascade pins commit to the code multiset plus the
/// anchors #3052 documented from the oracle, not emission order.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut codes: Vec<String> = sorted_rows(source).into_iter().map(|(c, _, _)| c).collect();
    codes.sort();
    codes
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// Broken GFT type-variable head in a member face (`void Function<T 5 6>()?
/// f;`): the chain breaks after the validated probe, `computeVoidGFT` falls
/// back to plain `void`, and the member lane cascades — 7 rows, matching the
/// oracle. The group recovery row anchors at the last type variable `T`
/// (26,27), as in `ticket_3052.rs`.
#[test]
fn broken_gft_type_var_head_cascades_in_member_face() {
    let rows = sorted_rows("class A {\n  void Function<T 5 6>()? f;\n}\n");
    assert!(
        rows.contains(&code("expected_token", 26, 27)),
        "type-variable group recovery row must anchor at `T`: {rows:?}"
    );
    assert_eq!(
        sorted_codes("class A {\n  void Function<T 5 6>()? f;\n}\n"),
        vec![
            "expected_token",
            "invalid_operator",
            "missing_const_final_var_or_type",
            "missing_function_body",
            "missing_function_body",
            "missing_keyword_operator",
            "missing_method_parameters",
        ],
        "oracle member cascade for `void Function<T 5 6>()? f;`"
    );
}

/// The same broken head nested in type arguments
/// (`List<void Function<T 5 6>()>? x;`): the outer type-argument group
/// recovers before the junk `Function` (`inDeclaration: true` recovery
/// tail) — its row anchors at `void` (17,21), matching the oracle — and the
/// `compute_rest` chain breaks on the non-`(` successor, so the member lane
/// cascades instead of consuming a valid nullable GFT field.
///
/// #3074 retightened the member cascade to the oracle 5-row set (this pin
/// froze the 9-row pre-#3074 face and named the members.rs fix as its
/// follow-up); the exact face now lives in `ticket_3074.rs`.
#[test]
fn broken_gft_type_var_head_in_type_args_cascades() {
    assert_eq!(
        sorted_rows("class A {\n  List<void Function<T 5 6>()>? x;\n}\n"),
        vec![
            code("expected_token", 17, 21),
            code("missing_const_final_var_or_type", 42, 43),
            code("missing_function_body", 42, 43),
            code("missing_identifier", 40, 41),
            code("missing_method_parameters", 22, 30),
        ],
        "#3074 member cascade for `List<void Function<T 5 6>()>? x;` (oracle 5 rows)"
    );
}

/// Valid chains stay silent: the type-variable head validates (`<T>`), the
/// probe lands on the `(`, and the GFT consumes — including the nested
/// type-argument face whose `compute_rest` continues past the group.
#[test]
fn valid_gft_type_var_chains_stay_silent() {
    assert!(sorted_rows("class A {\n  void Function<T>()? f;\n}\n").is_empty());
    assert!(sorted_rows("class A {\n  List<void Function<T>()>? x;\n}\n").is_empty());
}

/// The `(` after a type-argument group is not a `Function` intro: the chain
/// never enters (the `while` guard reads the follower kind before any probe
/// runs) and the shared-compound pre-check returns early, so the gate cannot
/// alter this face — the pin freezes it to guard against the fix
/// over-breaking the plain `id<...>(` member face.
///
/// Aligned by #3077 (was a frozen 4-row residual at #3072 close): the
/// oracle 3.13.1 row set for `Map<String, B<C>>()? x;` — the type-variable
/// list breaks at `B<C>` (type_info_impl.dart:512-522), the `>` row anchors
/// on `B` via `parseUnexpectedEnd` (:1658-1690), and the operator cluster
/// (parser_impl.dart:11204/:6288) re-enters at the `?`.
#[test]
fn paren_after_type_args_is_not_a_function_intro() {
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
        "oracle 7-row cascade (aligned by #3077; gate never enters this face)"
    );
}
