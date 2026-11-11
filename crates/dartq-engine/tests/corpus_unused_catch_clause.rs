//! Corpus slice: `unused_catch_clause` — a `catch`/`on` clause that can never
//! be reached or whose bound variable is unused. RED per
//! `conformance/status.tsv`; the slice pins the catch-slot cluster
//! (`scopes_bodies.rs` `reconcile_catch_slots` NEW-BREAK) burned by ticket
//! #4130 (W1 burn, unused/lint-face layer).

mod corpus_common;

#[test]
// Green since #4130's catch-slot reconciliation (`reconcile_catch_slots`).
fn unused_catch_clause_slice() {
    corpus_common::run_family("diagnostics/unused_catch_clause");
}
