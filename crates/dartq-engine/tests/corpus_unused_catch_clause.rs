//! Corpus slice: `unused_catch_clause` — a `catch`/`on` clause that can never
//! be reached or whose bound variable is unused. RED per
//! `conformance/status.tsv`; the slice pins the catch-slot cluster
//! (`scopes_bodies.rs` `reconcile_catch_slots` NEW-BREAK) burned by ticket
//! #4130 (W1 burn, unused/lint-face layer).

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
// Green since #4130's catch-slot reconciliation (`reconcile_catch_slots`).
fn unused_catch_clause_slice() {
    corpus_common::run_family("diagnostics/unused_catch_clause");
}
