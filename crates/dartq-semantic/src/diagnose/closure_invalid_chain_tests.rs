//! The closure-literal schema's invalid-argument recovery: the oracle's
//! `_inferFormalParameters` binds greatestClosure's `Object?` when the
//! context schema's element is the analyzer's `InvalidType` (an
//! unresolvable-import head), so the formal's member reads report
//! `unchecked_use_of_nullable_value`. Oracle provenance: `dart analyze`
//! 3.13.1 over the census⑨ F3 shapes — `Map<String, Und>.values.map`,
//! a constructor's field-formal slot, and a walked getter's
//! `List<Und>` return.

use super::test_rig::{code_names, report_ops};

/// `rules.values.map((node) => node.value)` with the map's value type an
/// undefined name: the producer table's element is the oracle's
/// `InvalidType` and the formal binds `Object?` — the member read
/// reports, exactly as the nullable-element control already did.
#[test]
fn map_values_invalid_element_binds_object_question() {
    let report = report_ops(
        "void f(Map<String, Und> rules) {\n\
         \x20 rules.values.map((node) => node.value);\n\
         }\n",
    );
    assert!(code_names(&report).contains(&"unchecked_use_of_nullable_value"));
}

/// `Eb._(reportError: (scopeName, node) { node.value; })`: the closure
/// literal at a constructor's named argument slot reads the selected
/// constructor's formal table — the field formal `required this
/// .reportError` carries the field's declared function type, and the
/// unresolvable `Und` parameter of that type binds `Object?`.
#[test]
fn constructor_field_formal_slot_types_the_closure() {
    let report = report_ops(
        "class Eb {\n\
         \x20 Eb._({required this.reportError});\n\
         \x20 final void Function(String scopeName, Und node) reportError;\n\
         }\n\
         void k() {\n\
         \x20 Eb._(reportError: (scopeName, node) {\n\
         \x20   node.value;\n\
         \x20 });\n\
         }\n",
    );
    assert!(code_names(&report).contains(&"unchecked_use_of_nullable_value"));
}

/// `libraryName.tokens.map((e) => e.lexeme)` over an abstract getter
/// declaring `List<Und>`: the walked getter's result argument runs the
/// same per-argument recovery — the chain forms on `Object?` elements
/// instead of silently dropping.
#[test]
fn walked_getter_invalid_element_arg_chains() {
    let report = report_ops(
        "abstract class LibId {\n\
         \x20 List<Und> get tokens;\n\
         }\n\
         void f(LibId libraryName) {\n\
         \x20 libraryName.tokens.map((e) => e.lexeme);\n\
         }\n",
    );
    assert!(code_names(&report).contains(&"unchecked_use_of_nullable_value"));
}
