//! Function subtyping (rule 13, `subtype.dart:86-182`).

mod common;

use common::{fixture, named};
use dartiq_types::UninternedFunction;

#[test]
fn identical_signatures_are_reflexive() {
    let fx = fixture();
    let sys = fx.system();
    let f = fx.fn_type(&[fx.int()], fx.string());
    assert!(sys.is_subtype(f, f));
}

#[test]
fn return_type_is_covariant() {
    let fx = fixture();
    let sys = fx.system();
    let int_fn = fx.fn_type(&[], fx.int());
    let num_fn = fx.fn_type(&[], fx.num());
    assert!(sys.is_subtype(int_fn, num_fn));
    assert!(!sys.is_subtype(num_fn, int_fn));
    // `dynamic` return is not a subtype of a specific return.
    let dyn_fn = fx.fn_type(&[], fx.dynamic());
    assert!(!sys.is_subtype(dyn_fn, int_fn));
}

#[test]
fn positional_parameters_are_contravariant() {
    let fx = fixture();
    let sys = fx.system();
    let int_param = fx.fn_type(&[fx.int()], fx.void_());
    let num_param = fx.fn_type(&[fx.num()], fx.void_());
    // `(num) -> void <: (int) -> void`
    assert!(sys.is_subtype(num_param, int_param));
    assert!(!sys.is_subtype(int_param, num_param));
}

#[test]
fn optional_positional_parameters() {
    let fx = fixture();
    let sys = fx.system();
    let optional = fx.fn_sig(UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: Vec::new(),
        required_positional: 0,
    });
    let required = fx.fn_type(&[fx.int()], fx.void_());
    // `([int]) -> void <: (int) -> void`: a required-arg call is accepted.
    assert!(sys.is_subtype(optional, required));
    // `(int) -> void </: ([int]) -> void`: zero-arg calls are not accepted.
    assert!(!sys.is_subtype(required, optional));
}

#[test]
fn positional_arity_must_cover_the_supertype() {
    let fx = fixture();
    let sys = fx.system();
    let one = fx.fn_type(&[fx.int()], fx.void_());
    let two = fx.fn_type(&[fx.int(), fx.int()], fx.void_());
    // A 2-arg call is not accepted by `one`; a 1-arg call is not accepted by
    // `two`. Neither direction holds (subtype.dart:168-179).
    assert!(!sys.is_subtype(one, two));
    assert!(!sys.is_subtype(two, one));
}

#[test]
fn named_parameters_match_by_name() {
    let fx = fixture();
    let sys = fx.system();
    let sig = |members: &[(&str, dartiq_types::TypeId, bool)]| {
        fx.fn_sig(UninternedFunction {
            return_type: fx.void_(),
            type_parameters: Vec::new(),
            positional: Vec::new(),
            named: members
                .iter()
                .map(|&(name, ty, required)| named(name, ty, required))
                .collect(),
            required_positional: 0,
        })
    };
    let x = sig(&[("x", fx.int(), false)]);
    let y = sig(&[("y", fx.int(), false)]);
    let x_required = sig(&[("x", fx.int(), true)]);
    let xy = sig(&[("x", fx.int(), false), ("y", fx.int(), false)]);
    let xy_required_x = sig(&[("x", fx.int(), true), ("y", fx.int(), false)]);

    assert!(sys.is_subtype(x, x));
    // f requires what g leaves optional: no.
    assert!(!sys.is_subtype(x_required, x));
    // g requires it, f doesn't: fine.
    assert!(sys.is_subtype(x, x_required));
    // Extra optional named parameters are fine in both directions of the
    // walk (f's extras must be optional; g's must all exist in f).
    assert!(sys.is_subtype(xy, x));
    assert!(!sys.is_subtype(x, xy));
    // Extra required named parameters are rejected.
    assert!(!sys.is_subtype(xy_required_x, x));
    // Name mismatch on the g side is rejected.
    assert!(!sys.is_subtype(y, x));
    // f's extra named parameter that g lacks must be optional.
    assert!(sys.is_subtype(xy, y));
    // Contravariant types under the same name.
    let x_int = sig(&[("x", fx.int(), false)]);
    let x_num = sig(&[("x", fx.num(), false)]);
    assert!(sys.is_subtype(x_num, x_int));
    assert!(!sys.is_subtype(x_int, x_num));
}

#[test]
fn positional_and_named_sections_are_independent() {
    let fx = fixture();
    let sys = fx.system();
    let plain = fx.fn_type(&[fx.int()], fx.void_());
    let with_named = fx.fn_sig(UninternedFunction {
        return_type: fx.void_(),
        type_parameters: Vec::new(),
        positional: vec![fx.int()],
        named: vec![named("x", fx.int(), false)],
        required_positional: 1,
    });
    assert!(sys.is_subtype(with_named, plain));
    assert!(!sys.is_subtype(plain, with_named));
}

#[test]
fn generic_functions_correlate_positionally() {
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
    assert!(sys.is_subtype(identity_t, identity_s));

    // Different type-parameter counts do not correlate.
    let monomorphic = fx.fn_type(&[fx.int()], fx.int());
    assert!(!sys.is_subtype(identity_t, monomorphic));

    // `<T>(T) -> void </: <S>(S) -> S`: the correlated return check needs
    // `void <: S`, which is false.
    let void_t = fx.fn_sig(UninternedFunction {
        return_type: fx.void_(),
        type_parameters: vec![fx.d.iterable_t],
        positional: vec![fx.type_param(fx.d.iterable_t)],
        named: Vec::new(),
        required_positional: 1,
    });
    assert!(!sys.is_subtype(void_t, identity_s));
    // `<S>(S) -> S <: <T>(T) -> void`: return `void` is a top-ish acceptor
    // for the correlated `S` (rule 1 Right Top).
    assert!(sys.is_subtype(identity_s, void_t));

    // Generic parameter positions substitute too.
    let list_t = fx.fn_sig(UninternedFunction {
        return_type: fx.void_(),
        type_parameters: vec![fx.d.iterable_t],
        positional: vec![fx.list_of(fx.type_param(fx.d.iterable_t), false)],
        named: Vec::new(),
        required_positional: 1,
    });
    let list_s = fx.fn_sig(UninternedFunction {
        return_type: fx.void_(),
        type_parameters: vec![fx.d.list_e],
        positional: vec![fx.list_of(fx.type_param(fx.d.list_e), false)],
        named: Vec::new(),
        required_positional: 1,
    });
    assert!(sys.is_subtype(list_t, list_s));
}

#[test]
fn function_nullability_follows_rule_11() {
    let fx = fixture();
    let sys = fx.system();
    let f = fx.fn_type(&[], fx.int());
    let f_question = fx
        .table
        .intern_function(
            UninternedFunction {
                return_type: fx.int(),
                type_parameters: Vec::new(),
                positional: Vec::new(),
                named: Vec::new(),
                required_positional: 0,
            },
            true,
        )
        .expect("valid");
    assert!(sys.is_subtype(f, f_question));
    assert!(!sys.is_subtype(f_question, f));
    assert!(sys.is_subtype(f_question, fx.dynamic()));
}
