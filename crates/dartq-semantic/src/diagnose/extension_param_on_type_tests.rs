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

/// The #2568 residual: an unbounded `on T` face over a NULLABLE explicit
/// receiver. The parameter instantiates to the receiver type (`T :=
/// String?`), so the face applies safely and the access is a `Single`, not
/// the `UnsafeSingle` that a non-nullable `on C` face earns — no
/// `unchecked_use_of_nullable_value` (oracle 3.13.1: clean; the
/// `expect/lib/static_type_helper.dart` `StaticType<T> on T` shape).
#[test]
fn unbounded_own_param_on_nullable_receiver_resolves_safely() {
    let report = report_ops(
        "class Exactly<S> {}\n\
         extension StaticType<T> on T {\n\
         \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         void f(String? s) {\n\
         \x20 s.expectStaticType<Exactly<String?>>();\n\
         }\n",
    );
    assert_silent(&report);
}

/// The self-nullable bound arm: `T extends num?` admits the nullable
/// receiver safely (the bound itself is nullable) — clean, where the
/// non-nullable bound below reports (oracle 3.13.1: clean).
#[test]
fn nullable_bound_own_param_on_nullable_receiver_resolves_safely() {
    let report = report_ops(
        "class Exactly<S> {}\n\
         extension NullBounded<T extends num?> on T {\n\
         \x20 T id<R extends Exactly<T>>() => this;\n\
         }\n\
         void f(int? s) {\n\
         \x20 s.id<Exactly<int?>>();\n\
         }\n",
    );
    assert_silent(&report);
}

/// The non-nullable bound stays UNSAFE: `T extends num` cannot instantiate
/// to `int?` (a `int?` receiver is not within `num`), so a nullable
/// receiver resolving the member reads a possibly-null target — the
/// oracle's `unchecked_use_of_nullable_value` (3.13.1). The nullable-safe
/// reclassification must never swallow this row.
#[test]
fn non_nullable_bound_own_param_on_nullable_receiver_reports_unchecked() {
    let report = report_ops(
        "class Exactly<S> {}\n\
         extension Bounded<T extends num> on T {\n\
         \x20 T id<R extends Exactly<T>>() => this;\n\
         }\n\
         void f(int? s) {\n\
         \x20 s.id<Exactly<int?>>();\n\
         }\n",
    );
    assert_eq!(code_names(&report), ["unchecked_use_of_nullable_value"]);
}
