//! Pins for #3442: instantiate-to-bounds must complete a parameter whose
//! bound mentions a *cycle member* with that member's substituted default —
//! the NDBD shape of `DefaultTypesBuilder._computeDefaultType`
//! (`pkg/analyzer/lib/src/summary2/default_types_builder.dart:128-187`,
//! mirrored by `calculateBounds` in `pkg/kernel/lib/src/bounds_checks.dart:
//! 213-295`), not the all-or-nothing Phi fallback of the progress-loop
//! fixpoint (`type_system.dart:735-811`).
//!
//! The defect: `typedef G<X extends A<X>, Y extends X>` — `X` is a
//! one-parameter SCC whose bound `A<X>` never grounds, so `X` defaults to
//! its substituted bound `A<dynamic>`; `Y` is *not* in that cycle (its bound
//! mentions `X`, but nothing in `X`'s bound mentions `Y`), yet the Phi
//! rewrite used to map every uncompleted parameter to `dynamic`, losing the
//! `X -> A<dynamic>` substitution the oracle's sequential grounding pass
//! (`default_types_builder.dart:164-180`) performs. The oracle answers
//! `Y = A<dynamic>`; dartq answered `Y = dynamic`.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::instantiate_to_bound::instantiate_type_formals_to_bounds_with;
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedType};

const A: TypeDeclId = TypeDeclId::from_raw(5);
const NUM: TypeDeclId = TypeDeclId::from_raw(7);

fn param(table: &TypeTable, parameter: TypeParamId) -> TypeId {
    table.intern(UninternedType::TypeParameter { parameter }, false)
}

fn a_of(table: &TypeTable, arg: TypeId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl: A,
            args: vec![arg],
        },
        false,
    )
}

fn plain(table: &TypeTable, decl: TypeDeclId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: Vec::new(),
        },
        false,
    )
}

/// Asserts `ty` is exactly `A<dynamic>`.
fn assert_a_of_dynamic(table: &TypeTable, ty: TypeId, who: &str) {
    let TypeKind::Interface { decl, args } = table.get(ty).kind() else {
        panic!(
            "{who}: expected `A<dynamic>`, got {:?}",
            table.get(ty).kind()
        );
    };
    assert!(decl == A && args.len() == 1, "{who}: expected `A<dynamic>`");
    assert!(
        matches!(
            table.get(table.arg(args.offset())).kind(),
            TypeKind::Dynamic
        ),
        "{who}: expected `A<dynamic>`"
    );
}

/// `typedef G<X extends A<X>, Y extends X>` — the reported defect. `X` is
/// the cycle; `Y` only *observes* it. The SCC rewrite touches `X`'s own
/// bound (`A<X>` -> `A<dynamic>`), and the sequential grounding pass
/// substitutes `X -> A<dynamic>` into `Y`'s bound — `Y` completes to
/// `A<dynamic>`, not `dynamic` (default_types_builder.dart:164-180).
#[test]
fn post_cycle_peer_takes_the_substituted_default() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(10), TypeParamId::from_raw(11));
    let x_bound = a_of(&table, param(&table, x));
    let y_bound = param(&table, x);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    });
    assert!(result.malbounded, "the self-cycle is still malbounded");
    assert_a_of_dynamic(&table, result.defaults[0], "X");
    assert_a_of_dynamic(&table, result.defaults[1], "Y");
}

/// `G<X extends num, Y extends X>` — regression guard: an acyclic chain
/// grounds both parameters to `num`, unchanged by the SCC formulation
/// (every singleton SCC rewrite is a no-op, then `X -> num` propagates).
#[test]
fn acyclic_chain_grounds_both_parameters() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(12), TypeParamId::from_raw(13));
    let num = plain(&table, NUM);
    let y_bound = param(&table, x);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
        if p == x {
            Some(num)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    });
    assert!(!result.malbounded);
    assert!(
        matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Interface { decl, .. } if decl == NUM
        ),
        "X defaults to `num`"
    );
    assert!(
        matches!(
            table.get(result.defaults[1]).kind(),
            TypeKind::Interface { decl, .. } if decl == NUM
        ),
        "Y defaults to `num`"
    );
}

/// `G<X extends A<X>>` — regression guard: a lone one-parameter SCC keeps
/// its head through the dynamic rewrite; the default is `A<dynamic>`
/// (default_types_builder.dart:144-162).
#[test]
fn lone_self_cycle_keeps_its_head() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(14);
    let bound = a_of(&table, param(&table, x));
    let result =
        instantiate_type_formals_to_bounds_with(&table, &[x], |p| (p == x).then_some(bound));
    assert!(result.malbounded);
    assert_a_of_dynamic(&table, result.defaults[0], "X");
}

/// `G<X extends Y, Y extends X>` — a two-parameter mutual cycle: every
/// member's bound is rewritten through the same-component map, so both
/// defaults are bare `dynamic`.
#[test]
fn mutual_cycle_rewrites_both_members_to_dynamic() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(15), TypeParamId::from_raw(16));
    let x_bound = param(&table, y);
    let y_bound = param(&table, x);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    });
    assert!(result.malbounded);
    assert!(matches!(
        table.get(result.defaults[0]).kind(),
        TypeKind::Dynamic
    ));
    assert!(matches!(
        table.get(result.defaults[1]).kind(),
        TypeKind::Dynamic
    ));
}

/// `G<X>` — an unbounded parameter reads `dynamic`
/// (default_types_builder.dart:136-140).
#[test]
fn unbounded_parameter_defaults_to_dynamic() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(17);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x], |_| None);
    assert!(!result.malbounded);
    assert!(matches!(
        table.get(result.defaults[0]).kind(),
        TypeKind::Dynamic
    ));
}

/// `G<X extends A<X>, Y extends X, Z extends Y>` — the substitution is
/// transitive, not one-hop: `X -> A<dynamic>` lands in `Y`'s bound when `Y`
/// is processed, and `Y -> A<dynamic>` lands in `Z`'s bound one sweep later
/// (default_types_builder.dart:164-180, declaration-order propagation).
#[test]
fn post_cycle_chain_grounds_transitively() {
    let table = TypeTable::new();
    let (x, y, z) = (
        TypeParamId::from_raw(18),
        TypeParamId::from_raw(19),
        TypeParamId::from_raw(20),
    );
    let x_bound = a_of(&table, param(&table, x));
    let y_bound = param(&table, x);
    let z_bound = param(&table, y);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y, z], |p| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else if p == z {
            Some(z_bound)
        } else {
            None
        }
    });
    assert!(result.malbounded, "the reachable chain is still malbounded");
    assert_a_of_dynamic(&table, result.defaults[0], "X");
    assert_a_of_dynamic(&table, result.defaults[1], "Y");
    assert_a_of_dynamic(&table, result.defaults[2], "Z");
}
