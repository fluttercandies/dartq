//! Corpus slice: `cast_to_non_type` — the `as`/`is` cast against a non-type
//! name family. Green per `conformance/status.tsv`; the slice proves the
//! wiring end-to-end (doc-range `[!…!]` stripping, caret/`[diag.x]` marker
//! removal, multi-row cases, `-` expected cells).

mod corpus_common;

#[test]
fn cast_to_non_type_slice() {
    corpus_common::run_family("diagnostics/cast_to_non_type");
}
