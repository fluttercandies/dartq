//! Ticket #3380 pins — a wildcard type parameter declares nothing in
//! every generic-function-body plane: `void f<_>()`, a static or instance
//! method's own `<_>`, a local function's, and a function literal's
//! `<_>` all skip the `_` element exactly like `TypeParameterScope`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/scope.dart:944-960), so a read of
//! `_` inside the body falls through the lexical chain to
//! `undefined_identifier`.
//!
//! # The defect this ticket fixes
//!
//! The existing gate (`Engine::push_type_parameter`'s `declares_nothing`
//! skip, ticket_3297's pins) covers the declaration-site `type_params` /
//! `pending_type_params` stack, but a generic declaration's `_` ALSO
//! enters the per-executable bound window (`exec_bounds` /
//! `method_exec_bounds` — the `(param, bound)` pairs
//! `param_bound`/`param_bound_of` consult) and the written-signature
//! capture's scope (`fn_type_capture`'s `scope`/`params`, which
//! `type_resolution`'s capture arm answers). A body read of `_` found it
//! declared there and resolved instead of falling through.
//!
//! The oracle emits `Undefined name '_'.` per read
//! (`co19/LanguageFeatures/Wildcards/binding_A01_t08`, 13 rows; `t09`,
//! 5 rows — `dart analyze` 3.13.7 machine format).
//!
//! The fixture declares `print` so each `_` read resolves on its own —
//! an unresolvable callee would fold the argument row into the callee's
//! miss and hide the binding face under test.

use super::FileReport;
use super::test_rig::{assert_silent, code_names, report_ops, report_ops_wildcards};

/// Every `undefined_identifier` message of `report`, in emission order.
fn identifier_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .map(|d| d.message.to_string())
        .collect()
}

/// Assert the report carries exactly `expected` undefined-name rows.
#[track_caller]
fn assert_identifier_rows(report: &FileReport, expected: &[&str]) {
    assert_eq!(
        identifier_rows(report),
        expected
            .iter()
            .map(|row| (*row).to_owned())
            .collect::<Vec<_>>(),
        "{:?}",
        code_names(report)
    );
}

// — `void f<_>() { print(_); }` across every function-body plane (t08) ——

/// A top-level generic function's wildcard parameter declares nothing:
/// the body read is undefined (`binding_A01_t08` `topLevelFunction`).
#[test]
fn wildcard_type_param_in_top_level_function_declares_nothing() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         void topLevelFunction<_>() {\n\
           print(_);\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// A static method's own `<_>` declares nothing (`binding_A01_t08`
/// `C.staticMethod`).
#[test]
fn wildcard_type_param_in_static_method_declares_nothing() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         class C {\n\
           static void staticMethod<_>() {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// An instance method's own `<_>` declares nothing (`binding_A01_t08`
/// `C.instanceMethod`).
#[test]
fn wildcard_type_param_in_instance_method_declares_nothing() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         class C {\n\
           void instanceMethod<_>() {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// A local function's own `<_>` declares nothing (`binding_A01_t08`
/// `localFunction`).
#[test]
fn wildcard_type_param_in_local_function_declares_nothing() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         main() {\n\
           void localFunction<_>() {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// A function literal's own `<_>` declares nothing (`binding_A01_t08`
/// `functionExpression`).
#[test]
fn wildcard_type_param_in_function_literal_declares_nothing() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         main() {\n\
           var functionExpression = <_>() {\n\
             print(_);\n\
           };\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

// — `class C<_>` / `mixin M<_>` member bodies (t09) ————————————————————

/// A class's own wildcard type parameter binds nothing for its members'
/// bodies (`binding_A01_t09` `C.m`): the `_` element is skipped at
/// `TypeParameterScope` even though the member can name `C`'s type
/// parameters.
#[test]
fn wildcard_type_param_on_a_class_binds_nothing_in_member_bodies() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         class C<_> {\n\
           void m() {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// A mixin's own wildcard type parameter binds nothing (`binding_A01_t09`
/// `M.m`).
#[test]
fn wildcard_type_param_on_a_mixin_binds_nothing_in_member_bodies() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         mixin M<_> {\n\
           void m() {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

// — the written-signature capture's own `<_>` ——————————————————————————

/// A generic function-TYPE's own wildcard parameter declares nothing
/// inside the signature: `void Function<_>(_)`'s `(_)` formal resolves
/// `_` as an undefined class — the capture's scope skips the `_` element
/// (`binding_A01_t08`'s annotation counterpart, oracle `Undefined class
/// '_'.` at the formal's name).
///
/// BLOCKED out-of-scope: the signature-internal `_` binds through the
/// `fn_type_capture` machinery itself (`fn_capture_type_variable` pushes
/// the spelling into `capture.scope`/`capture.params` unchecked, and the
/// capture's own tree path answers it before the type-use row reaches
/// `type_resolution`'s now-gated scope arm) — type_capture.rs /
/// listener.rs, outside this lane's scope.
#[ignore = "signature capture binds `_` before type_resolution (type_capture.rs) — outside this lane's scope"]
#[test]
fn wildcard_type_param_inside_a_signature_is_an_undefined_class() {
    let report = report_ops("typedef G = void Function<_>(_);\n");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_class")
            .map(|d| d.message.to_string())
            .collect::<Vec<_>>(),
        vec!["Undefined class '_'.".to_string()],
        "{:?}",
        code_names(&report)
    );
}

/// A method's parameter annotation `void Function<_>()` binds nothing
/// for the method BODY — the annotation's own `<_>` never leaks into the
/// enclosing executable's scope.
#[test]
fn wildcard_in_a_parameter_annotation_binds_nothing_in_the_body() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         class C {\n\
           void m(void Function<_>() f) {\n\
             print(_);\n\
           }\n\
         }\n",
    );
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

// — negative controls ————————————————————————————————————————————————

/// A named single-use type parameter still binds in a generic function
/// body — the wildcard gate must not swallow ordinary parameters.
#[test]
fn named_type_param_still_binds_in_the_body() {
    let report = report_ops(
        "void print(Object? o) {}\n\
         void f<T>() {\n\
           print(T);\n\
         }\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// Below 3.7 the wildcard gate is off: `<_>` is an ordinary type
/// parameter whose body read resolves — the `declares_nothing` feature
/// edge must keep the pre-feature answer.
#[test]
fn wildcard_type_param_without_the_feature_still_binds() {
    let report = report_ops_wildcards(
        "void print(Object? o) {}\n\
         void f<_>() {\n\
           print(_);\n\
         }\n",
        false,
    );
    assert_identifier_rows(&report, &[]);
}

/// A wildcard type parameter's own bound still resolves (`void f<_
/// extends num>()` — the declaration parses, only the name binding is
/// suppressed): no diagnostic fires on the bound annotation itself.
#[test]
fn wildcard_type_param_with_a_bound_stays_clean() {
    let report = report_ops("void f<_ extends num>() {}\n");
    assert_silent(&report);
}
