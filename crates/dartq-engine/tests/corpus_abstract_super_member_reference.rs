//! Corpus slice: `abstract_super_member_reference` — the `super.` member
//! access against an always-abstract supertype member. RED per
//! `conformance/status.tsv` (8 of 29 attributed cases fail; the slice's own
//! 20 cases show the same emission gaps the census reports).
//!
//! `#[ignore]`d: the wiring is the deliverable, the redness belongs to the
//! name/member-resolution burn wave — ticket #4119 (R781, `resolution.rs` +
//! member faces). Run explicitly with
//! `cargo test -p dartq-engine --test corpus_abstract_super_member_reference -- --ignored --exact`;
//! a green result here means the wave burned the family.

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
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn abstract_super_member_reference_slice() {
    corpus_common::run_family("diagnostics/abstract_super_member_reference");
}
