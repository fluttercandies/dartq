//! Corpus slice: `label_undefined` — a `break`/`continue` label that is
//! not defined in the enclosing scope. RED per `conformance/status.tsv`;
//! the slice pins the name-resolution face burned by ticket #4119 (W2
//! name/member wave).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn label_undefined_test_slice() {
    corpus_common::run_family("diagnostics/label_undefined/LabelUndefinedTest/");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn label_undefined_doc_slice() {
    corpus_common::run_family("diagnostics/label_undefined/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn label_undefined_fix_slice() {
    corpus_common::run_family("diagnostics/label_undefined/fix_");
}
