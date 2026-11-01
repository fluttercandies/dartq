//! Pins for #3311: instantiate-to-bounds when a type formal's written bound
//! goes through a `typedef` alias.
//!
//! The oracle never lets an alias reach the fixed point: for
//! `typedef F<T> = List<T>; class C<X extends F<X>> {}` the element model's
//! `typeParameter.bound` is the *expanded* `List<X>`, with the alias kept
//! only as a decoration on the expanded type (`instantiateTypeFormalsToBounds`
//! reads `typeParameter.bound`, `type_system.dart:786`). [`crate::TypeKind`]
//! carries no alias kind, so this crate's seam is the bound lookup: whoever
//! feeds [`crate::instantiate_type_formals_to_bounds_with`] must hand over the
//! unfolded shape (an interface head over the alias's written argument, the
//! signature of a function-RHS alias, and so on).
//!
//! These pins fix that seam's answers for the alias families, so the fixed
//! point cannot regress while the alias-expanding providers are built:
//!
//! * `C<X extends F<X>>` — the application mentions its own parameter, so the
//!   Phi substitution rewrites the alias argument (`List<dynamic>`);
//! * `C<X extends F<int>>` — a concrete argument never enters the Phi step;
//! * `C<X extends F<Y>, Y extends num>` — a peer reached *through* the alias
//!   is a candidate, not an opaque leaf, so both parameters ground;
//! * `C<X extends F<Y>, Y extends F<X>>` — a cycle running through two alias
//!   applications rewrites every member;
//! * `typedef F<T> = void Function(T)` used as the bound — the substitution
//!   reaches inside the signature (`void Function(Never)`, the lower map
//!   under the formal column's contravariant flip), never stopping at the
//!   head.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::instantiate_to_bound::instantiate_type_formals_to_bounds_with;
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedFunction, UninternedType};

const LIST: TypeDeclId = TypeDeclId::from_raw(5);
const INT: TypeDeclId = TypeDeclId::from_raw(6);
const NUM: TypeDeclId = TypeDeclId::from_raw(7);

fn plain(table: &TypeTable, decl: TypeDeclId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: Vec::new(),
        },
        false,
    )
}

/// `List<arg>` — the shape `typedef F<T> = List<T>` unfolds a bound into.
fn list_of(table: &TypeTable, arg: TypeId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl: LIST,
            args: vec![arg],
        },
        false,
    )
}

fn param(table: &TypeTable, parameter: TypeParamId) -> TypeId {
    table.intern(UninternedType::TypeParameter { parameter }, false)
}

/// Asserts `ty` is `List<_>` and returns the argument.
fn list_arg(table: &TypeTable, ty: TypeId) -> TypeId {
    match table.get(ty).kind() {
        TypeKind::Interface { decl, args } if decl == LIST && args.len() == 1 => {
            table.arg(args.offset())
        }
        other => panic!("expected `List<_>`, got {other:?}"),
    }
}

/// `class C<X extends F<X>> {}` over `typedef F<T> = List<T>;` — the expanded
/// bound `List<X>` mentions its own parameter, so the fixpoint cannot ground
/// it and the Phi substitution rewrites the alias argument: the default is
/// `List<dynamic>`, never the bare `dynamic` a dropped bound leaves behind.
#[test]
fn alias_self_bound_takes_the_phi_default_through_the_expanded_head() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(10);
    let bound = list_of(&table, param(&table, x));
    let result =
        instantiate_type_formals_to_bounds_with(&table, &[x], |p| (p == x).then_some(bound));
    assert!(
        result.malbounded,
        "a bound mentioning its own parameter is malbounded"
    );
    assert!(
        matches!(
            table.get(list_arg(&table, result.defaults[0])).kind(),
            TypeKind::Dynamic
        ),
        "the default is the substituted alias application `List<dynamic>`"
    );
}

/// `class C<X extends F<int>> {}` — the alias applied to a *concrete*
/// argument is ground on arrival: the default is that application verbatim and
/// no Phi substitution runs.
#[test]
fn alias_bound_with_a_concrete_argument_stays_ground() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(11);
    let bound = list_of(&table, plain(&table, INT));
    let result =
        instantiate_type_formals_to_bounds_with(&table, &[x], |p| (p == x).then_some(bound));
    assert!(
        !result.malbounded,
        "`X extends F<int>` needs no Phi substitution"
    );
    assert!(
        matches!(
            table.get(list_arg(&table, result.defaults[0])).kind(),
            TypeKind::Interface { decl, .. } if decl == INT
        ),
        "the default keeps the alias's written argument `int`"
    );
}

/// `class C<X extends F<Y>, Y extends num> {}` — a peer reached *through* the
/// alias grounds both parameters (`List<num>`, `num`) with no malboundedness:
/// the alias argument is a candidate, not an opaque leaf.
#[test]
fn alias_peer_bound_grounds_through_the_alias_argument() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(12), TypeParamId::from_raw(13));
    let x_bound = list_of(&table, param(&table, y));
    let y_bound = plain(&table, NUM);
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    });
    assert!(!result.malbounded, "a peer chain through the alias grounds");
    assert!(
        matches!(
            table.get(list_arg(&table, result.defaults[0])).kind(),
            TypeKind::Interface { decl, .. } if decl == NUM
        ),
        "`X` defaults to `List<num>`, the alias applied to `Y`'s default"
    );
    assert!(
        matches!(
            table.get(result.defaults[1]).kind(),
            TypeKind::Interface { decl, .. } if decl == NUM
        ),
        "`Y` defaults to its own bound `num`"
    );
}

/// `class C<X extends F<Y>, Y extends F<X>> {}` — a cycle running through two
/// alias applications: both members are malbounded and both Phi-rewrite to
/// `List<dynamic>`.
#[test]
fn alias_cycle_rewrites_every_member_through_the_expanded_head() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(14), TypeParamId::from_raw(15));
    let x_bound = list_of(&table, param(&table, y));
    let y_bound = list_of(&table, param(&table, x));
    let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    });
    assert!(result.malbounded, "the alias cycle never grounds");
    assert!(
        matches!(
            table.get(list_arg(&table, result.defaults[0])).kind(),
            TypeKind::Dynamic
        ),
        "`X` defaults to the Phi form `List<dynamic>`"
    );
    assert!(
        matches!(
            table.get(list_arg(&table, result.defaults[1])).kind(),
            TypeKind::Dynamic
        ),
        "`Y` defaults to the Phi form `List<dynamic>`"
    );
}

/// `class C<X extends F<X>> {}` over `typedef F<T> = void Function(T);` — a
/// function-RHS alias. The expanded function type mentions `X` in its formal
/// column, so the fixpoint sees the self-cycle and substitutes *inside* the
/// signature: the default is `void Function(Never)` with the alias's own
/// quantifier already gone, never the unrewritten `void Function(X)` a walk
/// that stopped at the head would leave behind. The formal column reads the
/// *lower* map: `_UpperLowerReplacementVisitor` flips the variance around
/// the formal list (default_types_builder.dart:242-258/444-451), so even a
/// covariant-seeded parameter grounds to `NeverTypeImpl.instance` there —
/// the oracle's answer, where the pre-#3468 `dynamic`-only map read
/// `dynamic`.
#[test]
fn alias_function_rhs_bound_substitutes_inside_the_signature() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(16);
    let void = table.intern(UninternedType::Void, false);
    let bound = table
        .intern_function(
            UninternedFunction {
                return_type: void,
                type_parameters: Vec::new(),
                positional: vec![param(&table, x)],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("the fixture signature is well formed");
    let result =
        instantiate_type_formals_to_bounds_with(&table, &[x], |p| (p == x).then_some(bound));
    assert!(
        result.malbounded,
        "the function-form self-bound never grounds"
    );
    let TypeKind::Function { signature } = table.get(result.defaults[0]).kind() else {
        panic!("the default keeps the function head");
    };
    let sig = table.signature(signature);
    assert!(
        sig.type_parameters.is_empty(),
        "the alias's own quantifier never reaches the declaration"
    );
    assert!(
        matches!(
            table.get(table.arg(sig.positional.offset())).kind(),
            TypeKind::Never
        ),
        "the formal column reads the directed substitution's lower map `Never`"
    );
}
