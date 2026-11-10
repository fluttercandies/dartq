//! The #3411 pins — the `@nonVirtual` override face and the setter
//! function-type arity the override verdict reads.
//!
//! * `invalid_override_of_non_virtual_member` (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/error/best_practices_verifier.dart:405-451` /
//!   `:595-648` over `InterfaceElement.getInheritedConcreteMember`,
//!   `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart:159-174`).
//!   The fixtures restate the corpus shapes
//!   (`pkg/analyzer/test/src/diagnostics/invalid_override_of_non_virtual_member_test.dart`)
//!   with a locally declared `nonVirtual` constant standing in for
//!   `package:meta` — the recognition is name-level, the same posture the
//!   `@mustCallSuper` / `@mustBeOverridden` faces keep.
//! * The setter arm: a setter's function type always carries exactly one
//!   parameter, so a malformed `set foo()` never fabricates an
//!   `invalid_override` against a well-formed override — while genuine
//!   setter contravariance still reports
//!   (`pkg/analyzer/test/src/dart/resolution/instance_member_inference_class_test.dart:1063`).

use super::FileReport;
use super::test_rig::report_ops;

/// The locally declared annotation stand-in every fixture below carries
/// (one line, so a fixture's own shape stays readable).
const NON_VIRTUAL: &str = "class nonVirtual { const nonVirtual(); } ";

/// `"code: message"` per diagnostic, in emission order.
fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// The `invalid_override_of_non_virtual_member` rows alone — the fixtures
/// that exercise this face may carry unrelated rows (an uninitialized
/// field, a setter-arity error), and the pin is the face's own output.
fn non_virtual_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override_of_non_virtual_member")
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// A method overriding a `@nonVirtual` superclass method — the corpus's
/// `test_class_method` shape. One row, anchored at the overriding name,
/// naming the DECLARING class.
#[test]
fn method_override_of_a_non_virtual_super_member_reports() {
    let fixture = format!(
        "{NON_VIRTUAL}class A {{ @nonVirtual void f() {{}} }} \
         class B extends A {{ void f() {{}} }}"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        non_virtual_rows(&report),
        vec![
            "invalid_override_of_non_virtual_member: The member 'f' is declared non-virtual in \
             'A' and can't be overridden in subclasses."
        ],
        "{:?}",
        rows(&report)
    );
}

/// The annotation is the gate: the same override over an UNANNOTATED super
/// member stays silent (the negative half of the pin above).
#[test]
fn method_override_of_a_plain_super_member_stays_silent() {
    let fixture =
        format!("{NON_VIRTUAL}class A {{ void f() {{}} }} class B extends A {{ void f() {{}} }}");
    let report = report_ops(&fixture);
    assert!(non_virtual_rows(&report).is_empty(), "{:?}", rows(&report));
}

/// `implements` is not inheritance for this face: the oracle reads the
/// SUPERCLASS-AND-MIXINS concrete map, so an interface member never
/// reports (the corpus's `test_class_implements_getter` / `_method`
/// negatives).
#[test]
fn an_interface_member_never_reports() {
    let fixture = format!(
        "{NON_VIRTUAL}abstract class A {{ @nonVirtual void f(); }} \
         class B implements A {{ void f() {{}} }}"
    );
    let report = report_ops(&fixture);
    assert!(non_virtual_rows(&report).is_empty(), "{:?}", rows(&report));
}

/// A mixin equal member: the class's `with` face is an inherited concrete
/// source, and the row names the MIXIN (the corpus's `test_mixin_method`).
#[test]
fn a_mixin_member_reports_and_names_the_mixin() {
    let fixture = format!(
        "{NON_VIRTUAL}mixin M {{ @nonVirtual void f() {{}} }} \
         class B with M {{ void f() {{}} }}"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        non_virtual_rows(&report),
        vec![
            "invalid_override_of_non_virtual_member: The member 'f' is declared non-virtual in \
             'M' and can't be overridden in subclasses."
        ],
        "{:?}",
        rows(&report)
    );
}

/// A FIELD whose getter name answers the inherited member — the corpus's
/// `test_class_getter_overriddenByField` shape, where the field's first
/// lookup (the getter name) already hits.
#[test]
fn a_field_over_a_non_virtual_getter_reports() {
    let fixture = format!(
        "{NON_VIRTUAL}class A {{ @nonVirtual int get g => 0; }} \
         class B extends A {{ int g = 0; }}"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        non_virtual_rows(&report),
        vec![
            "invalid_override_of_non_virtual_member: The member 'g' is declared non-virtual in \
             'A' and can't be overridden in subclasses."
        ],
        "{:?}",
        rows(&report)
    );
}

/// A FIELD whose getter lookup misses and whose setter lookup answers — the
/// corpus's `test_class_setter_overriddenByField` shape (the oracle's `??`
/// fallback from the getter name to the setter name).
#[test]
fn a_field_over_a_non_virtual_setter_reports_through_the_setter_fallback() {
    let fixture = format!(
        "{NON_VIRTUAL}class A {{ @nonVirtual set g(int v) {{}} }} \
         class B extends A {{ int g = 0; }}"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        non_virtual_rows(&report),
        vec![
            "invalid_override_of_non_virtual_member: The member 'g' is declared non-virtual in \
             'A' and can't be overridden in subclasses."
        ],
        "{:?}",
        rows(&report)
    );
}

/// A setter overriding a `@nonVirtual` super setter — the corpus's
/// `test_class_setter` shape, where the model spelling carries the `=`
/// marker and the report still renders the bare name.
#[test]
fn a_setter_override_reports_with_the_bare_display_name() {
    let fixture = format!(
        "{NON_VIRTUAL}class A {{ @nonVirtual set g(int v) {{}} }} \
         class B extends A {{ set g(int v) {{}} }}"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        non_virtual_rows(&report),
        vec![
            "invalid_override_of_non_virtual_member: The member 'g' is declared non-virtual in \
             'A' and can't be overridden in subclasses."
        ],
        "{:?}",
        rows(&report)
    );
}

/// A malformed super setter still never breaks a well-formed override: the
/// setter's function type holds one `dynamic` parameter whatever the
/// written list says, so `class A { set foo() {} }` overridden by
/// `set foo(x) {}` reports the ARITY error on the declaration and no
/// `invalid_override` at all (the corpus's
/// `test_setter_single_setter_withoutParameter`).
#[test]
fn a_malformed_super_setter_never_breaks_the_override() {
    let fixture = "class A { set foo() {} } class B implements A { set foo(x) {} }";
    let report = report_ops(fixture);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "invalid_override"),
        "{:?}",
        rows(&report)
    );
}

/// The normalization above must not swallow genuine setter contravariance:
/// a setter narrowing its super setter's parameter still reports.
#[test]
fn a_narrowing_setter_still_reports() {
    let fixture = "class A { set foo(num v) {} } class B implements A { set foo(int v) {} }";
    let report = report_ops(fixture);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "invalid_override"),
        "{:?}",
        rows(&report)
    );
}
