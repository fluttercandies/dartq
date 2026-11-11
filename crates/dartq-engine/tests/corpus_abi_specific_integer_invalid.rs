//! Corpus slice: `abi_specific_integer_invalid` — the `AbiSpecificInteger`
//! subclass-shape family (`dart:ffi`). Green per `conformance/status.tsv`;
//! exercises the verbatim-options path (doc cases carry options WITHOUT
//! `enable-experiment`, so zero experiments) and a real `dart:` import.

mod corpus_common;

#[test]
fn abi_specific_integer_invalid_slice() {
    corpus_common::run_family("diagnostics/abi_specific_integer_invalid");
}
