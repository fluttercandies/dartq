//! Corpus slice: `implements_non_class` — a class that implements a
//! non-class type. RED per `conformance/status.tsv`; the slice pins the
//! name/member-resolution face burned by ticket #4119 (W2 name/member
//! wave).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn implements_non_class_test_slice() {
    corpus_common::run_family("diagnostics/implements_non_class/ImplementsNonClassTest/");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn implements_non_class_doc_slice() {
    corpus_common::run_family("diagnostics/implements_non_class/doc_");
}
