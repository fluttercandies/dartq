//! Ticket #3898 — the `ensureTypeNotVoid` void face and the named-mixin
//! `with` splice. Three parser defects produced divergent
//! `expected_type_name` rows (census family routed from semantic #3887):
//!
//! 1. `invalidVoid` carried its fasta text "Type 'void' can't be used
//!    here." — but its `pseudoSharedCode` is `expectedTypeName`, so the
//!    analyzer converter re-renders it as the registry diagnostic
//!    (`error_converter.dart:96-99`). Only `Expected a type name.` is
//!    observable; the message now re-renders to that text
//!    (`message.rs::invalid_void`).
//! 2. `parse_type_not_void`'s `Void` arm emitted identifier+type events on
//!    the FOLLOWING token. Upstream `VoidType.ensureTypeNotVoid`
//!    (`type_info_impl.dart:427-432`) delegates to
//!    `simpleType.parseTypeNotVoid`, which consumes the `void` token itself
//!    as the type name — `extends void f()` anchors the type (and every
//!    downstream semantic row) on `void`, not `f`.
//! 3. `parseTypeList` (`with` lists) runs `ensureTypeOrVoid`
//!    (`parser_impl.dart:1266-1278`), not `ensureTypeNotVoid`: `with void`
//!    parses the void keyword silently (`handleVoidKeyword`), so the clause
//!    carries only the semantic non-class row. `parse_type_list` now picks
//!    the ensure flavor by clause (`With` → `ensure_type_or_void`).
//! 4. `parseMixinApplicationRest`'s missing-`with` recovery splices a
//!    synthetic `with` keyword and — when its follower cannot start a type
//!    — a synthetic identifier SILENTLY (`parser_impl.dart:1621-1627`).
//!    `class A = B class A {}` therefore reports `expected_token 'with'`
//!    at `class` and no `expected_type_name`; `parse_mixin_application_rest`
//!    now queues both synthetics.

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::FeatureSet;
use dartq_scanner::ScanConfig;

/// The collected listener output: parser rows `(code, start, end)`, the
/// parallel message texts, and the identifier events.
struct Out {
    errors: Vec<(String, u32, u32)>,
    texts: Vec<String>,
    idents: Vec<(String, u32, u32, usize)>,
}

/// Parses `source` and returns the collected listener output.
fn parse(source: &'static str) -> Out {
    let set = FeatureSet::latest();
    let config = ScanConfig::from_feature_set(set);
    let features = crate::ParserFeatures::from_feature_set(set);
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    Out {
        errors: parser.listener.errors.clone(),
        texts: parser.listener.texts.clone(),
        idents: parser.listener.idents.clone(),
    }
}

/// `(code, start, end)` rows.
fn rows(c: &Out) -> &[(String, u32, u32)] {
    &c.errors
}

/// The message text paired with the `expected_type_name` row at `start`.
fn type_name_text<'a>(c: &'a Out, start: u32) -> Option<&'a str> {
    c.errors
        .iter()
        .zip(c.texts.iter())
        .find(|((code, s, _), _)| code == "expected_type_name" && *s == start)
        .map(|(_, t)| t.as_str())
}

// ------------------------------------------------------------------
// face 1+2: `void` under `ensureTypeNotVoid` — analyzer text, `void` anchor
// ------------------------------------------------------------------

#[test]
fn mixin_on_void_reports_registry_text() {
    // `mixin M on void {}` — oracle: `expected_type_name` at `void`
    // (11,15) "Expected a type name." (+ the semantic constraint row).
    let c = parse("mixin M on void {}\n");
    assert!(
        rows(&c).contains(&("expected_type_name".to_owned(), 11, 15)),
        "expected expected_type_name at (11,15), got {:?}",
        rows(&c)
    );
    assert_eq!(
        type_name_text(&c, 11),
        Some("Expected a type name."),
        "invalidVoid must re-render as the registry text: {:?}",
        c.texts
    );
    // The void token itself is the clause type: the TypeReference
    // identifier event anchors on `void`, not the `{` follower.
    assert!(
        c.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 11 && *e == 15),
        "expected TypeReference identifier on `void` (11,15): {:?}",
        c.idents
    );
    assert!(
        !c.idents
            .iter()
            .any(|(ctx, s, _, _)| ctx == "TypeReference" && *s == 16),
        "type events must not anchor on the follower: {:?}",
        c.idents
    );
}

#[test]
fn extends_void_member_consumes_void_as_type() {
    // `class A extends void f() {}` — oracle: `expected_type_name` at
    // `void` (16,20) with the registry text; `f` is the member name.
    let c = parse("class A extends void f() {}\n");
    assert_eq!(
        type_name_text(&c, 16),
        Some("Expected a type name."),
        "expected registry text at void (16,20): {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
    assert!(
        rows(&c).contains(&("expected_type_name".to_owned(), 16, 20)),
        "expected expected_type_name at (16,20): {:?}",
        rows(&c)
    );
    assert!(
        c.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 16 && *e == 20),
        "TypeReference must anchor on `void`: {:?}",
        c.idents
    );
}

#[test]
fn implements_void_reports_registry_text() {
    // `class D implements void {}` — oracle: `expected_type_name` at `void`
    // (19,23) + `implements_non_class`.
    let c = parse("class D implements void {}\n");
    assert_eq!(
        type_name_text(&c, 19),
        Some("Expected a type name."),
        "expected registry text at void (19,23): {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
}

#[test]
fn is_as_void_report_registry_text_and_void_anchor() {
    // `a is void` / `x as void` — oracle: `expected_type_name` at `void`
    // with the registry text; the type events anchor on `void`.
    let c = parse("var k = a is void;\nvar m = x as void;\n");
    assert_eq!(
        type_name_text(&c, 13),
        Some("Expected a type name."),
        "is void: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
    assert_eq!(
        type_name_text(&c, 32),
        Some("Expected a type name."),
        "as void: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
    assert!(
        c.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 13 && *e == 17),
        "is void TypeReference must anchor on `void` (13,17): {:?}",
        c.idents
    );
    assert!(
        c.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 32 && *e == 36),
        "as void TypeReference must anchor on `void` (32,36): {:?}",
        c.idents
    );
}

#[test]
fn catch_on_void_reports_registry_text() {
    // `f() { try {} on void catch (e) {} }` — oracle: `expected_type_name`
    // at `void` (16,20) + `nullable_type_in_catch_clause` (semantic face,
    // not asserted).
    let c = parse("f() { try {} on void catch (e) {} }\n");
    assert_eq!(
        type_name_text(&c, 16),
        Some("Expected a type name."),
        "on void: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
}

#[test]
fn type_parameter_bound_void_reports_registry_text() {
    // `class G<T extends void> {}` — oracle: `expected_type_name` at `void`
    // (18,22) "Expected a type name."
    let c = parse("class G<T extends void> {}\n");
    assert_eq!(
        type_name_text(&c, 18),
        Some("Expected a type name."),
        "bound void: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
}

// ------------------------------------------------------------------
// face 3: `with` lists run `ensureTypeOrVoid` — `void` parses silently
// ------------------------------------------------------------------

#[test]
fn class_with_void_is_silent() {
    // `class D with void {}` — oracle: NO `expected_type_name`; only the
    // semantic `mixin_of_non_class` at `void`. The with-list ensure is
    // `ensureTypeOrVoid`, so the void keyword parses as a void type.
    let c = parse("class D with void {}\n");
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "with void must not report expected_type_name: {:?}",
        rows(&c)
    );
}

#[test]
fn enum_with_void_is_silent() {
    // `enum En with void { a }` — oracle: `mixin_of_non_class` at `void`
    // only; no `expected_type_name`.
    let c = parse("enum En with void { a }\n");
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "enum with void must not report expected_type_name: {:?}",
        rows(&c)
    );
}

#[test]
fn named_mixin_with_void_is_silent() {
    // `class C = Object with void;` — oracle: `mixin_of_non_class` at
    // `void` only; no `expected_type_name`.
    let c = parse("class C = Object with void;\n");
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "= Object with void must not report expected_type_name: {:?}",
        rows(&c)
    );
}

// ------------------------------------------------------------------
// face 4: missing `with` splices a silent synthetic identifier
// ------------------------------------------------------------------

#[test]
fn equals_name_class_keyword_splices_silently() {
    // `class A = B class A {}` — oracle: `expected_token ';'` at `B`,
    // `expected_token 'with'` at `class`, `duplicate_definition` at `A` —
    // NO `expected_type_name`. The missing-`with` recovery splices a
    // synthetic identifier ahead of `class` (silent) and the `class A {}`
    // tail reparses as a real declaration.
    let c = parse("class A = B class A {}\n");
    assert!(
        rows(&c).contains(&("expected_token".to_owned(), 12, 17)),
        "expected expected_token 'with' at `class` (12,17): {:?}",
        rows(&c)
    );
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "synthetic identifier splices silently — no expected_type_name: {:?}",
        rows(&c)
    );
}

#[test]
fn equals_name_modifier_splices_silently() {
    // `class A = B const A {}` — oracle: `expected_token ';'` at `B`,
    // `extraneous_modifier` + `expected_token 'with'` at `const`, then
    // `A {}` reparses — NO `expected_type_name`.
    let c = parse("class A = B const A {}\n");
    assert!(
        rows(&c).contains(&("expected_token".to_owned(), 12, 17)),
        "expected expected_token 'with' at `const` (12,17): {:?}",
        rows(&c)
    );
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "no expected_type_name before a modifier: {:?}",
        rows(&c)
    );
}

#[test]
fn equals_name_eof_splices_silently() {
    // `class A = B` at EOF — oracle: `expected_token ';'` at `B`,
    // `expected_token 'with'` at EOF (12,12), no `expected_type_name`.
    let c = parse("class A = B\n");
    assert!(
        rows(&c).contains(&("expected_token".to_owned(), 12, 12)),
        "expected expected_token 'with' at EOF (12,12): {:?}",
        rows(&c)
    );
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "no expected_type_name at EOF: {:?}",
        rows(&c)
    );
}

#[test]
fn equals_name_valid_follower_parses_mixin() {
    // `class A = B C {}` — the follower `C` IS a valid type reference, so
    // no synthetic identifier is spliced: `C` parses as the (single) mixin
    // and reports no type-name row either.
    let c = parse("class A = B C {}\n");
    assert!(
        !rows(&c)
            .iter()
            .any(|(code, _, _)| code == "expected_type_name"),
        "a real mixin name after missing `with` stays silent: {:?}",
        rows(&c)
    );
}

// ------------------------------------------------------------------
// controls: malformed variants must still report
// ------------------------------------------------------------------

#[test]
fn extends_literal_still_reports() {
    // `class C extends 5 {}` — oracle: `expected_type_name` + `unexpected_token`
    // at `5` (16,17). The NoType ensure path still reports.
    let c = parse("class C extends 5 {}\n");
    assert!(
        rows(&c).contains(&("expected_type_name".to_owned(), 16, 17)),
        "expected expected_type_name at `5` (16,17): {:?}",
        rows(&c)
    );
    assert_eq!(
        type_name_text(&c, 16),
        Some("Expected a type name."),
        "NoType.ensureTypeNotVoid keeps the registry text: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
}

#[test]
fn with_empty_clause_still_reports() {
    // `class F with {}` — oracle: `expected_type_name` at `{` (13,14).
    // `ensureTypeOrVoid` only tolerates `void`; a missing type still
    // reports (the NoType arm runs `ensureTypeNotVoid` upstream and ours
    // reports through the same ensure).
    let c = parse("class F with {}\n");
    assert!(
        rows(&c).contains(&("expected_type_name".to_owned(), 13, 14)),
        "expected expected_type_name at `{{` (13,14): {:?}",
        rows(&c)
    );
}

#[test]
fn implements_void_function_member_tail() {
    // `class E implements void f() {}` — the void arm consumed `void` as
    // the interface type and `f() {}` reparses as the member; oracle keeps
    // `implements_non_class` anchored on `void` (semantic tier).
    let c = parse("class E implements void f() {}\n");
    assert_eq!(
        type_name_text(&c, 19),
        Some("Expected a type name."),
        "implements void member tail: {:?}",
        c.errors.iter().zip(c.texts.iter()).collect::<Vec<_>>()
    );
    assert!(
        c.idents
            .iter()
            .any(|(ctx, s, e, _)| ctx == "TypeReference" && *s == 19 && *e == 23),
        "TypeReference must anchor on `void`: {:?}",
        c.idents
    );
}
