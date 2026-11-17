//! #4344 pins — the nameless recovery field's not-initialized obligation.
//!
//! Upstream's declaration walk iterates `variableList.variables`, the
//! parser-recovered synthetic name included: `variable.name.lexeme`
//! renders '' and the row anchors `.at(variable.name)` — the zero-width
//! synthetic token (`_checkForNotInitializedFieldDeclaration`,
//! error_verifier.dart:6658-6687; the top-level twin
//! `visitTopLevelVariableDeclaration`, :2102-2125). A recovered field is
//! a real element, never a dropped declaration, so the constructor-head
//! `<` group's field recovery (corpus
//! ErrorParserTest/test_constructorPartial{,2,3}) owes the same
//! `not_initialized_non_nullable_instance_field ''` row at its synthetic
//! name as a written name does.
//!
//! The initializer gate is upstream's `variable.initializer != null` read
//! off the token stream: the pattern-variable recovery face diet-skips
//! its `= (0, 1)` where upstream's `dropRange` keeps it parseable
//! (parser_impl.dart:5343-5345), so the `begin_variable_initializer`
//! flip alone would report those as uninitialized (corpus
//! Patterns/variable_declaration_A06_t0{1,2} — no rows there). The scan
//! stops at the group's `;`/`}`/EOF and at the next member's provable
//! heads (the `isModifier` keep-set and a metadata `@`), so a following
//! member's initializer never reads as the synthetic's own.
//!
//! Not covered here: a name LIST's continuation slot (`int f3 = -1, var
//! f4 = 0;`, `int f3 = -1, @a f4 = -1;` — corpus
//! variable_declaration_metadata_test_08) fires its synthetic under
//! `localVariableDeclaration` (upstream
//! `parseOptionallyInitializedIdentifier`'s own context,
//! parser_impl.dart:10080-10089), which this face never sees — that
//! under-report is the local arm's own gap.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

const INSTANCE: &str = "not_initialized_non_nullable_instance_field";
const VARIABLE: &str = "not_initialized_non_nullable_variable";
const FINAL: &str = "final_not_initialized";

fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

fn instance_message(name: &str) -> String {
    format!("Non-nullable instance field '{name}' must be initialized.")
}

/// The not-initialized family's rows: `(offset, length, message)` sorted
/// by offset — the failure face names every row.
fn init_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == INSTANCE || d.code.name == VARIABLE || d.code.name == FINAL)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// Exactly one instance-field row of `name` at `offset`, zero width.
#[track_caller]
fn assert_nameless_row(report: &FileReport, fixture: &str, needle_tail: &str, offset: usize) {
    assert_eq!(
        init_rows(report),
        [(offset as u32, 0, instance_message(""))],
        "fixture {fixture:?} (needle tail {needle_tail:?})"
    );
}

/// No not-initialized family row anywhere.
#[track_caller]
fn assert_no_init_rows(report: &FileReport, fixture: &str) {
    assert_eq!(
        init_rows(report),
        [] as [(u32, u32, String); 0],
        "fixture {fixture:?}"
    );
}

/// `class C { C< }` (corpus test_constructorPartial): the empty group's
/// field recovery stages the nameless field — the `''` row at the `}`'s
/// zero-width synthetic name.
#[test]
fn bare_lt_group_stages_nameless_field_row() {
    let fixture = "class C { C< }\n";
    let report = report_ops(fixture);
    assert_nameless_row(&report, fixture, "C< }", at(fixture, "C< }") + 3);
}

/// `class C { C<@Foo }` (corpus test_constructorPartial2): one annotation
/// inside the group — same staging, the row at the `}`.
#[test]
fn single_annotation_group_stages_nameless_field_row() {
    let fixture = "class C { C<@Foo }\n";
    let report = report_ops(fixture);
    assert_nameless_row(&report, fixture, "C<@Foo }", at(fixture, "C<@Foo }") + 7);
}

/// `class C { C<@Foo @Bar() }` (corpus test_constructorPartial3): the
/// consecutive-annotation regression shape — same staging, the row at
/// the `}`.
#[test]
fn consecutive_annotations_group_stages_nameless_field_row() {
    let fixture = "class C { C<@Foo @Bar() }\n";
    let report = report_ops(fixture);
    assert_nameless_row(
        &report,
        fixture,
        "@Bar() }",
        at(fixture, "C<@Foo @Bar() }") + "C<@Foo @Bar() ".len(),
    );
}

/// The written-name controls stay on their written spellings: `int x;`
/// reports `'x'` at the name, an initializer or a `?` silences, and the
/// parameterized probe shape (`C<int> x;` — a real name after the closed
/// group) reports through the instantiated type.
#[test]
fn real_name_fields_keep_written_name_rows() {
    let fixture = "class C { int x; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        init_rows(&report),
        [(at(fixture, "x;") as u32, 1, instance_message("x"))],
        "fixture {fixture:?}"
    );

    assert_no_init_rows(&report_ops("class C { int x = 0; }\n"), "int x = 0;");
    assert_no_init_rows(&report_ops("class C { int? x; }\n"), "int? x;");

    let fixture = "class C { C<int> x; }\n";
    let report = report_ops(fixture);
    assert_eq!(
        init_rows(&report),
        [(at(fixture, "x;") as u32, 1, instance_message("x"))],
        "fixture {fixture:?}"
    );
}

/// The pattern-variable recovery face keeps its initializer: `static
/// final (a, b) = (0, 1);` (corpus variable_declaration_A06_t02) and the
/// map-pattern spelling stay silent — the `=` is the declarator's own
/// and the `{` of a map pattern never reads as a group boundary.
#[test]
fn pattern_variable_initializer_stays_silent() {
    assert_no_init_rows(
        &report_ops("class C {\n  static final (a, b) = (0, 1);\n}\n"),
        "static final (a, b) = (0, 1);",
    );
    assert_no_init_rows(
        &report_ops("class C {\n  static final {\"k\": a} = {\"k\": 1};\n}\n"),
        "static final {\"k\": a} = {\"k\": 1};",
    );
    // Top level (corpus variable_declaration_A06_t01): same silence.
    assert_no_init_rows(
        &report_ops("final ((a, b) && r) = (1, 2);\n"),
        "final ((a, b) && r) = (1, 2);",
    );
}
