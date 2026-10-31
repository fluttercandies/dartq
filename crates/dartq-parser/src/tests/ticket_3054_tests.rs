//! #3054 pins: `compute_rest` GFT type-variable-head gate — the four
//! follower arms after a `Function <...>` head.
//!
//! Upstream `computeRest` (Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/parser/type_info_impl.dart:1094-1103`):
//! for each `Function` link the successor of the VALIDATED
//! `computeTypeParamOrArg(token, /* inDeclaration = */ true).skip(token)`
//! must be `(` for the chain to be a generic function type; any other
//! follower (`.` filler, declaration name, junk run) breaks the chain, the
//! `gftHasReturnType` stays unset, and the compute falls back to the bare
//! head type (`computeVoidGFT` :949-959, `computeNoTypeGFT` :744-754,
//! `computeIdentifierGFT` :962-974). dartq's gate mirrors exactly that
//! probe (`type_parse.rs` `compute_rest`: validated `group_skip_len`
//! instead of a raw `<>` balance, `None`/non-`(` successor → break).
//!
//! For the junk tail `<T 5 6>` the validated scan ends the group on the
//! recovery tail (`ComplexTypeParamOrArgInfo.compute` :1395-1423): the
//! member test fails on `5`, no closer follows, so the group's own end is
//! the synthetic `>` in front of the junk and the chain successor is the
//! junk itself — never `(`, so the GFT is rejected. The parse face
//! (`parseMethodTypeVar`, in-declaration) consumes the junk up to the real
//! `>` and cascades through the member lane exactly like the oracle.
//!
//! Row anchors follow the upstream member-recovery walk
//! (`parser_impl.dart`): `missingParameterMessage` reports at the
//! declaration name (:1660-1690), `parseFunctionBody` reports
//! `missingFunctionBody` at `token.next` (:6516-6590), junk member heads
//! report `expectedClassMember` and step one token
//! (`recoverFromInvalidMember` :11256-11316), `parseInvalidOperatorDeclaration`
//! reports `missingOperatorKeyword` at the operator token (:11173-11250),
//! `parseOperatorName` reports `invalidOperator` at the offending lexeme,
//! and `parseFields` reports `missingConstFinalVarOrType` at the name
//! (:4429-4434). The full `<T 5 6>` member cascade matches the oracle run
//! recorded on #3052 (group row + the six member-lane rows).

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted —
/// the parity contract is the row SET (the conformance harness compares
/// (severity, code, file) sets), so the pins fix anchors, not emission order.
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

/// Skip-length surface (in tokens) of `source` at offset 0, outside a
/// declaration — the `computeType` scan face of the gate.
fn skip_len(source: &'static str) -> usize {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.skip_type_len_in(0, false, 0)
}

/// The bug face (`void Function<T 5 6>()? f;` in a class): the junk `5 6`
/// run after the type variable is never `(`, so the chain breaks at the
/// official line, the type falls back to bare `void`, and `Function`
/// becomes the member name — the member lane then cascades exactly like the
/// oracle (group recovery row at `T`, then `missingFunctionBody` /
/// `missingKeywordOperator` / `invalidOperator` / `missingMethodParameters`
/// at the stray `?`, then `missingFunctionBody` /
/// `missingConstFinalVarOrType` at `f`).
#[test]
fn junk_tail_breaks_gft_chain_member_cascade() {
    let rows = sorted_rows("class A {\n  void Function<T 5 6>()? f;\n}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_token", 26, 27),
            code("invalid_operator", 34, 35),
            code("missing_const_final_var_or_type", 36, 37),
            code("missing_function_body", 34, 35),
            code("missing_function_body", 36, 37),
            code("missing_keyword_operator", 34, 35),
            code("missing_method_parameters", 34, 35),
        ],
        "`<T 5 6>` must break the GFT chain and cascade like the oracle"
    );
}

/// Positive arm: `(` after the validated `<T>` head is a GFT. Both the
/// parameterized `(T)` and nullable `()` forms are valid declarations and
/// stay silent.
#[test]
fn positive_gft_with_type_variables_stays_silent() {
    assert!(
        sorted_rows("class A {\n  void Function<T>(T) f;\n}\n").is_empty(),
        "`void Function<T>(T) f;` is a valid GFT field"
    );
    assert!(
        sorted_rows("class A {\n  void Function<T>()? f;\n}\n").is_empty(),
        "`void Function<T>()? f;` is a valid nullable GFT field"
    );
}

/// Identifier follower (`void Function<T> x;`): the chain breaks, the type
/// falls back to bare `void`, and the member lane cascades —
/// `missingMethodParameters` at the name `Function`, `missingFunctionBody`
/// at `x`, then the leftover `x;` field reports
/// `missingConstFinalVarOrType` (noType + no `var`/`final`/`const`).
#[test]
fn identifier_follower_breaks_chain_to_named_face() {
    let rows = sorted_rows("class A {\n  void Function<T> x;\n}\n");
    assert_eq!(
        rows,
        vec![
            code("missing_const_final_var_or_type", 29, 30),
            code("missing_function_body", 29, 30),
            code("missing_method_parameters", 17, 25),
        ],
        "`<T> x` is a type-arg list + name, never a GFT formal list"
    );
}

/// Dot follower (`void Function<T>.x;`): the chain breaks and the member
/// lane cascades — `missingMethodParameters` at the name `Function`,
/// `missingFunctionBody` at the stray `.`, the `.x;` junk member head
/// reports `expectedClassMember`, and the leftover `x;` field reports
/// `missingConstFinalVarOrType`.
#[test]
fn dot_follower_breaks_chain_to_named_face() {
    let rows = sorted_rows("class A {\n  void Function<T>.x;\n}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_class_member", 28, 29),
            code("missing_const_final_var_or_type", 29, 30),
            code("missing_function_body", 28, 29),
            code("missing_method_parameters", 17, 25),
        ],
        "`<T>.x` must not swallow the junk run into a GFT formal list"
    );
}

/// Skip surface of the four follower arms: a broken chain discards the
/// whole `Function` suffix (bare `void` fallback, 1 token), the complete
/// chain skips the whole type.
#[test]
fn gate_skip_surface_follower_arms() {
    assert_eq!(
        skip_len("void Function<T 5 6>()? f;"),
        1,
        "junk-tail follower: chain discarded, bare `void` fallback"
    );
    assert_eq!(
        skip_len("void Function<T>.x;"),
        1,
        "dot follower: chain discarded, bare `void` fallback"
    );
    assert_eq!(
        skip_len("void Function<T> x;"),
        1,
        "identifier follower: chain discarded, bare `void` fallback"
    );
    assert_eq!(
        skip_len("void Function<T>(T) f;"),
        8,
        "paren follower: the whole GFT is the type"
    );
}
