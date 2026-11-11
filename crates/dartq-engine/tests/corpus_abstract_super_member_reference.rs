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

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4119 (name/member-resolution wave)"]
fn abstract_super_member_reference_slice() {
    corpus_common::run_family("diagnostics/abstract_super_member_reference");
}
