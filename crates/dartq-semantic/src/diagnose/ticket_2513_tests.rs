//! The #2513 pins: a `var`/`final`-inferred top-level variable or field
//! carries its inferred declared type into the model read faces.
//!
//! The element model cannot answer these slots — the collector runs before
//! any inference, so `TopDecl::var_ty` and `MemberEntry::var_ty` stay
//! `Ty::Unknown` for every declaration without a written annotation
//! (upstream's own elements carry the inferred `type`, resolved lazily
//! from the initializer). The diagnosing pass walks the initializers, so it
//! infers the same face locally and overlays it
//! ([`crate::diagnose::Engine::bind_inferred_ty`]): every read surface — the
//! bare top-level read, the implicit-this member read, the receiver member
//! walk — answers the inferred type instead of `Unknown`, and the faces
//! built on it (member sends, writes) decide where they used to under-report.
//!
//! The pins below hold the two directions: an inferred declaration's faces
//! now DECIDE (`undefined_method` on a wrong send, `invalid_assignment` on a
//! wrong write), and a written annotation keeps its model face untouched.

use crate::diagnose::test_rig::{CORE_PRELUDE, assert_silent, code_names, report_ops};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// A top-level `var` inferred from a bare read of a typed declaration: the
/// model's `var_ty` is `Unknown`, so the send through it used to stay
/// silent; the overlay answers `A` and the miss reports at the selector.
#[test]
fn top_level_inferred_var_send_reports_undefined_method() {
    let fixture = "class A { void m() {} }\n\
A src = A();\n\
var a = src;\n\
void use() { a.nosuch(); }\n";
    let report = report_ops(fixture);
    let rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![at(fixture, "nosuch")],
        "rows: {:?}",
        report_ops_rows(&report)
    );
}

/// The field twin: an instance field inferred from its initializer answers
/// its declared type through the implicit-this member read.
#[test]
fn inferred_field_send_reports_undefined_method() {
    let fixture = "class A { void m() {} }\n\
class B {\n\
  var a = src;\n\
  void use() { a.nosuch(); }\n\
}\n\
A src = A();\n";
    let report = report_ops(fixture);
    let rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![at(fixture, "nosuch")],
        "rows: {:?}",
        report_ops_rows(&report)
    );
}

/// The write face: `f`'s inferred `int` is the assignment's accepted type,
/// so a `String` write is `invalid_assignment` (the declared-type slot used
/// to be `Unknown`, which accepts every right side silently).
#[test]
fn inferred_field_write_reports_invalid_assignment() {
    let fixture = "class A {\n\
  var f = 1;\n\
  void m() { f = 'x'; }\n\
}\n";
    let report = report_ops(fixture);
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "rows: {:?}",
        report_ops_rows(&report)
    );
}

/// The model-level slot's whole point: a WRITTEN annotation keeps the
/// element model's own face — the overlay only fills `Unknown`, so an
/// annotated declaration's reads behave exactly as before (a valid member
/// send stays silent).
#[test]
fn written_annotation_keeps_its_model_face() {
    let fixture = "class A { void m() {} }\n\
A src = A();\n\
A a = src;\n\
void use() { a.m(); }\n";
    assert_silent(&report_ops(fixture));
}

/// A `const` top-level variable is inferred too — its literal face reaches
/// the read surfaces (a wrong send on it decides).
#[test]
fn inferred_const_top_level_var_send_reports_undefined_method() {
    let fixture = "const s = 'x';\n\
void use() { s.nosuch(); }\n";
    let report = report_ops(fixture);
    let rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![at(fixture, "nosuch")],
        "rows: {:?}",
        report_ops_rows(&report)
    );
}

/// The failure face: every row as `code @offset`.
fn report_ops_rows(report: &crate::diagnose::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{} @{}", d.code.name, d.range.offset))
        .collect()
}
