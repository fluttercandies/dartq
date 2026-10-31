//! #3256 pins — the field / getter / setter face split (recon C5) and the
//! instance-member-inference edge (recon C11).
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `inheritance_override.dart:496-499` `_checkDeclaredField`: a field
//!   rides its implicit getter and — a non-final field only — its implicit
//!   setter, each `_checkDeclaredMember` run with its own element kind (the
//!   setter face swaps the diagnostic CODE to `invalid_override_setter` —
//!   same `invalid_override` registry name, different message template —
//!   and a kind mismatch skips the face outright);
//! - `instance_member_inferrer.dart` `_inferAccessorOrField`: a field with
//!   an implicit type infers from the combined superinterface signature —
//!   getters only → the combined getter type, setters only → the combined
//!   setter parameter type, both → the getter type for a `final` field and
//!   the getter type only when it equals the setter type otherwise
//!   (`dynamic` when the pair differs or a combine conflicts).
//!
//! The corpus faces these pins mirror:
//! `diagnostics/invalid_override/InvalidOverrideTest` `test_getter_overrides_
//! abstract_field_*` / `test_setter_overrides_abstract_field_*` /
//! `test_mixin_field_type_on`, and
//! `diagnostics/_misc/.../instance_member_inference_class/
//! InstanceMemberInferenceClassTest` `test_field_multiple_*` /
//! `test_field_covariant_fromSetter`.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope, and `#[path]`
//! keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::report_ops;

/// The override rows of one report, verbatim, in report order.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// Whether a row is the SETTER face's — the oracle's `invalidOverrideSetter`
/// shares the `invalid_override` code name with the getter face and differs
/// only in the message template (`The setter '…' isn't a valid override of
/// '…'.`), so the template is the face discriminator.
fn is_setter_row(message: &str) -> bool {
    message.starts_with("The setter '")
}

/// The getter-face rows only.
fn getter_rows(report: &FileReport) -> Vec<String> {
    override_rows(report)
        .into_iter()
        .filter(|m| !is_setter_row(m))
        .collect()
}

/// The setter-face rows only (`invalid_override_setter`'s template).
fn setter_rows(report: &FileReport) -> Vec<String> {
    override_rows(report)
        .into_iter()
        .filter(|m| is_setter_row(m))
        .collect()
}

// —— the two-face split over a *field* super face ————————————————

/// `test_mixin_field_type_on`: a field overriding a field satisfies BOTH
/// the getter and the setter face, so an unfit type reports twice — once
/// per face, with the setter's own code and message template.
#[test]
fn field_over_field_reports_both_faces() {
    let report = report_ops("class A { String foo = ''; } mixin M on A { int foo = 0; }");
    assert_eq!(
        getter_rows(&report),
        ["'M.foo' ('int Function()') isn't a valid override of 'A.foo' ('String Function()')."]
    );
    assert_eq!(
        setter_rows(&report),
        ["The setter 'M.foo' ('void Function(int)') isn't a valid override of \
          'A.foo' ('void Function(String)')."]
    );
}

/// `test_getter_overrides_abstract_field_invalid`: a written getter over an
/// abstract field compares against the field's implicit getter and reports
/// the getter face; the companion setter widens (contravariant parameter),
/// so the setter face stays silent.
#[test]
fn getter_over_abstract_field_reports_getter_face_only() {
    let report = report_ops(
        "abstract class A { abstract int x; } \
         abstract class B implements A { num get x; void set x(num value); }",
    );
    assert_eq!(
        getter_rows(&report),
        ["'B.x' ('num Function()') isn't a valid override of 'A.x' ('int Function()')."]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_setter_overrides_abstract_field_invalid`: the setter face narrows
/// the parameter, so it reports with the setter code while the getter face
/// (`int` over `num`) fits.
#[test]
fn setter_over_abstract_field_reports_setter_face_only() {
    let report = report_ops(
        "abstract class A { abstract num x; } \
         abstract class B implements A { int get x; void set x(int value); }",
    );
    assert!(getter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
    assert_eq!(
        setter_rows(&report),
        ["The setter 'B.x' ('void Function(int)') isn't a valid override of \
          'A.x' ('void Function(num)')."]
    );
}

/// `test_getter_overrides_abstract_field_final_invalid`: a final field has
/// no implicit setter (`field.setter == null` upstream), so the written
/// setter never faces a super setter and only the getter face reports.
#[test]
fn final_abstract_field_has_no_setter_face() {
    let report = report_ops(
        "abstract class A { abstract final int x; } \
         abstract class B implements A { num get x; void set x(num value); }",
    );
    assert_eq!(
        getter_rows(&report),
        ["'B.x' ('num Function()') isn't a valid override of 'A.x' ('int Function()')."]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// A field over a getter-only super: the getter face compares (and
/// reports), the setter face finds no candidate and stays silent.
#[test]
fn field_over_getter_only_super_checks_getter_face() {
    let report = report_ops(
        "abstract class A { String get foo; } class B implements A { int foo = 0; }",
    );
    assert_eq!(
        getter_rows(&report),
        ["'B.foo' ('int Function()') isn't a valid override of 'A.foo' ('String Function()')."]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// A field over a setter-only super: only the setter face has a candidate,
/// so a narrow field type reports the setter variant and the getter face
/// stays silent.
#[test]
fn field_over_setter_only_super_checks_setter_face() {
    let report = report_ops(
        "abstract class A { set foo(String v); } class B implements A { int foo = 0; }",
    );
    assert!(getter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
    assert_eq!(
        setter_rows(&report),
        ["The setter 'B.foo' ('void Function(int)') isn't a valid override of \
          'A.foo' ('void Function(String)')."]
    );
}

// —— the combined-signature inference (C11) ————————————————————

/// `test_field_multiple_gettersSetters_notFinal_combined_notSame`: a
/// non-final field overriding both a getter set and a setter set infers
/// the getter type only when it equals the combined setter type — here
/// `int` vs `String`, so the field stays `dynamic` and every getter face
/// reports while the `dynamic` setter parameter still accepts every
/// inherited setter.
#[test]
fn inferred_field_both_faces_differ_is_dynamic() {
    let report = report_ops(
        "class A { num get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C { set foo(String _) {} } \
         class X implements A, B, C { var foo; }",
    );
    assert_eq!(
        getter_rows(&report),
        [
            "'X.foo' ('dynamic Function()') isn't a valid override of 'A.foo' ('num Function()').",
            "'X.foo' ('dynamic Function()') isn't a valid override of 'B.foo' ('int Function()').",
        ]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_gettersSetters_notFinal_combined_same`: the same
/// shape with an `int` setter — the combined getter type (`int`) equals the
/// combined setter type, so the field infers `int` and every face fits.
#[test]
fn inferred_field_both_faces_equal_infers_getter_type() {
    let report = report_ops(
        "class A { num get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C { set foo(int _) {} } \
         class X implements A, B, C { var foo; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_getters_combined`: getters-only infers the combined
/// getter type — the most specific of `num`/`int`, i.e. `int`.
#[test]
fn inferred_field_getters_only_merges_most_specific() {
    let report = report_ops(
        "class A { num get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C implements A, B { var foo; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_getters_incompatible`: a getter set with no common
/// subtype conflicts (`combineSignatureTypes` → null), so the field stays
/// `dynamic` and both getter faces report.
#[test]
fn inferred_field_getters_incompatible_is_dynamic() {
    let report = report_ops(
        "class A { String get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C implements A, B { var foo; }",
    );
    assert_eq!(
        getter_rows(&report),
        [
            "'C.foo' ('dynamic Function()') isn't a valid override of \
             'A.foo' ('String Function()').",
            "'C.foo' ('dynamic Function()') isn't a valid override of 'B.foo' ('int Function()').",
        ]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_setters_combined`: setters-only infers the combined
/// setter parameter type — the common SUPERTYPE of `num`/`int`, i.e. `num`
/// (a setter parameter widens where a getter result narrows).
#[test]
fn inferred_field_setters_only_merges_common_supertype() {
    let report = report_ops(
        "class A { set foo(num _) {} } \
         class B { set foo(int _) {} } \
         class C implements A, B { var foo; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_setters_incompatible`: an unrelated setter
/// parameter set conflicts, so the field stays `dynamic` — which still
/// accepts every inherited setter parameter.
#[test]
fn inferred_field_setters_incompatible_is_dynamic() {
    let report = report_ops(
        "class A { set foo(String _) {} } \
         class B { set foo(int _) {} } \
         class C implements A, B { var foo; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_fields_incompatible`: two conflicting fields pair
/// into conflicting getter and setter candidate sets, so both combine arms
/// answer `dynamic` and the getter faces report.
#[test]
fn inferred_field_over_conflicting_fields_is_dynamic() {
    let report = report_ops(
        "class A { int foo = 0; } \
         class B { String foo = ''; } \
         class C implements A, B { var foo; }",
    );
    assert_eq!(
        getter_rows(&report),
        [
            "'C.foo' ('dynamic Function()') isn't a valid override of 'A.foo' ('int Function()').",
            "'C.foo' ('dynamic Function()') isn't a valid override of \
             'B.foo' ('String Function()').",
        ]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_gettersSetters_final_combined`: a FINAL field over
/// both a getter and a setter set infers the combined getter type and never
/// the setter type (and has no setter face at all).
#[test]
fn inferred_final_field_ignores_setter_candidates() {
    let report = report_ops(
        "class A { num get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C { set foo(String _) {} } \
         class X implements A, B, C { final foo; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// `test_field_multiple_gettersSetters_final_incompatible`: a final field
/// whose getter candidate set conflicts infers `dynamic` and reports both
/// getter faces; the setter set never contributes (no setter face).
#[test]
fn inferred_final_field_conflicting_getters_is_dynamic() {
    let report = report_ops(
        "class A { String get foo => throw 0; } \
         class B { int get foo => throw 0; } \
         class C { set foo(String _) {} } \
         class X implements A, B, C { final foo; }",
    );
    assert_eq!(
        getter_rows(&report),
        [
            "'X.foo' ('dynamic Function()') isn't a valid override of \
             'A.foo' ('String Function()').",
            "'X.foo' ('dynamic Function()') isn't a valid override of 'B.foo' ('int Function()').",
        ]
    );
    assert!(setter_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

// —— the covariance propagation into a field's induced setter ————

/// `test_field_covariant_fromSetter`: the overridden setter's covariant
/// parameter propagates onto the overriding field's induced setter
/// (`_inferAccessorOrField` marks `field.setter.valueFormalParameter`
/// BEFORE the implicit-type gate), so the narrowed field type is the
/// runtime-checked covariant narrowing, not an override break.
#[test]
fn field_covariant_from_super_setter_is_valid() {
    let report = report_ops("class A { set foo(covariant num _) {} } class B implements A { int foo = 0; }");
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}

/// The same propagation one hop further out: the mark rides the chain, so a
/// grandchild field narrowing again still fits.
#[test]
fn field_covariant_propagates_through_a_field_hop() {
    let report = report_ops(
        "class A { set foo(covariant num _) {} } \
         class B implements A { int foo = 0; } \
         class C implements B { int foo = 0; }",
    );
    assert!(override_rows(&report).is_empty(), "{:#?}", override_rows(&report));
}
