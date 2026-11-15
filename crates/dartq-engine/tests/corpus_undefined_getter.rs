//! Corpus slice: `undefined_getter` — a getter/member access on a receiver
//! whose member cannot be resolved. RED per `conformance/status.tsv`; the
//! slice pins the `row_resolution` windows (`values.first` getter after the
//! extension decision, assignment-window receiver declared face) burned by
//! ticket #4131 (cases 0113/0228/0248/0249). The member-resolution face of the
//! same code is also in scope for ticket #4119 (W2 name/member wave).

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
#[ignore = "red family: burn owned by #4131 (W3 residual A row_resolution windows); member face shared with #4119"]
fn undefined_getter_slice() {
    corpus_common::run_family("diagnostics/undefined_getter");
}
