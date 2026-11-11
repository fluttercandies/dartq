//! #3396 pins — the callee-own-parameter closure formal's member-miss
//! suppression (`inferred_formal_containers`) only covers a container the
//! local member walk cannot see; a same-file or sibling-graph class answers
//! real `undefined_*` rows the oracle reports.
//!
//! Upstream `_inferFormalParameters` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/function_expression_resolver.dart`)
//! runs after the non-literal arguments' inference stage: a closure
//! formal's downward-inferred type closes against the call's solved
//! type-inference regardless of the literal's position in the argument
//! list (`b.reduce(Ann(), (a) => …)` binds `a := Ann` exactly as
//! `b.reduce((a) => …, Ann())` does). A member access on the bound formal
//! is then an ordinary property/invocation check — the oracle resolves it
//! through the same member machinery as any other receiver.
//!
//! The suppression exists because OUR member walk under-resolves the
//! extension/import members an inference-bound receiver can carry
//! (manifest_item.dart's `items.write` family): for a container whose
//! member table is genuinely out of reach the `undefined_*` rows stay
//! silent. The pre-#3396 insert suppressed unconditionally, which also
//! swallowed the real `undefined_method` row a same-file class produces
//! (`a.noSuchEver()` against a file-local `Ann`). The insert is now gated
//! on `resolve_container` missing the class — an in-scope class keeps its
//! honest verdict.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — `UNDEFINED_METHOD … 'noSuchEver' isn't defined for
//! the type 'Ann'`, `… 'nope' isn't defined for the type 'A'`.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

/// The `undefined_method` rows the report carries, as `(code, message)`
/// pairs for the assertion face.
fn undefined_methods(report: &FileReport) -> Vec<(&'static str, &str)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .map(|d| (d.code.name, d.message.as_str()))
        .collect()
}

/// The member-send closure-LAST case: a method-parameter formal's member
/// miss reports the solved class, not a silent face. Red before #3396 —
/// the unconditional `inferred_formal_containers` insert masked the
/// miss. Oracle: `UNDEFINED_METHOD|…|5|35|10|The method 'noSuchEver' isn't
/// defined for the type 'Ann'.`
#[test]
fn member_send_closure_last_reports_undefined_on_solved_formal() {
    let report = report_ops(
        "class Box<T> { T reduce<S>(S base, S Function(S a) f) => f(base); }\n\
         class Ann { void z() {} }\n\
         void main() {\n\
         Box<int> b = Box<int>();\n\
         b.reduce(Ann(), (a) { a.noSuchEver(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].1,
        "The method 'noSuchEver' isn't defined for the type 'Ann'.",
        "{:#?}",
        rows(&report)
    );
}

/// The #2324 shape: a parameter-receiver method-parameter solve
/// (`bw.take<T>`) reports the same-file class's miss. The oracle's
/// `UNDEFINED_METHOD 'A'` was previously swallowed by the blanket
/// suppression the ticket's "same-file misspell shares that silence" note
/// described; that note predates the oracle check, which reports the row.
/// Oracle: `UNDEFINED_METHOD|…|4|32|4|The method 'nope' isn't defined for
/// the type 'A'.`
#[test]
fn parameter_receiver_solve_reports_undefined_on_same_file_class() {
    let report = report_ops(
        "class BW { void take<T>(T? o, void Function(T x) w) {} }\n\
         class A { void ok() {} }\n\
         void f(BW bw) {\n\
         bw.take(A(), (a) { a.ok(); a.nope(); });\n\
         }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].1,
        "The method 'nope' isn't defined for the type 'A'.",
        "{:#?}",
        rows(&report)
    );
}

/// Regression — a defined member on the inference-bound receiver stays a
/// resolved use: `a.ok()` against the same-file `Ann` reports no
/// `undefined_*` row (the gate only drops the suppression, it does not
/// invent verdicts).
#[test]
fn member_send_defined_member_on_solved_formal_stays_silent() {
    let report = report_ops(
        "class Box<T> { T reduce<S>(S base, S Function(S a) f) => f(base); }\n\
         class Ann { void z() {} }\n\
         void main() {\n\
         Box<int> b = Box<int>();\n\
         b.reduce(Ann(), (a) { a.z(); });\n\
         }\n",
    );
    assert!(
        undefined_methods(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// Regression — the bare free-function path was never suppressed
/// (`callee_own_head` requires `Receiver::Typed`): `g(Ann(), (a) => …)`
/// keeps reporting `undefined_method 'Ann'` after the gate.
#[test]
fn free_function_closure_formal_still_reports_undefined() {
    let report = report_ops(
        "T g<T>(T base, T Function(T a) f) => f(base);\n\
         class Ann { void z() {} }\n\
         void main() { g(Ann(), (a) { a.noSuchEver(); }); }\n",
    );
    let hits = undefined_methods(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].1,
        "The method 'noSuchEver' isn't defined for the type 'Ann'.",
        "{:#?}",
        rows(&report)
    );
}
