//! #3178 pins: parser-recovery shapes whose AST determines the
//! `undefined_class`/`undefined_identifier` semantic rows (oracle 3.13.7).
//! The parser's contract is the recovery row set plus the type events that
//! mark each name a type reference — the `undefined_class` rows themselves
//! are semantic-tier and out of `parser.listener.errors`. Upstream
//! citations live per-test.

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

/// `handle type` events emitted for `source` — a name the resolver reads as
/// a type reference (`undefined_class` head).
fn type_event_count(source: &'static str) -> usize {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    collector
        .log
        .iter()
        .filter(|e| *e == &("handle", "type"))
        .count()
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// `class` + a name-slot keyword that `looksLikeStartOfNextTopLevelDeclaration`
/// (`typedef` is `isTopLevelKeyword`) splices a synthetic name and leaves the
/// keyword for the declaration loop — `typedef A = B Function(C, D)` then
/// reparses as a generalized-function-type typedef, so `B`/`C`/`D` are type
/// references (three `handle type` events: the GFT return type and the two
/// parameters), not a variable-initializer expression
/// (`ClassOrMixinOrExtensionIdentifierContext.ensureIdentifier`,
/// identifier_context_impl.dart:80-86).
#[test]
fn class_typedef_reparses_as_gft_typedef() {
    let rows = sorted_rows("class typedef A = B Function(C, D);\n");
    assert_eq!(
        rows,
        vec![
            code("expected_body", 6, 13),
            code("missing_identifier", 6, 13),
        ],
        "oracle: the `class` name recovery reports `missing_identifier` + \
         `expected_body` on `typedef` (6,13) and nothing else — `typedef` \
         reparses as the typedef head"
    );
    assert_eq!(
        type_event_count("class typedef A = B Function(C, D);\n"),
        3,
        "oracle: `B`/`C`/`D` are GFT type references — three `handle type` \
         events (the `B` return type plus `C`/`D` parameters)"
    );
}

/// `extension`'s `on`-clause recovery consumes the offending clause keyword
/// before `computeType` (parser_impl.dart:3713-3721: `token` advances to the
/// `extends` so `computeType` reads `C`, not the keyword). `extension E
/// extends C {}` reports `expected_instead 'on'` on `extends` and resolves
/// `C` as a type — one `handle type` event, the `undefined_class` head.
#[test]
fn extension_extends_resolves_type_after_clause_keyword() {
    let rows = sorted_rows("extension E extends C {}\n");
    assert_eq!(
        rows,
        vec![code("expected_instead", 12, 19)],
        "oracle: only `expected_instead 'on'` on `extends` (12,19) — `C` \
         parses as the `on` type, no `expected_type_name`/`unexpected_token`"
    );
    assert_eq!(
        type_event_count("extension E extends C {}\n"),
        1,
        "oracle: `C` is the `on`-clause type — one `handle type` event"
    );
}

/// A primary-constructor parameter `a b` reads `a` as the type and `b` as
/// the name (parser_impl.dart parsePrimaryConstructor →
/// parseFormalParameters) — `a` is a type reference (`handle type`), the
/// `undefined_class 'a'` head. The `;` body is the constructor's own.
#[test]
fn primary_constructor_param_reads_type_and_name() {
    let rows = sorted_rows("class C(a b);\n");
    assert_eq!(
        rows,
        vec![],
        "oracle: `class C(a b);` parses clean — `a` the param type, `b` the \
         name, `;` the constructor body"
    );
    assert_eq!(
        type_event_count("class C(a b);\n"),
        1,
        "oracle: `a` is the parameter type — one `handle type` event"
    );
}

/// A plain `if (expr);` condition parses `complicated.expression.foo()` as a
/// method-call expression — `complicated` is the identifier head
/// (`undefined_identifier`, never a prefixed type). The doc-comment
/// `[complicated.expression]` reference that yields `undefined_class
/// 'expression'` is the comment-reference resolver's prefixed-type grammar,
/// not this statement parse (comment_reference_resolver.dart).
#[test]
fn if_condition_method_call_keeps_expression_head() {
    let rows = sorted_rows("void f() {\n  if (complicated.expression.foo());\n}\n");
    assert_eq!(
        rows,
        vec![],
        "oracle: `if (complicated.expression.foo());` is a clean method-call \
         condition — `complicated` resolves as the identifier head"
    );
}
