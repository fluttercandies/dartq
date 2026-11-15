//! #4287 pins: member-send result on locally-typed receivers.
//!
//! `consume_send` computes `send_result = Unknown` for member sends on
//! locally-typed receivers — `C c = ...; ...c.m()` where `m` has a nullable
//! declared return: the oracle emits a spread row, dartq stays silent.
//! #4282's fix records nothing when upstream can't decide (correct posture),
//! and one pin test is `#[ignore]`d at `ticket_4282_tests.rs:129` awaiting
//! this ticket.
//!
//! The fix wires receiver-local-declared-type → member signature → return
//! type so it flows through #4282's `send_results` table.
//!
//! Receipts: dart analyze 3.13.7 (default PATH dart), machine format, on
//! the probe family `/tmp/dq-4287/probe/lib/p1.dart`:
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

#![allow(clippy::semicolon_if_nothing_returned)]
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
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value" && d.message == SPREAD)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn row(offset: u32, length: u32) -> (u32, u32, String) {
    (offset, length, SPREAD.to_string())
}

/// A method call's nullable return on a locally-typed receiver (oracle
/// line 20: `o.m()`, length 5). The member send's static type must flow
/// through `send_results` so the spread operand's callee-keyed leg
/// answers `List<int>?`.
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

/// A method call's non-nullable return on a locally-typed receiver stays
/// silent (oracle line 21).
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

/// A method call's nullable return on a local VARIABLE receiver (not a
/// parameter) — the receiver's type comes from the local declaration.
#[test]
fn local_variable_receiver_nullable_return_in_spread_reports() {
    let fixture = "class C {\n\
                   List<int>? m() => null;\n\
                 }\n\
                 void h() {\n\
                   C c = C();\n\
                   var d = <int>[...c.m()];\n\
                   print(d);\n\
                 }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...c.m()]", "c.m()"), 5)]
    );
}

/// A generic method's nullable return on a locally-typed receiver.
#[test]
fn generic_method_nullable_return_in_spread_reports() {
    let fixture = "class C {\n\
                   List<T>? m<T>() => null;\n\
                 }\n\
                 void h(C o) {\n\
                   var d = <int>[...o.m<int>()];\n\
                   print(d);\n\
                 }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...o.m<int>()]", "o.m<int>()"), 10)]
    );
}

/// A Future-returning method's nullable face on a locally-typed receiver.
#[test]
fn future_method_nullable_return_in_spread_reports() {
    let fixture = "class C {\n\
                   Future<List<int>?> m() => Future.value(null);\n\
                 }\n\
                 void h(C o) {\n\
                   var d = <int>[...o.m()];\n\
                   print(d);\n\
                 }\n";
    let report = report_ops(fixture);
    // `Future<List<int>?>` is not nullable itself — the spread operand
    // is the Future, not the List. The oracle stays silent here.
    assert_eq!(spread_rows(&report), Vec::new());
}

/// A bare generic function's nullable return in spread — the
/// `send_results` key must match the token before `(` even when a
/// type-argument list intervenes.
#[test]
fn bare_generic_function_nullable_return_in_spread_reports() {
    let fixture = "List<T>? g<T>() => null;\n\
                   void f() {\n\
                     var a = <int>[...g<int>()];\n\
                     print(a);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        spread_rows(&report),
        vec![row(at(fixture, "<int>[...g<int>()]", "g<int>()"), 8)]
    );
}

/// A nullable method return in plain assignment (not spread) — the
/// member send's static type must still flow for assignability checks.
#[test]
fn method_call_nullable_return_in_assignment() {
    let fixture = "class C {\n\
                   List<int>? m() => null;\n\
                 }\n\
                 void h(C o) {\n\
                   List<int> x = o.m()!;\n\
                   print(x);\n\
                 }\n";
    let report = report_ops(fixture);
    // `o.m()!` asserts non-null — the assignment is clean.
    assert_eq!(spread_rows(&report), Vec::new());
}
