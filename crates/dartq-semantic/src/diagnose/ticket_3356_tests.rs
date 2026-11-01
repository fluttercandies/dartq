//! #3356 — the `unnecessary_type_check` / `unnecessary_null_comparison`
//! operand-static-type coverage pins.
//!
//! Both verdicts read the operand's static type (`leftNode.typeOrThrow`,
//! best_practices_verifier.dart:879 / binary_expression_resolver.dart:163-175)
//! through the row-shape faces [`Engine::type_check_operand_ty`] and
//! [`Engine::null_comparison`] consult. Before this ticket the untyped-row
//! face ([`Engine::untyped_operand_ty`]) never reached either walk: a
//! `Bare(Container)` operand (`C is T`, `C == null`), a `Member{recv:Prefix}`
//! type literal (`p.C is T`, `p.C == null`), or a `var` declarator whose
//! initializer carried a `<...>` group (`var c = C<int>`) all answered
//! `Ty::Unknown` and both verdicts suppressed where the oracle reports.
//!
//! The listener's `end_variable_initializer` pairs the window's expression
//! row by absolute row count (`rows.len() == mark + 1`) — a `TypeUse`-tailed
//! window (the `<int>`/`{String,int}` argument rows) failed every arm, so a
//! `var c = C<int>` never bound `c`'s static type.

use super::*;
use crate::diagnose::test_rig::{RigImport, report_ops, report_ops_imports};

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .count()
}

// —— the baseline arm (already wired before this ticket) ——

/// `x is int` over `int x` reports `unnecessary_type_check` — the
/// constant-true verdict the whole family rides.
#[test]
fn typed_operand_same_head_reports() {
    let report = report_ops("void f(int x) { x is int; }\n");
    assert_eq!(
        count(&report, "unnecessary_type_check"),
        1,
        "x is int over int reports: {:?}",
        names(&report)
    );
}

/// `x is! int` over `int x` reports the `_false` polarity — the strict
/// subtype answer is `true`, `is!` inverts.
#[test]
fn typed_operand_negated_reports_false() {
    let report = report_ops("void f(int x) { x is! int; }\n");
    assert_eq!(
        count(&report, "unnecessary_type_check"),
        1,
        "x is! int over int reports: {:?}",
        names(&report)
    );
}

/// `x is int` over `Object x` stays informative — the strict relation
/// answers false, no constant-true verdict exists.
#[test]
fn supertype_operand_stays_silent() {
    let report = report_ops("void f(Object x) { x is int; }\n");
    assert!(
        !names(&report).contains(&"unnecessary_type_check"),
        "Object x is int is informative: {:?}",
        names(&report)
    );
}

/// `x is Null` over `int x` is the `type_check_with_null` family — the
/// `Null` checked arm fires regardless of the operand face.
#[test]
fn null_checked_family_reports() {
    let report = report_ops("void f(int x) { x is Null; }\n");
    assert_eq!(
        count(&report, "type_check_with_null"),
        1,
        "x is Null reports: {:?}",
        names(&report)
    );
}

// —— the untyped-row faces this ticket opens ——

/// A bare container name in value position is a TypeLiteral — `C is Type`
/// reports constant-true (`Type <: Type`). Before the untyped-row face
/// reached the verdict, the `Bare(Container)` operand answered `Unknown`.
#[test]
fn container_literal_is_type_reports() {
    let report = report_ops("class C {}\nvoid f() { C is Type; }\n");
    assert_eq!(
        count(&report, "unnecessary_type_check"),
        1,
        "C is Type reports: {:?}",
        names(&report)
    );
}

/// `C is C` — the TypeLiteral's static type `Type` is NOT `C`, so the check
/// is informative and stays silent (upstream's
/// `isSubtypeOf(Type, C)` answers false).
#[test]
fn container_literal_is_itself_stays_silent() {
    let report = report_ops("class C {}\nvoid f() { C is C; }\n");
    assert!(
        !names(&report).contains(&"unnecessary_type_check"),
        "C is C is informative: {:?}",
        names(&report)
    );
}

/// `C == null` is `unnecessary_null_comparison` — `Type` is strictly
/// non-nullable. The `null_comparison` walk never read the untyped-row
/// face before this ticket.
#[test]
fn container_eq_null_reports() {
    let report = report_ops("class C {}\nvoid f() { C == null; }\n");
    assert_eq!(
        count(&report, "unnecessary_null_comparison"),
        1,
        "C == null reports: {:?}",
        names(&report)
    );
}

/// `C != null` — the `!=` polarity of the same arm.
#[test]
fn container_neq_null_reports() {
    let report = report_ops("class C {}\nvoid f() { C != null; }\n");
    assert_eq!(
        count(&report, "unnecessary_null_comparison"),
        1,
        "C != null reports: {:?}",
        names(&report)
    );
}

/// The prefixed `p.C` TypeLiteral — `p.C is Type` reports. The
/// `Member{recv:Prefix}` row's untyped face resolves the scoped container.
#[test]
fn prefixed_container_is_type_reports() {
    let report = report_ops_imports(
        "import 'package:t/other.dart' as p;\n\
         void f() { p.C is Type; }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: Some("p"),
            source: "class C {}\n",
        }],
    );
    assert_eq!(
        count(&report, "unnecessary_type_check"),
        1,
        "p.C is Type reports: {:?}",
        names(&report)
    );
}

/// `p.C == null` — the prefixed literal twin of the null-comparison arm.
#[test]
fn prefixed_container_eq_null_reports() {
    let report = report_ops_imports(
        "import 'package:t/other.dart' as p;\n\
         void f() { p.C == null; }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: Some("p"),
            source: "class C {}\n",
        }],
    );
    assert_eq!(
        count(&report, "unnecessary_null_comparison"),
        1,
        "p.C == null reports: {:?}",
        names(&report)
    );
}

/// `x == null` over `int x` — a typed local already worked; a member read
/// (`this.f == null` where `f` is declared `int`) reports the same arm
/// through the untyped-row face the member walk resolves.
#[test]
fn member_read_eq_null_reports() {
    let report = report_ops("class C {\n\
         \x20 int f = 0;\n\
         \x20 void m() { f == null; }\n\
         }\n");
    assert_eq!(
        count(&report, "unnecessary_null_comparison"),
        1,
        "f == null over int reports: {:?}",
        names(&report)
    );
}

/// The nullable member's `== null` stays informative — `int? f` is no
/// never-null face.
#[test]
fn nullable_member_eq_null_stays_silent() {
    let report = report_ops("class C {\n\
         \x20 int? f;\n\
         \x20 void m() { f == null; }\n\
         }\n");
    assert!(
        !names(&report).contains(&"unnecessary_null_comparison"),
        "int? f == null is informative: {:?}",
        names(&report)
    );
}
