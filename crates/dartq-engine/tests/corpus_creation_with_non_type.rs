//! Corpus slice: `creation_with_non_type` — an instance creation (`new`/
//! implicit) whose named type does not resolve to a class. RED per
//! `conformance/status.tsv`; the slice pins the bare-creation static-type
//! surface (the creation row's resolved type feeding assignability/return
//! windows) burned by ticket #4131 (W3 residual A, `creation_args.rs` +
//! `row_resolution.rs`).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced into
//! one `#[test]` per extracted `TestClass` subdir plus the flat `doc_`/`fix_`
//! cases; every id prefix is covered exactly once (zero silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn creation_with_non_type_test_slice() {
    corpus_common::run_family("diagnostics/creation_with_non_type/CreationWithNonTypeTest/");
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn creation_with_non_type_doc_slice() {
    corpus_common::run_family("diagnostics/creation_with_non_type/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn creation_with_non_type_fix_slice() {
    corpus_common::run_family("diagnostics/creation_with_non_type/fix_");
}
