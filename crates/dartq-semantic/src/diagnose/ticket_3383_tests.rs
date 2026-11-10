//! The #3383 pins: the typedef-alias binder-bound feed.
//!
//! Oracle semantics: instantiating a parameterized alias expands the right-
//! hand side AND rebinds every function signature's own `<T>` group fresh
//! (`_FreshTypeParametersSubstitutor.freshTypeParameters` +
//! `visitFunctionType`, upstream Dart SDK source, pkg/analyzer/lib/src/
//! dart/element/type_algebra.dart:241-271 and :416-460). Each fresh
//! binder's declared bound is the template's bound substituted by the
//! use's argument map — `F<num>` yields a `Y` bounded by `num`, `F<int>`
//! a different `Y` bounded by `int`, and the shared interned shape never
//! leaks the alias's own parameter leaf as the bound.
//!
//! The defect the pins close: `flatten_rhs` minted the fresh binder ids
//! but wrote `ParamFacts { bound: None }`, so a bound the alias's own
//! parameter group declared (`void Function<Y extends X>()`) never
//! reached `facts` at all — the type-variable bound rule had nothing to
//! decide with where the oracle decides.
//!
//! The pins read `facts` through the mounted snapshot (`types_for`):
//! `class A<T extends F<num>>` feeds `F<num>` through `clause_arg_ty`'s
//! typedef arm, and `testing_param_bound_of` gives `T`'s expanded bound —
//! the function signature whose binder's own bound the walk reads back.

use super::test_rig::types_for;
use crate::resolved_ops::LibraryTypes;
use dartq_source::FileId;
use dartq_types::{TypeId, TypeKind, UninternedType};

/// `file`'s type-parameter `spelling`'s expanded bound inside `types` —
/// the clause-capture feed's answer (`T extends F<num>` maps the `F<num>`
/// expansion, a `Function` signature carrying the fresh binder).
fn param_bound(
    types: &LibraryTypes,
    strings: &dartq_source::StringTable,
    file: FileId,
    spelling: &str,
) -> TypeId {
    types
        .testing_param_bound_of(file, strings.intern(spelling))
        .unwrap_or_else(|| panic!("`{spelling}`'s bound must map"))
}

/// Interns the named interface (`num`, `int`) of `types`'s registry —
/// the expected bound TypeId a pin compares against.
fn named_ty(types: &LibraryTypes, strings: &dartq_source::StringTable, name: &str) -> TypeId {
    let decl = types
        .testing_decl_of(strings.intern(name))
        .unwrap_or_else(|| panic!("`{name}` registers"));
    types.testing_table().intern(
        UninternedType::Interface {
            decl,
            args: Vec::new(),
        },
        false,
    )
}

/// `typedef F<X> = void Function<Y extends X>()` used as `F<num>` in a
/// parameter bound: the expanded `Function`'s `Y` binder carries bound
/// `num` — the substituted argument, not the template's `X` leaf and not
/// the former `None`.
#[test]
fn ticket_3383_typedef_binder_bound_reaches_facts_substituted() {
    let source = r#"
typedef F<X> = void Function<Y extends X>();
class A<T extends F<num>> {}
"#;
    let (types, strings, file) = types_for(source);
    let bound = param_bound(&types, &strings, file, "T");
    // The bound is the expanded function signature.
    assert!(
        matches!(
            types.testing_table().get(bound).kind(),
            TypeKind::Function { .. }
        ),
        "`F<num>`'s bound must be the expanded function type, got {:?}",
        types.testing_table().get(bound).kind()
    );
    let binders = types.testing_fn_binder_bounds(bound);
    assert_eq!(binders.len(), 1, "one binder `<Y>`");
    let num = named_ty(&types, &strings, "num");
    assert_eq!(
        binders[0],
        Some(num),
        "`Y`'s bound reads `num` — the substituted argument"
    );
}

/// The bound rides through an `Interface` argument: `typedef F<X> =
/// List<void Function<Y extends X>()>` — `Y`'s `num` bound still lands
/// on the nested signature's fresh binder.
#[test]
fn ticket_3383_nested_interface_arg_binder_bound_reaches_facts() {
    let source = r#"
typedef F<X> = List<void Function<Y extends X>()>;
class A<T extends F<num>> {}
"#;
    let (types, strings, file) = types_for(source);
    let bound = param_bound(&types, &strings, file, "T");
    assert!(
        matches!(
            types.testing_table().get(bound).kind(),
            TypeKind::Interface { .. }
        ),
        "`F<num>`'s bound must be the `List<..>` instantiation"
    );
    let binders = types.testing_fn_binder_bounds(bound);
    assert_eq!(binders.len(), 1, "one nested binder `<Y>`");
    let num = named_ty(&types, &strings, "num");
    assert_eq!(
        binders[0],
        Some(num),
        "the nested `Y` reads bound `num` through the Interface arg"
    );
}

/// A bound-omitted parameter keeps `None` — no `dynamic` is synthesized:
/// `typedef F<X> = void Function<Y>()` used as `F<num>` yields a binder
/// whose `facts.bound` stays unset (the under-report direction — the
/// unbounded read, never an invented constraint).
#[test]
fn ticket_3383_unbounded_binder_keeps_none() {
    let source = r#"
typedef F<X> = void Function<Y>();
class A<T extends F<num>> {}
"#;
    let (types, strings, file) = types_for(source);
    let bound = param_bound(&types, &strings, file, "T");
    let binders = types.testing_fn_binder_bounds(bound);
    assert_eq!(binders.len(), 1, "one binder `<Y>`");
    assert_eq!(
        binders[0], None,
        "an unwritten bound stays `None`, never `dynamic`"
    );
}

/// Two uses instantiate the same shared shape — each gets its own fresh
/// binder and its own substituted bound: `F<num>`'s `Y` reads `num` and
/// `F<int>`'s reads `int`, so the alias's own `X` leaf never stands in as
/// the bound through the shared template.
#[test]
fn ticket_3383_two_uses_keep_per_use_bounds() {
    let source = r#"
typedef F<X> = void Function<Y extends X>();
class A<T extends F<num>> {}
class B<U extends F<int>> {}
"#;
    let (types, strings, file) = types_for(source);
    let num = named_ty(&types, &strings, "num");
    let int = named_ty(&types, &strings, "int");

    let t_bound = param_bound(&types, &strings, file, "T");
    let t_binders = types.testing_fn_binder_bounds(t_bound);
    assert_eq!(t_binders.len(), 1);
    assert_eq!(t_binders[0], Some(num), "`F<num>`'s `Y` reads bound `num`");

    let u_bound = param_bound(&types, &strings, file, "U");
    let u_binders = types.testing_fn_binder_bounds(u_bound);
    assert_eq!(u_binders.len(), 1);
    assert_eq!(u_binders[0], Some(int), "`F<int>`'s `Y` reads bound `int`");

    // The two expansions are distinct interned signatures — the per-use
    // fresh binder is what lets the facts differ.
    assert_ne!(
        t_bound, u_bound,
        "the two uses mint different binders — no shared-shape leak"
    );
}
