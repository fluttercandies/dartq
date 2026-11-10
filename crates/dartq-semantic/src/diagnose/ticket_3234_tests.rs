//! #3234 pins — the void-operand verdict of a binary expression was gated
//! on the operator mark, which only a `Read` left operand ever takes, so
//! every non-relation operator over an INVOKED operand stayed silent.
//!
//! Upstream `visitBinaryOperatorInvocation`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/generated/
//! error_verifier.dart:461-467) checks `node.leftOperand` for EVERY binary
//! operator — there is no spelling whitelist and no "mark" concept. The
//! engine located the left operand through `mark_operator`'s
//! `Intent::Operator` tag, which `begin_binary_expression` only applies to
//! `Read` rows; an invocation node's row is `Intent::Invoke`, so
//! `fold_binary` found no mark and bailed for every operator outside the
//! statically-`bool` set (`== != && || < > <= >=`) — the void check never
//! ran: `void v() {} void main() { v() + 1; }` emitted `[]`.
//!
//! The fold's operand locator now falls back to the last row wholly before
//! the operator for every spelling (`engine_core.rs` `fold_binary`), the
//! row data #3224 already writes (the invocation row carries the send's
//! static type). The pins below cover the non-relation operators, the
//! relation path that already worked, and the non-`void` operand that must
//! stay silent.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `tests.rs` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves — the same convention `ticket_3224_tests.rs` keeps).

use crate::diagnose::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

/// The message the oracle spells for every face (messages.yaml
/// `useOfVoidResult`).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// The offsets of one code's rows, ascending.
fn offsets(report: &crate::diagnose::types::FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The source offset of the fixture's LAST `needle`, as a report offset
/// (the rig prepends [`CORE_PRELUDE`]).
fn at_last(fixture: &str, needle: &str) -> u32 {
    u32::try_from(CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle")).expect("offset")
}

/// The reported face: `v() + 1` consumes the `void` result `v()` returns.
/// The oracle anchors a `MethodInvocation` operand at its method name
/// (`.at(expression)` / the invocation's own token, length 1) — byte-exact
/// here, since the invoked operand's row starts at `v`.
#[test]
fn ticket_3234_binary_plus_invocation_reports() {
    let fixture = "void v() {}\nvoid g() {\n  v() + 1;\n}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "v()")],
        "{:?}",
        rows(&report)
    );
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "use_of_void_result")
        .expect("void row");
    assert_eq!(diagnostic.message, VOID_MSG);
    assert_eq!(diagnostic.range.length, 1, "{:?}", rows(&report));
}

/// The other non-relation user operators take the same operand locator:
/// every spelling consumes the `void` result and reports at the name.
#[test]
fn ticket_3234_binary_arithmetic_invocation_reports() {
    for op in ["*", "-", "/", "%", "~/", "<<", "&", "|", "^"] {
        let fixture = format!("void v() {{}}\nvoid g() {{\n  v() {op} 1;\n}}");
        let report = report_ops(&fixture);
        assert_eq!(
            offsets(&report, "use_of_void_result"),
            [at_last(&fixture, "v()")],
            "`{op}`: {:?}",
            rows(&report)
        );
    }
}

/// The relation path already worked through `fold_binary`'s statically-
/// `bool` fallback — the widened locator must not disturb it.
#[test]
fn ticket_3234_binary_relation_invocation_still_reports() {
    let fixture = "void v() {}\nvoid g() {\n  v() == 1;\n}";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "use_of_void_result"),
        [at_last(fixture, "v()")],
        "{:?}",
        rows(&report)
    );
}

/// A non-`void` invoked operand stays clean: the fallback runs the void
/// verdict on the send's static type, not on the callee's function type,
/// and the fold's operand faces report nothing for `int + int`.
#[test]
fn ticket_3234_non_void_invocation_operand_stays_silent() {
    let fixture = "int f() { return 1; }\nvoid g() {\n  f() + 1;\n}";
    let report = report_ops(fixture);
    assert!(code_names(&report).is_empty(), "{:?}", rows(&report));
}
