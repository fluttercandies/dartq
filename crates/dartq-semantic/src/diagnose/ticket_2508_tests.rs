//! The #2508 pins: the extension-application row's structural face — the
//! mounted `LibraryTypes` snapshot answers applicability, the
//! instantiated-bound check, and the `_isMoreSpecific` steps 3-5 through
//! the real `dartq_types` lattice, where the name-level reads could only
//! pattern-match heads. The pins isolate the questions the structural arm
//! decides that the name-level arms could not:
//!
//! - a generic supertype hop's ARGUMENT projection (`Box<A> implements
//!   Iterable<Wrapper<A>>` — `Box<Leaf>` reaches
//!   `Iterable<Wrapper<Leaf>>`, so `on Iterable<Wrapper<Leaf>>` accepts
//!   and `on Iterable<Str>` rejects; the name-level `reaches` walk saw
//!   only head names);
//! - a parameterized bound check over the same hop (`T extends
//!   Comparable<T>` — the bound `Comparable<T>` substitutes `T := arg`
//!   and the structural subtype decides; a head with no `implements`
//!   edge fails);
//! - a strictly-more-specific generic `on` face silently dominating its
//!   supertype twin in the step 3-4 ordering (no
//!   `ambiguous_extension_member_access`).

use super::test_rig::{code_names, report_ops_bare as report_ops};

/// The generic hop's argument projection: `Box<A>` declares
/// `implements Iterable<Wrapper<A>>`, so `Box<Leaf>` projects to
/// `Iterable<Wrapper<Leaf>>`. `on Iterable<Wrapper<Leaf>>` applies (the
/// receiver's projection matches positionally) and `on Iterable<Str>`
/// does not — the name-level arm could only read `Iterable`'s head and
/// gated on the flat argument pair, where `Wrapper<Leaf>` is a nested
/// group it left unmodeled. The structural `unify_extended` walk carries
/// the projection and decides both.
#[test]
fn generic_hop_projection_decides() {
    // `Box<Leaf>` against `on Iterable<Wrapper<Leaf>>`: the projection's
    // `Wrapper<Leaf>` argument binds `T := Wrapper<Leaf>` and the
    // instantiated on-type matches — the member resolves silently.
    let applies = report_ops(
        "class Leaf {} class Str {} class Wrapper<T> {} \
         class Iterable<T> { T get first => throw 0; } \
         class Box<A> implements Iterable<Wrapper<A>> { \
           Wrapper<A> get first => throw 0; } \
         extension FirstExt<T> on Iterable<Wrapper<T>> { void tapped() {} } \
         void use(Box<Leaf> box) { box.tapped(); }",
    );
    assert_eq!(
        code_names(&applies),
        Vec::<&str>::new(),
        "{:?}",
        code_names(&applies)
    );
    // `Box<Leaf>` against `on Iterable<Str>`: the projection is
    // `Iterable<Wrapper<Leaf>>` — `Wrapper<Leaf>` is not `Str`, the face
    // never applies, and the access keeps `undefined_method`.
    let rejects = report_ops(
        "class Leaf {} class Str {} class Wrapper<T> {} \
         class Iterable<T> { T get first => throw 0; } \
         class Box<A> implements Iterable<Wrapper<A>> { \
           Wrapper<A> get first => throw 0; } \
         extension StrExt on Iterable<Str> { void tapped() {} } \
         void use(Box<Leaf> box) { box.tapped(); }",
    );
    assert_eq!(
        code_names(&rejects),
        ["undefined_method"],
        "{:?}",
        code_names(&rejects)
    );
}

/// The parameterized bound check: `T extends Comparable<T>` rejects a
/// receiver argument whose head carries no `implements` edge (a plain
/// class declaring nothing — `NoImpl` is not `Comparable<NoImpl>`), and
/// accepts `Iterable<UserC>` where `UserC implements Comparable<UserC>`
/// makes `UserC <: Comparable<UserC>` true after the `T := UserC`
/// substitution.
#[test]
fn parameterized_bound_decides() {
    // `NoImpl` declares no supertype edge to `Comparable`: the bound
    // `Comparable<T>` instantiates `T := NoImpl`, the subtype answer is
    // `false`, the face never applies, and `go` keeps `undefined_method`.
    let rejects = report_ops(
        "class Comparable<T> {} class Iterable<T> {} class NoImpl {} \
         extension E<T extends Comparable<T>> on Iterable<T> { void go() {} } \
         void use(Iterable<NoImpl> values) { values.go(); }",
    );
    assert_eq!(
        code_names(&rejects),
        ["undefined_method"],
        "{:?}",
        code_names(&rejects)
    );
    // `UserC implements Comparable<UserC>`: after `T := UserC` the bound
    // reads `Comparable<UserC>` and the supertype closure's own edge
    // satisfies it — the member resolves silently.
    let accepts = report_ops(
        "class Comparable<T> {} class Iterable<T> {} \
         class UserC implements Comparable<UserC> {} \
         extension E<T extends Comparable<T>> on Iterable<T> { void go() {} } \
         void use(Iterable<UserC> values) { values.go(); }",
    );
    assert_eq!(
        code_names(&accepts),
        Vec::<&str>::new(),
        "{:?}",
        code_names(&accepts)
    );
}

/// The step 3-4 ordering across instantiated generic faces: `on
/// List<Base>` and `on List<Derived>` both apply to a `List<Derived>`
/// receiver — the structural subtype orders `List<Derived> <:
/// List<Base>` and not the converse, so the specific face dominates
/// silently and no `ambiguous_extension_member_access` row fires. The
/// name-level arm ordered the same pair covariantly already; the pin
/// keeps the structural answer from regressing it.
#[test]
fn specificity_steps_34_order_generic_faces() {
    let report = report_ops(
        "class Base {} class Derived implements Base {} class List<T> {} \
         extension WideExt on List<Base> { void tap() {} } \
         extension NarrowExt on List<Derived> { void tap() {} } \
         void use(List<Derived> values) { values.tap(); }",
    );
    assert_eq!(
        code_names(&report),
        Vec::<&str>::new(),
        "{:?}",
        code_names(&report)
    );
}
