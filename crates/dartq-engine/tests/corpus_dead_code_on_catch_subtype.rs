//! Corpus slice: `dead_code_on_catch_subtype` — dead code reported on a catch
//! clause whose exception type is a subtype that can never be thrown. RED per
//! `conformance/status.tsv`; the slice pins the catch-slot cluster
//! (`scopes_bodies.rs` `reconcile_catch_slots` NEW-BREAK) burned by ticket
//! #4130 (W1 burn, unused/lint-face layer).

mod corpus_common;

#[test]
// Green since #4130's catch-slot reconciliation (`reconcile_catch_slots`).
fn dead_code_on_catch_subtype_slice() {
    corpus_common::run_family("diagnostics/dead_code_on_catch_subtype");
}
