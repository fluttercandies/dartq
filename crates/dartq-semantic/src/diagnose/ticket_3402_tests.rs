//! The #3402 pins — a call site's static type is the send's RESULT,
//! resolved through the callee's declared return, never the callee face
//! the invocation row stores.
//!
//! Oracle anchor (`dart analyze` 3.13.7, probe `/tmp/dq3402/a.dart`):
//! `typedef F<T> = T Function(); F<int> mk();` — `var f = mk()` types `f`
//! `F<int>` (`int Function()`), so `f()` answers `int`: assigning it to
//! `String` reports `invalid_assignment` 'int' can't be assigned to
//! 'String'. The pre-fix row kept the callee's stored face — it reported
//! `invalid_assignment` 'T' (the alias's own parameter spelling) at the
//! assignment, a row the real oracle never emits.
//!
//! Runs under the core-preloaded rig (`report_ops_bare` over
//! [`CORE_PRELUDE`]): the fixtures name `Function`, `int`, `String`,
//! `bool` — core spellings the bare rig cannot member-resolve.
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use super::test_rig::{CORE_PRELUDE, report_ops_bare};
use super::*;

/// `(code, offset, written text at the row)` — the span-exact projection:
/// code + offset + text pin a row's code, anchor, and length together.
/// The vector is sorted by `(code, offset, text)`, so expected rows are
/// listed in code-name order.
fn oracle_rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, u32, &'a str)> {
    let mut rows: Vec<(&'static str, u32, &'a str)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// `f()` over a local typed by a parameterized function-type alias
/// answers the substituted signature's return: `int`, so the `String`
/// assignment reports `invalid_assignment` at `f()` — the oracle's own
/// row, where the pre-fix face reported the free `T`.
#[test]
fn alias_typed_local_invoke_reports_the_substituted_return() {
    let fixture = "typedef F<T> = T Function();\n\
                   F<int> mk() => () => 0;\n\
                   void g() {\n\
                   \x20 var f = mk();\n\
                   \x20 String s = f();\n\
                   }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let send = source.rfind("f()").expect("fixture has the f() send") as u32;
    // `f` is read by the send so only `s` reports unused.
    let local = source.find(" s =").map(|i| i as u32 + 1).expect("fixture has s");
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("invalid_assignment", send, "f()"),
            ("unused_local_variable", local, "s"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// The send result feeds the NEXT send transitively: `mk()` answers
/// `F<int>` (`int Function()`), and invoking that result (`mk()()`)
/// answers `int` — assigning it to `String` reports `invalid_assignment`,
/// exactly as the oracle reports.
///
/// `#[ignore]`d: the engine's nested-send row ranges the callee head
/// `mk()` (span 4) where the oracle's `invalid_assignment` spans the
/// whole outer send `mk()()` (span 6) — a row-RANGE shape gap in the
/// nested-send model, not a type answer (the engine's 'int' verdict is
/// the oracle's). Routing: the nested invoke row's range, owned by the
/// listener's send close.
#[test]
#[ignore = "nested-send row range covers the callee head, not the whole send"]
fn invoke_result_feeds_the_next_send() {
    let fixture = "typedef F<T> = T Function();\n\
                   F<int> mk() => () => 0;\n\
                   void g() {\n\
                   \x20 String s = mk()();\n\
                   }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let send = source.find("mk()()").expect("fixture has the send") as u32;
    // `s` is declared and never read — the rig pins the warning beside the
    // error, exactly as the oracle reports both.
    let local = source.find(" s =").map(|i| i as u32 + 1).expect("fixture has s");
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("invalid_assignment", send, "mk()()"),
            ("unused_local_variable", local, "s"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}
