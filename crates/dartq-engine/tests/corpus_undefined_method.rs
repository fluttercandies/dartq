//! Corpus slice: `undefined_method` — a method invocation on a receiver
//! that does not define it. RED per `conformance/status.tsv`; the slice
//! pins the name/member-resolution face burned by ticket #4119 (W2
//! name/member wave).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_method_test_slice() {
    corpus_common::run_family("diagnostics/undefined_method/UndefinedMethodTest/");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_method_doc_slice() {
    corpus_common::run_family("diagnostics/undefined_method/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_method_fix_slice() {
    corpus_common::run_family("diagnostics/undefined_method/fix_");
}
