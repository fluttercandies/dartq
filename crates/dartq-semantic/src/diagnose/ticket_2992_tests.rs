//! The #2992 pins: a function type's `is`-operand tear-off must never
//! panic — `tear_off_fn_ty` interned the rebuilt signature through the
//! shared `args` RefCell while its `borrow_mut` was still held across the
//! `fold_alias_ty` return fold, and the fold's own
//! `instantiated_head_ty` tail re-entered that same cell (`RefCell
//! already borrowed`, ~111 crashers over the `function_type*` corpus).
//! The fold now resolves before the borrow is taken, so a bare generic
//! return head — `List`/`Set`/`Map` or a raw `F` alias — folds through
//! the instantiate-to-bounds fill without re-entering a held borrow, and
//! the `is` verdict stays honest either way.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `dead_code` rows as the exact anchored source text, source order.
fn dead_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The `unnecessary_type_check` rows as the exact anchored source text,
/// source order.
fn verdict_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// A top-level function whose tear-off signature's return type is a BARE
/// generic head (`List` — `Named{List, EMPTY}`): the return fold descends
/// into `instantiated_head_ty`, which interns the I2B defaults through
/// `args.borrow_mut()` — the exact re-entrant borrow the held guard used
/// to collide with. With the borrow broken the operand resolves and the
/// `is` stays a runtime question (a `List` is no `int Function(int)`).
#[test]
fn bare_generic_return_head_fold_does_not_panic() {
    let fixture = concat!(
        "typedef F0<T> = int Function(int);\n",
        "List f0(int x) => throw 'uncalled';\n",
        "void t() {\n",
        "  if (f0 is F0<int>) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    // The mismatched pair stays informative: no constant verdict and no
    // dead arm — the pin is that the check resolved without panicking.
    assert!(
        report.diagnostics.is_empty(),
        "the mismatched pair must stay informative: {:?}",
        report.diagnostics
    );
}

/// A generic METHOD's tear-off under `is` (`x is T Function()` shape —
/// `m<T>` inside a container, checked against a function-typed alias):
/// the operand's declared return folds through the same path.
#[test]
fn generic_method_tear_off_is_check_does_not_panic() {
    let fixture = concat!(
        "typedef F0<T> = int Function(int);\n",
        "class C {\n",
        "  List m0(int x) => throw 'uncalled';\n",
        "  void t() {\n",
        "    if (m0 is F0<int>) throw 'bad';\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "the mismatched pair must stay informative: {:?}",
        report.diagnostics
    );
}

/// A matching bare-generic pair still folds honestly: the operand's
/// `Set`-returning tear-off against an alias whose return is `Set` —
/// the borrow fix must not change the verdict, only remove the panic.
#[test]
fn matching_bare_generic_pair_still_folds() {
    let fixture = concat!(
        "typedef Fun = Set Function(int);\n",
        "Set g(int a) => throw 'uncalled';\n",
        "void t() {\n",
        "  if (g is! Fun) throw 'bad';\n",
        "}\n",
    );
    let report = report_ops(fixture);
    let full = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        dead_spans(&report, &full),
        ["throw 'bad';"],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        verdict_spans(&report, &full),
        ["g is! Fun"],
        "{:?}",
        report.diagnostics
    );
}
