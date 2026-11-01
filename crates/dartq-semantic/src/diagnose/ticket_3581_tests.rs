//! #3581 — two faces of the instantiate-to-bound check on alias heads:
//! the super-bounded report of a raw generic head substitutes the bound
//! through the INVERTED arguments, and a written alias-headed argument
//! keeps its use-site `?` through the alias expansion.
//!
//! Oracle contract (upstream Dart SDK source, the pinned 3.13.7
//! reference): `_checkForTypeArgumentNotMatchingBounds` runs the regular
//! pass over the instantiation, and where super-bounded types are allowed
//! (an annotation site) re-checks the inverted type — `replaceTopAndBottom`
//! — with `invertedSubstitution = Substitution.fromPairs2(typeParameters,
//! invertedTypeArguments)` (pkg/analyzer/lib/src/error/
//! type_arguments_verifier.dart), reporting the inverted argument against
//! the bound substituted by the inverted arguments. The alias face takes
//! the same route (`TypeBoundedHelper._isSuperBounded` →
//! `_isRegularBounded(invertedType)`, pkg/analyzer/lib/src/dart/element/
//! well_bounded.dart). The inversion is variance-directed over an alias
//! head: a parameter the alias body uses contravariantly (`X Function(Y)`'s
//! `Y`) flips `Never` to `Object?`, a covariant one flips `dynamic` to
//! `Never` (`ReplaceTopBottomVisitor._instantiatedTypeAlias`,
//! pkg/analyzer/lib/src/dart/element/replace_top_bottom_visitor.dart).
//!
//! The rows below are the oracle's own on the co19 instantiate-to-bound
//! typedef suite (`typedef_l2_01_t01`, `typedef_l2_02_t01`,
//! `typedef_l2_06_t01`, the `test(G g)` annotation), quoted verbatim from
//! `conformance/expected.tsv`; every expectation is the full row set.

use super::test_rig::{code_names, report_ops, rows};

/// `typedef G<X extends A<X>, Y extends A<Y>> = X Function(Y)` raw: the
/// instantiation is `G<A<dynamic>, A<Never>>` (X covariant grounds to
/// `dynamic`, Y contravariant to `Never`), the inverted type is
/// `G<A<Never>, A<Object?>>`, and the report reads Y's bound through the
/// inverted Y — `'A<A<Object?>>'`, never the un-inverted `'A<A<Never>>'`.
///
/// Oracle: `type_argument_not_matching_bounds|…typedef_l2_01_t01.dart|60:6|1|
/// 'A<Object?>' doesn't conform to the bound 'A<A<Object?>>' of the type
/// parameter 'Y'.`
#[test]
fn self_cyclic_contravariant_parameter_reads_the_inverted_bound() {
    let report = report_ops(
        "class A<X> {}\n\
         typedef G<X extends A<X>, Y extends A<Y>> = X Function(Y);\n\
         void f(G g) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'A<Object?>' doesn't conform to the bound 'A<A<Object?>>' of the type parameter 'Y'.",
        "{:#?}",
        rows(&report)
    );
}

/// `typedef G<X extends A<X>, Y extends X> = X Function(Y)` raw: the
/// instantiation is `G<A<dynamic>, Never>`, the inverted type
/// `G<A<Never>, Object?>`; Y's bound `X` substitutes the INVERTED X —
/// `'A<Never>'` — so the cross-parameter leaf reads the inverted peer,
/// never the instantiation's own `A<dynamic>`.
///
/// Oracle: `type_argument_not_matching_bounds|…typedef_l2_02_t01.dart|61:6|1|
/// 'Object?' doesn't conform to the bound 'A<Never>' of the type parameter
/// 'Y'.`
#[test]
fn cross_parameter_bound_reads_the_inverted_peer() {
    let report = report_ops(
        "class A<X> {}\n\
         typedef G<X extends A<X>, Y extends X> = X Function(Y);\n\
         void f(G g) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'Object?' doesn't conform to the bound 'A<Never>' of the type parameter 'Y'.",
        "{:#?}",
        rows(&report)
    );
}

/// `typedef G<X extends A<Y>, Y extends A<X>> = X Function(Y)` raw (one
/// two-member cycle): the instantiation is `G<A<Never>, A<Never>>`
/// (the component rewrite grounds Y's leaf through the contravariant
/// seed and X's `A<Y>` reads it back), the inverted type is
/// `G<A<Never>, A<Object?>>`; Y's bound `A<X>` substitutes the inverted
/// X — `'A<A<Never>>'`.
///
/// Oracle: `type_argument_not_matching_bounds|…typedef_l2_06_t01.dart|61:6|1|
/// 'A<Object?>' doesn't conform to the bound 'A<A<Never>>' of the type
/// parameter 'Y'.`
#[test]
fn mutual_cycle_reads_the_inverted_bound() {
    let report = report_ops(
        "class A<X> {}\n\
         typedef G<X extends A<Y>, Y extends A<X>> = X Function(Y);\n\
         void f(G g) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'A<Object?>' doesn't conform to the bound 'A<A<Never>>' of the type parameter 'Y'.",
        "{:#?}",
        rows(&report)
    );
}

/// A written alias-headed argument carries its use-site `?` into the
/// expansion (`TypeAliasElementImpl.instantiateImpl` applies the use
/// site's suffix onto the substituted aliased type): `G<int>?` over
/// `typedef G<X> = X Function()` is `int Function()?`, which the
/// non-nullable bound `G` (`G<dynamic>`, `dynamic Function()`) refuses in
/// the regular pass and — `int` being neither top nor bottom — in the
/// inverted pass alike. The nullable-RHS controls stay silent: `B<N<int>>`,
/// `B<G<int>?>` and `B<N<int>?>` over `typedef N<X> = X Function()?` all
/// fit `N<dynamic>` (`dynamic Function()?`).
///
/// Oracle (3.13.7 probe): `type_argument_not_matching_bounds 5:10 'G<int>?'
/// doesn't conform to the bound 'G<dynamic>' of the type parameter 'X'.`
/// and nothing on the `B` annotations.
#[test]
fn nullable_alias_argument_keeps_its_question_against_the_bound() {
    let report = report_ops(
        "typedef G<X> = X Function();\n\
         typedef N<X> = X Function()?;\n\
         class A<X extends G> {}\n\
         class B<X extends N> {}\n\
         void f(A<G<int>?> a1, A<G<int>> a3, B<N<int>> b1, B<G<int>?> b2, B<N<int>?> b3) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'G<int>?' doesn't conform to the bound 'G<dynamic>' of the type parameter 'X'.",
        "{:#?}",
        rows(&report)
    );
}

/// The same use-site `?` under the super-bounded inversion: `G<dynamic>?`
/// inverts (covariant `X`, top → `Never`) to `G<Never>?`, still nullable,
/// still refused by the non-nullable `G<dynamic>` — the report spells the
/// inverted argument with its `?`.
///
/// Oracle (3.13.7 probe): `type_argument_not_matching_bounds 5:25 'G<Never>?'
/// doesn't conform to the bound 'G<dynamic>' of the type parameter 'X'.` —
/// the same row the co19 Simple-bounds `class_typedef_l1_t01` case carries
/// at `F<A<G<dynamic>?>?>?`.
#[test]
fn nullable_alias_argument_inverts_with_its_question() {
    let report = report_ops(
        "typedef G<X> = X Function();\n\
         class A<X extends G> {}\n\
         void f(A<G<dynamic>?> a) {}\n",
    );
    assert_eq!(
        code_names(&report),
        ["type_argument_not_matching_bounds"],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "'G<Never>?' doesn't conform to the bound 'G<dynamic>' of the type parameter 'X'.",
        "{:#?}",
        rows(&report)
    );
}
