//! The extension-application gates' two parameter faces: an `on` clause
//! spelling the extension's own type parameter (`extension<T extends
//! AstNode> on T` — the parameter instantiates to the receiver's type, so
//! the head gate becomes the parameter's declared bound), and an `on`
//! argument compared against a bare scope-parameter argument the torn-down
//! window lost (`List<T>` over the enclosing class's own `T extends Enum`
//! — a head the name level cannot order keeps the face, exactly as the
//! oracle's InvalidType subtypes every bound). Oracle provenance:
//! `dart analyze` 3.13.1 — scope_context.dart's `visitWithOverride` and
//! the drift converters' `values.byName` census shapes.

use super::test_rig::{assert_silent, code_names, report_ops};

/// `extension<T extends AstNode> on T` applies to every receiver the
/// bound admits — the chained `node.superclass?.visitWithOverride(…)`
/// shape resolves silently, not as `undefined_method`.
#[test]
fn own_param_on_type_extension_applies_to_bound_subtypes() {
    let report = report_ops(
        "class AstNode {}\n\
         class Decl extends AstNode {}\n\
         extension<T extends AstNode> on T {\n\
         \x20 void visit(int v) {}\n\
         }\n\
         void f(Decl node) {\n\
         \x20 node.visit(1);\n\
         }\n",
    );
    assert_silent(&report);
}

/// The bound gate still orders real heads: a receiver outside the
/// parameter's bound applies no face and the member access reports —
/// the oracle's `applicableTo` fails the instantiation.
#[test]
fn own_param_on_type_extension_rejects_out_of_bound_receivers() {
    let report = report_ops(
        "class AstNode {}\n\
         extension<T extends AstNode> on T {\n\
         \x20 void visit(int v) {}\n\
         }\n\
         void f(int node) {\n\
         \x20 node.visit(1);\n\
         }\n",
    );
    assert_eq!(code_names(&report), ["undefined_method"]);
}

/// `values.byName(s)` over `List<T>` with the class's own
/// `T extends Enum` against `on Iterable<T extends Enum>`: the
/// receiver's argument is a bare scope parameter the row-time window
/// cannot resolve — the face keeps applying (the under-report posture),
/// never an `undefined_method` over-report.
#[test]
fn class_scope_parameter_argument_keeps_the_face() {
    let report = report_ops(
        "extension EnumLookup<T extends Enum> on Iterable<T> {\n\
         \x20 T byName(String name) => throw name;\n\
         }\n\
         abstract class C<T extends Enum> {\n\
         \x20 final List<T> values;\n\
         \x20 const C(this.values);\n\
         \x20 T lookup(String s) => values.byName(s);\n\
         }\n",
    );
    assert_silent(&report);
}
