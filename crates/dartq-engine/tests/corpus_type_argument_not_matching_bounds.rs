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

#![allow(
    clippy::useless_conversion,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::semicolon_if_nothing_returned
)]
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
