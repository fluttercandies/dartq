//! Corpus slice: `dot_shorthand_undefined_member` — a dot-shorthand
//! member access whose member cannot be resolved. RED per
//! `conformance/status.tsv`; the slice pins the name/member-resolution
//! face burned by ticket #4119 (W2 name/member wave).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn dot_shorthand_undefined_member_slice() {
    corpus_common::run_family("diagnostics/dot_shorthand_undefined_member");
}
