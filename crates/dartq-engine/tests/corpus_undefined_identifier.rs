//! Corpus slice: `undefined_identifier` — a name that cannot be resolved in
//! the current scope (including the private-member `undefined_identifier`
//! cluster). RED per `conformance/status.tsv`; the slice pins the
//! name/member-resolution layer burned by ticket #4119 (W2 burn,
//! `resolution.rs` + member faces, C-debt heavy).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (W2 name/member-resolution layer)"]
fn undefined_identifier_slice() {
    corpus_common::run_family("diagnostics/undefined_identifier");
}
