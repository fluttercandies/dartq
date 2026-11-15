//! Corpus slice: `abi_specific_integer_invalid` — the `AbiSpecificInteger`
//! subclass-shape family (`dart:ffi`). Green per `conformance/status.tsv`;
//! exercises the verbatim-options path (doc cases carry options WITHOUT
//! `enable-experiment`, so zero experiments) and a real `dart:` import.

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
fn abi_specific_integer_invalid_slice() {
    corpus_common::run_family("diagnostics/abi_specific_integer_invalid");
}
