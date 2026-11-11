//! Corpus slice: `unused_local_variable` — a local variable declared but
//! never read. RED per `conformance/status.tsv`; the slice pins the
//! unused/lint-face layer (`scopes_bodies.rs`) burned by ticket #4130 (W1
//! burn, unused_finalization + scopes_bodies remainder).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4130 (W1 unused/lint-face layer)"]
fn unused_local_variable_slice() {
    corpus_common::run_family("diagnostics/unused_local_variable");
}
