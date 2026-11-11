//! Corpus slice: `type_test_with_undefined_name` — an `is`/`as` type test
//! whose type name cannot be resolved. RED per `conformance/status.tsv`;
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
fn type_test_with_undefined_name_test_slice() {
    corpus_common::run_family(
        "diagnostics/type_test_with_undefined_name/TypeTestWithUndefinedNameTest/",
    );
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn type_test_with_undefined_name_doc_slice() {
    corpus_common::run_family("diagnostics/type_test_with_undefined_name/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn type_test_with_undefined_name_fix_slice() {
    corpus_common::run_family("diagnostics/type_test_with_undefined_name/fix_");
}
