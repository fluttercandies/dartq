//! Corpus slice: `return_of_invalid_type` — a `return` whose expression type
//! is not assignable to the enclosing function's return type. RED per
//! `conformance/status.tsv`; the slice pins the bare-creation static-type rows
//! in return windows burned by ticket #4131 (W3 residual A, `creation_args.rs`
//! + `row_resolution.rs`).

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
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn return_of_invalid_type_slice() {
    corpus_common::run_family("diagnostics/return_of_invalid_type");
}
