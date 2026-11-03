//! Ticket #3721 pins: the Map-kind pattern head row and the two
//! relational-pattern signature verdicts.
//!
//! * `checkPatternNeverMatchesValueType`'s Map kind (upstream Dart SDK
//!   source, `resolveMapPattern` at pkg/analyzer/lib/src/generated/
//!   resolver.dart:1599-1640 fed by `analyzeMapPattern`,
//!   pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
//!   1257-1295): a `case {...}` over a matched face that cannot be a `Map`
//!   reports `pattern_never_matches_value_type` at the whole `{...}` node
//!   with `Map<Object?, Object?>` as the required type. The emit site
//!   existed for the entry/empty/duplicate-key faces but the head row had
//!   no call site.
//! * `analyzeRelationalPattern`'s `operator != null` arm
//!   (type_analyzer.dart:1985-2006): the operand constant's static type
//!   must be assignable to the resolved operator's first parameter type
//!   (`relational_pattern_operand_type_not_assignable` at the operand
//!   expression) and the operator's return type must be assignable to
//!   `bool` (`relational_pattern_operator_return_type_not_assignable_
//!   to_bool` at the operator token). `==`/`!=` resolve `operator ==`,
//!   their parameter type testing under `makeNullable` while the message
//!   keeps the declared spelling (resolver.dart:1703-1711, 1742-1744).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::FileReport;

/// The prelude shifts every offset; the fixtures below are ASCII.
fn prelude_len() -> u32 {
    CORE_PRELUDE.chars().map(char::len_utf16).sum::<usize>() as u32
}

/// The absolute UTF-16 offset of `needle`'s first occurrence inside
/// `context` inside the fixture.
fn at(fixture: &str, context: &str, needle: &str) -> u32 {
    let context_start = fixture.find(context).expect("context is in the fixture");
    let within = context.find(needle).expect("needle is in the context");
    let upto = context_start + within;
    prelude_len() + fixture[..upto].chars().map(char::len_utf16).sum::<usize>() as u32
}

fn rows_of<'a>(report: &'a FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn row(offset: u32, length: u32, message: &str) -> (u32, u32, String) {
    (offset, length, message.to_string())
}

/// The ticket's minimal repro: `{...}` over `int` can never hold a `Map`.
#[test]
fn map_pattern_over_int_reports_never_matches() {
    let fixture = "void f(int i) { switch (i) { case {1: var a}: break; } }";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, "pattern_never_matches_value_type"),
        vec![row(
            at(fixture, "case {1: var a}", "{"),
            "{1: var a}".chars().map(char::len_utf16).sum::<usize>() as u32,
            "The matched value type 'int' can never match the required type 'Map<Object?, Object?>'."
        )],
        "{:?}",
        rows(&report)
    );
}

/// Control: the same spelling over an actual `Map` is silent.
#[test]
fn map_pattern_over_map_is_silent() {
    let fixture = "void f(Map<int, int> m) { switch (m) { case {1: var a}: break; } }";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "pattern_never_matches_value_type").is_empty(),
        "{:?}",
        rows(&report)
    );
}

/// `Object` matched: an `Object` value can be a `Map`, so the row stays
/// off — the `canBeSubtypeOf` posture the oracle measures.
#[test]
fn map_pattern_over_object_is_silent() {
    let fixture = "void f(Object m) { switch (m) { case {1: var a}: break; } }";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "pattern_never_matches_value_type").is_empty(),
        "{:?}",
        rows(&report)
    );
}

/// The operand-type verdict at `.at(pattern.operand)` — the operand
/// literal's own span, never the operator token.
#[test]
fn relational_operand_mismatch_reports_at_operand() {
    let fixture = "class A { bool operator >(A o) => true; } \
                   void f(A x) { switch (x) { case > 0: break; } }";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, "relational_pattern_operand_type_not_assignable"),
        vec![row(
            at(fixture, "case > 0", "0"),
            1,
            "The constant expression type 'int' is not assignable to the parameter type 'A' of the '>' operator."
        )],
        "{:?}",
        rows(&report)
    );
}

/// The covariant-`==` arm (`!=` resolves the same `operator ==`): the
/// parameter tests under `makeNullable` but the message keeps `A`.
#[test]
fn relational_equality_covariant_parameter_reports() {
    let fixture = "class A { bool operator ==(covariant A o) => true; } \
                   void f(A x) { switch (x) { case == 0: break; } }";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, "relational_pattern_operand_type_not_assignable"),
        vec![row(
            at(fixture, "case == 0", "0"),
            1,
            "The constant expression type 'int' is not assignable to the parameter type 'A' of the '==' operator."
        )],
        "{:?}",
        rows(&report)
    );
}

/// The return-type verdict anchors `.at(pattern.operator)` — the operator
/// token itself.
#[test]
fn relational_return_not_bool_reports_at_operator() {
    let fixture = "class A { int operator >(int o) => 0; } \
                   void f(A x) { if (x case > 0) {} }";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(
            &report,
            "relational_pattern_operator_return_type_not_assignable_to_bool"
        ),
        vec![row(
            at(fixture, "case > 0", ">"),
            1,
            "The return type of operators used in relational patterns must be assignable to 'bool'."
        )],
        "{:?}",
        rows(&report)
    );
}

/// A non-`bool` return from a real supertype answers the same verdict
/// (upstream's `Object` pin — the rig's `Object` is a names-only stub
/// whose assignability is undecidable, so the pin rides `num`).
#[test]
fn relational_return_supertype_reports() {
    let fixture = "class A { num operator >(int o) => 0; } \
                   void f(A x) { if (x case > 0) {} }";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(
            &report,
            "relational_pattern_operator_return_type_not_assignable_to_bool"
        ),
        vec![row(
            at(fixture, "case > 0", ">"),
            1,
            "The return type of operators used in relational patterns must be assignable to 'bool'."
        )],
        "{:?}",
        rows(&report)
    );
}

/// Negative controls: a well-typed relational pattern stays silent —
/// `int`'s own `>` takes `int` and returns `bool`, and `dynamic`'s return
/// is assignable to `bool` upstream.
#[test]
fn relational_well_typed_pattern_is_silent() {
    let fixture = "void f(int x) { switch (x) { case > 0: break; } }";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "relational_pattern_operand_type_not_assignable").is_empty()
            && rows_of(
                &report,
                "relational_pattern_operator_return_type_not_assignable_to_bool"
            )
            .is_empty(),
        "{:?}",
        rows(&report)
    );
}

#[test]
fn relational_dynamic_return_is_silent() {
    let fixture = "class A { dynamic operator >(int o) => 0; } \
                   void f(A x) { if (x case > 0) {} }";
    let report = report_ops(fixture);
    assert!(
        rows_of(
            &report,
            "relational_pattern_operator_return_type_not_assignable_to_bool"
        )
        .is_empty(),
        "{:?}",
        rows(&report)
    );
}

/// `==` over `int` resolves `Object.==` — parameter `Object`/`Object?`,
/// return `bool` — both verdicts stay off.
#[test]
fn relational_equality_default_is_silent() {
    let fixture = "void f(int x) { switch (x) { case == 0: break; } }";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "relational_pattern_operand_type_not_assignable").is_empty()
            && rows_of(
                &report,
                "relational_pattern_operator_return_type_not_assignable_to_bool"
            )
            .is_empty(),
        "{:?}",
        rows(&report)
    );
}

/// The nullable-receiver equality consult: upstream reads `Object`'s face
/// FIRST for a potentially-nullable matched type, so `A?` resolves
/// `Object.==` — never `A`'s own covariant `==` — and the operand check
/// stays off even when `int` could not inhabit `A`.
#[test]
fn relational_equality_nullable_matched_is_silent() {
    let fixture = "class A { bool operator ==(covariant A o) => true; } \
                   void f(A? x) { switch (x) { case == 0: break; } }";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "relational_pattern_operand_type_not_assignable").is_empty(),
        "{:?}",
        rows(&report)
    );
}
