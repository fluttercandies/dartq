//! Corpus slice: `extends_non_class` — an `extends` clause naming something
//! that is not a class. RED per `conformance/status.tsv`; the slice pins the
//! name/member-resolution layer burned by ticket #4119 (W2 burn,
//! `resolution.rs` + member faces).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (W2 name/member-resolution layer)"]
fn extends_non_class_slice() {
    corpus_common::run_family("diagnostics/extends_non_class");
}
