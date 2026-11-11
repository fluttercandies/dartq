//! Corpus slice: `return_of_invalid_type` — a `return` whose expression type
//! is not assignable to the enclosing function's return type. RED per
//! `conformance/status.tsv`; the slice pins the bare-creation static-type rows
//! in return windows burned by ticket #4131 (W3 residual A, `creation_args.rs`
//! + `row_resolution.rs`).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A bare-creation static-type rows)"]
fn return_of_invalid_type_slice() {
    corpus_common::run_family("diagnostics/return_of_invalid_type");
}
