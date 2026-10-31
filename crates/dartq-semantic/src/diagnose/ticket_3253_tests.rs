//! #3253 pins — the disallowed-supertype clause gate (recon C2):
//! `CorrectOverrideHelper.verify()` returns before EVERY member check when
//! any written direct-supertype clause names a non-subtypable face, so no
//! `invalid_override` row fires on that container's members at all.
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `inheritance_override.dart:312-314` `verify()` — the
//!   `_checkDirectSuperTypes()` early return, ahead of the interface build
//!   and the whole member loop;
//! - `:649-688` `_checkDirectSuperTypes` — the four clause walks
//!   (`implements`, `on`, `extends`, `with`);
//! - `:590-620` `_checkDirectSuperType` — the `dart:*` library exemption,
//!   the core-`Enum` arm (concrete classes only) and
//!   `isNonSubtypableClass`.
//!
//! The rig declares its own `num`/`int`-shaped classes, and
//! `non_subtypable_clause_head_spelling` exempts any spelling that resolves
//! to a user declaration (the `isNonSubtypableClass` contract keys on the
//! resolved element's `dart:*` library), so the gated fixtures ride the
//! names-only core stubs (`int`, `String`, `FutureOr`, `Enum`) that carry no
//! index entry.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope, and `#[path]` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{report_ops_bare, rows};

/// The `invalid_override` rows of one report, verbatim.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// `extends int` is a disallowed clause, so the shared-signature row
/// `C.m(String)` vs `I.m(int)` never fires: the oracle reports the clause
/// error at the `extends` node and returns from `verify()` before the member
/// loop. Without the gate this fixture lands exactly one row.
#[test]
fn disallowed_superclass_suppresses_the_member_override_row() {
    let report = report_ops_bare(
        "\
abstract class I { void m(int a); }
class C extends int implements I { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert!(
        found.is_empty(),
        "the disallowed `extends int` clause suppresses every member row: {}",
        rows(&report).join("\n")
    );
}

/// The `with` and `implements` clause walks carry the same gate: a
/// disallowed mixin face and a disallowed interface face each stand the
/// member checks down for the whole container.
#[test]
fn disallowed_with_and_interface_faces_suppress_member_rows() {
    let report = report_ops_bare(
        "\
abstract class I { void m(int a); }
class C with FutureOr implements I { void m(String a) {} }
class D implements String, I { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert!(
        found.is_empty(),
        "both the `with FutureOr` and the `implements String` clause gate: {}",
        rows(&report).join("\n")
    );
}

/// The core-`Enum` arm is container-kind sensitive upstream
/// (`:610-620`): only a CONCRETE class's `Enum` face is a clause error — an
/// abstract class, an enum and a mixin may name it freely, so their member
/// checks keep running. Gating on the name alone would drop the abstract
/// class's legitimate row.
#[test]
fn enum_face_gates_only_a_concrete_class() {
    let report = report_ops_bare(
        "\
abstract class I { void m(int a); }
abstract class Abstract implements Enum, I { void m(String a) {} }
class Concrete implements Enum, I { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "only the abstract class keeps its member row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("'Abstract.m'"),
        "the surviving row belongs to the abstract class: {}",
        rows(&report).join("\n")
    );
}

/// The name list is only the closed-spelling face: a spelling that resolves
/// to a USER declaration is never a disallowed supertype
/// (`isNonSubtypableClass` keys on the resolved element's `dart:*` library),
/// so `class num {}` declared here keeps its member checks running.
#[test]
fn user_declaration_shadowing_a_restricted_name_does_not_gate() {
    let report = report_ops_bare(
        "\
class num { void m(int a) {} }
class C extends num { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "the user `num` is not a restricted face: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("of 'num.m'"),
        "the row compares against the user declaration: {}",
        rows(&report).join("\n")
    );
}

/// Regression: a clean superclass clause keeps the ordinary
/// `invalid_override` row — the gate must not turn into a blanket
/// suppression.
#[test]
fn clean_superclass_keeps_the_override_row() {
    let report = report_ops_bare(
        "\
class Base { void m(int a) {} }
class C extends Base { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("of 'Base.m'"),
        "the row anchors at the overridden method: {}",
        rows(&report).join("\n")
    );
}
