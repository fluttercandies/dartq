//! Corpus slice: `unused_import` — an import directive whose imported
//! names are never referenced. RED per `conformance/status.tsv`; the slice
//! pins the unused/lint-face layer (`scopes_bodies.rs`) burned by ticket
//! #4130 (W1 burn, unused_finalization + scopes_bodies remainder).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_import_test_slice() {
    corpus_common::run_family("diagnostics/unused_import/UnusedImportTest/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_import_doc_slice() {
    corpus_common::run_family("diagnostics/unused_import/doc_");
}
