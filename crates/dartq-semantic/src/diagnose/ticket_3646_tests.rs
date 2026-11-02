//! #3646 — the `undefined_function` extras on legitimate local functions.
//!
//! The pass-1 latch that pushes a local function's declaration row
//! (`pending_local_fn`) is single-slot: a nested local-function declaration
//! or a function expression inside the outer body clears it before
//! `finish_local_fn_site` runs, and the body's own self-references always
//! precede the declaration's end event. Both leave a `BareInfo::Missing`
//! row where upstream resolves the name — `resolution_visitor.dart`'s
//! `_defineLocalElements` binds EVERY block's local elements before the
//! statements visit, and `_visitStatementInScope` wraps a non-block
//! sub-statement in an implicit scope binding just itself. The pass-2 fix
//! rebuilds the visible regions from the token table and suppresses the
//! `undefined_function` arm's emission — the row stays a miss for the
//! model (no `used_names` credit, no local-use record, `record_miss` still
//! runs), matching the oracle's silent face while `unused_element` and the
//! `referenced_before_declaration` family keep their own verdicts.
//!
//! Pins: one per census group — self-recursion (block and non-block),
//! self-recursion through a nested closure, a sibling call after a
//! clobbered declaration, and a forward sibling reference — plus the
//! negative control (a genuinely absent callee still reports with the
//! oracle's message and span) and the forward-reference control (a local
//! variable read before its declaration is never rescued).

use super::*;
use crate::diagnose::test_rig::report_ops;

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.message.to_string())
        .collect()
}

/// A self-recursive local function in a block resolves its own name — the
/// oracle reports only the declaration's `unused_element` verdict (its
/// self-call never credits a use).
#[test]
fn recursive_local_block_scope() {
    let report = report_ops(
        "void f() {\n\
         \x20 g() { g(); }\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(extras.is_empty(), "self-recursion resolves: {extras:?}");
}

/// The same shape inside a non-block `if` body — the statement's implicit
/// scope still binds the declaration into its own body (upstream
/// `_visitStatementInScope`), so the self-call resolves and only
/// `unused_element` remains.
#[test]
fn recursive_local_if_statement_non_block() {
    let report = report_ops(
        "void f(bool c) {\n\
         \x20 if (c) g() { g(); }\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "non-block self-scope resolves: {extras:?}"
    );
    let unused = messages_named(&report, "unused_element");
    assert_eq!(unused.len(), 1, "the decl keeps its verdict: {unused:?}");
}

/// A self-call through a nested closure (`exec_self_name`'s
/// `local_fn_stack` answer covers it) — `self_scope_rows` latches the
/// offset at push, no token-extent probe needed.
#[test]
fn recursive_local_through_closure() {
    let report = report_ops(
        "void f() {\n\
         \x20 fun() { (() { fun(); })(); }\n\
         \x20 fun();\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "closure-nested self-call resolves: {extras:?}"
    );
}

/// The `pending_local_fn` clobber family: a nested declaration inside a
/// local function's body wipes the outer's pending entry, so the outer's
/// declaration row never lands — yet the name stays visible for the whole
/// enclosing block upstream, and the post-declaration sibling call
/// resolves.
#[test]
fn sibling_call_after_nested_decl() {
    let report = report_ops(
        "void test() {\n\
         \x20 f() { g() {} }\n\
         \x20 f();\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "clobbered sibling call resolves: {extras:?}"
    );
    let unused_g = messages_named(&report, "unused_element");
    assert!(
        unused_g.iter().any(|m| m.contains("'g'")),
        "the nested decl keeps its verdict: {unused_g:?}"
    );
}

/// The same clobber through a function EXPRESSION (a lambda body): the
/// `begin_function_expression` listener clears the latch identically, so
/// the outer `f`'s call resolves through the token-scanned block extent.
#[test]
fn sibling_call_after_nested_lambda() {
    let report = report_ops(
        "void test() {\n\
         \x20 f() { var h = () {}; }\n\
         \x20 f();\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "lambda-clobbered sibling call resolves: {extras:?}"
    );
}

/// A forward sibling reference resolves at scope level — upstream binds
/// the block's local functions before visiting statements, and the oracle
/// reports `referenced_before_declaration` (the `HiddenElements` face),
/// never `undefined_function`. The suppressed miss leaves the
/// forward-reference family's own verdict untouched.
#[test]
fn forward_sibling_reference() {
    let report = report_ops(
        "void test() {\n\
         \x20 f8() => f9();\n\
         \x20 f9() => 0;\n\
         \x20 f8();\n\
         }\n",
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "forward sibling ref is not undefined_function: {extras:?}"
    );
}

/// A genuinely absent callee keeps the oracle's exact row: the code, the
/// message, and the span all match `conformance/expected.tsv`
/// (`MethodInvocationResolutionTest/test_error_undefinedFunction#0`,
/// `3:3|3` over `foo();`).
#[test]
fn genuine_undefined_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 foo();\n\
         }\n",
    );
    let undefined = messages_named(&report, "undefined_function");
    assert_eq!(undefined.len(), 1, "one miss reports: {undefined:?}");
    assert_eq!(undefined[0], "The function 'foo' isn't defined.");
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_function")
        .expect("the miss row exists");
    // `report_ops` prepends `CORE_PRELUDE` — the span is relative to the
    // padded source, so only the length is pinned against the oracle's
    // `|3|`; the message/code carry the naming.
    assert_eq!(row.range.length, 3, "the callee's name span");
}

/// The forward-reference control: a local VARIABLE read before its
/// declaration must still report — the local-fn scope fix never rescues a
/// name whose declaration is not a function (`var x` scans no extent), so
/// the read keeps whatever verdict its own family reports. The pin locks
/// the observable contract — the row is not silently resolved — without
/// fixing which arm owns it (`referenced_before_declaration` vs the
/// miss family's own face).
#[test]
fn local_variable_forward_reference_unrescued() {
    let report = report_ops(
        "void f() {\n\
         \x20 var a = x;\n\
         \x20 var x = 0;\n\
         }\n",
    );
    assert!(
        !report.diagnostics.is_empty(),
        "the variable forward reference reports: {:?}",
        codes(&report)
    );
    let extras = messages_named(&report, "undefined_function");
    assert!(
        extras.is_empty(),
        "no undefined_function may mint on `x`: {extras:?}"
    );
}
