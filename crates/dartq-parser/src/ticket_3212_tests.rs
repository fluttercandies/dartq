//! Ticket #3212 — F4 recovery-position family pins. One positive + one
//! negative per repaired recovery arm, each frozen to the oracle 3.13.7 row
//! (`dart analyze --format=json` on the upstream partial_code source with
//! the `//`-marker expectation lines stripped — they are the test DSL, not
//! source):
//!
//! * `DottedName` — a top-level-declaration head in a conditional-uri name
//!   slot splices a synthetic name and stays for the declaration loop
//!   (`DottedNameIdentifierContext.ensureIdentifier`,
//!   identifier_context_impl.dart:259-266): `import 'a.dart' if ( class A {}`
//!   reports `missing_identifier` at `class`, the `)`-expect anchors there,
//!   and the `;`-expect anchors on the last consumed token `(`.
//! * `FieldDeclaration` — a member-start head in the field name slot splices
//!   (`FieldDeclarationIdentifierContext.ensureIdentifier`, :484-492):
//!   `A f, var f;` reports `missing_identifier` at `var` and the `;`-expect
//!   anchors on the `,` (`findPreviousNonZeroLengthToken` walk-back).
//! * `MethodDeclaration` — the same member-start keep-set on the method name
//!   slot (:1159-1166): `A f, void g() {}` reports at `void`, not the
//!   keyword-as-name face.
//! * Bare `super` initializer tail — a non-`(` follower after `super` takes
//!   `insertParens(token, includeIdentifier: false)`
//!   (parser_impl.dart:4918-4924): `expectedAfterButGot('(')` anchors ON the
//!   follower, the follower stays current, and the constructor carries
//!   `missing_function_body` at the same token.
//! * `super.` + reserved non-statement keyword — the template consumes the
//!   keyword as the member name with `expectedIdentifierButGotKeyword`
//!   (:429-441); statement-start keywords (`var`) splice instead
//!   (:461-467).
//! * `catch (e, <junk>` — `CatchParameterIdentifierContext.ensureIdentifier`
//!   (:33-44) reports `catchSyntax` and consumes non-keyword junk in doubt.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::{latest_features, Collector};
use dartq_scanner::ScanConfig;

/// Parses `source` and returns the scanner+parser rows as a set of
/// `(code, start, end)` (the oracle rows compare position-sorted).
fn rows(source: &'static str) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let mut rows: BTreeSet<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn assert_rows(source: &'static str, expected: &[(&str, u32, u32)]) {
    let expected: BTreeSet<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| ((*c).to_owned(), *s, *e))
        .collect();
    assert_eq!(rows(source), expected, "rows for {source:?}");
}

#[test]
fn dotted_name_reserved_head_splices() {
    // Oracle: partial_code/import_directive/ImportDirectivesTest/
    // test_import_directive_ifParen_class — `import 'a.dart' if ( class A {}`
    // reports `missing_identifier` at `class` (the reserved head splices a
    // synthetic name and stays for the declaration loop), `expected_token
    // ')'` at `class`, `expected_token ';'` anchored on the last consumed
    // token `(`, and `expected_string_literal` at `class`.
    assert_rows(
        "\nimport 'a.dart' if ( class A {}\n",
        &[
            ("expected_string_literal", 22, 27),
            ("expected_token", 20, 21),
            ("expected_token", 22, 23),
            ("missing_identifier", 22, 27),
        ],
    );
    // Negative: the legal conditional-uri sibling stays silent.
    assert_rows("\nimport 'a.dart' if (dart.library.io) 'b.dart';\n", &[]);
}

#[test]
fn dotted_name_identifier_head_kept_on_grammar_follower() {
    // `import.b ==` — a BUILT-IN top-level head keeps the name when the
    // follower continues the dotted-name grammar (keep-set `.` `==` `)` EOF,
    // identifier_context_impl.dart:236-241): `import` is the configuration
    // name here and the conditional URI is legal.
    assert_rows("\nimport 'a.dart' if (import.b == 'x') 'b.dart';\n", &[]);
}

#[test]
fn field_declaration_member_start_head_splices() {
    // Oracle: partial_code/field_declaration/MethodTest/
    // test_field_declaration_type_name_comma_field — `A f, var f;` reports
    // `missing_identifier` at `var` (the member-start head splices and
    // reparses as the next field) and `expected_token ';'` anchored on the
    // `,` — never `expected_identifier_but_got_keyword`.
    assert_rows(
        "\nclass C { A f, var f; }\n",
        &[("expected_token", 14, 15), ("missing_identifier", 16, 19)],
    );
    assert_rows(
        "\nclass C { A f, final f; }\n",
        &[("expected_token", 14, 15), ("missing_identifier", 16, 21)],
    );
    // `A f, const f;` splices the same way upstream
    // (`missing_identifier` at `const` + `;`-expect at `,`); the oracle's
    // `const_instance_field` row on the reparsed member is a semantic-tier
    // face outside the parser's event stream, and dartq's parse-tier
    // `const_not_initialized` rides the same slot — neither is asserted
    // here.
    // Negative: the legal comma list stays silent.
    assert_rows("\nclass C { A f, g; }\n", &[]);
}

#[test]
fn method_declaration_member_start_head_splices() {
    // Oracle: partial_code/field_declaration/MethodTest/
    // test_field_declaration_type_name_comma_methodVoid — `A f, void g()`
    // reports `missing_identifier` at `void` (member-start head on the
    // method name slot) and `expected_token ';'` anchored on the `,`.
    assert_rows(
        "\nclass C { A f, void g() {} }\n",
        &[("expected_token", 14, 15), ("missing_identifier", 16, 20)],
    );
    // Negative: the two-member sibling stays silent.
    assert_rows("\nclass C { A f; void g() {} }\n", &[]);
}

#[test]
fn super_initializer_bare_follower_reports_on_follower() {
    // Oracle: partial_code/constructor_declaration/ConstructorTest/
    // test_constructor_super_eof — `C() : super}` reports
    // `expected_token '('` ON the `}` (the insertParens follower anchor,
    // parser_impl.dart:4918-4924) plus `missing_function_body` at the same
    // token; the `}` stays current for the class-body close.
    assert_rows(
        "\nclass C {C() : super}",
        &[("expected_token", 21, 22), ("missing_function_body", 21, 22)],
    );
    // test_constructor_super_getter — `C() : super int get a => 0;`
    // anchors the same pair on `int` (the construct delimiter), then the
    // `int get a => 0;` member reparses clean.
    assert_rows(
        "\nclass C {C() : super int get a => 0;}",
        &[("expected_token", 22, 25), ("missing_function_body", 22, 25)],
    );
    // Negative: a real `super()` call stays silent.
    assert_rows("\nclass C {C() : super();}", &[]);
}

#[test]
fn super_dot_keyword_name_faces() {
    // `super. var f;` — a statement-start head splices instead
    // (`insertSyntheticIdentifier`, identifier_context_impl.dart:461-467):
    // `missing_identifier` at `var`, the `(`-expect anchors on `var`
    // itself (the spliced name's follower is the keyword), and the
    // nameless body position carries `missing_function_body` at the same
    // token (oracle test_constructor_super_dot_field).
    assert_rows(
        "\nclass C {C() : super. var f;}",
        &[
            ("missing_identifier", 23, 26),
            ("expected_token", 23, 26),
            ("missing_function_body", 23, 26),
        ],
    );
}

#[test]
fn annotation_local_ampersand_labeled() {
    // Oracle: partial_code/annotation/AnnotationTest/
    // test_annotation_local_ampersand_labeled — `f() { @ l: {} }` reports
    // `missing_identifier` at `:` (the annotation-name slot's junk head
    // splices) and `expected_token` at `:` — both anchored on the trigger
    // token, no phantom `unexpected_token`.
    assert_rows(
        "\nf() { @ l: {} }\n",
        &[("missing_identifier", 10, 11), ("expected_token", 10, 11)],
    );
    // Negative: a real labeled statement stays silent.
    assert_rows("\nf() { l: {} }\n", &[]);
}
