//! The #3295 pins (UNDEF): the `undefined_operator` / `undefined_identifier`
//! faces this ticket clustered — the two biggest remaining red families in
//! the parity sweep (371 combined case fails at the ticket's census).
//!
//! # What the cluster pass found
//!
//! The census was re-measured at HEAD over every corpus case whose oracle
//! rows carry either code (580 of 581 candidate cases materialize and run):
//! 548 already agree 1:1, and the residual is a flat long tail — no single
//! lookup arm owns it. The dominant *shapes* are nonetheless stable and
//! cheap to lock, and this file pins them:
//!
//! - operator faces: builtin (`bool`) binary/unary/relational receivers,
//!   index read vs index write as distinct spellings (`[]` / `[]=`), an
//!   extension spelling rescuing a member miss, the triple-shift spelling
//!   on a user container, a declared container spelling staying silent;
//! - the silent-negative receivers: `dynamic` (the
//!   `isDynamicBounded` short-circuit, type_property_resolver.dart:88-94);
//! - identifier faces: a bare unresolved name, the wildcard `_` read under
//!   the `wildcard-variables` feature (a lone `_` declares nothing, so the
//!   read falls through the lexical chain — scope.dart:537-557), the same
//!   read resolving when the feature is off, and the two positions the
//!   wildcard gate must NOT reach (a frame-local `var _` still declares
//!   nothing, a top-level `_` is an ordinary library-scope name), plus a
//!   label reference staying silent.
//!
//! Oracle provenance: `dart analyze` 3.13.7 (the pinned SDK) — every
//! reported/ silent verdict below was probed against that binary before
//! this file was written; the residuals the probe exposed are recorded in
//! the ticket, not pinned here.

use super::test_rig::{code_names, report_ops, report_ops_wildcards};

/// The messages of `report`'s rows carrying `code`, in emission order.
fn rows_of(report: &super::FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// Every `undefined_operator` message of `report`, in emission order.
fn operator_rows(report: &super::FileReport) -> Vec<String> {
    rows_of(report, "undefined_operator")
}

/// Every `undefined_identifier` message of `report`, in emission order.
fn identifier_rows(report: &super::FileReport) -> Vec<String> {
    rows_of(report, "undefined_identifier")
}

/// The builtin `bool` face declares no operator, so each applied spelling
/// reports its own row at the operator token: binary `+`/`-`, the
/// `unary-` spelling, and the relational `<`. The rendered receiver is the
/// container spelling the oracle prints (`type_property_resolver.dart`'s
/// `instanceReceiverType` display).
#[test]
fn bool_builtin_operator_faces_report() {
    let report = report_ops("void f(bool a, bool b) { a + b; a - b; -a; a < b; }\n");
    let rows = operator_rows(&report);
    assert_eq!(rows.len(), 4, "{rows:?}");
    for (row, spelling) in rows.iter().zip(["'+'", "'-'", "'unary-'", "'<'"]) {
        assert!(
            row.contains(spelling) && row.contains("'bool'"),
            "{row:?} does not name {spelling} on 'bool'"
        );
    }
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
}

/// A container declaring no `*` reports the applied spelling; the
/// declared-operator twin below stays silent, so the pair pins both
/// directions of the same member walk.
#[test]
fn container_without_the_operator_reports_it() {
    let report = report_ops("class C {} void f(C a, C b) { a * b; }\n");
    assert_eq!(
        operator_rows(&report),
        vec!["The operator '*' isn't defined for the type 'C'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// The declared spelling keeps the walk silent — the regression twin of
/// [`container_without_the_operator_reports_it`].
#[test]
fn declared_operator_stays_silent() {
    let report = report_ops("class C { int operator +(C o) => 0; } void f(C a, C b) { a + b; }\n");
    assert!(
        operator_rows(&report).is_empty(),
        "{:?}",
        operator_rows(&report)
    );
}

/// An index read and an index write are distinct lookup keys: a plain read
/// consults `[]` alone, an assignment consults `[]=` alone
/// (`operator_spellings`), so a container declaring neither reports the
/// spelling the access actually needs.
#[test]
fn index_read_and_write_are_distinct_spellings() {
    let read = report_ops("class A {} void f(A a) { a[0]; }\n");
    assert_eq!(
        operator_rows(&read),
        vec!["The operator '[]' isn't defined for the type 'A'.".to_owned()],
        "{:?}",
        code_names(&read)
    );
    let write = report_ops("class A {} void f(A a) { a[0] = 1; }\n");
    assert_eq!(
        operator_rows(&write),
        vec!["The operator '[]=' isn't defined for the type 'A'.".to_owned()],
        "{:?}",
        code_names(&write)
    );
}

/// An applicable extension declaring the spelling rescues the member miss:
/// the implicit-application face resolves user operators
/// (extension_member_resolver.dart's `getExtension` + the `[]` lookup).
#[test]
fn extension_operator_rescues_the_member_miss() {
    let report = report_ops(
        "class A {}\n\
         extension E on A { int operator [](int i) => i; }\n\
         void f(A a) { a[0]; }\n",
    );
    assert!(
        operator_rows(&report).is_empty(),
        "{:?}",
        operator_rows(&report)
    );
}

/// The triple-shift spelling is a user-definable operator: a container
/// without it reports `>>>` at the operator token.
#[test]
fn triple_shift_on_a_container_reports() {
    let report = report_ops("class C {} void f(C c) { c >>> 1; }\n");
    assert_eq!(
        operator_rows(&report),
        vec!["The operator '>>>' isn't defined for the type 'C'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// A `dynamic` receiver dispatches at run time: the
/// `isDynamicBounded` short-circuit clears `needsGetterError`, so no
/// operator row ever fires on it (type_property_resolver.dart:88-94).
#[test]
fn dynamic_receiver_stays_silent() {
    let report = report_ops("void f(dynamic d) { d + 1; d[0]; d[0] = 1; }\n");
    assert!(
        operator_rows(&report).is_empty(),
        "{:?}",
        operator_rows(&report)
    );
}

/// A bare identifier no declaration binds reports `undefined_identifier`
/// naming the spelling.
#[test]
fn bare_unresolved_name_reports() {
    let report = report_ops("void f() { unknown; }\n");
    assert_eq!(
        identifier_rows(&report),
        vec!["Undefined name 'unknown'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// Under `wildcard-variables` a lone `_` declares nothing at any frame
/// level, so a read of a wildcarded parameter falls through the lexical
/// chain and reports.
#[test]
fn wildcard_parameter_read_reports_under_the_feature() {
    let report = report_ops_wildcards("void f(int _, int b) { _; b; }\n", true);
    assert_eq!(
        identifier_rows(&report),
        vec!["Undefined name '_'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// The pre-3.7 twin: without the feature a lone `_` is an ordinary name
/// and the same read resolves silently.
#[test]
fn wildcard_parameter_read_resolves_without_the_feature() {
    let report = report_ops_wildcards("void f(int _, int b) { _; b; }\n", false);
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
}

/// A frame-local `var _` takes the same wildcard gate: the binding is
/// skipped, the read reports.
#[test]
fn wildcard_local_read_reports_under_the_feature() {
    let report = report_ops_wildcards("void f() { var _ = 0; _; }\n", true);
    assert_eq!(
        identifier_rows(&report),
        vec!["Undefined name '_'.".to_owned()],
        "{:?}",
        code_names(&report)
    );
}

/// The wildcard gate stops at frame scopes: a top-level `_` is an ordinary
/// library-scope name, so the read keeps resolving even with the feature
/// on.
#[test]
fn top_level_underscore_read_resolves_under_the_feature() {
    let report = report_ops_wildcards("int _ = 0;\nvoid f() { _; }\n", true);
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
}

/// A label reference resolves to its statement label — never an
/// `undefined_identifier` row.
#[test]
fn label_reference_resolves_silently() {
    let report = report_ops("void f() { outer: for (var i = 0; i < 1; i++) { break outer; } }\n");
    assert!(
        identifier_rows(&report).is_empty(),
        "{:?}",
        identifier_rows(&report)
    );
}
