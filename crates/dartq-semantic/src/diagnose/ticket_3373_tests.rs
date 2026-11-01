//! The #3373 pins: the enum member-set and mixin-application faces.
//!
//! Each pin pairs an oracle-anchored repro with its negative. The offsets
//! are the fixture's own coordinates (the rig's [`CORE_PRELUDE`] shift
//! subtracted), and the messages pin the argument wiring of the shared
//! registry codes (`illegal_concrete_enum_member` and `illegal_enum_values`
//! each carry two upstream unique names whose texts differ).
//!
//! Upstream anchors (dart 3.13.7, the pinned oracle):
//! * `illegal_concrete_enum_member` declaration — the member name token
//!   (`.at(name)`, inheritance_override.dart:785-800);
//! * `illegal_concrete_enum_member` inheritance — the class name token
//!   (`.at(classNameToken)`, :803-840);
//! * `illegal_enum_values` declaration — the member name token (:842-846);
//! * `illegal_enum_values` inheritance — the class name token (:848-866);
//! * `enum_with_abstract_member` — the WHOLE member declaration
//!   (`.at(member)`, :912-946);
//! * `values_declaration_in_enum` — the member name token
//!   (member_duplicate_definition_verifier.dart:585-589);
//! * `mixin_application_not_implemented_interface` — the `with` entry's
//!   name token (`.at(mixinName.name)`, error_verifier.dart:6076-6116).

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// The `code` rows of `report` as `(offset, length)` in the fixture's own
/// coordinates.
fn spans(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset - shift, row.range.length))
        .collect()
}

/// The `code` rows of `report`, as messages in row order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| row.message.to_string())
        .collect()
}

/// The UTF-16 offset of `needle`'s first occurrence in `source`.
fn offset_of(source: &str, needle: &str) -> u32 {
    let at = source.find(needle).expect("needle present in fixture");
    u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32")
}

/// The span `needle` occupies in `source`.
fn span_of(source: &str, needle: &str) -> (u32, u32) {
    let length = u32::try_from(needle.encode_utf16().count()).expect("needle fits u32");
    (offset_of(source, needle), length)
}

/// The span of `needle`'s `nth` occurrence (0-based) in `source`.
fn nth_span_of(source: &str, needle: &str, nth: usize) -> (u32, u32) {
    let mut from = 0usize;
    let mut at = 0usize;
    for _ in 0..=nth {
        at = source[from..].find(needle).expect("needle present") + from;
        from = at + needle.len();
    }
    let offset = u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32");
    let length = u32::try_from(needle.encode_utf16().count()).expect("needle fits u32");
    (offset, length)
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .count()
}

// —— illegal_concrete_enum_member: declaration ——

/// An enum's concrete `==` reports at the operator name token.
#[test]
fn illegal_concrete_enum_member_declaration_anchors_at_the_name() {
    let source = "enum E {\n  v;\n  bool operator ==(Object other) => false;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "illegal_concrete_enum_member"),
        vec![span_of(source, "==")]
    );
    assert_eq!(
        messages(&report, "illegal_concrete_enum_member"),
        vec![
            "A concrete instance member named '==' can't be declared in a class that \
             implements 'Enum'."
                .to_string()
        ]
    );
}

/// A bodyless accessor is abstract, so the declaration face stays silent —
/// the same member with a body reports.
#[test]
fn illegal_concrete_enum_member_declaration_skips_abstract_members() {
    let bodyless = "abstract class A implements Enum {\n  int get hashCode;\n}\n";
    assert_eq!(count(&report_ops(bodyless), "illegal_concrete_enum_member"), 0);

    let concrete = "abstract class A implements Enum {\n  int get hashCode => 0;\n}\n";
    let report = report_ops(concrete);
    assert_eq!(
        spans(&report, "illegal_concrete_enum_member"),
        vec![span_of(concrete, "hashCode")]
    );
}

// —— illegal_concrete_enum_member: inheritance ——

/// A concrete `hashCode` inherited through `extends` reports at the class
/// name, naming the declaring class.
#[test]
fn illegal_concrete_enum_member_inheritance_anchors_at_the_class_name() {
    let source = "class A {\n  int hashCode = 0;\n}\n\n\
                  abstract class B extends A implements Enum {}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "illegal_concrete_enum_member"),
        vec![span_of(source, "B")]
    );    assert_eq!(
        messages(&report, "illegal_concrete_enum_member"),
        vec![
            "A concrete instance member named 'hashCode' can't be inherited from 'A' in a \
             class that implements 'Enum'."
                .to_string()
        ]
    );
}

/// An `implements` face is not inherited, so the same member contributes
/// nothing through `implements A, Enum`.
#[test]
fn illegal_concrete_enum_member_inheritance_ignores_implements() {
    let source = "class A {\n  int hashCode = 0;\n}\n\n\
                  abstract class B implements A, Enum {}\n";
    assert_eq!(count(&report_ops(source), "illegal_concrete_enum_member"), 0);
}

// —— illegal_enum_values ——

/// A class implementing `Enum` may not declare an instance `values`.
#[test]
fn illegal_enum_values_declaration_anchors_at_the_member() {
    let source = "abstract class A implements Enum {\n  int values = 0;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "illegal_enum_values"),
        vec![span_of(source, "values")]
    );
    assert_eq!(
        messages(&report, "illegal_enum_values"),
        vec![
            "An instance member named 'values' can't be declared in a class that \
             implements 'Enum'."
                .to_string()
        ]
    );
}

/// A static `values` is not an instance member — silent.
#[test]
fn illegal_enum_values_declaration_skips_static_members() {
    let source = "abstract class A implements Enum {\n  static int values = 0;\n}\n";
    assert_eq!(count(&report_ops(source), "illegal_enum_values"), 0);
}

/// The inherited `values` reads the whole interface (`implements`
/// included) and reports at the class name.
#[test]
fn illegal_enum_values_inheritance_anchors_at_the_class_name() {
    let source = "class A {\n  int values = 0;\n}\n\n\
                  abstract class B implements A, Enum {}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "illegal_enum_values"),
        vec![span_of(source, "B")]
    );
    assert_eq!(
        messages(&report, "illegal_enum_values"),
        vec![
            "An instance member named 'values' can't be inherited from 'A' in a class that \
             implements 'Enum'."
                .to_string()
        ]
    );
}

// —— enum_with_abstract_member ——

/// The bodyless enum member reports over the WHOLE declaration (`.at(member)`
/// includes the return type and the terminating `;`).
#[test]
fn enum_with_abstract_member_spans_the_whole_declaration() {
    let source = "enum E {\n  v;\n  void foo();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "enum_with_abstract_member"),
        vec![span_of(source, "void foo();")]
    );
    assert_eq!(
        messages(&report, "enum_with_abstract_member"),
        vec!["'foo' must have a method body because 'E' is an enum.".to_string()]
    );
}

/// An enum whose only member is a constant has nothing abstract — silent.
#[test]
fn enum_with_abstract_member_skips_complete_enums() {
    let source = "enum E {\n  v;\n}\n";
    assert_eq!(count(&report_ops(source), "enum_with_abstract_member"), 0);
}

// —— values_declaration_in_enum ——

/// An enum body may not declare a member called `values`, static or not.
#[test]
fn values_declaration_in_enum_anchors_at_the_member() {
    let source = "enum E {\n  v;\n  final int values = 0;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "values_declaration_in_enum"),
        vec![span_of(source, "values")]
    );
    assert_eq!(
        messages(&report, "values_declaration_in_enum"),
        vec!["A member named 'values' can't be declared in an enum.".to_string()]
    );
}

/// An enum constant called `values` is a member too (the field arm).
#[test]
fn values_declaration_in_enum_reports_the_constant() {
    let source = "enum E {\n  values\n}\n";
    assert_eq!(
        spans(&report_ops(source), "values_declaration_in_enum"),
        vec![span_of(source, "values")]
    );
}

// —— mixin_application_not_implemented_interface ——

/// A `with` entry whose `on` constraint the supertype does not satisfy
/// reports at the mixin name token.
#[test]
fn mixin_application_constraint_anchors_at_the_mixin_name() {
    let source = "class A {}\n\nmixin M on A {}\n\nclass C extends Object with M {}\n";
    let report = report_ops(source);
    assert_eq!(
        spans(&report, "mixin_application_not_implemented_interface"),
        vec![nth_span_of(source, "M", 1)]
    );
    assert_eq!(
        messages(&report, "mixin_application_not_implemented_interface"),
        vec![
            "'M' can't be mixed onto 'Object' because 'Object' doesn't implement 'A'."
                .to_string()
        ]
    );
}

/// A satisfied constraint (the supertype itself) is silent.
#[test]
fn mixin_application_constraint_accepts_a_matching_supertype() {
    let source = "class A {}\n\nmixin M on A {}\n\nclass C extends A with M {}\n";
    assert_eq!(
        count(&report_ops(source), "mixin_application_not_implemented_interface"),
        0
    );
}

/// An earlier mixin satisfying the constraint keeps the application legal
/// (the `previousMixin` arm of the official loop).
#[test]
fn mixin_application_constraint_accepts_an_earlier_mixin() {
    let source = "class A {}\n\nmixin M on A {}\n\n\
                  class C extends Object with A, M {}\n";
    assert_eq!(
        count(&report_ops(source), "mixin_application_not_implemented_interface"),
        0
    );
}
