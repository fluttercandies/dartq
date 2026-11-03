//! Ticket #3740 — recovered-identifier contexts and built-in type-argument
//! members: two recovery decision-order fixes against oracle 3.13.7.
//!
//! (a) `ensureIdentifierPotentiallyRecovered` (`identifier_context_impl.dart`):
//! the FieldDeclaration (:512-535) and MethodDeclaration (:1187-1209, shared
//! by the continuation/operatorName/primaryConstructor constructors)
//! context classes override the default `=> ensureIdentifier` route. On the
//! recovered route an identifier-shaped head is consumed silently and a
//! keyword head is consumed AS THE NAME under
//! `expectedIdentifierButGotKeyword` — the keep-set reparse is bypassed, so
//! `int const(int x) {}` names the method `const` instead of reparsing
//! `const` as the next member's modifier (the dartq-side cascade was
//! `const_method`/`extraneous_modifier`/`missing_function_body`/
//! `missing_method_parameters` against the oracle's single
//! `expected_identifier_but_got_keyword`).
//!
//! (b) `computeType` inside `<...>` (`type_info_impl.dart:1348-1425` group
//! scan): the member slot calls `computeType(required: true)`, which admits
//! built-in keywords via `computeBuiltinOrVarAsType` — `extension <as> on
//! List {}` produces a single `built_in_identifier_in_declaration` on the
//! type variable `as`. dartq rejected `as` (the mainline
//! `is_valid_non_record_type_reference` requires a `.` follower for
//! built-ins), so the whole group was dropped and the tail `on List {}`
//! produced a cascade of `expected_token`/`expected_type_name`/
//! `expected_body` rows the oracle never emits.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{Feature, FeatureSet};
use dartq_scanner::ScanConfig;

/// Parses `source` at `set` and returns the scanner+parser rows as
/// `(code, start, end)` — UTF-16 offsets, matching the ticket_3718 harness.
fn rows_at(source: &'static str, set: FeatureSet) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig::from_feature_set(set);
    let features = crate::ParserFeatures::from_feature_set(set);
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
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
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn augment_set() -> FeatureSet {
    FeatureSet::latest().with_experiment(Feature::Augmentations)
}

// ------------------------------------------------------------------
// recovered keyword names: `int <keyword>(...)` / `int <keyword> = 0;`
// ------------------------------------------------------------------

#[test]
fn keyword_named_methods_report_single_keyword_error() {
    // Oracle on `class C { int const(int x) {} ... }` (each line offset 7,
    // len 5 for `const`/`final`, 3 for `var`, 4 for `void`): exactly one
    // `expected_identifier_but_got_keyword` per member — the keyword is the
    // method name, and the `(...)` is its parameter list.
    let rows = rows_at(
        "class C {\n  int const(int x) {}\n  int final(int x) {}\n  int var(int x) {}\n  int void(int x) {}\n}\n",
        augment_set(),
    );
    // `const` at offset 16, `final` at 38, `var` at 60, `void` at 80.
    for (span_start, span_end, name) in [
        (16, 21, "const"),
        (38, 43, "final"),
        (60, 63, "var"),
        (80, 84, "void"),
    ] {
        assert!(
            rows.contains(&("expected_identifier_but_got_keyword".to_owned(), span_start, span_end)),
            "expected expected_identifier_but_got_keyword at ({span_start},{span_end}) for {name}, got {rows:?}"
        );
    }
    // The whole recovery cascade the oracle does NOT emit must be absent:
    // `const`/`var`/`void` are names, not modifiers.
    for code in [
        "const_method",
        "extraneous_modifier",
        "missing_function_body",
        "missing_method_parameters",
        "expected_token",
        "missing_identifier",
        "expected_body",
    ] {
        assert!(
            !rows.iter().any(|(c, _, _)| c == code),
            "cascade row {code} survived, got {rows:?}"
        );
    }
}

#[test]
fn keyword_named_fields_report_single_keyword_error() {
    // Oracle on `class C { int const = 0; ... }`: exactly one
    // `expected_identifier_but_got_keyword` per field.
    let rows = rows_at(
        "class C {\n  int const = 0;\n  int final = 0;\n  int var = 0;\n  int void = 0;\n}\n",
        augment_set(),
    );
    // `const` at 16, `final` at 33, `var` at 50, `void` at 65.
    for (span_start, span_end, name) in [
        (16, 21, "const"),
        (33, 38, "final"),
        (50, 53, "var"),
        (65, 69, "void"),
    ] {
        assert!(
            rows.contains(&("expected_identifier_but_got_keyword".to_owned(), span_start, span_end)),
            "expected expected_identifier_but_got_keyword at ({span_start},{span_end}) for {name}, got {rows:?}"
        );
    }
    for code in [
        "const_method",
        "extraneous_modifier",
        "expected_token",
        "missing_identifier",
    ] {
        assert!(
            !rows.iter().any(|(c, _, _)| c == code),
            "cascade row {code} survived, got {rows:?}"
        );
    }
}

#[test]
fn identifier_named_method_still_consumed_silently() {
    // Negative control: a normal `int m(int x) {}` produces no identifier
    // diagnostics — the recovered fast path must not report on plain names.
    let rows = rows_at(
        "class C {\n  int m(int x) { return x; }\n}\n",
        augment_set(),
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "expected_identifier_but_got_keyword"
                || c == "missing_identifier"),
        "plain method name was reported: {rows:?}"
    );
}

// ------------------------------------------------------------------
// built-in keyword as type-argument member: `extension <as> on List {}`
// ------------------------------------------------------------------

#[test]
fn extension_builtin_type_variable_reports_single_builtin_error() {
    // `test_extension_as` (`built_in_identifier_as_type_parameter_name_test`):
    // oracle emits exactly `built_in_identifier_in_declaration` on `as`
    // (offset 11, len 2) — `as` is admitted as a recovered group member, then
    // the TypeVariableDeclaration identifier check flags it.
    let rows = rows_at("extension <as> on List {}\n", augment_set());
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 11, 13)),
        "expected built_in_identifier_in_declaration at (11,13), got {rows:?}"
    );
    // The rejected-group cascade (`expected_token 'on'` twice,
    // `expected_type_name`, `expected_body`) must be gone entirely.
    for code in [
        "expected_token",
        "expected_type_name",
        "expected_body",
        "missing_identifier",
        "unexpected_token",
    ] {
        assert!(
            !rows.iter().any(|(c, _, _)| c == code),
            "cascade row {code} survived, got {rows:?}"
        );
    }
}

#[test]
fn named_extension_builtin_type_variable_reports_single_builtin_error() {
    // `extension Ext <as> on List {}` — same admission path; `as` at
    // offset 15, len 2.
    let rows = rows_at("extension Ext <as> on List {}\n", augment_set());
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 15, 17)),
        "expected built_in_identifier_in_declaration at (15,17), got {rows:?}"
    );
    for code in [
        "expected_token",
        "expected_type_name",
        "expected_body",
        "missing_identifier",
    ] {
        assert!(
            !rows.iter().any(|(c, _, _)| c == code),
            "cascade row {code} survived, got {rows:?}"
        );
    }
}
