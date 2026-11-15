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

#![allow(
    clippy::useless_conversion,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::semicolon_if_nothing_returned
)]
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
