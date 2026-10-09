//! Least upper bound (`least_upper_bound.dart:391-672` with the documented
//! M1 fallbacks).

mod common;

use common::fixture;
use dartiq_types::{
    CoreDecls, TypeDeclId, TypeHierarchy, TypeParamId, TypeSystem, TypeTable, UninternedRecord,
    UninternedType,
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
