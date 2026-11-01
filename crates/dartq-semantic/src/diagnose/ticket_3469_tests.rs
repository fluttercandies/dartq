//! #3469 — alias-headed bounds: the alias's own instantiate-to-bound
//! defaults ground by its *typedef* variance, and a bound naming a bare
//! generic alias decides the type-argument faces the way the written alias
//! spelling does.
//!
//! Oracle contract (upstream Dart SDK source):
//!
//! * `VarianceBuilder._functionTypeAlias`/`_genericTypeAlias`
//!   (pkg/analyzer/lib/src/dart/element/variance_builder.dart:153-225)
//!   compute a typedef parameter's stored variance from its occurrences in
//!   the aliased body, and `DefaultTypesBuilder._computeDefaultType`
//!   (pkg/analyzer/lib/src/summary2/default_types_builder.dart:128-187)
//!   seeds that parameter's component rewrite with it. `typedef G<X extends
//!   A<X>, Y extends X> = Y Function(X)` stores `X` contravariant (its
//!   occurrence sits in the formal list), so `X`'s default grounds to
//!   `A<Never>` and the raw `G` reads `G<A<Never>, A<Never>>` — the
//!   class-side covariant seed leaves `A<dynamic>`.
//! * `_checkForTypeArgumentNotMatchingBounds`
//!   (pkg/analyzer/lib/src/error/type_arguments_verifier.dart:342-540)
//!   checks every written type argument against its parameter's substituted
//!   bound; a bound naming a bare generic alias is that alias instantiated
//!   to bounds (`class E<T extends A>` over `typedef A<X> = C<X, X>` reads
//!   `A<dynamic>`), so the written argument answers against the expanded
//!   shape.
//!
//! Probed against `dart analyze` 3.13.7 (the pinned oracle at
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`): every expectation
//! below is a live verdict, quoted per fixture.

use super::test_rig::{code_names, report_ops, rows};
use super::*;

/// The raw `G` argument displays its *bounds-grounded* alias form: the
/// alias's parameters instantiate by their contravariant occurrences in
/// the aliased body, `X` grounding to `A<Never>` (never to the class-side
/// `A<dynamic>`). `t`'s parameter type is `G<A<Never>, A<Never>>`, i.e.
/// `A<Never> Function(A<Never>)`, so passing the raw `G` (the same shape)
/// into it reports.
///
/// Probed: `error - main.dart:3:42 - The argument type
/// 'G<A<Never>, A<Never>>' can't be assigned to the parameter type
/// 'A<Never>'. - argument_type_not_assignable` (1 row).
#[test]
fn alias_parameter_defaults_ground_by_typedef_variance() {
    let report = report_ops(
        "class A<T> {}\n\
         typedef G<X extends A<X>, Y extends X> = Y Function(X);\n\
         void f(G g, G<A<Never>, A<Never>> t) { t(g); }\n",
    );
    assert_eq!(
        code_names(&report),
        ["argument_type_not_assignable"],
        "{:#?}",
        rows(&report)
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains("'G<A<Never>, A<Never>>' can't be assigned to the parameter type 'A<Never>'"),
        "{:#?}",
        rows(&report)
    );
}

/// A bare generic alias in a bound position is the alias instantiated to
/// bounds: `typedef A<X> = C<X, X>` reads `A<dynamic>`, i.e.
/// `C<dynamic, dynamic>`, so `E<int>`'s argument does not conform.
///
/// Probed: `error - main.dart:4:22 - 'int' doesn't conform to the bound
/// 'A<dynamic>' of the type parameter 'T'. - type_argument_not_matching_bounds`
/// (1 row, at the written annotation).
///
/// Live since #3510: the annotation face judges the snapshot-expanded
/// bound and renders the written alias decoration.
#[test]
fn bare_class_shaped_alias_bound_decides_the_annotation() {
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
}

/// The same bound at a creation site: the annotation and the creation each
/// carry the row (`checkNamedType` runs at every `NamedType`).
///
/// Probed: two rows, `main.dart:4:14` (the annotation) and `main.dart:4:28`
/// (the creation), both `'int' doesn't conform to the bound 'A<dynamic>' of
/// the type parameter 'T'. - type_argument_not_matching_bounds`.
///
/// Live since #3510, with its annotation twin above: the creation face
/// reads the same split (judged expansion, written message).
#[test]
fn bare_class_shaped_alias_bound_decides_the_creation() {
    let report = report_ops(
        "class C<X, Y> {}\n\
         typedef A<X> = C<X, X>;\n\
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
}
