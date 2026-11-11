//! Corpus slice: `type_argument_not_matching_bounds` — a type argument that
//! does not satisfy the corresponding type parameter's bound. RED per
//! `conformance/status.tsv`; the slice pins the creation-inference bound rows
//! (`bounds.rs` `check_creation_inference` type-formals-to-bounds closure)
//! burned by ticket #4132 (W3 residual B, cases 0171/0258 and the bound-row
//! cluster).
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced into
//! one `#[test]` per extracted `TestClass` subdir plus the flat `doc_`/`fix_`
//! cases; every id prefix is covered exactly once (zero silent drops).

mod corpus_common;

#[test]
#[ignore = "red: 5 TestClass cases remain (type-literal, function-alias, and redirecting-constructor bound faces); the creation-inference rows are green"]
fn type_argument_not_matching_bounds_test_slice() {
    corpus_common::run_family(
        "diagnostics/type_argument_not_matching_bounds/TypeArgumentNotMatchingBoundsTest/",
    );
}

#[test]
fn type_argument_not_matching_bounds_doc_slice() {
    corpus_common::run_family("diagnostics/type_argument_not_matching_bounds/doc_");
}

#[test]
fn type_argument_not_matching_bounds_fix_slice() {
    corpus_common::run_family("diagnostics/type_argument_not_matching_bounds/fix_");
}
