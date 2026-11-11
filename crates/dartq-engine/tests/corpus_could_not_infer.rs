//! Corpus slice: `could_not_infer` — type inference fails for a creation or
//! invocation whose type arguments cannot be solved. RED per
//! `conformance/status.tsv`; the slice pins the creation-inference face
//! cluster (`bounds.rs`/`invocation.rs`: alias creation detail bounds,
//! `emit_twin`, named-ctor re-solve, declared-ctor re-solve) burned by ticket
//! #4132 (W3 residual B).

mod corpus_common;

#[test]
#[ignore = "red: 12 cases remain (generic-function argument typing, generic-metadata function argument, instantiated-to-bounds annotation, top-level list argument); the creation-inference rows are green"]
fn could_not_infer_slice() {
    corpus_common::run_family("diagnostics/could_not_infer");
}
