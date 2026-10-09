//! Canonicalization: interning, structural equality, hashing, substitution.

mod common;

use common::{fixture, named};
use dartq_types::{TypeError, TypeKind, UninternedFunction, UninternedRecord, UninternedType};

#[test]
fn interning_is_canonical_for_identical_structures() {
    let fx = fixture();
    let len_before = fx.table.len();

    let a = fx.int();
    let b = fx.int();
    let q1 = fx.int_question();
    let q2 = fx.int_question();
    let l1 = fx.list_of(fx.int(), false);
    let l2 = fx.list_of(fx.int(), false);
    let fo1 = fx.future_or_of(fx.int(), true);
    let fo2 = fx.future_or_of(fx.int(), true);

    assert_eq!(a, b);
    assert_eq!(q1, q2);
    assert_eq!(l1, l2);
    assert_eq!(fo1, fo2);
    assert_ne!(a, q1);
    assert_ne!(l1, fx.list_of(fx.num(), false));
    assert_ne!(fx.list_of(fx.int(), false), fx.list_of(fx.int(), true));
    let _ = len_before; // ids dedup against fixture types; only identity matters
}

#[test]
fn never_question_collapses_to_null() {
    let fx = fixture();
    // Official `Never?` behaves as `Null` (type.dart:1207-1209).
    let never_question = fx.table.intern(UninternedType::Never, true);
    assert_eq!(never_question, fx.null());
    assert_ne!(never_question, fx.never());
    assert!(fx.system().is_null(never_question));
}

#[test]
fn unit_kinds_force_their_nullability() {
    let fx = fixture();
    // `dynamic` is always stored non-`?` (type.dart:90-92); same for the
    // other unit kinds.
    assert_eq!(fx.table.intern(UninternedType::Dynamic, true), fx.dynamic());
    assert_eq!(fx.table.intern(UninternedType::Void, true), fx.void_());
    assert_eq!(fx.table.intern(UninternedType::Invalid, true), fx.invalid());
    assert_eq!(fx.table.intern(UninternedType::Null, true), fx.null());
}

#[test]
fn named_member_order_is_irrelevant() {
    let fx = fixture();
    let unsorted = UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: vec![named("b", fx.int(), false), named("a", fx.num(), false)],
        required_positional: 1,
    };
    let sorted = UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: vec![named("a", fx.num(), false), named("b", fx.int(), false)],
        required_positional: 1,
    };
    let left = fx.fn_sig(unsorted);
    let right = fx.fn_sig(sorted);
    assert_eq!(left, right);
}

#[test]
fn duplicate_named_members_are_rejected() {
    let fx = fixture();
    let dup = UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: Vec::new(),
        named: vec![named("x", fx.int(), false), named("x", fx.num(), false)],
        required_positional: 0,
    };
    assert_eq!(
        fx.table.intern_function(dup, false),
        Err(TypeError::DuplicateNamedMember("x".to_string()))
    );
}

#[test]
fn required_positional_count_is_validated() {
    let fx = fixture();
    let sig = UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: Vec::new(),
        required_positional: 2,
    };
    assert_eq!(
        fx.table.intern_function(sig, false),
        Err(TypeError::RequiredPositionalOutOfBounds {
            required: 2,
            total: 1,
        })
    );
}

#[test]
fn record_duplicate_named_fields_are_rejected() {
    let fx = fixture();
    let dup = UninternedRecord {
        positional: vec![fx.int()],
        named: vec![named("x", fx.int(), false), named("x", fx.string(), false)],
    };
    assert_eq!(
        fx.table.intern_record(dup, false),
        Err(TypeError::DuplicateNamedMember("x".to_string()))
    );
}

#[test]
fn record_structural_equality() {
    let fx = fixture();
    let r1 = fx.record(&[fx.int(), fx.string()], &[named("x", fx.num(), false)]);
    let r2 = fx.record(&[fx.int(), fx.string()], &[named("x", fx.num(), false)]);
    let other_order = fx.record(&[fx.int(), fx.string()], &[named("x", fx.num(), true)]);
    let positional_matters = fx.record(&[fx.string(), fx.int()], &[]);
    // Record field `required` is meaningless and normalized away.
    assert_eq!(r1, r2);
    assert_eq!(r1, other_order);
    assert_ne!(r1, positional_matters);
    assert_ne!(r1, fx.record(&[fx.int(), fx.string()], &[]));
}

#[test]
fn substitute_rewrites_type_parameters() {
    let fx = fixture();
    let t = fx.type_param(fx.d.iterable_t);
    let list_of_t = fx.list_of(t, false);
    let substituted = fx
        .table
        .substitute(list_of_t, &[(fx.d.iterable_t, fx.int())]);
    assert_eq!(substituted, fx.list_of(fx.int(), false));

    // Unbound parameters survive; empty maps are identities.
    let partial = fx.table.substitute(list_of_t, &[(fx.d.list_e, fx.int())]);
    assert_eq!(partial, list_of_t);
    assert_eq!(fx.table.substitute(list_of_t, &[]), list_of_t);

    // Function signatures substitute their bodies, not their binders.
    let t2 = fx.type_param(fx.d.list_e);
    let sig = UninternedFunction {
        return_type: t,
        type_parameters: vec![fx.d.iterable_t],
        positional: vec![fx.list_of(t2, false)],
        named: vec![named("x", fx.future_or_of(t, false), false)],
        required_positional: 0,
    };
    let generic = fx.table.intern_function(sig, false).expect("valid");
    let out = fx.table.substitute(generic, &[(fx.d.iterable_t, fx.int())]);
    assert_ne!(out, generic);
    let TypeKind::Function { signature } = fx.table.get(out).kind() else {
        panic!("expected a function type");
    };
    let out_sig = fx.table.signature(signature);
    assert_eq!(out_sig.return_type, fx.int());
    // `List<ListE>` inside the parameters is untouched: `list_e` is a
    // different declaration's parameter and must not be captured.
    assert_eq!(
        fx.table.arg(out_sig.positional.offset()),
        fx.list_of(t2, false)
    );
    let named_member = &fx.table.named_members(out_sig.named)[0];
    assert_eq!(named_member.ty, fx.future_or_of(fx.int(), false));
}

#[test]
fn substitute_reaches_a_parameter_that_appears_only_in_a_return_type() {
    let fx = fixture();
    let t = fx.type_param(fx.d.iterable_t);
    let int = fx.int();
    // `T Function()`: the parameter appears only in the return type, so
    // neither the positional nor the named section changes.
    let f = fx.fn_type(&[], t);
    assert_eq!(
        fx.table.substitute(f, &[(fx.d.iterable_t, int)]),
        fx.fn_type(&[], int)
    );

    // Nested: `T Function() Function()` — the parameter sits two return
    // levels deep, inside the outer signature's return type.
    let outer = fx.fn_type(&[], fx.fn_type(&[], t));
    assert_eq!(
        fx.table.substitute(outer, &[(fx.d.iterable_t, int)]),
        fx.fn_type(&[], fx.fn_type(&[], int))
    );

    // No occurrence anywhere keeps the original id (fast path).
    let untouched = fx.fn_type(&[], fx.void_());
    assert_eq!(
        fx.table.substitute(untouched, &[(fx.d.iterable_t, int)]),
        untouched
    );
}

#[test]
fn substitute_unites_type_parameter_and_replacement_nullabilities() {
    let fx = fixture();
    let t = fx.d.iterable_t;
    let t_question = fx
        .table
        .intern(UninternedType::TypeParameter { parameter: t }, true);

    // The official substitutor unites the node's suffix with the
    // replacement's (type_algebra.dart:97-106, `visitTypeParameterType`:
    // 592-602): nullable when either side is.
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fx.int())]),
        fx.int_question()
    );
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fx.int_question())]),
        fx.int_question()
    );
    assert_eq!(
        fx.table.substitute(fx.type_param(t), &[(t, fx.int())]),
        fx.int()
    );
    assert_eq!(
        fx.table
            .substitute(fx.type_param(t), &[(t, fx.int_question())]),
        fx.int_question()
    );

    // Unit-kind normalization still applies to the united result:
    // `T?=Never` is `Null` (type.dart:1207-1209), and `dynamic`/`Null`
    // never grow a `?` twin.
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fx.never())]),
        fx.null()
    );
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fx.dynamic())]),
        fx.dynamic()
    );
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fx.null())]),
        fx.null()
    );

    // Compound replacements re-intern with the promoted suffix.
    let fn_type = UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: Vec::new(),
        required_positional: 1,
    };
    let fn_none = fx
        .table
        .intern_function(fn_type.clone(), false)
        .expect("valid");
    assert_eq!(
        fx.table.substitute(t_question, &[(t, fn_none)]),
        fx.table.intern_function(fn_type, true).expect("valid")
    );
    assert_eq!(
        fx.table
            .substitute(t_question, &[(t, fx.future_or_of(fx.int(), false))]),
        fx.future_or_of(fx.int(), true)
    );
    let rec = UninternedRecord {
        positional: vec![fx.int()],
        named: Vec::new(),
    };
    let rec_none = fx.table.intern_record(rec.clone(), false).expect("valid");
    assert_eq!(
        fx.table.substitute(t_question, &[(t, rec_none)]),
        fx.table.intern_record(rec, true).expect("valid")
    );

    // The promotion registers as a change for compound parents:
    // `List<T?>` with `T=int` becomes `List<int?>`, not `List<int>`.
    let list_of_t_question = fx.list_of(t_question, false);
    assert_eq!(
        fx.table.substitute(list_of_t_question, &[(t, fx.int())]),
        fx.list_of(fx.int_question(), false)
    );
}

#[test]
fn function_type_identity_includes_its_own_type_parameters() {
    let fx = fixture();
    // Alpha-equivalent but distinct declarations are distinct types, matching
    // the analyzer's element-identity `==` (type.dart:255).
    let a = fx.fn_sig(UninternedFunction {
        return_type: fx.type_param(fx.d.iterable_t),
        type_parameters: vec![fx.d.iterable_t],
        positional: vec![fx.type_param(fx.d.iterable_t)],
        named: Vec::new(),
        required_positional: 1,
    });
    let b = fx.fn_sig(UninternedFunction {
        return_type: fx.type_param(fx.d.list_e),
        type_parameters: vec![fx.d.list_e],
        positional: vec![fx.type_param(fx.d.list_e)],
        named: Vec::new(),
        required_positional: 1,
    });
    assert_ne!(a, b);
}
