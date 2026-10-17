//! The subtype rule table, ported from
//! `subtype.dart:262-523`. Test names cite the rule numbers.

mod common;

use common::{Fixture, fixture, named};
use dartq_types::{
    CoreDecls, TypeDeclId, TypeHierarchy, TypeId, TypeParamId, TypeSystem, TypeTable,
    UninternedFunction, UninternedRecord, UninternedType,
};

#[test]
fn rule0a_reflexivity() {
    let fx = fixture();
    let sys = fx.system();
    let list = fx.list_of(fx.int(), false);
    assert!(sys.is_subtype(fx.int(), fx.int()));
    assert!(sys.is_subtype(list, list));
    assert!(sys.is_subtype(fx.never(), fx.never()));
}

#[test]
fn rule0c_invalid_type_suppresses_in_both_directions() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.invalid(), fx.int()));
    assert!(sys.is_subtype(fx.int(), fx.invalid()));
    assert!(sys.is_subtype(fx.invalid(), fx.invalid()));
    assert!(sys.is_subtype(fx.invalid(), fx.object_question()));
}

#[test]
fn rule1_right_top() {
    let fx = fixture();
    let sys = fx.system();
    for top in [fx.dynamic(), fx.void_(), fx.object_question()] {
        assert!(sys.is_subtype(fx.int(), top), "int <: top");
        assert!(sys.is_subtype(fx.cat(), top), "Cat <: top");
        assert!(sys.is_subtype(fx.never(), top), "Never <: top");
    }
}

#[test]
fn rule2_left_top_follows_object_question() {
    let fx = fixture();
    let sys = fx.system();
    // dynamic/void <: T iff Object? <: T.
    assert!(sys.is_subtype(fx.dynamic(), fx.dynamic()));
    assert!(sys.is_subtype(fx.dynamic(), fx.void_()));
    assert!(sys.is_subtype(fx.dynamic(), fx.object_question()));
    assert!(!sys.is_subtype(fx.dynamic(), fx.int()));
    assert!(!sys.is_subtype(fx.void_(), fx.int()));
    assert!(!sys.is_subtype(fx.void_(), fx.animal()));
    // Subtype vs assignability split: `dynamic` flows into `int` by
    // assignment (type_system.dart:924) but is not a subtype.
    assert!(sys.is_assignable(fx.dynamic(), fx.int(), false));
    // `dynamic` is Right Top, so even `void <: dynamic` holds (rule 1).
    assert!(sys.is_subtype(fx.void_(), fx.dynamic()));
}

#[test]
fn rule3_left_bottom() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.never(), fx.int()));
    assert!(sys.is_subtype(fx.never(), fx.animal()));
    assert!(sys.is_subtype(fx.never(), fx.future_or_of(fx.int(), false)));
    assert!(!sys.is_bottom(fx.null()));
    // `Never?` is interned as `Null`, so `Null <: T?` covers it.
    assert!(sys.is_subtype(fx.null(), fx.int_question()));
}

#[test]
fn rule4_right_object() {
    let fx = fixture();
    let sys = fx.system();
    let object = fx.itf(fx.d.object, &[], false);
    assert!(sys.is_subtype(fx.int(), object));
    assert!(sys.is_subtype(fx.cat(), object));
    assert!(sys.is_subtype(fx.future_or_of(fx.int(), false), object));
    assert!(sys.is_subtype(fx.never(), object));
    assert!(!sys.is_subtype(fx.int_question(), object));
    assert!(!sys.is_subtype(fx.null(), object));
    assert!(!sys.is_subtype(fx.dynamic(), object));
    assert!(!sys.is_subtype(fx.void_(), object));
    assert!(!sys.is_subtype(fx.future_or_of(fx.int_question(), false), object));
}

#[test]
fn rule5_left_null() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.null(), fx.null()));
    assert!(sys.is_subtype(fx.null(), fx.int_question()));
    assert!(sys.is_subtype(fx.null(), fx.list_of(fx.int(), true)));
    // Rule 1 (Right Top) catches `dynamic` before Left Null runs.
    assert!(sys.is_subtype(fx.null(), fx.dynamic()));
    assert!(sys.is_subtype(fx.null(), fx.future_or_of(fx.int_question(), false)));
    assert!(!sys.is_subtype(fx.null(), fx.int()));
    assert!(!sys.is_subtype(fx.null(), fx.never()));
    assert!(!sys.is_subtype(fx.null(), fx.future_or_of(fx.int(), false)));
    assert!(!sys.is_subtype(fx.null(), fx.animal()));
}

#[test]
fn rule6_left_future_or() {
    let fx = fixture();
    let sys = fx.system();
    // Rule 6 needs BOTH `int <: T1` and `Future<int> <: T1`; `Animal` and
    // `num` accept the int leg but not the Future leg.
    assert!(!sys.is_subtype(fx.future_or_of(fx.int(), false), fx.animal()));
    assert!(!sys.is_subtype(fx.future_or_of(fx.int(), false), fx.num()));
    // Future<int> is not an int, and int is not Null.
    assert!(!sys.is_subtype(fx.future_or_of(fx.int(), false), fx.int_question()));
    assert!(!sys.is_subtype(
        fx.future_or_of(fx.int(), false),
        fx.list_of(fx.int(), false)
    ));
    assert!(sys.is_subtype(
        fx.future_or_of(fx.int(), false),
        fx.future_or_of(fx.num(), false)
    ));
    assert!(!sys.is_subtype(
        fx.future_or_of(fx.int(), false),
        fx.future_or_of(fx.string(), false)
    ));
}

#[test]
fn rule7_left_nullable() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.int(), fx.int_question()));
    assert!(!sys.is_subtype(fx.int_question(), fx.int()));
    assert!(sys.is_subtype(fx.int_question(), fx.int_question()));
    // `Cat? <: Animal` fails on the `Null <: Animal` leg.
    assert!(!sys.is_subtype(fx.cat_question(), fx.animal()));
    assert!(sys.is_subtype(fx.cat_question(), fx.animal_question()));
    assert!(!sys.is_subtype(fx.int_question(), fx.never()));
    assert!(!sys.is_subtype(fx.int_question(), fx.null()));
    assert!(sys.is_subtype(fx.int_question(), fx.dynamic()));
    // Nullable source into a non-null target is the invalid-assignment core
    //: `S? <: T` requires `Null <: T` as well.
    assert!(!sys.is_subtype(
        fx.list_of(fx.int_question(), false),
        fx.list_of(fx.int(), false)
    ));
}

#[test]
fn type_parameters_are_opaque_without_bounds() {
    let fx = fixture();
    let sys = fx.system();
    let x = fx.type_param(fx.d.iterable_t);
    let y = fx.type_param(fx.d.list_e);
    let x_question = fx.table.intern(
        UninternedType::TypeParameter {
            parameter: fx.d.iterable_t,
        },
        true,
    );
    assert!(sys.is_subtype(x, x));
    assert!(sys.is_subtype(x, x_question));
    assert!(!sys.is_subtype(x, y));
    // M1 deviation: no declared bounds, so `X <: Object` is `false`
    // (official rule 4 consults the bound; the unbound default `Object?` is
    // not a subtype of `Object` either, so the answer matches there).
    assert!(!sys.is_subtype(x, fx.itf(fx.d.object, &[], false)));
    assert!(!sys.is_subtype(fx.null(), x));
}

#[test]
fn rule10_right_future_or() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.int(), fx.future_or_of(fx.int(), false)));
    assert!(sys.is_subtype(fx.future_of(fx.int()), fx.future_or_of(fx.int(), false)));
    assert!(sys.is_subtype(fx.never(), fx.future_or_of(fx.int(), false)));
    assert!(!sys.is_subtype(fx.int(), fx.future_or_of(fx.string(), false)));
    // `int <: Future<int?>` holds (covariant arguments), so
    // `int <: FutureOr<int?>` holds.
    assert!(sys.is_subtype(fx.int(), fx.future_or_of(fx.int_question(), false)));
    // Nullable FutureOr is `S?`.
    assert!(sys.is_subtype(fx.int(), fx.future_or_of(fx.int(), true)));
    assert!(!sys.is_subtype(fx.cat(), fx.future_or_of(fx.int(), true)));
}

#[test]
fn rule11_right_nullable() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.cat(), fx.animal_question()));
    assert!(sys.is_subtype(fx.int(), fx.int_question()));
    assert!(sys.is_subtype(fx.null(), fx.int_question()));
    assert!(sys.is_subtype(fx.list_of(fx.int(), false), fx.list_of(fx.int(), true)));
    assert!(!sys.is_subtype(fx.cat(), fx.int_question()));
}

#[test]
fn rule12_interface_walk() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_subtype(fx.int(), fx.num()));
    assert!(!sys.is_subtype(fx.num(), fx.int()));
    assert!(sys.is_subtype(fx.cat(), fx.animal()));
    assert!(sys.is_subtype(fx.cat(), fx.itf(fx.d.object, &[], false)));
    // Covariant arguments (`List<int> <: List<int?>`; class
    // parameters are covariant, element.dart:11560-11561).
    assert!(sys.is_subtype(fx.list_of(fx.int(), false), fx.list_of(fx.num(), false)));
    assert!(sys.is_subtype(
        fx.list_of(fx.int(), false),
        fx.list_of(fx.int_question(), false)
    ));
    assert!(!sys.is_subtype(
        fx.list_of(fx.int_question(), false),
        fx.list_of(fx.int(), false)
    ));
    // Transitive instantiation through `List<E> <: Iterable<E>`.
    assert!(sys.is_subtype(fx.list_of(fx.int(), false), fx.iterable_of(fx.int(), false)));
    assert!(sys.is_subtype(fx.list_of(fx.int(), false), fx.iterable_of(fx.num(), false)));
    assert!(!sys.is_subtype(
        fx.list_of(fx.int(), false),
        fx.iterable_of(fx.string(), false)
    ));
    assert!(!sys.is_subtype(fx.iterable_of(fx.int(), false), fx.list_of(fx.int(), false)));
    assert!(!sys.is_subtype(fx.int(), fx.iterable_of(fx.int(), false)));
    // Classes never subtype bare `Function` (subtype.dart:206-209).
    assert!(!sys.is_subtype(fx.cat(), fx.itf(fx.d.function, &[], false)));
    // Nullability travels along the walk.
    assert!(sys.is_subtype(fx.cat_question(), fx.animal_question()));
}

#[test]
fn rule12_instantiation_substitutes_into_function_typed_supertype_arguments() {
    // `class Supplier<S>` / `class C<T> implements Supplier<T Function()>`:
    // instantiating `C<int>` walks C's supertype edge, and the substitution
    // step (subtype.dart:213-214) must reach *inside* the function-typed
    // argument of the template — `T` sits only in the function's return
    // type.
    let table = TypeTable::new();
    let (object, future, function) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
    );
    let (supplier, class_c, int_d, string_d) = (
        TypeDeclId::from_raw(4),
        TypeDeclId::from_raw(5),
        TypeDeclId::from_raw(6),
        TypeDeclId::from_raw(7),
    );
    let (s, t) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
    let itf = |decl: TypeDeclId, args: &[TypeId]| {
        table.intern(
            UninternedType::Interface {
                decl,
                args: args.to_vec(),
            },
            false,
        )
    };
    let return_fn = |ret: TypeId| {
        table
            .intern_function(
                UninternedFunction {
                    return_type: ret,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: Vec::new(),
                    required_positional: 0,
                },
                false,
            )
            .expect("valid fixture signature")
    };

    let object_none = itf(object, &[]);
    let int = itf(int_d, &[]);
    let t_param = table.intern(UninternedType::TypeParameter { parameter: t }, false);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![s])
    .decl(function, Vec::new())
    .decl(supplier, vec![s])
    .decl(class_c, vec![t])
    .decl(int_d, Vec::new())
    .decl(string_d, Vec::new())
    .supertype(int_d, object_none)
    .supertype(string_d, object_none)
    .supertype(class_c, itf(supplier, &[return_fn(t_param)]))
    .build(&table)
    .expect("fixture hierarchy is valid");
    let sys = dartq_types::TypeSystem::new(&table, &hierarchy);

    let c_of_int = itf(class_c, &[int]);
    // `C<int> <: Supplier<int Function()>`: the walk instantiates the
    // template's `T Function()` to `int Function()` and compares it.
    assert!(sys.is_subtype(c_of_int, itf(supplier, &[return_fn(int)])));
    // A different return type breaks the match.
    assert!(!sys.is_subtype(c_of_int, itf(supplier, &[return_fn(itf(string_d, &[]))])));
}

#[test]
fn rule13_function_type_vs_bare_function() {
    let fx = fixture();
    let sys = fx.system();
    let f = fx.fn_type(&[], fx.int());
    assert!(sys.is_subtype(f, fx.itf(fx.d.function, &[], false)));
    assert!(sys.is_subtype(f, function_question(&fx)));
    assert!(sys.is_subtype(f, fx.itf(fx.d.object, &[], false)));
}

#[test]
fn rule14_record_shape_equality_beyond_reflexivity() {
    // Rule 14 (subtype.dart:227-259): record <: record is arity equality
    // plus pairwise covariant fields; canonically interned shapes make
    // structural equality observable through any distinct pair.
    let fx = fixture();
    let sys = fx.system();
    let r = fx.record(&[fx.int(), fx.string()], &[]);
    // Identical shape, distinct internings of the same structure (the pair
    // is structurally equal, so interning returns one type — reflexivity).
    let same = fx.record(&[fx.int(), fx.string()], &[]);
    assert!(sys.is_subtype(r, same));
    // Covariant positional fields.
    assert!(sys.is_subtype(r, fx.record(&[fx.num(), fx.string()], &[])));
    assert!(sys.is_subtype(r, fx.record(&[fx.int(), fx.string_question()], &[])));
    // The inverse direction is not covariant.
    assert!(!sys.is_subtype(fx.record(&[fx.num(), fx.string()], &[]), r));
}

#[test]
fn rule14_record_arity_mismatch_is_false() {
    let fx = fixture();
    let sys = fx.system();
    let r = fx.record(&[fx.int(), fx.string()], &[]);
    // Positional arity (subtype.dart:241-247).
    assert!(!sys.is_subtype(r, fx.record(&[fx.int()], &[])));
    assert!(!sys.is_subtype(fx.record(&[fx.int()], &[]), r));
    // Named arity: adding a named field to the target breaks it
    // (subtype.dart:248-258).
    assert!(!sys.is_subtype(
        r,
        fx.record(&[fx.int(), fx.string()], &[named("x", fx.num(), false)])
    ));
}

#[test]
fn rule14_record_named_fields_pair_by_name() {
    let fx = fixture();
    let sys = fx.system();
    let r = fx.record(&[fx.int()], &[named("x", fx.int(), false)]);
    // Covariant named field.
    assert!(sys.is_subtype(r, fx.record(&[fx.int()], &[named("x", fx.num(), false)])));
    // Name mismatch is false even with related types.
    assert!(!sys.is_subtype(r, fx.record(&[fx.int()], &[named("y", fx.num(), false)])));
    // Named order is canonical (interning sorts), so the reordered pair is
    // the same type.
    let reordered = fx.record(&[fx.int()], &[named("x", fx.int(), false)]);
    assert!(sys.is_subtype(r, reordered));
}

#[test]
fn rule14_record_subtypes_only_object_like_targets() {
    let fx = fixture();
    let sys = fx.system();
    let r = fx.record(&[fx.int()], &[]);
    // Records subtype no class: record-vs-`Function` falls through rule 14
    // (subtype.dart:512-519); `Object` targets are consumed by rule 4.
    assert!(!sys.is_subtype(r, fx.itf(fx.d.function, &[], false)));
    assert!(sys.is_subtype(r, fx.itf(fx.d.object, &[], false)));
}

#[test]
fn rule14_record_subtypes_the_record_class() {
    // `record <: Record` is the shortcut arm (subtype.dart:230-233); it
    // needs the `Record` class declared, which the shared fixture omits.
    let table = TypeTable::new();
    let (object, future, function, record) = (
        TypeDeclId::from_raw(1),
        TypeDeclId::from_raw(2),
        TypeDeclId::from_raw(3),
        TypeDeclId::from_raw(4),
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
    .decl(record, Vec::new())
    .build(&table)
    .expect("fixture hierarchy is valid");
    let sys = TypeSystem::new(&table, &hierarchy);
    let record_class = sys.table().intern(
        UninternedType::Interface {
            decl: record,
            args: Vec::new(),
        },
        false,
    );
    let r = sys
        .table()
        .intern_record(
            UninternedRecord {
                positional: vec![sys.table().intern(
                    UninternedType::Interface {
                        decl: object,
                        args: Vec::new(),
                    },
                    false,
                )],
                named: Vec::new(),
            },
            false,
        )
        .expect("valid fixture record");
    assert!(sys.is_subtype(r, record_class));
    assert!(!sys.is_subtype(record_class, r));
}

#[test]
fn recursion_depth_limit_aborts_the_walk() {
    let fx = fixture();
    let sys = fx.system();
    // Nesting in `List` adds one subtype-recursion step per level (rule 12 →
    // `arguments_subtype`), so chain length = walk depth.
    let nest = |seed: TypeId, depth: u32| {
        let mut ty = seed;
        for _ in 0..depth {
            ty = fx.list_of(ty, false);
        }
        ty
    };
    // 99 levels sit below the guard: the chains are genuinely related and
    // the walk completes.
    assert!(sys.is_subtype(nest(fx.int(), 99), nest(fx.num(), 99)));
    // 101 levels exceed the guard (`DEPTH_LIMIT` = 100, the official
    // `_maxTypeOperationDepth`, type_system.dart:85): `isSubtypeOf` returns
    // `false` (subtype.dart:37-41) even though the chains are structurally
    // related.
    assert!(!sys.is_subtype(nest(fx.int(), 101), nest(fx.num(), 101)));
    // Top-level reflexivity survives the swap: the guard runs first but
    // top-level queries start at depth 0, so rule 0a answers.
    let deep = nest(fx.int(), 101);
    assert!(sys.is_subtype(deep, deep));
}

#[test]
fn depth_guard_precedes_reflexivity_beyond_the_limit() {
    // The oracle enters the type operation before consulting any rule
    // (subtype.dart:33-41, type_system.dart:372-378), so a *reflexive* pair
    // arriving beyond the limit also answers `false`. Drive one: two
    // alpha-equivalent generic functions correlate positionally, and the
    // correlated return check compares identical types one level deeper —
    // nesting 99 `List`s puts it at depth 100, past the guard. Before the
    // guard swapped ahead of rule 0a this query answered `true`.
    let fx = fixture();
    let sys = fx.system();
    let identity_t = fx.fn_sig(UninternedFunction {
        return_type: fx.type_param(fx.d.iterable_t),
        type_parameters: vec![fx.d.iterable_t],
        positional: vec![fx.type_param(fx.d.iterable_t)],
        named: Vec::new(),
        required_positional: 1,
    });
    let identity_s = fx.fn_sig(UninternedFunction {
        return_type: fx.type_param(fx.d.list_e),
        type_parameters: vec![fx.d.list_e],
        positional: vec![fx.type_param(fx.d.list_e)],
        named: Vec::new(),
        required_positional: 1,
    });
    let nest = |seed: TypeId, depth: u32| {
        let mut ty = seed;
        for _ in 0..depth {
            ty = fx.list_of(ty, false);
        }
        ty
    };
    // Flat, the pair is related (the existing generic-correlation test
    // pins `true`); one List shy of the limit it still is.
    assert!(sys.is_subtype(identity_t, identity_s));
    assert!(sys.is_subtype(nest(identity_t, 98), nest(identity_s, 98)));
    // At the limit the guard fires before rule 0a can.
    assert!(!sys.is_subtype(nest(identity_t, 99), nest(identity_s, 99)));
}

fn function_question(fx: &Fixture) -> TypeId {
    fx.itf(fx.d.function, &[], true)
}
