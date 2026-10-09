//! Nullability API surface: suffix mapping, semantic nullability predicates,
//! and assignability (spec §1.2, §2.2, §2.3).

mod common;

use common::fixture;
use dartq_types::{NullabilitySuffix, TypeError, UninternedType};

#[test]
fn suffix_mapping() {
    // `TypeError` carries no `PartialEq` (its `SupertypeNotInterface` payload
    // is a pool-position `TypeKind`), so `Result` equality is unavailable —
    // the outcomes are matched instead.
    assert!(matches!(
        NullabilitySuffix::Question.as_nullable(),
        Ok(true)
    ));
    assert!(matches!(NullabilitySuffix::None.as_nullable(), Ok(false)));
    assert!(matches!(
        NullabilitySuffix::Star.as_nullable(),
        Err(TypeError::LegacyNullability)
    ));
}

#[test]
fn stored_types_never_carry_star() {
    let fx = fixture();
    for ty in [
        fx.int(),
        fx.int_question(),
        fx.never(),
        fx.dynamic(),
        fx.future_or_of(fx.int(), true),
    ] {
        let stored = fx.table.get(ty);
        assert_ne!(stored.nullability_suffix(), NullabilitySuffix::Star);
    }
    assert_eq!(
        fx.table.get(fx.int()).nullability_suffix(),
        NullabilitySuffix::None
    );
    assert_eq!(
        fx.table.get(fx.int_question()).nullability_suffix(),
        NullabilitySuffix::Question
    );
}

#[test]
fn semantic_nullability_predicates() {
    let fx = fixture();
    let sys = fx.system();
    for nullable in [
        fx.dynamic(),
        fx.void_(),
        fx.null(),
        fx.invalid(),
        fx.int_question(),
        fx.future_or_of(fx.int_question(), false),
        fx.future_or_of(fx.int(), true),
    ] {
        assert!(sys.is_nullable(nullable));
        assert!(!sys.is_non_nullable(nullable));
    }
    for non_nullable in [
        fx.int(),
        fx.cat(),
        fx.never(),
        fx.future_or_of(fx.int(), false),
        fx.fn_type(&[], fx.int()),
    ] {
        assert!(!sys.is_nullable(non_nullable));
        assert!(sys.is_non_nullable(non_nullable));
    }
    // Type parameters are neither (no declared bounds in M1).
    let x = fx.type_param(fx.d.iterable_t);
    assert!(!sys.is_nullable(x));
    assert!(!sys.is_non_nullable(x));
}

#[test]
fn top_bottom_null_object_classes() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_top(fx.dynamic()));
    assert!(sys.is_top(fx.void_()));
    assert!(sys.is_top(fx.invalid()));
    assert!(sys.is_top(fx.object_question()));
    assert!(sys.is_top(fx.future_or_of(fx.dynamic(), false)));
    assert!(!sys.is_top(fx.int()));
    assert!(sys.is_bottom(fx.never()));
    assert!(!sys.is_bottom(fx.null()));
    assert!(sys.is_null(fx.null()));
    // `Never?` interns as `Null`, hence null but not bottom.
    assert!(sys.is_null(fx.table.intern(UninternedType::Never, true)));
    assert!(sys.is_object(fx.itf(fx.d.object, &[], false)));
    assert!(sys.is_object(fx.future_or_of(fx.itf(fx.d.object, &[], false), false)));
    assert!(!sys.is_object(fx.int()));
    assert!(!sys.is_object(fx.future_or_of(fx.int(), false)));
}

#[test]
fn strictly_non_nullable() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_strictly_non_nullable(fx.int()));
    assert!(sys.is_strictly_non_nullable(fx.never()));
    assert!(sys.is_strictly_non_nullable(fx.fn_type(&[], fx.int())));
    assert!(sys.is_strictly_non_nullable(fx.future_or_of(fx.int(), false)));
    assert!(!sys.is_strictly_non_nullable(fx.int_question()));
    assert!(!sys.is_strictly_non_nullable(fx.dynamic()));
    assert!(!sys.is_strictly_non_nullable(fx.void_()));
    assert!(!sys.is_strictly_non_nullable(fx.null()));
    assert!(!sys.is_strictly_non_nullable(fx.invalid()));
    // Stricter than `is_non_nullable`: `Null` is nullable, `FutureOr<int?>`
    // is nullable through its argument.
    assert!(!sys.is_strictly_non_nullable(fx.future_or_of(fx.int_question(), false)));
    // M1 deviation: type parameters have no bounds, hence `false`.
    assert!(!sys.is_strictly_non_nullable(fx.type_param(fx.d.iterable_t)));
}

#[test]
fn assignability() {
    let fx = fixture();
    let sys = fx.system();
    assert!(sys.is_assignable(fx.int(), fx.num(), false));
    assert!(sys.is_assignable(fx.int(), fx.num(), true));
    assert!(sys.is_assignable(fx.int(), fx.int_question(), false));
    assert!(!sys.is_assignable(fx.int_question(), fx.int(), false));
    assert!(!sys.is_assignable(fx.cat(), fx.dog(), false));
    // `dynamic` downcasts are allowed by default and rejected by
    // `strict-casts` (type_system.dart:919-924).
    assert!(sys.is_assignable(fx.dynamic(), fx.int(), false));
    assert!(!sys.is_assignable(fx.dynamic(), fx.int(), true));
    // Invalid types flow through as error recovery (type_system.dart:901-904).
    assert!(sys.is_assignable(fx.invalid(), fx.int(), true));
    assert!(sys.is_assignable(fx.invalid(), fx.dog(), false));
    // `Never` flows everywhere.
    assert!(sys.is_assignable(fx.never(), fx.int(), true));
    // `Null <: dynamic` is a subtype (Right Top), so assignable regardless.
    assert!(sys.is_assignable(fx.null(), fx.dynamic(), true));
}

#[test]
fn is_more_top_total_order() {
    // `isMoreTop` (type_system.dart:1124-1189), the tiebreak behind the LUB
    // top merge.
    let fx = fixture();
    let sys = fx.system();
    let object = fx.itf(fx.d.object, &[], false);
    // MORETOP(void, S) = true / MORETOP(T, void) = false.
    assert!(sys.is_more_top(fx.void_(), fx.dynamic()));
    assert!(!sys.is_more_top(fx.dynamic(), fx.void_()));
    // MORETOP(dynamic, S) = true / MORETOP(T, dynamic) = false (`Invalid`
    // shares `dynamic`'s rows).
    assert!(sys.is_more_top(fx.dynamic(), fx.int()));
    assert!(!sys.is_more_top(fx.int(), fx.dynamic()));
    assert!(sys.is_more_top(fx.invalid(), fx.int()));
    assert!(!sys.is_more_top(fx.int(), fx.invalid()));
    // MORETOP(Object, S) = true / MORETOP(T, Object) = false.
    assert!(sys.is_more_top(object, fx.int()));
    assert!(!sys.is_more_top(fx.int(), object));
    // MORETOP(T?, S?) = MORETOP(T, S); unrelated non-tops are unordered.
    assert!(!sys.is_more_top(fx.int(), fx.cat()));
    assert!(!sys.is_more_top(fx.cat(), fx.int()));
    assert_eq!(
        sys.is_more_top(fx.int_question(), fx.cat_question()),
        sys.is_more_top(fx.int(), fx.cat())
    );
    // MORETOP(T, S?) = true / MORETOP(T?, S) = false.
    assert!(sys.is_more_top(fx.int(), fx.int_question()));
    assert!(!sys.is_more_top(fx.int_question(), fx.int()));
    // MORETOP(FutureOr<T>, FutureOr<S>) = MORETOP(T, S).
    assert!(sys.is_more_top(
        fx.future_or_of(fx.dynamic(), false),
        fx.future_or_of(fx.int(), false)
    ));
}
