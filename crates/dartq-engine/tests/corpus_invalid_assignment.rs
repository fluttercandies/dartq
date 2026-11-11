//! Corpus slice: `invalid_assignment` — an assignment whose RHS type is not
//! assignable to the LHS type (including the bare-creation static-type rows in
//! init/assignment windows). RED per `conformance/status.tsv`; the slice pins
//! case 0012 (`initializer_bare_creation_reports`) and the assignment-window
//! receiver faces burned by ticket #4131 (W3 residual A, `creation_args.rs` +
//! `row_resolution.rs`).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced into
//! one `#[test]` per extracted `TestClass` subdir plus the flat `doc_`/`fix_`
//! cases; every id prefix is covered exactly once (zero silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn invalid_assignment_test_slice() {
    corpus_common::run_family("diagnostics/invalid_assignment/InvalidAssignmentTest/");
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn invalid_assignment_implicit_call_reference_slice() {
    corpus_common::run_family(
        "diagnostics/invalid_assignment/InvalidAssignment_ImplicitCallReferenceTest/",
    );
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn invalid_assignment_strict_casts_slice() {
    corpus_common::run_family(
        "diagnostics/invalid_assignment/InvalidAssignmentWithStrictCastsTest/",
    );
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn invalid_assignment_doc_slice() {
    corpus_common::run_family("diagnostics/invalid_assignment/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn invalid_assignment_fix_slice() {
    corpus_common::run_family("diagnostics/invalid_assignment/fix_");
}
