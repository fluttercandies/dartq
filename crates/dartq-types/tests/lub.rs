//! Least upper bound (`least_upper_bound.dart:391-672` with the documented
//! M1 fallbacks).

mod common;

use common::{Fixture, fixture};
use dartq_types::{
    CoreDecls, TypeDeclId, TypeHierarchy, TypeId, TypeParamId, TypeSystem, TypeTable,
    UninternedRecord, UninternedType,
};

#[test]
fn subtype_related_pairs_return_the_supertype() {
    let fx = fixture();
    let sys = fx.system();
    assert_eq!(sys.least_upper_bound(fx.int(), fx.int()), fx.int());
    assert_eq!(sys.least_upper_bound(fx.int(), fx.num()), fx.num());
    assert_eq!(sys.least_upper_bound(fx.cat(), fx.animal()), fx.animal());
    assert_eq!(
        sys.least_upper_bound(fx.list_of(fx.int(), false), fx.iterable_of(fx.int(), false)),
        fx.iterable_of(fx.int(), false)
    );
}

#[test]
fn nullability_merges_to_question() {
    let fx = fixture();
    let sys = fx.system();
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.int_question()),
        fx.int_question()
    );
    assert_eq!(
        sys.least_upper_bound(fx.int_question(), fx.int()),
        fx.int_question()
    );
    assert_eq!(
        sys.least_upper_bound(fx.int_question(), fx.int_question()),
        fx.int_question()
    );
    assert_eq!(
        sys.least_upper_bound(fx.cat_question(), fx.dog()),
        fx.itf(fx.d.animal, &[], true)
    );
}

#[test]
fn interface_hierarchy_fallback() {
    let fx = fixture();
    let sys = fx.system();
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.string()),
        fx.itf(fx.d.object, &[], false)
    );
    assert_eq!(
        sys.least_upper_bound(fx.int_question(), fx.string_question()),
        fx.itf(fx.d.object, &[], true)
    );
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.string_question()),
        fx.itf(fx.d.object, &[], true)
    );
    // Same declaration: pointwise covariant merge.
    assert_eq!(
        sys.least_upper_bound(fx.list_of(fx.int(), false), fx.list_of(fx.double(), false)),
        fx.list_of(fx.num(), false)
    );
    // Cat vs Dog meet at Animal, not Object.
    assert_eq!(sys.least_upper_bound(fx.cat(), fx.dog()), fx.animal());
}

#[test]
fn top_and_bottom_types() {
    let fx = fixture();
    let sys = fx.system();
    assert_eq!(sys.least_upper_bound(fx.dynamic(), fx.int()), fx.dynamic());
    assert_eq!(sys.least_upper_bound(fx.int(), fx.void_()), fx.void_());
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.object_question()),
        fx.object_question()
    );
    assert_eq!(sys.least_upper_bound(fx.dynamic(), fx.void_()), fx.void_());
    assert_eq!(sys.least_upper_bound(fx.never(), fx.int()), fx.int());
    assert_eq!(sys.least_upper_bound(fx.int(), fx.never()), fx.int());
    // UP(Null, T) = T? (least_upper_bound.dart:505-513).
    assert_eq!(
        sys.least_upper_bound(fx.null(), fx.int()),
        fx.int_question()
    );
    assert_eq!(
        sys.least_upper_bound(fx.null(), fx.int_question()),
        fx.int_question()
    );
    assert_eq!(sys.least_upper_bound(fx.never(), fx.null()), fx.null());
}

#[test]
fn future_or_merging() {
    let fx = fixture();
    let sys = fx.system();
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.future_or_of(fx.int(), false)),
        fx.future_or_of(fx.int(), false)
    );
    assert_eq!(
        sys.least_upper_bound(fx.future_of(fx.int()), fx.future_or_of(fx.int(), false)),
        fx.future_or_of(fx.int(), false)
    );
    assert_eq!(
        sys.least_upper_bound(
            fx.future_or_of(fx.int(), false),
            fx.future_or_of(fx.string(), false)
        ),
        fx.future_or_of(fx.itf(fx.d.object, &[], false), false)
    );
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.future_or_of(fx.string(), false)),
        fx.future_or_of(fx.itf(fx.d.object, &[], false), false)
    );
    assert_eq!(
        sys.least_upper_bound(fx.int(), fx.future_or_of(fx.int_question(), false)),
        fx.future_or_of(fx.int_question(), false)
    );
}

#[test]
fn type_parameters_fall_back_to_object_question() {
    let fx = fixture();
    let sys = fx.system();
    let x = fx.type_param(fx.d.iterable_t);
    let y = fx.type_param(fx.d.list_e);
    assert_eq!(sys.least_upper_bound(x, y), fx.object_question());
    // M1: no bounds, so even `X` vs `Object` goes through the default-bound
    // fallback.
    assert_eq!(
        sys.least_upper_bound(x, fx.itf(fx.d.object, &[], false)),
        fx.object_question()
    );
}

#[test]
fn function_types_fall_back_to_bare_function() {
    let fx = fixture();
    let sys = fx.system();
    let f = fx.fn_type(&[fx.int()], fx.int());
    let g = fx.fn_type(&[], fx.string());
    let bare = fx.itf(fx.d.function, &[], false);
    // Identical functions are caught by the equality shortcut; distinct ones
    // use the M1 coarse fallback.
    assert_eq!(sys.least_upper_bound(f, g), bare);
    assert_eq!(
        sys.least_upper_bound(f, fx.int()),
        fx.itf(fx.d.object, &[], false)
    );
    assert_eq!(sys.least_upper_bound(f, bare), bare);
}

#[test]
fn tied_max_depths_walk_down_to_the_unique_candidate() {
    // Diamond: X and Y both extend M1 and M2. The supertype intersection is
    // {M1, M2, Object} with M1/M2 tied at depth 2, so the rule walks down a
    // level to the unique depth-1 candidate, `Object`
    // (`_computeTypeAtMaxUniqueDepth`, least_upper_bound.dart:327-357).
    let table = TypeTable::new();
    let (object, future, function) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
    );
    let (m1, m2, x, y) = (
        TypeDeclId::from_raw(4),
        TypeDeclId::from_raw(5),
        TypeDeclId::from_raw(6),
        TypeDeclId::from_raw(7),
    );
    let itf = |decl: TypeDeclId| {
        table.intern(
            UninternedType::Interface {
                decl,
                args: Vec::new(),
            },
            false,
        )
    };
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(m1, Vec::new())
    .decl(m2, Vec::new())
    .decl(x, Vec::new())
    .decl(y, Vec::new())
    .supertype(m1, itf(object))
    .supertype(m2, itf(object))
    .supertype(x, itf(m1))
    .supertype(x, itf(m2))
    .supertype(y, itf(m1))
    .supertype(y, itf(m2))
    .build(&table)
    .expect("fixture hierarchy is valid");
    let sys = TypeSystem::new(&table, &hierarchy);
    assert_eq!(sys.least_upper_bound(itf(x), itf(y)), itf(object));
}

#[test]
fn records_fall_back_to_object() {
    let fx = fixture();
    let sys = fx.system();
    let r1 = fx.record(&[fx.int()], &[]);
    let r2 = fx.record(&[fx.string()], &[]);
    let object = fx.itf(fx.d.object, &[], false);
    assert_eq!(sys.least_upper_bound(r1, r2), object);
    assert_eq!(sys.least_upper_bound(r1, fx.string()), object);
}

#[test]
fn record_vs_record_class_returns_the_record_class() {
    // UP((...), Record) = Record (least_upper_bound.dart:637-644). Needs the
    // `Record` class declared in the snapshot; the shared fixture has
    // `record: None` (the fallback test above pins that path).
    let table = TypeTable::new();
    let (object, future, function, int, record) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
        TypeDeclId::from_raw(4),
        TypeDeclId::from_raw(5),
    );
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: Some(record),
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(1)])
    .decl(function, Vec::new())
    .decl(int, Vec::new())
    .decl(record, Vec::new())
    .build(&table)
    .expect("fixture hierarchy is valid");
    let sys = TypeSystem::new(&table, &hierarchy);

    let int_ty = sys.table().intern(
        UninternedType::Interface {
            decl: int,
            args: Vec::new(),
        },
        false,
    );
    let record_class = sys.table().intern(
        UninternedType::Interface {
            decl: record,
            args: Vec::new(),
        },
        false,
    );
    let bare_object = sys.table().intern(
        UninternedType::Interface {
            decl: object,
            args: Vec::new(),
        },
        false,
    );
    let pair = sys
        .table()
        .intern_record(
            UninternedRecord {
                positional: vec![int_ty, int_ty],
                named: Vec::new(),
            },
            false,
        )
        .expect("valid fixture record");
    assert_eq!(sys.least_upper_bound(pair, record_class), record_class);
    assert_eq!(sys.least_upper_bound(record_class, pair), record_class);

    // A record shape is not the `Record` class: two distinct shapes stay on
    // the official record-vs-other fallback
    // (`UP(Object, T2)`, least_upper_bound.dart:652-659).
    let other = sys
        .table()
        .intern_record(
            UninternedRecord {
                positional: vec![int_ty],
                named: Vec::new(),
            },
            false,
        )
        .expect("valid fixture record");
    assert_eq!(sys.least_upper_bound(pair, other), bare_object);

    // Record vs an unrelated class never sees the Record-class rule.
    assert_eq!(sys.least_upper_bound(pair, int_ty), bare_object);
}

#[test]
fn both_object_types_pick_by_more_top() {
    // OBJECT(T1) && OBJECT(T2) dispatches ahead of the single-object rules
    // (least_upper_bound.dart:526-560): `FutureOr<Object>` is an object type
    // (`isObject(FutureOr<T>) = isObject(T)`, type_system.dart:1236-1254), so
    // the pair resolves by MORETOP to plain `Object` instead of returning the
    // first object operand.
    let fx = fixture();
    let sys = fx.system();
    let object = fx.itf(fx.d.object, &[], false);
    let future_or_object = fx.future_or_of(object, false);
    assert_eq!(sys.least_upper_bound(future_or_object, object), object);
    assert_eq!(sys.least_upper_bound(object, future_or_object), object);
}

/// One fixture build observed end to end: the whole-hierarchy debug dump
/// (supertype edges plus the crate-private precomputed depth map) and a LUB
/// matrix over the interned ids. Two builds of the same declarations in the
/// same order must produce identical values.
fn build_observation(fx: &Fixture) -> (String, Vec<(TypeId, TypeId, TypeId)>) {
    let sys = fx.system();
    let object = fx.itf(fx.d.object, &[], false);
    let matrix = vec![
        // Resolves through the supertype-set intersection and the
        // depth-ranked candidate walk (cat/dog meet at Animal, not Object).
        (
            fx.cat(),
            fx.dog(),
            sys.least_upper_bound(fx.cat(), fx.dog()),
        ),
        (
            fx.future_or_of(object, false),
            object,
            sys.least_upper_bound(fx.future_or_of(object, false), object),
        ),
        (
            fx.int(),
            fx.string_question(),
            sys.least_upper_bound(fx.int(), fx.string_question()),
        ),
        (
            fx.cat_question(),
            fx.dog(),
            sys.least_upper_bound(fx.cat_question(), fx.dog()),
        ),
        (
            fx.list_of(fx.int(), false),
            fx.iterable_of(fx.int(), false),
            sys.least_upper_bound(fx.list_of(fx.int(), false), fx.iterable_of(fx.int(), false)),
        ),
    ];
    (format!("{:?}", fx.hierarchy), matrix)
}

#[test]
fn identical_hierarchy_builds_are_byte_identical() {
    // Pins the #154 determinism claim: building the identical hierarchy twice
    // (same declaration order) yields the same snapshot and the same interned
    // ids, so `supertypes()` slices agree per declaration, the debug dump
    // (which includes the depth map `depth()` feeds) matches byte-for-byte,
    // and the LUB matrix answers identically on both builds.
    let fx1 = fixture();
    let fx2 = fixture();
    for d in [
        fx1.d.object,
        fx1.d.future,
        fx1.d.function,
        fx1.d.num,
        fx1.d.int,
        fx1.d.double,
        fx1.d.string,
        fx1.d.boolean,
        fx1.d.iterable,
        fx1.d.list,
        fx1.d.animal,
        fx1.d.cat,
        fx1.d.dog,
    ] {
        assert_eq!(fx1.hierarchy.supertypes(d), fx2.hierarchy.supertypes(d));
    }
    let (dump1, matrix1) = build_observation(&fx1);
    let (dump2, matrix2) = build_observation(&fx2);
    assert_eq!(dump1, dump2);
    assert_eq!(matrix1, matrix2);
}
