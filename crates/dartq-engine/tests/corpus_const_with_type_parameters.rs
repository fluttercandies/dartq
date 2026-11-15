//! Corpus slice: `const_with_type_parameters` — a const constructor/class
//! used with explicit type arguments where the implicit creation's type
//! arguments are not correctly qualified. RED per `conformance/status.tsv`;
//! the slice pins case 0142 (`const_eval_faces`: the qualified implicit
//! creation `A<T>.named()` leaks an unqualified `T` argument) burned by
//! ticket #4131.

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
#[ignore = "red family: burn owned by #4131 (W3 residual A const_eval implicit-creation type args)"]
fn const_with_type_parameters_slice() {
    corpus_common::run_family("diagnostics/const_with_type_parameters");
}
