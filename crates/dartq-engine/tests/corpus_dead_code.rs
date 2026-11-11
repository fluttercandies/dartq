//! Corpus slice: `dead_code` — code after a guaranteed exit point
//! (return/throw/break/continue) that can never execute. RED per
//! `conformance/status.tsv`; the slice pins the dead-code face burned by
//! ticket #4130 (W1 burn, unused_finalization + scopes_bodies remainder).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_test_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_language_212_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTestCases_Language212/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_anonymous_methods_experiment_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest_AnonymousMethodsExperiment/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_language_219_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest_Language219/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_doc_slice() {
    corpus_common::run_family("diagnostics/dead_code/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_fix_slice() {
    corpus_common::run_family("diagnostics/dead_code/fix_");
}
