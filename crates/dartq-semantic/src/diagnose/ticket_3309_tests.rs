//! #3309 — the extension-type declaration faces this module owns: the
//! per-member `extension_type_declares_member_of_object` scan and the
//! representation dependency cycle.
//!
//! Oracle anchors (3.13.7,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart`):
//! `_checkForExtensionDeclaresMemberOfObject` reports at `node.name` for
//! every METHOD-shaped member — getter, setter, method or operator, static or
//! instance — whose name is an `Object` member (:1524, :4779-4796); a field
//! takes the instance-field row instead (:4807-4820). The representation
//! formal's own name is checked separately (:4955-4966). The cycle face reads
//! the linker's `hasRepresentationSelfReference`
//! (`src/summary2/extension_type.dart:97-135`), reported at the type name
//! (:4893-4900).

use crate::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The fixture offset of `needle`, shifted past the prelude `report_ops`
/// prepends.
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).expect("anchor present")) as u32
}

/// An instance getter shadowing an `Object` member reports at its name.
#[test]
fn instance_getter_shadowing_an_object_member_reports() {
    let fixture = "extension type E(int it) {\n  int get hashCode => 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_declares_member_of_object"),
        vec![at(fixture, "hashCode")],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// A static method shadowing an `Object` member reports too — the oracle's
/// pass runs for static and instance members alike (:1524).
#[test]
fn static_method_shadowing_an_object_member_reports() {
    let fixture = "extension type E(int it) {\n  static int hashCode() => 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_declares_member_of_object"),
        vec![at(fixture, "hashCode")]
    );
}

/// A setter's model key carries the `=` suffix (`hashCode=`); the oracle
/// compares the name token's plain lexeme (:4791) — the anchor is the name,
/// without the `=`.
#[test]
fn static_setter_shadowing_an_object_member_reports_at_the_name() {
    let fixture = "extension type E(int it) {\n  static set hashCode(int _) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_declares_member_of_object"),
        vec![at(fixture, "hashCode")]
    );
}

/// An operator keeps its operator spelling as its name, and `==` is an
/// `Object` member.
#[test]
fn operator_equality_reports_at_the_operator() {
    let fixture = "extension type E(int it) {\n  bool operator ==(Object other) => false;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_declares_member_of_object"),
        vec![at(fixture, "==")]
    );
}

/// The representation formal's own name is the other arm of the same code
/// (:4955-4966): one row, at the formal.
#[test]
fn representation_formal_shadowing_an_object_member_reports() {
    let fixture = "extension type E(Object? hashCode) {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_declares_member_of_object"),
        vec![at(fixture, "hashCode")]
    );
}

/// A FIELD named after an `Object` member is not this face: the oracle's
/// `FieldDeclaration` visit reports `extension_type_declares_instance_field`
/// (:4807-4820) and never the member-of-`Object` row.
#[test]
fn a_field_named_after_an_object_member_is_not_this_face() {
    let fixture = "extension type E(int it) {\n  int hashCode = 0;\n}\n";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "extension_type_declares_member_of_object").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// An ordinary member name is silent — the scan keys on the `Object` member
/// set, not on "any member exists".
#[test]
fn an_ordinary_member_name_is_silent() {
    let fixture = "extension type E(int it) {\n  int get value => 0;\n  static void run() {}\n}\n";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "extension_type_declares_member_of_object").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The direct self-reference: the representation type IS the extension type.
#[test]
fn direct_representation_self_reference_reports_at_the_type_name() {
    let fixture = "extension type A(A it) {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_representation_depends_on_itself"),
        vec![at(fixture, "A(")],
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// A reference through a type argument is the same self-reference
/// (the oracle's dependency collector walks type arguments,
/// `extension_type.dart:41-57`).
#[test]
fn type_argument_self_reference_reports() {
    let fixture = "extension type A(List<A> it) {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "extension_type_representation_depends_on_itself"),
        vec![at(fixture, "A(")]
    );
}

/// Two declarations whose representation types point at each other are one
/// cycle: the linker marks EVERY node of the cycle, so both report.
#[test]
fn a_two_declaration_cycle_reports_on_both_members() {
    let fixture = "extension type A(B it) {}\nextension type B(A it) {}\n";
    let report = report_ops(fixture);
    let mut expect = vec![at(fixture, "A("), at(fixture, "B(")];
    expect.sort_unstable();
    assert_eq!(
        offsets(&report, "extension_type_representation_depends_on_itself"),
        expect
    );
}

/// A chain that does not come back to its start is no cycle — the oracle
/// reports nothing for `A(B it)` over `extension type B(int it)`.
#[test]
fn an_acyclic_representation_chain_is_silent() {
    let fixture = "extension type A(B it) {}\nextension type B(int it) {}\n";
    let report = report_ops(fixture);
    assert!(
        offsets(&report, "extension_type_representation_depends_on_itself").is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// An ordinary representation type stays silent.
#[test]
fn an_ordinary_representation_type_is_silent() {
    let report = report_ops("extension type A(int it) {}\n");
    assert!(
        offsets(&report, "extension_type_representation_depends_on_itself").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
