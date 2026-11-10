//! The #3251 pins: the `void` T3 emit sites — the constructor field
//! initializer's void/assignability ordering, the formal parameter's
//! default-value assignability face, and the for-part declaration
//! initializer.
//!
//! Upstream `checkForFieldInitializerNotAssignable` asks assignability
//! FIRST and only then the void use (`if (fieldType is! VoidType)
//! checkForUseOfVoidResult(expression)` — error_detection_helpers.dart), so
//! an object-typed field answers `field_initializer_not_assignable` while
//! the hosts that accept `void` (`dynamic`, `Object?`) answer
//! `use_of_void_result`; `_checkForDefaultValueAssignableAtType`
//! (error_verifier.dart) runs `checkForAssignableExpressionAtType` over the
//! formal's declared type with the assignment reporter; and a `for`-part
//! declaration initializer is an ordinary declaration initializer
//! (`variable_declaration_resolver.dart`).

use crate::diagnose::test_rig::{report_ops, rows};

/// The message the oracle spells for the void face — parameterless
/// (messages.yaml:19638 `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// The `(code, offset)` rows of one fixture, offsets relative to the
/// fixture's own text.
fn rows_at(report: &super::FileReport, prelude: usize) -> Vec<(&'static str, usize)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                (d.range.offset as usize).saturating_sub(prelude),
            )
        })
        .collect()
}

/// A default value the formal's declared type rejects answers
/// `invalid_assignment` at the value expression — the face that had no
/// caller before this ticket.
#[test]
fn ticket_3251_default_value_assignability() {
    let fixture = "void f({int x = 's'}) {}";
    let report = report_ops(fixture);
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let at = fixture.find("'s'").unwrap();
    assert!(
        rows_at(&report, prelude)
            .iter()
            .any(|(code, offset)| *code == "invalid_assignment" && *offset == at),
        "{:?}",
        rows(&report)
    );
}

/// An assignable default stays silent — the caller must not fire on the
/// ordinary shape.
#[test]
fn ticket_3251_default_value_assignable_stays_silent() {
    let fixture = "void f({int x = 1}) {}";
    let report = report_ops(fixture);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "invalid_assignment"),
        "{:?}",
        rows(&report)
    );
}

/// A `void` default value reports the void use, not `invalid_assignment`
/// (upstream `checkForAssignableExpressionAtType`'s void short-circuit).
#[test]
fn ticket_3251_default_value_void_use() {
    let fixture = "void v() {}\nvoid f({int x = v()}) {}";
    let report = report_ops(fixture);
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let at = fixture.rfind("v()").unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message == VOID_MSG
                && (d.range.offset as usize).saturating_sub(prelude) == at),
        "{:?}",
        rows(&report)
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "invalid_assignment"),
        "{:?}",
        rows(&report)
    );
}

/// A constructor field initializer against a host that ACCEPTS `void`
/// reports the void use at the initializer.
#[test]
fn ticket_3251_ctor_field_init_void_accepted() {
    let fixture = "void v() {}\nclass F {\n  dynamic x;\n  F() : x = v();\n}";
    let report = report_ops(fixture);
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let at = fixture.rfind("v();").unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message == VOID_MSG
                && (d.range.offset as usize).saturating_sub(prelude) == at),
        "{:?}",
        rows(&report)
    );
}

/// A constructor field initializer against a NON-object host answers
/// `field_initializer_not_assignable` — assignability decides before the
/// void face.
#[test]
fn ticket_3251_ctor_field_init_not_assignable() {
    let fixture = "void v() {}\nclass F {\n  Object x;\n  F() : x = v();\n}";
    let report = report_ops(fixture);
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let at = fixture.rfind("v();").unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "field_initializer_not_assignable"
                && (d.range.offset as usize).saturating_sub(prelude) == at),
        "{:?}",
        rows(&report)
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "use_of_void_result"),
        "{:?}",
        rows(&report)
    );
}

/// A `for`-part declaration initializer is an ordinary declaration
/// initializer: the `void` value reports at the initializer.
#[test]
fn ticket_3251_for_part_decl_initializer_void() {
    let fixture = "void v() {}\nvoid main() {\n  for (int i = v(); false;) {}\n}";
    let report = report_ops(fixture);
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len();
    let at = fixture.rfind("v()").unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message == VOID_MSG
                && (d.range.offset as usize).saturating_sub(prelude) == at),
        "{:?}",
        rows(&report)
    );
}
