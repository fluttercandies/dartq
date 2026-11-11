//! Corpus slice: `const_with_type_parameters` — a const constructor/class
//! used with explicit type arguments where the implicit creation's type
//! arguments are not correctly qualified. RED per `conformance/status.tsv`;
//! the slice pins case 0142 (`const_eval_faces`: the qualified implicit
//! creation `A<T>.named()` leaks an unqualified `T` argument) burned by
//! ticket #4131.

mod corpus_common;

#[test]
#[ignore = "red family: burn owned by #4131 (W3 residual A const_eval implicit-creation type args)"]
fn const_with_type_parameters_slice() {
    corpus_common::run_family("diagnostics/const_with_type_parameters");
}
