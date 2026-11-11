//! Corpus slice: `undefined_super_member` — a `super.` member access
//! whose member cannot be resolved on the superclass. RED per
//! `conformance/status.tsv`; the slice pins the name/member-resolution
//! face burned by ticket #4119 (W2 name/member wave).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn undefined_super_member_slice() {
    corpus_common::run_family("diagnostics/undefined_super_member");
}
