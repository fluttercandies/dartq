//! The #3804 pins: a typedef parameter referenced inside its own aliased
//! function type's *binder bounds* is `invariant` over the whole signature
//! (variance_builder.dart:131-140 — "if [variable] is referenced in a bound
//! at all, it makes the variance of [variable] in the entire type
//! invariant"), so its instantiate-to-bound default reads `dynamic`, never
//! the `Never` a `contravariant` occurrence alone would ground.
//!
//! Oracle contract (upstream Dart SDK source, 3.13.7):
//!
//! * `VarianceBuilder._computeFunctionType`
//!   (pkg/analyzer/lib/src/summary2/variance_builder.dart:133-140) forces
//!   `invariant` when the walked parameter occurs in any bound of the
//!   signature's own type parameters — `typedef G<X extends Y, Y extends
//!   A<X>> = void Function<X1 extends X, Y1 extends Y>(X, Y)` stores both
//!   `X` and `Y` invariant (each appears in `X1 extends X`/`Y1 extends Y`),
//!   so the raw `G` reads `G<dynamic, A<dynamic>>` — probed against `dart
//!   analyze` 3.13.7, which reports `parameter type 'G<dynamic,
//!   A<dynamic>>'`, not the `G<Never, A<Never>>` a `contravariant` seed
//!   grounds.
//! * `DefaultTypesBuilder._computeDefaultType`
//!   (pkg/analyzer/lib/src/summary2/default_types_builder.dart:128-187)
//!   seeds the component rewrite with that stored variance: `invariant`
//!   reads the `upper` map (`dynamic`), never the `lower` map (`Never`).
//!
//! The bound-invariance leg reads the signature binder's declared bound
//! through the facts face — a binder whose bound lives only in an overlay
//! the walk never saw reads `None`, the leg stays silent, and the parameter
//! keeps `contravariant` where the oracle says `invariant`. The pins read
//! the end-to-end diagnostic face — a raw alias in a declared type renders
//! through `Snapshot::itb_default_args` →
//! `instantiate_type_formals_to_bounds(&snapshot.facts)`, the producer the
//! census observes and the one that had to be fed the binder overlay to
//! answer `dynamic`. The class-shaped pin additionally guards the
//! non-function path, where no signature binder exists and the covariant
//! seed must survive untouched.

use super::test_rig::{report_ops, rows, types_for};
use dartq_types::UninternedType;

/// Counter-direction guard, on the face the fix lives on: a signature
/// binder whose bound does NOT mention the alias's own parameters
/// (`U extends num`) must not flip the alias parameter to `invariant`.
/// `T` occurs contravariantly in the formal list and its own bound
/// `A<T>` is a self-cycle, so the `contravariant` seed still grounds `T`
/// to `A<Never>` — an unrelated binder bound leaking into the walk would
/// read `A<dynamic>` instead, the oracle's `invariant` answer it must not
/// borrow. This is the over-application tripwire for the binder-overlay
/// feed in `bounds.rs`.
#[test]
fn ticket_3804_rendered_unrelated_binder_bound_keeps_never() {
    let source = "class A<Z> {}\n\
                  typedef F<T extends A<T>> = void Function<U extends num>(T);\n\
                  void f() {\n\
                    F x = 0;\n\
                  }\n";
    let report = report_ops(source);
    let rendered = rows(&report);
    assert!(
        rendered.iter().any(|row| row.contains("A<Never>")),
        "expected the contravariant default `A<Never>`, got {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|row| row.contains("A<dynamic>")),
        "an unrelated binder bound must not force invariant: {rendered:?}"
    );
}

/// The non-function twin: a class-shaped alias has no signature binder at
/// all, so the bound-invariance leg cannot fire and the parameter keeps
/// its covariant seed. `typedef N<X extends A<X>> = A<X>` grounds the
/// self-bound to `A<dynamic>` (the class-side answer), never the `Never`
/// a function-bodied twin's `contravariant` occurrence would produce.
#[test]
fn ticket_3804_non_function_alias_is_untouched() {
    let source = "class A<Z> {} \
         typedef N<X extends A<X>> = A<X>;";
    let (types, strings, _file) = types_for(source);
    let defaults = types
        .testing_typedef_defaults(strings.intern("N"))
        .expect("the alias's shape registers with its defaults");
    assert_eq!(defaults.len(), 1);
    let decl = types
        .testing_decl_of(strings.intern("A"))
        .expect("`A` registers");
    let dynamic = types.testing_table().intern(UninternedType::Dynamic, false);
    let a_dynamic = types.testing_table().intern(
        UninternedType::Interface {
            decl,
            args: vec![dynamic],
        },
        false,
    );
    assert_eq!(
        defaults[0], a_dynamic,
        "a class-shaped alias keeps the covariant seed -> `A<dynamic>`"
    );
}

/// The end-to-end twin of the exact fixture, through the diagnostic face
/// the census reads. A raw `G` in a declared type renders through the
/// snapshot's instantiate-to-bound (`Snapshot::itb_default_args` →
/// `instantiate_type_formals_to_bounds(&snapshot.facts)`), seeded by the
/// alias-variance walk in `bounds.rs`; that walk was fed the GFT binder
/// overlay for `X`/`Y` to store `invariant`. Before that feed the row read
/// `G<Never, A<Never>>`, the `contravariant` collapse; the oracle reads
/// `G<dynamic, A<dynamic>>`.
///
/// This is the pin that proves the census-visible number moves: the four
/// above exercise the `resolved_ops` defaults face, this one the
/// `bounds.rs` snapshot face the display path actually consumes.
#[test]
fn ticket_3804_rendered_alias_defaults_match_the_oracle() {
    let source = "class A<Z> {}\n\
                  typedef G<X extends Y, Y extends A<X>> = void Function<X1 extends X, Y1 extends Y>(X, Y);\n\
                  void f() {\n\
                    G g = 0;\n\
                  }\n";
    let report = report_ops(source);
    let rendered = rows(&report);
    assert!(
        rendered
            .iter()
            .any(|row| row.contains("G<dynamic, A<dynamic>>")),
        "expected the instantiated alias `G<dynamic, A<dynamic>>` in a row, got {rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|row| row.contains("G<Never, A<Never>>")),
        "the contravariant collapse must not survive: {rendered:?}"
    );
}
