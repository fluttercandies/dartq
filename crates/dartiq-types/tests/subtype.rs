//! The subtype rule table (spec §2), ported from
//! `subtype.dart:262-523`. Test names cite the rule numbers.

mod common;

use common::{Fixture, fixture, named};
use dartiq_types::{TypeId, UninternedType};

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
    // (spec §2.1): `S? <: T` requires `Null <: T` as well.
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
    // Covariant arguments (spec §2.1: `List<int> <: List<int?>`; class
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
fn rule13_function_type_vs_bare_function() {
    let fx = fixture();
    let sys = fx.system();
    let f = fx.fn_type(&[], fx.int());
    assert!(sys.is_subtype(f, fx.itf(fx.d.function, &[], false)));
    assert!(sys.is_subtype(f, function_question(&fx)));
    assert!(sys.is_subtype(f, fx.itf(fx.d.object, &[], false)));
}

#[test]
fn rule14_record_pairs_beyond_reflexivity_are_false() {
    let fx = fixture();
    let sys = fx.system();
    let r = fx.record(&[fx.int(), fx.string()], &[]);
    // Rule 0a reflexivity is the only `true` record/record answer in M1.
    assert!(sys.is_subtype(r, r));
    // Rule 14 (subtype.dart:512-519) is an M1 stub: every non-identical
    // record/record pair is `false`, even related shapes.
    assert!(!sys.is_subtype(r, fx.record(&[fx.num(), fx.string()], &[])));
    assert!(!sys.is_subtype(r, fx.record(&[fx.int()], &[])));
    assert!(!sys.is_subtype(
        r,
        fx.record(&[fx.int(), fx.string()], &[named("x", fx.num(), false)])
    ));
    // Records subtype no class either: record-vs-`Function` falls through to
    // rule 14 (`Object` targets are consumed by rule 4, where records pass
    // like any other class).
    assert!(!sys.is_subtype(r, fx.itf(fx.d.function, &[], false)));
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
    // Reflexivity (rule 0a) short-circuits before the depth check.
    let deep = nest(fx.int(), 101);
    assert!(sys.is_subtype(deep, deep));
}

fn function_question(fx: &Fixture) -> TypeId {
    fx.itf(fx.d.function, &[], true)
}
