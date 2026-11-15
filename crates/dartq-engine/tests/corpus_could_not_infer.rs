//! Corpus slice: `could_not_infer` — type inference fails for a creation or
//! invocation whose type arguments cannot be solved. RED per
//! `conformance/status.tsv`; the slice pins the creation-inference face
//! cluster (`bounds.rs`/`invocation.rs`: alias creation detail bounds,
//! `emit_twin`, named-ctor re-solve, declared-ctor re-solve) burned by ticket
//! #4132 (W3 residual B).

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
#[ignore = "red: 12 cases remain (generic-function argument typing, generic-metadata function argument, instantiated-to-bounds annotation, top-level list argument); the creation-inference rows are green"]
fn could_not_infer_slice() {
    corpus_common::run_family("diagnostics/could_not_infer");
}
