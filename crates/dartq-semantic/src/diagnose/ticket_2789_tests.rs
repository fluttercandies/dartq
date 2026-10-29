//! The #2789 pins: the types-tier pair against the wave-3 conformance
//! clusters. The bounds face follows the official
//! `TypeArgumentsVerifier` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart):
//! `checkNamedType` runs at every resolved type annotation, so a
//! supertype/constraint clause (`extends`/`with`/`implements`/`on` and
//! the class-type-alias `= Super<Mixins>` head) checks its written
//! arguments against the named head's substituted bounds (:342-465), the
//! super-bounded pass is denied on every clause context
//! (`_shouldAllowSuperBoundedTypes`, :737-748 — a regular violation
//! reports with no inverted-type context row), and the offending
//! argument's own node anchors the report (`_typeArgumentErrorNode`,
//! :757-768). The assignment faces follow
//! pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart:
//! `resolveIfNull` closes with the same
//! `_checkForInvalidAssignment(writeAcceptedType, node.value, valueType)`
//! the plain face uses (:697-703), reported at the right operand
//! (:783-800), and the checked value type is the flow-PROMOTED static
//! type — an untyped parameter read under `y is String` writes `String`,
//! not `dynamic`.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn bounds_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn assignment_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The `extends G<B>` clause cluster
/// (TypeArgumentNotMatchingBoundsTest/test_extends): the violation
/// reports at the offending argument with the parameter's written bound.
#[test]
fn extends_clause_bad_argument_reports_at_the_argument() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
class C extends G<B> {}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B> {}"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The class-type-alias head (test_classTypeAlias): `class D = G<B> with
/// C;` checks the `=` head's arguments — the `ClassTypeAlias` context
/// denies the super pass, so the report carries no context row.
#[test]
fn class_type_alias_head_checks_its_arguments() {
    let source = r#"
class A {}
class B {}
mixin C {}
class G<E extends A> {}
class D = G<B> with C;
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B> with"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The `on` clause is a constraint annotation, not a supertype edge —
/// but `checkNamedType` still bounds-checks it
/// (`MixinOnClause` in the deny list).
#[test]
fn on_clause_bad_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
mixin M on G<B> {}
"#;
    let report = report_ops(source);
    assert_eq!(bounds_rows(&report).len(), 1, "{:?}", report.diagnostics);
}

/// The `implements` face
/// (TypeArgumentNotMatchingBoundsTest/test_classTypeAlias_implements
/// family): the clause context denies the super pass.
#[test]
fn implements_clause_bad_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
class C implements G<B> {}
"#;
    let report = report_ops(source);
    assert_eq!(bounds_rows(&report).len(), 1, "{:?}", report.diagnostics);
}

/// A satisfied clause argument stays silent; an in-scope BOUNDED type
/// parameter as the argument suppresses (the official subtype test
/// succeeds through the parameter's own bound, which the M1 face cannot
/// answer — the under-report posture).
#[test]
fn satisfied_and_in_scope_bounded_clause_arguments_stay_silent() {
    let satisfied = report_ops(
        r#"
class A {}
class G<E extends A> {}
class C extends G<A> {}
"#,
    );
    assert!(
        bounds_rows(&satisfied).is_empty(),
        "{:?}",
        satisfied.diagnostics
    );
    let in_scope = report_ops(
        r#"
class A {}
class G<E extends A> {}
class C<T extends A> extends G<T> {}
"#,
    );
    assert!(
        bounds_rows(&in_scope).is_empty(),
        "{:?}",
        in_scope.diagnostics
    );
}

/// The `??=` face (assignment_static_test_05 family): the plain
/// assignability question — the RHS against the target's write type —
/// reported at the right operand, with the target's declared (nullable)
/// type in the message.
#[test]
fn if_null_assignment_checks_the_rhs_against_the_write_type() {
    let source = r#"
class A {}
class B {}
void f(B? b, A a) {
  b ??= a;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        assignment_rows(&report),
        [(
            at(source, "a;"),
            1,
            "A value of type 'A' can't be assigned to a variable of type 'B?'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The control: a conforming `??=` value stays silent.
#[test]
fn if_null_assignment_with_conforming_rhs_stays_silent() {
    let report = report_ops(
        r#"
class B {}
void f(B? b, B c) {
  b ??= c;
}
"#,
    );
    assert!(
        assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The promoted-RHS-read cluster
/// (InvalidAssignmentTest/test_localVariable_promotion): an untyped
/// parameter promoted by `y is String` writes `String` into `int` — the
/// oracle checks the promoted value type, not the declared `dynamic`.
#[test]
fn promoted_untyped_rhs_reports_at_the_read() {
    let source = r#"
void f(y) {
  if (y is String) {
    int x = y;
    print(x);
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        assignment_rows(&report),
        [(
            at(source, "y;"),
            1,
            "A value of type 'String' can't be assigned to a variable of type 'int'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The member-write twin (test_instanceVariable): `a.x = y` reads the
/// promoted `y` against the field's declared type.
#[test]
fn promoted_rhs_reports_through_the_member_write() {
    let source = r#"
class A {
  int x = 7;
}
void f(y) {
  A a = A();
  if (y is String) {
    a.x = y;
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        assignment_rows(&report),
        [(
            at(source, "y;"),
            1,
            "A value of type 'String' can't be assigned to a variable of type 'int'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The control: the same untyped parameter WITHOUT the type test stays
/// silent (`dynamic` is assignable to everything).
#[test]
fn unpromoted_dynamic_rhs_stays_silent() {
    let report = report_ops(
        r#"
void f(y) {
  int x = y;
  print(x);
}
"#,
    );
    assert!(
        assignment_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}
