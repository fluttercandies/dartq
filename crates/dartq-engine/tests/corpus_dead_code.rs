//! Corpus slice: `dead_code` — code after a guaranteed exit point
//! (return/throw/break/continue) that can never execute. RED per
//! `conformance/status.tsv`; the slice pins the dead-code face burned by
//! ticket #4130 (W1 burn, `unused_finalization` + `scopes_bodies` remainder).
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
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_test_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_language_212_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTestCases_Language212/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_anonymous_methods_experiment_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest_AnonymousMethodsExperiment/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_language_219_slice() {
    corpus_common::run_family("diagnostics/dead_code/DeadCodeTest_Language219/");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_doc_slice() {
    corpus_common::run_family("diagnostics/dead_code/doc_");
}

#[test]
#[ignore = "red family: burn owned by #4130 (unused/lint-face layer)"]
fn dead_code_fix_slice() {
    corpus_common::run_family("diagnostics/dead_code/fix_");
}
