//! The #3523 pins: extension specificity's step 5 substitutes the
//! *to-bounds* face into each `on` type, and a raw alias instantiates by
//! the *default-type* face seeded with the typedef's own variance.
//!
//! Oracle contract (upstream Dart SDK source, 3.13.7):
//!
//! * `_isMoreSpecific` step 5 (pkg/analyzer/lib/src/dart/resolver/
//!   extension_member_resolver.dart:496-502) compares the two extensions'
//!   `_instantiateToBounds` types (:455-462) — `Substitution.fromPairs2`
//!   over `TypeSystemImpl.instantiateTypeFormalsToBounds`
//!   (type_system.dart:735-811), the fixpoint-plus-`Phi` fill whose
//!   substitution is position-blind and nullability-uniting and whose
//!   stuck parameters read `dynamic`. `bounds_projection` substituted the
//!   default-type face instead (`defaultType`'s variance-directed
//!   upper/lower rewrite, default_types_builder.dart:128-187), which
//!   answers the same bounds differently (#3487): a `List<X?>` leaf
//!   projected verbatim and hid an ambiguity the oracle reports, and a
//!   cycle's dependent grounded to the member's own default and
//!   fabricated one the oracle never reports.
//! * `instantiateTypeAliasToBounds` (type_system.dart:719-729) reads
//!   `_defaultTypeArguments` — each parameter's `defaultType`, whose
//!   component rewrite seeds from the variance `VarianceBuilder`
//!   computed over the aliased body (variance_builder.dart:153-225).
//!   `typedef G<X extends A<X>, Y extends X> = Y Function(X)` stores `X`
//!   contravariant and grounds to `A<Never>`, so the raw `G` reads
//!   `G<A<Never>, A<Never>>` (#3469's oracle probe); the snapshot's
//!   alias pass seeded every parameter covariant and read `A<dynamic>`.
//!
//! The four extension fixtures are the #3487 oracle probes verbatim,
//! with each class's own parameter renamed where the snapshot's
//! name-keyed parameter registry would fold a class parameter into a
//! face's parameter of the same spelling (a class parameter's name is
//! local to its class, so the oracle verdicts carry over). All four
//! extension faces spell function-typed bounds or arguments, which map
//! column by column since #3562 gave `map_ty_strict`'s `Ty::Function`
//! arm a structural face. Pin 4 decides through that face; pin 1 stays
//! an ignored live spec because the RECEIVER side of its fixture
//! degrades (see its own note) — the bound map is no longer the gate.

use super::test_rig::{assert_silent, code_names, report_ops, rows, types_for};
use dartq_types::{TypeKind, UninternedType};

const AMBIGUOUS: &str = "ambiguous_extension_member_access";

/// The ambiguity message's two-candidate template, byte-aligned with the
/// oracle's (`messages.yaml` `ambiguousExtensionMemberAccessTwo`): the
/// member name and both extension displays ride the quoted argument
/// slots. The slots' element rendering is `extension_display`'s face
/// (row_resolution.rs); these fixtures pin the template text and both
/// candidate names.
fn assert_ambiguity_template(report: &super::FileReport, member: &str) {
    let message = &report.diagnostics[0].message;
    assert!(
        message.starts_with(&format!("A member named '{member}' is defined in '"))
            && message.ends_with("', and neither is more specific."),
        "the ambiguity template must ride the member name and the quoted \
         candidate slots, got: {message:?}"
    );
    assert!(
        message.contains("'extension E1") && message.contains("'extension E2"),
        "both candidates belong in the slots, got: {message:?}"
    );
}

/// Pin 1 — `X extends void Function(X)`, a self cycle through a
/// function-type formal. The to-bounds face never completes and `Phi`
/// maps `X` to `dynamic`, so `E1`'s projection equals `E2`'s
/// `A<void Function(dynamic)>` and the pair is ambiguous; the default
/// face's `A<void Function(Never)>` would have made `E2` strictly more
/// specific and stayed silent.
///
/// Oracle (3.13.7, #3487 probe): `ambiguous_extension_member_access` at
/// the access.
///
/// The strict projection reaches this pair's bound since #3562 mapped the
/// function-typed bound (`map_ty_strict`'s `Ty::Function` arm), so the
/// to-bounds switch is no longer the gate. The remaining one is the
/// RECEIVER: `A<void Function(dynamic)>` arrives at the extension gate
/// with its function-typed argument degraded to `dynamic` (the flow read
/// face spells no signature for a function type in an argument position),
/// and `A<dynamic>` is not a subtype of `A<void Function(dynamic)>` — so
/// `E2` is never applicable and the access keeps a single candidate. This
/// body is the specification the receiver face must keep.
#[test]
#[ignore = "receiver face degrades `A<void Function(dynamic)>` to `A<dynamic>` — E2 never becomes applicable"]
fn function_formal_cycle_reports_ambiguity() {
    let report = report_ops(
        "class A<T> {} \
         extension E1<X extends void Function(X)> on A<X> { int get foo => 1; } \
         extension E2 on A<void Function(dynamic)> { int get foo => 2; } \
         void use(A<void Function(dynamic)> a) { a.foo; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
    assert_ambiguity_template(&report, "foo");
}

/// Pin 2 — `X extends A<X>, Y extends List<X>`: `Y` only depends on the
/// cycle. The to-bounds face maps both stuck parameters through `Phi` to
/// `dynamic`, so `E1`'s projection is `B<A<dynamic>, List<dynamic>>` and
/// `E2`'s written-bounds projection `B<A<dynamic>, List<A<dynamic>>>` is
/// strictly more specific — no ambiguity row; the default face grounded
/// `Y` to `List<A<dynamic>>` too and fabricated the ambiguity the oracle
/// never reports.
///
/// Oracle (3.13.7, #3487 probe): `E2.foo` resolves (its `String` return
/// is what the receiver sees), no `ambiguous_extension_member_access`.
#[test]
fn cycle_dependent_does_not_fabricate_ambiguity() {
    let report = report_ops(
        "class A<T> {} class D extends A<D> {} class B<S, T> {} \
         extension E1<X extends A<X>, Y extends List<X>> on B<X, Y> { int get foo => 1; } \
         extension E2<P extends A<dynamic>, Q extends List<A<dynamic>>> on B<P, Q> { int get foo => 2; } \
         void use(B<D, List<D>> b) { b.foo; }",
    );
    assert!(
        !code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
}

/// Pin 3 — `X extends int, Y extends List<X?>`: no cycle; the faces split
/// on the nullable leaf. The to-bounds substitution unites the leaf's `?`
/// (`List<int?>`), so `E1`'s projection equals `E2`'s
/// `B<int, List<int?>>` and the pair is ambiguous; the default face's
/// verbatim leaf (`List<int>`) made `E1` strictly more specific and hid
/// the ambiguity the oracle reports.
///
/// Oracle (3.13.7, #3487 probe): `ambiguous_extension_member_access` at
/// the access.
#[test]
fn nullable_leaf_reports_ambiguity() {
    let report = report_ops(
        "class B<S, T> {} \
         extension E1<X extends int, Y extends List<X?>> on B<X, Y> { int get foo => 1; } \
         extension E2<P extends int, Q extends List<int?>> on B<P, Q> { int get foo => 2; } \
         void use(B<int, List<int>> b) { b.foo; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
    assert_ambiguity_template(&report, "foo");
}

/// Pin 4 — `X extends num, Y extends void Function(X)`: no cycle; the
/// faces split at the formal. The to-bounds substitution is
/// position-blind (`void Function(num)`), so `E1`'s projection equals
/// `E2`'s written-bounds projection and the pair is ambiguous; the
/// default face's formal column read the lower map (`void
/// Function(Never)`) and made `E2` strictly more specific.
///
/// Oracle (3.13.7, #3487 probe): `ambiguous_extension_member_access` at
/// the access.
///
/// The strict projection reaches this face since #3562 mapped the
/// function-typed bound (`map_ty_strict`'s `Ty::Function` arm), so the
/// to-bounds switch decides it instead of the name-level pair test.
#[test]
fn acyclic_formal_reports_ambiguity() {
    let report = report_ops(
        "class B<S, T> {} \
         extension E1<X extends num, Y extends void Function(X)> on B<X, Y> { int get foo => 1; } \
         extension E2<P extends num, Q extends void Function(num)> on B<P, Q> { int get foo => 2; } \
         void use(B<int, void Function(num)> b) { b.foo; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
}

/// The silent twin: an unambiguous single best extension stays silent.
/// `Narrow<X extends int>` and `Wide<X extends num>` instantiate equally
/// against the receiver (`B<int>` both), so step 5 orders the
/// instantiate-to-bounds projections — `B<int>` <: `B<num>` one way and
/// not the other — and `Narrow` dominates. Both faces agree on these
/// acyclic scalar bounds; the pin keeps the to-bounds switch from
/// regressing the silence.
#[test]
fn single_best_extension_stays_silent() {
    let report = report_ops(
        "class B<S> {} \
         extension Wide<X extends num> on B<X> { int get foo => 1; } \
         extension Narrow<X extends int> on B<X> { int get foo => 2; } \
         void use(B<int> b) { b.foo; }",
    );
    assert_silent(&report);
}

/// The rider: a raw alias's defaults ground by the typedef's own
/// variance. `typedef G<X extends A<X>, Y extends X> = Y Function(X)`
/// stores `X` contravariant (its occurrence sits in the formal list), so
/// the component rewrite grounds `X` to `A<Never>` and the grounding
/// sweep hands `Y` the same — the oracle's raw `G<A<Never>, A<Never>>`
/// (#3469 probe). The covariant seeds this pass previously used read
/// `A<dynamic>` on both.
#[test]
fn alias_defaults_ground_by_typedef_variance() {
    let source = "class A<Z> {} \
         typedef G<X extends A<X>, Y extends X> = Y Function(X);";
    let (types, strings, _file) = types_for(source);
    let defaults = types
        .testing_typedef_defaults(strings.intern("G"))
        .expect("the alias's shape registers with its defaults");
    let decl = types
        .testing_decl_of(strings.intern("A"))
        .expect("`A` registers");
    let never = types.testing_table().intern(UninternedType::Never, false);
    let a_never = types.testing_table().intern(
        UninternedType::Interface {
            decl,
            args: vec![never],
        },
        false,
    );
    assert!(
        defaults.iter().all(|&default| {
            matches!(
                types.testing_table().get(default).kind(),
                TypeKind::Interface { .. }
            ) && default == a_never
        }),
        "both defaults must ground to `A<Never>`, got {} entries",
        defaults.len()
    );
}
