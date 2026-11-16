//! The #4320 pins: resolved prefixed-type (`p.C`) verdict gaps closed.
//!
//! Spec: the pinned 2026-11 oracle probe round for prefixed types — the
//! oracle's full-report behavior for resolved `p.C` shapes (all six
//! verdict families below); UNRESOLVED `p.X` keeps the under-report arm.
//!
//! UNRESOLVED prefixed types (`p.X` where `X` does not exist) keep the
//! Invalid lattice's own posture exactly: `undefined_class` only, never
//! a verdict gain — the negative pins below carry that contract.

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{
    RigImport, assert_silent, code_names, report_ops, report_ops_imports, rows,
};

/// The imported library every fixture shares: `Real` is a plain class
/// (instance member `n`, method `m`), `Generic<T>` takes one type
/// parameter — the shapes `main.dart` exercises.
const LIB: RigImport<'static> = RigImport {
    uri: "package:t/lib.dart",
    prefix: Some("p"),
    source: "class Real { int n = 0; void m() {} }\nclass Generic<T> {}\n",
};

/// The sorted code-name set of `report` — the oracle's diagnostic
/// surface compared as a set (source order is the engine's own).
fn codes(report: &FileReport) -> Vec<&'static str> {
    let mut names = code_names(report);
    names.sort_unstable();
    names
}

/// (a) var-init: `p.Real a = "s";` — the written head resolves through
/// the joined container, so `assignable`'s primitive-vs-declared arm
/// reads `String` against the real class shape and reports
/// `invalid_assignment`, exactly as the unprefixed `Real a = "s";` does.
#[test]
fn var_init_wrong_value_reports_invalid_assignment() {
    let report = report_ops_imports("p.Real a = \"s\";\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// (b) assignable silent accept: `List<p.Real> b = <int>[];` — the
/// covariant argument walk asks `int <: p.Real`; the joined head
/// resolves to its class shape inside `slot_subtype`/`assignable`, so
/// the definite miss reports `invalid_assignment` (the oracle's row 2).
/// The type-argument spelling `p.Real` keeps its joined head through the
/// bounded/alias/header captures (`handle_type`'s `head_id` and
/// `begin_type_arguments`'s `capture_head` translation), so the armed
/// target reads `List<p.Real>` exactly as `named_type_is_type`'s joined
/// leg answers it.
#[test]
fn list_arg_wrong_element_reports_invalid_assignment() {
    let report = report_ops_imports("List<p.Real> b = <int>[];\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// Param annotation: `void c(p.Real v) {} c(42);` — the argument verdict
/// asks `int <: p.Real`; the resolved class shape makes the pair
/// decidable-false, reporting `argument_type_not_assignable`.
#[test]
fn param_wrong_argument_reports_argument_type_not_assignable() {
    let report = report_ops_imports("void c(p.Real v) {}\nvoid useC() { c(42); }\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
}

/// Field-decl init: `class D { p.Real f = "x"; }` — the field's
/// initializer check runs the same `assignment_verdict` the var-init
/// face does; the resolved head makes `String <: p.Real` decidable.
#[test]
fn field_decl_init_wrong_value_reports_invalid_assignment() {
    let report = report_ops_imports("class D { p.Real f = \"x\"; }\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// Ctor-init: `class D { final p.Real g; D() : g = 1; }` — the
/// initializer-list entry stages through `begin/end_initializers` and
/// re-decides via `assignment_verdict` at teardown; the joined head
/// resolves there too, reporting `field_initializer_not_assignable`.
#[test]
fn ctor_init_wrong_value_reports_field_initializer_not_assignable() {
    let report = report_ops_imports("class D { final p.Real g; D() : g = 1; }\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["field_initializer_not_assignable"],
        "{:?}",
        rows(&report)
    );
}

/// Return type: `p.Real e() { return "s"; }` — `check_return`'s
/// `named_type_is_type` gate now sees the joined head as a real type
/// (never `InvalidType`), so the verdict runs and `String <: p.Real`
/// decides false, reporting `return_of_invalid_type`.
#[test]
fn return_wrong_value_reports_return_of_invalid_type() {
    let report = report_ops_imports("p.Real e() { return \"s\"; }\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["return_of_invalid_type"],
        "{:?}",
        rows(&report)
    );
}

/// Prefixed generic arity: `p.Generic<int, String> f = p.Generic<int>();`
/// — `check_prefixed_annotation_arity` already resolves the leaf through
/// `prefixed_container` (independent of the joined intern table); the
/// pin keeps the verdict live beside the new head-container wiring.
#[test]
fn prefixed_generic_wrong_arity_reports_wrong_number_of_type_arguments() {
    let report = report_ops_imports("p.Generic<int, String> f = p.Generic<int>();\n", &[LIB]);
    assert_eq!(
        codes(&report),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        rows(&report)
    );
}

/// Not-initialized: `void g2() { p.Real h; take(h); }` — the local's
/// declared type is the joined `p.Real`; `written_potentially_non_nullable`
/// reads it through `type_head_resolves` → `head_container` as
/// potentially non-nullable, so the read reports
/// `not_assigned_potentially_non_nullable_local_variable`.
#[test]
fn uninit_local_reports_not_assigned_potentially_non_nullable() {
    let report = report_ops_imports(
        "void take(p.Real r) {}\nvoid g2() { p.Real h; take(h); }\n",
        &[LIB],
    );
    assert_eq!(
        codes(&report),
        ["not_assigned_potentially_non_nullable_local_variable"],
        "{:?}",
        rows(&report)
    );
}

/// Receiver member miss: `p.Real r = p.Real(); r.missing();
/// r.missingField;` — `check_instance_member` resolves the receiver's
/// declared type to the joined head; `container_kind_of` now answers
/// `Some(Class)` through `head_container`, so the miss reaches the
/// class-family tail and reports `undefined_method` + `undefined_getter`
/// (the oracle's rows 9-10).
#[test]
fn receiver_member_miss_reports_undefined_method_and_getter() {
    let report = report_ops_imports(
        "void i() {\n  p.Real r = p.Real();\n  r.missing();\n  r.missingField;\n}\n",
        &[LIB],
    );
    assert_eq!(
        codes(&report),
        ["undefined_getter", "undefined_method"],
        "{:?}",
        rows(&report)
    );
}

/// Negative pin 1 — UNRESOLVED `p.X`: `p.Undefined x = null;` keeps the
/// Invalid lattice's posture exactly: `undefined_class` at the leaf,
/// no assignability avalanche, no member-walk verdicts. The joined leg
/// in `named_type_is_type`/`head_container` answers `None`/`false` for
/// the missing leaf, so every gate keeps today's suppression.
#[test]
fn unresolved_prefixed_type_keeps_invalid_lattice_posture() {
    let report = report_ops_imports("p.Undefined x = null;\n", &[LIB]);
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// Negative pin 2 — non-prefixed baseline unchanged: the same-file
/// `Real x = "s";` reports `invalid_assignment` through the ordinary
/// bare-scope path, proving the joined leg did not disturb the
/// unprefixed verdict surface.
#[test]
fn non_prefixed_baseline_unchanged() {
    let report = report_ops("class Real {}\nReal x = \"s\";\n");
    assert_eq!(
        codes(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
}

/// Negative pin 3 — a RESOLVED `p.C` used correctly stays silent: the
/// joined leg grants type-ness but never invents verdicts — `p.Real r`
/// assigned a `p.Real` value and calling its real member `m()`/reading
/// its field `n` reports nothing (the member walk resolves through
/// `walk_climb_impl`'s own joined leg, untouched by this ticket).
#[test]
fn resolved_prefixed_type_correct_use_stays_silent() {
    let report = report_ops_imports(
        "void ok() {\n  p.Real r = p.Real();\n  r.m();\n  r.n;\n}\n",
        &[LIB],
    );
    assert_silent(&report);
}
