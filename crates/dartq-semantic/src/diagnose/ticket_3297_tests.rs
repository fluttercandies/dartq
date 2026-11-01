//! The #3297 pins (UNDEF binding tail): the binding faces `scopes_bodies`
//! owns, at the shapes #3295's measured residual left unresolved.
//!
//! # The shapes
//!
//! - **wildcard type parameters**: under `wildcard-variables` a type
//!   parameter named `_` is a wildcard declaration, so it binds nothing —
//!   upstream `TypeParameterScope` skips the element exactly like
//!   `LocalScope.add` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/element/scope.dart:944-960) — and a read
//!   falls through the lexical chain to `undefined_identifier`. The same
//!   parameter is an ordinary name below 3.7 (`declares_nothing`'s gate).
//! - **pattern scope**: an if-case / switch-case binder is bound to its
//!   case scope alone (upstream `GuardedPattern` case scope) — it reads in
//!   the clause's guard and its own arm, and falls through in a *later*
//!   arm, in the `else` branch, and after the statement. The frames bind
//!   flat, so the arm scoping is the `pattern_scope` shadow/restore pair's
//!   job; these pins lock both directions.
//!
//! # Residuals this ticket could not close in scope
//!
//! The case-scope exit needs three files this ticket does not own
//! (`resolution.rs`, `pattern_scope.rs`, `listener.rs`):
//!
//! 1. `Engine::resolve_bare`'s frame loop (resolution.rs:1816-1828) answers
//!    a frame-local name off `Frame::locals` membership alone and never
//!    reads `LocalDecl::retired` — so the retirement
//!    `pattern_scope::retire_binders` already stamps at the case close is
//!    invisible to the bare-name decision, and the read resolves past its
//!    scope.
//! 2. `note_pattern_binder` (listener.rs:8108-8113) registers the retire
//!    hook only when the binder SHADOWS an outer tracked local
//!    (`shadows_local`), so an if-case binder that shadows nothing is never
//!    retired at all.
//! 3. Moving the binder off `Frame::locals` instead (it can ride
//!    `Frame::var_types`, which `restore_var_types` already scopes) is the
//!    other half — but ~20 `frames.iter().any(|f| f.locals.contains(..))`
//!    shadow/suppression gates in `invocation.rs`, `assignability.rs`,
//!    `engine_core.rs`, and `resolution.rs` treat that set as "is a local
//!    in scope", so the move cannot land from the binding side alone.
//!
//! The three pins below are the oracle verdicts for that work and stay
//! `#[ignore]`d until the owning lanes land it.
//!
//! Oracle provenance: the co19 cases the cluster was measured on,
//! `.third/co19/LanguageFeatures/Wildcards/binding_A01_t08`/`t09`,
//! `Patterns/scope_A04_t01`/`t02`, `A06_t01`/`t02` (pinned 3.13.7 corpus)
//! — the marked sites are the reported rows, the unmarked ones the silent
//! reads.

use super::test_rig::{code_names, report_ops, report_ops_wildcards};

/// Every `undefined_identifier` message of `report`, in emission order.
fn identifier_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .map(|d| d.message.to_string())
        .collect()
}

/// Assert the report carries exactly `expected` undefined-name rows.
#[track_caller]
fn assert_identifier_rows(report: &super::FileReport, expected: &[&str]) {
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

// — wildcard type parameters ————————————————————————————————————————————

/// A generic function's wildcard type parameter declares nothing: the body
/// read falls through to `undefined_identifier` (`binding_A01_t08`).
#[test]
fn wildcard_type_parameter_on_a_function_declares_nothing() {
    let report = report_ops("void f<_>() {\n  _;\n}\n");
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// The class-header twin (`binding_A01_t09`): `class C<_>` binds nothing for
/// the instance members' bodies either.
#[test]
fn wildcard_type_parameter_on_a_class_declares_nothing() {
    let report = report_ops("class C<_> {\n  void m() {\n    _;\n  }\n}\n");
    assert_identifier_rows(&report, &["Undefined name '_'."]);
}

/// The silent-negative: a named type parameter is a TypeParameterScope
/// entry, so its bare read answers the `Type` literal and reports nothing.
#[test]
fn named_type_parameter_still_resolves() {
    let report = report_ops("void f<T>() {\n  T;\n}\n");
    assert_identifier_rows(&report, &[]);
}

/// Below 3.7 the wildcard gate is off and `<_>` is an ordinary name that
/// keeps resolving — the `declares_nothing` feature edge.
#[test]
fn wildcard_type_parameter_without_the_feature_is_an_ordinary_name() {
    let report = report_ops_wildcards("void f<_>() {\n  _;\n}\n", false);
    assert_identifier_rows(&report, &[]);
}

// — pattern scope ——————————————————————————————————————————————————————

/// An if-case binder reads in the then branch (`scope_A04_t01` test3).
#[test]
fn if_case_binder_resolves_in_the_then_branch() {
    let report = report_ops("void test() {\n  if (42 case var v) {\n    v;\n  }\n}\n");
    assert_identifier_rows(&report, &[]);
}

/// ... and falls through in the `else` branch (`scope_A06_t01`): the case
/// scope closed, so the read is undefined.
#[test]
#[ignore = "case-scope exit: resolve_bare ignores LocalDecl::retired (module docs, residual 1)"]
fn if_case_binder_is_out_of_scope_in_the_else_branch() {
    let report = report_ops(
        "void test() {\n  if (42 case var v) {\n    v;\n  } else {\n    v;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// ... and after the statement (`scope_A04_t01` test3's second read).
#[test]
#[ignore = "case-scope exit: resolve_bare ignores LocalDecl::retired (module docs, residual 1)"]
fn if_case_binder_is_out_of_scope_after_the_statement() {
    let report = report_ops("void test() {\n  if (42 case var v) {\n    v;\n  }\n  v;\n}\n");
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}

/// A switch-case binder reads in its own arm.
#[test]
fn switch_case_binder_resolves_in_its_own_arm() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case var v:\n      v;\n      break;\n    default:\n      break;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &[]);
}

/// ... and falls through in a later sibling arm (`scope_A04_t01` test1's
/// `case 0` / `default` reads): each guarded pattern introduces its own
/// case scope.
#[test]
#[ignore = "case-scope exit: resolve_bare ignores LocalDecl::retired (module docs, residual 1)"]
fn switch_case_binder_is_out_of_scope_in_a_later_arm() {
    let report = report_ops(
        "void test() {\n  switch (42) {\n    case var v:\n      v;\n      break;\n    default:\n      v;\n  }\n}\n",
    );
    assert_identifier_rows(&report, &["Undefined name 'v'."]);
}
