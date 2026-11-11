//! Corpus slice: `non_type_in_catch_clause` — a `catch`/`on` clause whose
//! type name is not a type. RED per `conformance/status.tsv`; the slice
//! pins the name-resolution face burned by ticket #4119 (W2 name/member
//! wave).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn non_type_in_catch_clause_test_slice() {
    corpus_common::run_family("diagnostics/non_type_in_catch_clause/NonTypeInCatchClauseTest/");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn non_type_in_catch_clause_doc_slice() {
    corpus_common::run_family("diagnostics/non_type_in_catch_clause/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn non_type_in_catch_clause_fix_slice() {
    corpus_common::run_family("diagnostics/non_type_in_catch_clause/fix_");
}
