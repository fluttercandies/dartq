//! The #3341 pins: the `type_alias_cannot_reference_itself` face.
//!
//! Oracle semantics (dart 3.13.7, the pinned conformance oracle): the
//! summary linker runs `TypeAliasSelfReferenceFinder` over every typedef
//! before the types build (`link.dart:409`), a purely syntactic walk that
//! descends the right-hand side — return type, formals, type arguments,
//! the declaration's own parameter bounds — and marks the fragment when a
//! `NamedType` head resolves to the typedef itself, either directly or
//! through another typedef's right-hand side / a class-or-mixin
//! type-parameter bound (`type_alias.dart:10-133`). The verifier then
//! reports one row per marked fragment, anchored at the declaration's name
//! token (`_checkForTypeAliasCannotReferenceItself`,
//! error_verifier.dart:7116-7127), with the message `Typedefs can't
//! reference themselves directly or recursively via another typedef.`
//! (diagnostic.g.dart:16041-16051).
//!
//! The paired instantiation effect: a self-referencing alias's
//! `instantiateImpl` answers `dynamic` rather than substituting into a
//! right-hand side that mentions the alias (`element.dart:11277-11288`),
//! so a use of such an alias types as `dynamic` — never as the unfolded
//! recursive shape.
//!
//! Matrix pinned here: the direct self-reference (1 row at the name
//! token), the co-alias pair (`typedef A = B; typedef B = A;` — 2 rows,
//! one per declaration), the non-recursive negative (0 rows), the
//! class-bound descent (0-or-1 by the walk's direction: the alias is
//! flagged when a class its right-hand side names carries a bound
//! mentioning it), and the fold→`dynamic` collapse.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::FileReport;

/// The `type_alias_cannot_reference_itself` rows of `report` in source
/// order: `(offset, length)`.
fn self_reference_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_alias_cannot_reference_itself")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    rows.sort_unstable();
    rows
}

/// The absolute offset of the typedef NAME inside `decl`, where `decl` is
/// the declaration's opening text (`"typedef A = B;"`). The rig prelude
/// shifts every reported offset.
fn declaration_name_offset(source: &str, decl: &str) -> u32 {
    let start = source.find(decl).expect("the fixture declares it");
    u32::try_from(CORE_PRELUDE.len() + start + "typedef ".len()).expect("offset fits")
}

/// `typedef F = F Function();` — the alias's own right-hand return type
/// names the alias: exactly one row, at the `F` name token.
#[test]
fn ticket_3341_direct_self_reference_reports_at_the_name_token() {
    let source = "typedef F = F Function();\n";
    let report = report_ops(source);
    let name = declaration_name_offset(source, "typedef F");
    assert_eq!(
        self_reference_rows(&report),
        vec![(name, 1)],
        "one row at the name token: {:?}",
        report.diagnostics
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_alias_cannot_reference_itself")
        .map(|d| d.message.to_string())
        .expect("the row exists");
    assert_eq!(
        message,
        "Typedefs can't reference themselves directly or recursively via another typedef."
    );
}

/// `typedef A = B; typedef B = A;` — the pair references itself through
/// the co-alias, so each declaration reports: two rows, one per name
/// token.
#[test]
fn ticket_3341_co_alias_pair_reports_both_declarations() {
    let source = "typedef A = B;\ntypedef B = A;\n";
    let report = report_ops(source);
    let a = declaration_name_offset(source, "typedef A");
    let b = declaration_name_offset(source, "typedef B");
    assert_eq!(
        self_reference_rows(&report),
        vec![(a, 1), (b, 1)],
        "both declarations report: {:?}",
        report.diagnostics
    );
}

/// `typedef G = int Function();` — no reference to the alias anywhere in
/// the right-hand side: no row.
#[test]
fn ticket_3341_non_recursive_alias_stays_silent() {
    let source = "typedef G = int Function();\n";
    let report = report_ops(source);
    assert_eq!(
        self_reference_rows(&report),
        Vec::new(),
        "no self reference: {:?}",
        report.diagnostics
    );
}

/// The bound descent: `typedef F = C;` over `class C<X extends F> {}` —
/// the alias's right-hand side names `C`, whose type-parameter bound names
/// the alias. The upstream walk descends a class declaration's parameters
/// (`type_alias.dart:101-116`), so `F` is flagged at its own name token.
#[test]
fn ticket_3341_class_bound_descent_reports_the_alias() {
    let source = "class C<X extends F> {}\ntypedef F = C;\n";
    let report = report_ops(source);
    let name = declaration_name_offset(source, "typedef F");
    assert_eq!(
        self_reference_rows(&report),
        vec![(name, 1)],
        "the bound descent flags the alias: {:?}",
        report.diagnostics
    );
}

/// The fold collapse: a self-referencing alias instantiates to `dynamic`,
/// so assigning one to an `int` reports nothing, while the identical
/// non-self-referencing shape (`typedef R = int Function();`) still
/// reports its `invalid_assignment`. Before the #3341 fold, `F` unfolded
/// to `F Function()` and the first statement reported too.
#[test]
fn ticket_3341_self_referencing_alias_folds_to_dynamic() {
    let source = "typedef F = F Function();\n\
                  typedef R = int Function();\n\
                  void main(F f, R r) {\n\
                  \x20 int x = f;\n\
                  \x20 int y = r;\n\
                  }\n";
    let report = report_ops(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "only the non-recursive control reports: {rows:?}"
    );
    assert!(
        rows[0].contains("Function"),
        "the reported row is the control's function type: {rows:?}"
    );
}
