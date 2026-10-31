//! #3230 (wave-B cluster B) — the function-identifier receiver face.
//!
//! An operand whose static type is a resolved FUNCTION (a top-level or a
//! local function declaration head, i.e. a tear-off read) carries a
//! function type upstream: `binary_expression_resolver.dart:425-478`
//! types the operand from the element it resolves to
//! (`MethodElement.type` / the function's own signature), and the
//! operator consult on a `FunctionType` receiver misses every spelling
//! but `==`, so `f + 1` reports `undefined_operator` naming the tear-off
//! type. The member face mirrors that: a function type declares only
//! `call` (the synthetic invocation method), so `f.gone` reports
//! `undefined_getter`.
//!
//! The engine staged no face for a bare function-declaration read
//! (`BareInfo::Plain` carries the declared type of a `var`/field/local,
//! never a declaration's signature), so the row fell to
//! `Receiver::Unknown` and the whole family stayed silent — the
//! under-report the wave-B census counted.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `tests.rs` keeps
//! the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

/// The source offset of the fixture's LAST `needle`, as a report offset
/// (the rig prepends [`CORE_PRELUDE`]).
fn at_last(fixture: &str, needle: &str) -> u32 {
    u32::try_from(CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle")).expect("offset")
}

/// A top-level function read as an operand: `f + 1` resolves `+` on the
/// tear-off's function type and misses.
#[test]
fn ticket_3230_top_level_fn_operand_reports() {
    let fixture = "int f(int x) { return x; }\nvoid g() {\n  f + 1;\n}";
    let report = report_ops(fixture);
    let shown = rows(&report);
    assert_eq!(
        code_names(&report),
        ["undefined_operator"],
        "rows: {shown:#?}"
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_operator")
        .expect("operator row");
    assert_eq!(row.range.offset, at_last(fixture, "+"), "rows: {shown:#?}");
    assert_eq!(
        row.message,
        "The operator '+' isn't defined for the type 'int Function(int)'."
    );
}

/// The local-function twin: a statement-scoped declaration head reads as
/// its own signature (`closure_init_ty`'s written formals).
#[test]
fn ticket_3230_local_fn_operand_reports() {
    let fixture = "void g() {\n  int h(int x) => x;\n  h + 1;\n}";
    let report = report_ops(fixture);
    let shown = rows(&report);
    assert_eq!(
        code_names(&report),
        ["undefined_operator"],
        "rows: {shown:#?}"
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "undefined_operator")
            .expect("operator row")
            .message,
        "The operator '+' isn't defined for the type 'int Function(int)'."
    );
}

/// The member face of the same receiver: a function type declares `call`
/// only, so an absent getter reports.
#[test]
fn ticket_3230_fn_tear_off_absent_getter_reports() {
    let fixture = "void f() {}\nvoid g() {\n  f.gone;\n}";
    let report = report_ops(fixture);
    let shown = rows(&report);
    assert_eq!(code_names(&report), ["undefined_getter"], "rows: {shown:#?}");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "undefined_getter")
            .expect("getter row")
            .message,
        "The getter 'gone' isn't defined for the type 'void Function()'."
    );
}

/// `f.call` resolves on the synthetic invocation method — silent.
#[test]
fn ticket_3230_fn_tear_off_call_silent() {
    let fixture = "void f() {}\nvoid g() {\n  f.call;\n}";
    let report = report_ops(fixture);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}
