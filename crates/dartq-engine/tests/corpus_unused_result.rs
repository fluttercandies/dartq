//! Corpus slice: `unused_result` — a method/function call whose result is
//! never used. RED per `conformance/status.tsv`; the slice pins the
//! unused/lint-face layer (`scopes_bodies.rs`) burned by ticket #4130 (W1
//! burn, unused_finalization + scopes_bodies remainder).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_result_test_slice() {
    corpus_common::run_family("diagnostics/unused_result/UnusedResultTest/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_result_doc_slice() {
    corpus_common::run_family("diagnostics/unused_result/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn unused_result_fix_slice() {
    corpus_common::run_family("diagnostics/unused_result/fix_");
}
