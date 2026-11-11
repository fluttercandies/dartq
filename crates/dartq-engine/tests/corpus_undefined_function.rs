//! Corpus slice: `undefined_function` — a function invocation whose name
//! cannot be resolved. RED per `conformance/status.tsv`; the slice pins
//! the name-resolution face burned by ticket #4119 (W2 name/member wave).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_function_slice() {
    corpus_common::run_family("diagnostics/undefined_function");
}
