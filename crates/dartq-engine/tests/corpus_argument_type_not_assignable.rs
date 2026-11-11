//! Corpus slice: `argument_type_not_assignable` — an argument whose static
//! type is not assignable to the parameter type. RED per
//! `conformance/status.tsv`; the slice pins the assignability surface burned
//! by ticket #4132 (the highest-value drop named by wave 1).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4132 (assignability surface)"]
fn argument_type_not_assignable_test_slice() {
    corpus_common::run_family(
        "diagnostics/argument_type_not_assignable/ArgumentTypeNotAssignableTest/",
    );
}

#[test]
#[ignore = "red family: burn owned by #4132 (assignability surface)"]
fn argument_type_not_assignable_strict_casts_slice() {
    corpus_common::run_family(
        "diagnostics/argument_type_not_assignable/ArgumentTypeNotAssignableWithStrictCastsTest/",
    );
}

#[test]
#[ignore = "red family: burn owned by #4132 (assignability surface)"]
fn argument_type_not_assignable_doc_slice() {
    corpus_common::run_family("diagnostics/argument_type_not_assignable/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4132 (assignability surface)"]
fn argument_type_not_assignable_fix_slice() {
    corpus_common::run_family("diagnostics/argument_type_not_assignable/fix_");
}
