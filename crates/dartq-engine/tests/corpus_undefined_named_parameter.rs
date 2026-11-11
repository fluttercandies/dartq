//! Corpus slice: `undefined_named_parameter` — a named argument whose
//! name does not match any parameter of the invoked function. RED per
//! `conformance/status.tsv`; the slice pins the name/member-resolution
//! face burned by ticket #4119 (W2 name/member wave).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_named_parameter_test_slice() {
    corpus_common::run_family("diagnostics/undefined_named_parameter/UndefinedNamedParameterTest/");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_named_parameter_doc_slice() {
    corpus_common::run_family("diagnostics/undefined_named_parameter/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_named_parameter_fix_slice() {
    corpus_common::run_family("diagnostics/undefined_named_parameter/fix_");
}
