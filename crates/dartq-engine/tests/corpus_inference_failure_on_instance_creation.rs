//! Corpus slice: `inference_failure_on_instance_creation` — inference fails
//! while solving the type arguments of an instance creation expression. RED
//! per `conformance/status.tsv`; the slice pins the creation-inference face
//! burned by ticket #4132 (W3 residual B, `bounds.rs`/`invocation.rs`).

mod corpus_common;

#[test]
#[ignore = "red: the strict-inference `inference_failure_on_instance_creation` emitter is not implemented (8 cases)"]
fn inference_failure_on_instance_creation_slice() {
    corpus_common::run_family("diagnostics/inference_failure_on_instance_creation");
}
