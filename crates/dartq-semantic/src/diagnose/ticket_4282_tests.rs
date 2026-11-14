//! #4282 pins: the spread operand's CALLEE-KEYED leg answers a bare
//! callee's call result.
//!
//! `...g()` over a bare top-level function whose declared return is
//! nullable missed the oracle's row before #4279's group-face fix and kept
//! missing it after it: the argument-list exclusion rightly refuses the
//! group leg (`g`'s `(` opens a call's arguments, not an expression
//! group), and the callee-keyed leg — the completed-send swap
//! (`Engine::invoked_receiver_face` → `callable_result_of`) — only knew
//! `Ty::Function` faces. A bare top-level function's flow read stages its
//! own spelling (`Ty::Named { name: g }`, the tear-off face the `refs`
//! ledger carries), so `callable_result_of` declined and the operand
//! grounded as a non-nullable named type.
//!
//! The fix records the completed send's static type BY CALLEE OFFSET
//! (`Engine::send_results`, populated at `consume_send`'s write-back)
//! and consults it in the operand ladder's callee leg
//! (`Engine::operand_expression_ty` step 3). The row stack is drained
//! before the teardown operand scan, so the offset-keyed twin is the
//! only surviving witness of the invocation node's type.
//!
//! Held: `...o.m()` (a member send) still misses — `consume_send`
//! computes `Unknown` for the member's return type (a separate
//! member-resolution gap, not the operand ladder's), so `send_results`
//! records nothing. The bare-callee and function-typed-variable arms
//! below pin the faces this lane fixes.
//!
//! Receipts: dart analyze 3.13.7 (default PATH dart), machine format, on
//! the probe family `/tmp/dq-4282/probe/lib/p1.dart`:
//!
//! ```text
//! ERROR|COMPILE_TIME_ERROR|UNCHECKED_USE_OF_NULLABLE_VALUE|p1.dart|11|20|3|A nullable expression can't be used in a spread.   // ...g()
//! ERROR|...|13|20|5|...                                                                                                    // ...g1(1)
//! ERROR|...|20|20|5|...                                                                                                    // ...o.m()
//! ERROR|...|22|21|3|...                                                                                                    // ...t()
//! ```
//!
//! Every fixture below reproduces one row (or the oracle's silence for
//! the non-nullable returns); the offsets are the oracle's own
//! `.at(node.expression)` anchors.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

const SPREAD: &str = "A nullable expression can't be used in a spread.";

/// The prelude shifts every offset; the fixtures below are ASCII.
fn prelude_len() -> u32 {
    CORE_PRELUDE.chars().map(char::len_utf16).sum::<usize>() as u32
}

/// The absolute UTF-16 offset of `needle`'s first occurrence inside
/// `context` inside the fixture.
fn at(fixture: &str, context: &str, needle: &str) -> u32 {
    let context_start = fixture.find(context).expect("context is in the fixture");
    let within = context.find(needle).expect("needle is in the context");
    let upto = context_start + within;
    prelude_len() + fixture[..upto].chars().map(char::len_utf16).sum::<usize>() as u32
}

/// Every spread-operand row as `(offset, length, message)`.
fn spread_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unchecked_use_of_nullable_value" && d.message.to_string() == SPREAD
        })
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn row(offset: u32, length: u32) -> (u32, u32, String) {
    (offset, length, SPREAD.to_string())
}

/// A bare top-level function's nullable return: the row anchors at the
/// whole invocation `g()` (oracle line 11: offset of `g`, length 3).
#[test]
fn bare_top_level_nullable_return_in_spread_reports() {
    let fixture = "List<int>? g() => null;\n\
                   void f() {\n\
                     var a = <int>[...g()];\n\
                     print(a);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...g()]", "g()"), 3)]
    );
}

/// A bare top-level function's non-nullable return stays silent (oracle
/// line 12's `...g2()` emits nothing).
#[test]
fn bare_top_level_non_nullable_return_in_spread_stays_silent() {
    let fixture = "List<int> g2() => <int>[];\n\
                   void f() {\n\
                     var b = <int>[...g2()];\n\
                     print(b);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(spread_rows(&report), Vec::new());
}

/// The same bare callee with a positional argument (the argument-list
/// exclusion's non-empty form; oracle line 13: `g1(1)`, length 5).
#[test]
fn bare_top_level_nullable_return_with_argument_reports() {
    let fixture = "List<int>? g1(int x) => null;\n\
                   void f() {\n\
                     var c = <int>[...g1(1)];\n\
                     print(c);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...g1(1)]", "g1(1)"), 5)]
    );
}

/// A method call's nullable return (oracle line 20: `o.m()`, length 5).
/// #4287 wires the receiver-local-declared-type → member signature →
/// return type path so `consume_send` answers `List<int>?` and
/// `send_results` records it for the operand ladder's callee-keyed leg.
#[test]
fn method_call_nullable_return_in_spread_reports() {
    let fixture = "class C {\n\
                   List<int>? m() => null;\n\
                 }\n\
                 void h(C o) {\n\
                   var d = <int>[...o.m()];\n\
                   print(d);\n\
                 }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...o.m()]", "o.m()"), 5)]
    );
}

/// A method call's non-nullable return stays silent (oracle line 21).
#[test]
fn method_call_non_nullable_return_in_spread_stays_silent() {
    let fixture = "class C {\n\
                   List<int> mm() => <int>[];\n\
                 }\n\
                 void h(C o) {\n\
                   var e = <int>[...o.mm()];\n\
                   print(e);\n\
                 }\n";
    let report = report_ops(fixture);
    assert_eq!(spread_rows(&report), Vec::new());
}

/// Invoking a function-typed PARAMETER whose return is nullable (oracle
/// line 22: `t()`, length 3).
#[test]
fn function_typed_variable_nullable_return_in_spread_reports() {
    let fixture = "void h(List<int>? Function() t) {\n\
                     var f1 = <int>[...t()];\n\
                     print(f1);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...t()]", "t()"), 3)]
    );
}

/// Invoking a function-typed parameter whose return is non-nullable stays
/// silent (oracle line 23's `...t2()`).
#[test]
fn function_typed_variable_non_nullable_return_in_spread_stays_silent() {
    let fixture = "void h(List<int> Function() t2) {\n\
                     var f2 = <int>[...t2()];\n\
                     print(f2);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(spread_rows(&report), Vec::new());
}

/// A null-aware spread (`...?`) keeps the oracle's silence even over a
/// nullable bare-call result (`!node.isNullAware`'s gate).
#[test]
fn null_aware_spread_of_nullable_call_stays_silent() {
    let fixture = "List<int>? g() => null;\n\
                   void f() {\n\
                     var a = <int>[...?g()];\n\
                     print(a);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(spread_rows(&report), Vec::new());
}

/// A local variable's nullable read is the already-correct control: the
/// flow row grounds the operand and the row anchors at the bare name.
#[test]
fn nullable_local_read_in_spread_reports() {
    let fixture = "void f(List<int>? xs) {\n\
                     var a = <int>[...xs];\n\
                     print(a);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...xs]", "xs"), 2)]
    );
}
