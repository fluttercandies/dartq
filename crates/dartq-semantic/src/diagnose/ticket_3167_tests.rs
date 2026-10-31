//! #3167 — the `undefined_class` emit faces in [`crate::diagnose`]'s
//! `resolve_type_row` dispatch.
//!
//! Every remaining corpus divergence on this code is upstream of the emit
//! faces (parser-recovery row production, the verdict producer in the
//! type layer, the core-name table) — the faces themselves are
//! oracle-converged. These pins lock the converged gates so co-tenant
//! lanes (#3150/#3152/#3153/#3154/#3163/#3164) cannot shift them:
//!
//! - the `Missing` fallthrough names the head only, never the written
//!   type arguments (`Unresolved<int>` reports `Undefined class
//!   'Unresolved'.`),
//! - the `Prefixed` face names the CONTINUATION (`p.A` with a resolved
//!   `p` reports `Undefined class 'A'.`),
//! - the explicit creation head replaces the fallthrough with
//!   `creation_with_non_type` (named_type_resolver.dart:285-289/:313-330
//!   — the `ConstructorInvocation` parent routes both the unresolved name
//!   and the resolved non-class shape),
//! - the `on`-clause entry's miss is ADDITIVE: the constraint code and
//!   the resolver's `undefined_class` land beside each other
//!   (scope_analyzer.dart's clause switch; upstream Dart SDK source,
//!   pkg/analyzer/lib/src/error/lookup_failure_reporter.dart's
//!   UnqualifiedLookupDomain pairing),
//! - a type-argument-position non-type is replaced by
//!   `non_type_as_type_argument` (named_type_resolver.dart:760-770's
//!   context chain),
//! - an implicit function-invocation miss on an unresolved head stays on
//!   the `undefined_function` face (both engines agree the creation
//!   split's `undefined_class` arm is a TYPE-position face only).

use super::test_rig::report_ops;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    rows_named(report, name)
        .into_iter()
        .map(|d| d.message.to_string())
        .collect()
}

/// A plain unresolved type annotation reports `undefined_class` naming the
/// head (`f() { C c; }`).
#[test]
fn plain_unresolved_annotation_names_the_head() {
    let report = report_ops("void f() { C c; }\n");
    let classes = messages_named(&report, "undefined_class");
    assert_eq!(classes.len(), 1, "one class miss: {classes:?}");
    assert_eq!(
        classes[0], "Undefined class 'C'.",
        "the miss names the head: {classes:?}"
    );
}

/// An unresolved generic head names the head WITHOUT its written type
/// arguments (`void f(Unresolved<int> a)` reports `Undefined class
/// 'Unresolved'.`).
#[test]
fn generic_head_miss_strips_the_written_arguments() {
    let report = report_ops("void f(Unresolved<int> a) {}\n");
    let classes = messages_named(&report, "undefined_class");
    assert_eq!(classes.len(), 1, "one class miss: {classes:?}");
    assert_eq!(
        classes[0], "Undefined class 'Unresolved'.",
        "the miss names the bare head: {classes:?}"
    );
}

/// A prefixed miss names the CONTINUATION (`import 'dart:math' as p;
/// p.A a;` reports `Undefined class 'A'.`).
#[test]
fn prefixed_miss_names_the_continuation() {
    let report = report_ops("import 'dart:math' as p;\np.A a;\n");
    let classes = messages_named(&report, "undefined_class");
    assert_eq!(classes.len(), 1, "one class miss: {classes:?}");
    assert_eq!(
        classes[0], "Undefined class 'A'.",
        "the miss names the continuation: {classes:?}"
    );
}

/// An explicit `new` creation head on an unresolved name reports
/// `creation_with_non_type` — the fallthrough never fires
/// (named_type_resolver.dart:285-289).
#[test]
fn explicit_creation_head_replaces_the_fallthrough() {
    let report = report_ops("void f() { var x = new Missing(); }\n");
    let classes = rows_named(&report, "undefined_class");
    assert!(classes.is_empty(), "no class miss: {classes:?}");
    let creations = messages_named(&report, "creation_with_non_type");
    assert_eq!(creations.len(), 1, "one creation miss: {creations:?}");
    assert_eq!(
        creations[0], "The name 'Missing' isn't a class.",
        "the creation face reports: {creations:?}"
    );
}

/// The `on`-clause entry's miss is additive: the constraint code and the
/// resolver's `undefined_class` land beside each other (`mixin M on B
/// implements I {}` reports both at `B`).
#[test]
fn on_clause_miss_keeps_both_codes() {
    let report = report_ops("mixin M on B implements I {}\n");
    let constraints = messages_named(&report, "mixin_super_class_constraint_non_interface");
    assert_eq!(constraints.len(), 1, "one constraint row: {constraints:?}");
    let classes = messages_named(&report, "undefined_class");
    assert_eq!(classes.len(), 1, "one class miss: {classes:?}");
    assert_eq!(
        classes[0], "Undefined class 'B'.",
        "the constraint's resolver row stays: {classes:?}"
    );
}

/// A resolved non-type in a type-argument position is replaced by
/// `non_type_as_type_argument` — the fallthrough never fires
/// (named_type_resolver.dart:760-770's context chain).
#[test]
fn non_type_type_argument_replaces_the_fallthrough() {
    let report = report_ops("void f(int i) { List<i> x; }\n");
    let classes = rows_named(&report, "undefined_class");
    assert!(classes.is_empty(), "no class miss: {classes:?}");
    let args = messages_named(&report, "non_type_as_type_argument");
    assert_eq!(args.len(), 1, "one type-argument row: {args:?}");
    assert_eq!(
        args[0],
        "The name 'i' isn't a type, so it can't be used as a type argument.",
        "the context face reports: {args:?}"
    );
}

/// An implicit invocation miss on an unresolved head stays on the
/// `undefined_function` face — the creation split's `undefined_class` arm
/// is a TYPE-position face only (both engines agree).
#[test]
fn implicit_invocation_miss_stays_on_the_function_face() {
    let report = report_ops("void f() { var x = Missing(); }\n");
    let classes = rows_named(&report, "undefined_class");
    assert!(classes.is_empty(), "no class miss: {classes:?}");
    let functions = messages_named(&report, "undefined_function");
    assert_eq!(functions.len(), 1, "one function miss: {functions:?}");
    assert_eq!(
        functions[0], "The function 'Missing' isn't defined.",
        "the invocation face reports: {functions:?}"
    );
}
