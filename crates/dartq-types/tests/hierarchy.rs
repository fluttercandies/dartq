//! Hierarchy snapshot validation: one test per [`TypeError`] path raised by
//! [`TypeHierarchyBuilder::build`] (spec §6, R5). `TypeError` carries no
//! `PartialEq` (its `SupertypeNotInterface` payload is a pool-position
//! [`TypeKind`]), so errors are asserted by matching.

use dartq_types::{
    CoreDecls, TypeDeclId, TypeError, TypeHierarchy, TypeId, TypeKind, TypeParamId, TypeSystem,
    TypeTable, UninternedType,
};

fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: args.to_vec(),
        },
        false,
    )
}

#[test]
fn duplicate_decl_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(object, Vec::new())
    .build(&table)
    .expect_err("duplicate declaration");
    assert!(matches!(err, TypeError::DuplicateDecl(d) if d == object));
}

#[test]
fn undeclared_supertype_target_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (a, missing) = (TypeDeclId::from_raw(10), TypeDeclId::from_raw(11));
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .supertype(a, itf(&table, missing, &[]))
    .build(&table)
    .expect_err("undeclared supertype target");
    assert!(matches!(err, TypeError::UndeclaredDecl(d) if d == missing));
}

#[test]
fn supertype_must_be_an_interface() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let a = TypeDeclId::from_raw(10);
    let dynamic = table.intern(UninternedType::Dynamic, false);
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .supertype(a, dynamic)
    .build(&table)
    .expect_err("non-interface supertype");
    assert!(matches!(
        err,
        TypeError::SupertypeNotInterface {
            decl,
            actual: TypeKind::Dynamic,
        } if decl == a
    ));
}

#[test]
fn supertype_arity_is_validated() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (a, iterable) = (TypeDeclId::from_raw(10), TypeDeclId::from_raw(11));
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .decl(iterable, vec![TypeParamId::from_raw(2)])
    .supertype(a, itf(&table, iterable, &[]))
    .build(&table)
    .expect_err("wrong supertype arity");
    assert!(matches!(
        err,
        TypeError::ArityMismatch {
            decl,
            expected: 1,
            actual: 0,
        } if decl == iterable
    ));
}

#[test]
fn foreign_type_parameter_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (a, iterable) = (TypeDeclId::from_raw(10), TypeDeclId::from_raw(11));
    // `iterable`'s own parameter, referenced from `a`'s edge: `a` does not
    // own it, so instantiation at query time could not bind it.
    let iterable_t = TypeParamId::from_raw(2);
    let iterable_of_t = table.intern(
        UninternedType::TypeParameter {
            parameter: iterable_t,
        },
        false,
    );
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .decl(iterable, vec![iterable_t])
    .supertype(a, itf(&table, iterable, &[iterable_of_t]))
    .build(&table)
    .expect_err("supertype references a foreign type parameter");
    assert!(matches!(
        err,
        TypeError::ForeignTypeParameter { decl, param } if decl == a && param == iterable_t
    ));
}

#[test]
fn cyclic_hierarchy_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (a, b) = (TypeDeclId::from_raw(10), TypeDeclId::from_raw(11));
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .decl(b, Vec::new())
    .supertype(a, itf(&table, b, &[]))
    .supertype(b, itf(&table, a, &[]))
    .build(&table)
    .expect_err("supertype cycle");
    let TypeError::CyclicHierarchy(cycle) = err else {
        panic!("expected CyclicHierarchy, got {err:?}");
    };
    // The reported path closes the cycle (first == last); which end the DFS
    // enters from follows `HashMap` iteration order, so only the set is
    // pinned here.
    assert_eq!(cycle.len(), 3);
    assert_eq!(cycle[0], cycle[2]);
    assert!(cycle.contains(&a) && cycle.contains(&b));
}

#[test]
fn object_supertype_edge_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let other = TypeDeclId::from_raw(10);
    // An edge *owned by* `Object` would put a class above the hierarchy
    // root and break the LUB unique-depth-1 anchor
    // (least_upper_bound.dart:327-357). Edges *to* `Object` stay fine.
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(other, Vec::new())
    .supertype(object, itf(&table, other, &[]))
    .build(&table)
    .expect_err("supertype edge owned by `Object`");
    assert!(matches!(err, TypeError::ObjectSupertype(d) if d == object));
}

#[test]
fn nullable_supertype_edge_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (a, animal) = (TypeDeclId::from_raw(10), TypeDeclId::from_raw(11));
    // A `?` edge would leak a nullable id into the LUB supertype closures
    // and break the max-unique-depth invariant.
    let animal_question = table.intern(
        UninternedType::Interface {
            decl: animal,
            args: Vec::new(),
        },
        true,
    );
    let err = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(a, Vec::new())
    .decl(animal, Vec::new())
    .supertype(a, animal_question)
    .build(&table)
    .expect_err("nullable supertype edge");
    assert!(matches!(err, TypeError::NullableSupertype(d) if d == a));
}

#[test]
fn instantiation_unites_the_supertype_template_nullability() {
    // `class A<T> implements Iterable<T?>`: the edge's argument node is
    // nullable, and instantiation unites nullabilities
    // (type_algebra.dart:97-106) — `A<num>` is a subtype of
    // `Iterable<num?>`, not of `Iterable<num>`.
    let table = TypeTable::new();
    let (object, future, function) = core();
    let (num, a, iterable) = (
        TypeDeclId::from_raw(10),
        TypeDeclId::from_raw(11),
        TypeDeclId::from_raw(12),
    );
    let (a_t, iterable_t) = (TypeParamId::from_raw(3), TypeParamId::from_raw(4));
    let t_question = table.intern(UninternedType::TypeParameter { parameter: a_t }, true);
    let object_none = itf(&table, object, &[]);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(num, Vec::new())
    .decl(a, vec![a_t])
    .decl(iterable, vec![iterable_t])
    .supertype(num, object_none)
    .supertype(iterable, object_none)
    .supertype(a, itf(&table, iterable, &[t_question]))
    .build(&table)
    .expect("valid hierarchy");
    let system = TypeSystem::new(&table, &hierarchy);
    let num_none = itf(&table, num, &[]);
    let num_question = table.intern(
        UninternedType::Interface {
            decl: num,
            args: Vec::new(),
        },
        true,
    );
    let a_of_num = itf(&table, a, &[num_none]);

    // The instantiated edge is `Iterable<num?>`: `num? <: num` is false, so
    // the `Iterable<num>` query must fail. Without the union this edge
    // collapses to `Iterable<num>` and both answers flip.
    assert!(!system.is_subtype(a_of_num, itf(&table, iterable, &[num_none])));
    assert!(system.is_subtype(a_of_num, itf(&table, iterable, &[num_question])));
}

fn core() -> (TypeDeclId, TypeDeclId, TypeDeclId) {
    (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
    )
}

#[test]
fn sealed_and_enum_facts_round_trip() {
    // The M2 exhaustiveness inputs: the sealed bit, the enum constant
    // count, and the direct-subclass edges restricted to sealed/enum
    // targets (types/sealed.dart `_createSubtypes`).
    let table = TypeTable::new();
    let (object, future, function, shape, circle, square, color) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
        TypeDeclId::from_raw(4),
        TypeDeclId::from_raw(5),
        TypeDeclId::from_raw(6),
        TypeDeclId::from_raw(7),
    );
    let object_none = itf(&table, object, &[]);
    let shape_ty = itf(&table, shape, &[]);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(shape, Vec::new())
    .decl(circle, Vec::new())
    .decl(square, Vec::new())
    .decl(color, Vec::new())
    .supertype(shape, object_none)
    .supertype(circle, shape_ty)
    .supertype(square, shape_ty)
    .supertype(color, object_none)
    .sealed(shape)
    .enumeration(color, 3)
    .build(&table)
    .expect("valid hierarchy");

    assert!(hierarchy.is_sealed(shape));
    assert!(!hierarchy.is_sealed(circle));
    assert_eq!(hierarchy.enum_constant_count(color), Some(3));
    assert_eq!(hierarchy.enum_constant_count(shape), None);
    // Direct subclasses list only sealed/enum targets, in insertion order.
    assert_eq!(
        hierarchy.direct_subclasses(shape),
        Some(&[circle, square][..])
    );
    assert_eq!(hierarchy.direct_subclasses(object), None);
}

#[test]
fn sealed_declaration_must_exist() {
    let table = TypeTable::new();
    let (object, future, function, ghost) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
        TypeDeclId::from_raw(4),
    );
    let error = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .sealed(ghost)
    .build(&table)
    .expect_err("undeclared sealed class must be rejected");
    assert!(matches!(error, dartq_types::TypeError::UndeclaredDecl(d) if d == ghost));
}

#[test]
fn duplicate_enum_declaration_is_rejected() {
    let table = TypeTable::new();
    let (object, future, function, color) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
        TypeDeclId::from_raw(4),
    );
    let error = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(color, Vec::new())
    .enumeration(color, 2)
    .enumeration(color, 3)
    .build(&table)
    .expect_err("duplicate enum declaration must be rejected");
    assert!(matches!(error, dartq_types::TypeError::DuplicateDecl(d) if d == color));
}
