//! #3510 — raw generic alias-headed bounds: the judgment reads the
//! snapshot-expanded shape, the message keeps the written alias
//! decoration.
//!
//! Oracle contract (upstream Dart SDK source, the pinned 3.13.7
//! reference): a written bound naming a bare generic alias is that alias
//! instantiated to bounds — `NamedTypeResolver` resolves the alias head
//! through `typeSystem.instantiateTypeAliasToBounds`
//! (pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:305-310),
//! so `_checkForTypeArgumentNotMatchingBounds` compares the argument
//! against the EXPANDED structural type while `getDisplayString` prints
//! the alias decoration the resolved type still carries
//! (pkg/analyzer/lib/src/dart/element/display_string_builder.dart
//! :379-382) — `class E<T extends A>` over `typedef A<X> = C<X, X>`
//! reports `'int' doesn't conform to the bound 'A<dynamic>' of the type
//! parameter 'T'`, never `C<dynamic, dynamic>` and never a silent pass.
//!
//! Fixtures quote the oracle rows pinned by #3469's live probes (the two
//! shared shapes) and the upstream display path for the rest; every
//! expectation is the full row set (no unlisted codes).

use super::test_rig::{code_names, report_ops, rows};

/// The class-shaped fixture's one-row annotation face (`void f(E<int> e)`):
/// the argument misses the bound the alias expands to, and the message
/// keeps the decorated spelling.
///
/// Probed by #3469's twin: `error - main.dart:4:22 - 'int' doesn't
/// conform to the bound 'A<dynamic>' of the type parameter 'T'. -
/// type_argument_not_matching_bounds`.
#[test]
fn bare_class_shaped_alias_bound_reports_at_the_annotation() {
    let report = report_ops(
        "class C<X, Y> {}\n\
         typedef A<X> = C<X, X>;\n\
         class E<T extends A> {}\n\
         void f(E<int> e) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("'int' doesn't conform to the bound 'A<dynamic>' of the type parameter 'T'"),
        "{:#?}",
        rows(&report)
    );
}

/// The `List`-shaped right-hand side decides the same way: the raw alias
/// instantiates to `A<dynamic>` (i.e. `List<dynamic>`), which `int` does
/// not conform to.
#[test]
fn bare_list_shaped_alias_bound_decides() {
    let report = report_ops(
        "typedef A<X> = List<X>;\n\
         class E<T extends A> {}\n\
         void f(E<int> e) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("'int' doesn't conform to the bound 'A<dynamic>' of the type parameter 'T'"),
        "{:#?}",
        rows(&report)
    );
}

/// The non-generic control keeps working — and its message keeps the
/// BARE alias spelling: a non-generic alias instantiates with no
/// arguments, so the decoration is `A` (`_writeTypeArguments` skips an
/// empty list), never `A<...>` and never the expansion `C<int, int>`.
/// Both NamedType sites report (the annotation and the creation), the
/// same pair the generic twin pins.
#[test]
fn non_generic_alias_bound_keeps_the_bare_decoration() {
    let report = report_ops(
        "class C<X, Y> {}\n\
         typedef A = C<int, int>;\n\
         class E<T extends A> {}\n\
         E<int> e = E<int>();\n",
    );
    assert_eq!(
        code_names(&report),
        [
            "type_argument_not_matching_bounds",
            "type_argument_not_matching_bounds"
        ],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics.iter().all(|diagnostic| diagnostic
            .message
            .contains("'int' doesn't conform to the bound 'A' of the type parameter 'T'")),
        "{:#?}",
        rows(&report)
    );
}

/// The function-shaped right-hand side at the type-argument site: the
/// raw `F` instantiates to `F<dynamic>` (`dynamic Function(dynamic)`),
/// so `E<int>` misses the bound — and the decoration is the alias
/// spelling, not the function shape.
#[test]
fn bare_function_shaped_alias_bound_decides() {
    let report = report_ops(
        "typedef F<X> = X Function(X);\n\
         class E<T extends F> {}\n\
         void f(E<int> e) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("'int' doesn't conform to the bound 'F<dynamic>' of the type parameter 'T'"),
        "{:#?}",
        rows(&report)
    );
}

/// A bounded alias parameter whose stored variance is CONTRAvariant
/// grounds its self-referential bound to `Never`: over
/// `int Function(X)` the parameter occurs only in the formal list, so
/// `VarianceBuilder` stores `X` contravariant and
/// `_UpperLowerReplacementVisitor.visitTypeParameterType`'s
/// contravariant arm reads the lower (`Never`) substitution — the raw
/// `A` displays `A<Wrap<Never>>` (default_types_builder.dart:167-176,
/// :481-487), the same grounding #3469 probed live at the argument face.
#[test]
fn contravariant_alias_parameter_decorates_with_never() {
    let report = report_ops(
        "class Wrap<T> {}\n\
         typedef A<X extends Wrap<X>> = int Function(X);\n\
         class E<T extends A> {}\n\
         void f(E<int> e) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0].message.contains(
            "'int' doesn't conform to the bound 'A<Wrap<Never>>' of the type parameter 'T'"
        ),
        "{:#?}",
        rows(&report)
    );
}

/// The invariant twin: over `X Function(X)` the parameter occurs in the
/// return (covariant) AND the formal list (contravariant), so
/// `VarianceBuilder._combine` stores `X` invariant — and the visitor's
/// `visitTypeParameterType` sends everything that is NOT contravariant
/// to the upper substitution (`dynamic`, default_types_builder.dart
/// :481-487). The raw `A` displays `A<Wrap<dynamic>>`, pinning that the
/// engine's variance feed is the stored one, not a blanket seed.
#[test]
fn invariant_alias_parameter_decorates_with_dynamic() {
    let report = report_ops(
        "class Wrap<T> {}\n\
         typedef A<X extends Wrap<X>> = X Function(X);\n\
         class E<T extends A> {}\n\
         void f(E<int> e) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0].message.contains(
            "'int' doesn't conform to the bound 'A<Wrap<dynamic>>' of the type parameter 'T'"
        ),
        "{:#?}",
        rows(&report)
    );
}

/// The body-level twin — `T extends F` deciding an assignment inside the
/// container — reads the bound through `Engine::param_bound`
/// (`bound_face`'s engine_core.rs site) and the assignability chain, the
/// two rider-owned consumers of #3510's expanded-bound accessor. This
/// body is the live specification for that rider: the oracle reports
/// `A value of type 'T' can't be assigned to a variable of type
/// 'int Function(int)'` (the bound's expansion `dynamic Function(dynamic)`
/// is not a subtype of `int Function(int)`).
#[test]
#[ignore = "the body-level bound reads ride param_bound/bound_face (engine_core.rs) — the rider patch in #3510's report owns the site"]
fn bare_function_shaped_alias_bound_decides_the_body_assignment() {
    let report = report_ops(
        "typedef F<X> = X Function(X);\n\
         class E<T extends F> {\n\
           void m(T t) {\n\
             int Function(int) g = t;\n\
             g;\n\
           }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["invalid_assignment"],
        "{:#?}",
        rows(&report)
    );
}
