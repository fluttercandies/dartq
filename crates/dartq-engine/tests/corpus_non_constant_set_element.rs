//! Corpus slice: `non_constant_set_element` — an element of a const set (or
//! map) literal that is not a constant. RED per `conformance/status.tsv`;
//! the slice pins the const-eval faces (deferred if-elements, the spread's
//! SetOrMap classification) burned by ticket #4339.
//!
//! The family exceeds the ~60-case single-fn budget, so it is sub-sliced
//! into one `#[test]` per extracted `TestClass` subdir plus the flat
//! `doc_`/`fix_` cases; every id prefix is covered exactly once (zero
//! silent drops).

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
#[ignore = "red family: burn owned by #4339 (deferred if-element + spread SetOrMap faces)"]
fn non_constant_set_element_test_slice() {
    corpus_common::run_family("diagnostics/non_constant_set_element/NonConstantSetElementTest/");
}

#[test]
#[ignore = "red family: burn owned by #4339 (deferred if-element + spread SetOrMap faces)"]
fn non_constant_set_element_doc_slice() {
    corpus_common::run_family("diagnostics/non_constant_set_element/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4339 (deferred if-element + spread SetOrMap faces)"]
fn non_constant_set_element_fix_slice() {
    corpus_common::run_family("diagnostics/non_constant_set_element/fix_");
}
