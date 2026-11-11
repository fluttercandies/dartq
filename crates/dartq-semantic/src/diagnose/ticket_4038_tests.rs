//! #4038 — a typed collection literal initializer binds the local's type
//! only where the literal ENDS the declarator.
//!
//! The declaration-site byte-scan (`typed_literal_initializer_ty`) read
//! `= <` and the literal's opening bracket and answered the written face
//! without looking past it: `final x = <int>[1].first` bound `List<int>`
//! though the initializer's value is the member access's `int`, so the
//! constant-true check verdict fired on the container face the oracle
//! never binds. The fix mirrors the end-of-declarator discipline the
//! diagnosing pass's token-stream face runs: after the bracket-balanced
//! close the next code must be `;`, `,`, or the cascade `..` — a
//! selector or operator continuation (`.first`, `as`) types a different
//! expression, so the declaration side keeps the unknown face and the
//! read-side initializer consult (#4013) answers what it can spell.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the shapes below — the
//! oracle types the whole-initializer expression (`x` and the grouped
//! `g2` are `int`, `z` is `Object`) and answers `unnecessary_type_check`
//! constant-true on `x is int` and `z is Object`; those whole-expression
//! spellings are beyond the byte-scan's grammar, so the negative pins
//! hold the under-report direction (silent) rather than the oracle's own
//! rows. The container face never leaks: the oracle's
//! `x is List<int>` / `z is List<int>` / `g2 is List<int>` are silent,
//! and so are ours.

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The written face binds where the literal ends the declarator — the
/// plain `;` form, the cascade `..` (whose static type is its target's),
/// and a comma group's earlier declarator — all three read `List<int>`.
#[test]
fn literal_ended_declarators_keep_the_written_face() {
    let report = report_ops(
        "final y = <int>[1];\n\
         final c = <int>[1]..add(2);\n\
         final g1 = <int>[1], g2 = <int>[1];\n\
         void main() {\n\
         \x20 print(y is List<int>);\n\
         \x20 print(c is List<int>);\n\
         \x20 print(g1 is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        3,
        "the `;`, cascade and comma-group declarators read the written \
         `List<int>` face; got {utc:?}"
    );
}

/// The negative control: a selector (`.first`) or operator (`as`)
/// continuation after the literal types a different expression — the
/// container face never binds those locals, including inside a comma
/// group. Before the fix each read answered constant-true on
/// `List<int>`.
#[test]
fn continuation_initializers_never_bind_the_container_face() {
    let report = report_ops(
        "final x = <int>[1].first;\n\
         final z = <int>[1] as Object;\n\
         final g1 = <int>[1], g2 = <int>[1].first;\n\
         void main() {\n\
         \x20 print(x is List<int>);\n\
         \x20 print(z is List<int>);\n\
         \x20 print(g2 is List<int>);\n\
         }\n",
    );
    assert!(
        texts(&report, "unnecessary_type_check").is_empty(),
        "the `.first` and `as` continuations type a different expression: \
         the container face never binds them, so no check reads \
         constant-true"
    );
}
