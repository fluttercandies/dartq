//! The flow layer's lexical-scope boundaries as the semantic faces observe
//! them: a member selector never resolves in the lexical scope, and a
//! constructor field initializer's write never reaches a like-named formal.
//!
//! Oracle provenance (`dart analyze` 3.13.1): every shape is silent.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// `var manifest = element.manifest?.instance`: the selector `manifest`
/// is not a read of the local being declared — the receiver's own field
/// (`H?`) keeps the `?.` live.
#[test]
fn member_selector_never_reads_a_like_named_local() {
    let report = report_ops(
        "class H { int get instance => 0; }\n\
         class Em { H? manifest; }\n\
         class R {\n\
           int _lock = 0;\n\
           void rec({required Em element}) {\n\
             if (_lock != 0) { return; }\n\
             var manifest = element.manifest?.instance;\n\
             if (manifest == null) { return; }\n\
             manifest;\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `uri = uri ?? file.toUri()` in an initializer list writes the field:
/// the formal `uri` stays `U?`, so a later `uri ?? ...` keeps its live
/// right operand (an optional positional and a named formal alike).
#[test]
fn field_initializer_write_never_promotes_the_formal() {
    let report = report_ops(
        "class U {}\n\
         class F { U toUri() => U(); String get path => ''; }\n\
         int g(String s) => 0;\n\
         int h(U u) => 0;\n\
         class S {\n\
           final F file; final U uri; final int id;\n\
           S(this.file, [U? uri]) : uri = uri ?? file.toUri(), id = g('${uri ?? file.toUri()}@${file.path}');\n\
         }\n\
         class D {\n\
           final U dv; final int salt;\n\
           D({U? dv}) : dv = dv ?? U(), salt = h(dv ?? U());\n\
         }\n",
    );
    assert_silent(&report);
}

/// The body's own `??` on a non-nullable local still reports both faces
/// (the dead row at `??`, the dead null-aware row at the right operand):
/// the formal discipline above is about the initializer list, not the
/// operator.
#[test]
fn non_nullable_if_null_in_a_body_still_reports() {
    let report = report_ops("class U {}\nU f(U u) { return u ?? U(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["dead_code", "dead_null_aware_expression"],
        "{:?}",
        rows(&report)
    );
}
