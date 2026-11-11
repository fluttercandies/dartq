//! Corpus slice: `undefined_getter` — a getter/member access on a receiver
//! whose member cannot be resolved. RED per `conformance/status.tsv`; the
//! slice pins the row_resolution windows (`values.first` getter after the
//! extension decision, assignment-window receiver declared face) burned by
//! ticket #4131 (cases 0113/0228/0248/0249). The member-resolution face of the
//! same code is also in scope for ticket #4119 (W2 name/member wave).

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A row_resolution windows); member face shared with #4119"]
fn undefined_getter_slice() {
    corpus_common::run_family("diagnostics/undefined_getter");
}
