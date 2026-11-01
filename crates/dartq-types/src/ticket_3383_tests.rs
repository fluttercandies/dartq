//! The #3383 table-side pin: `substitute`/`rebind_function_params`
//! binder discipline.
//!
//! Two facts the typedef-alias bound feed depends on:
//!
//! * `Subst::Slice` (the use-site argument substitution) NEVER rewrites a
//!   function signature's declared `type_parameters` — binder ids are
//!   globally unique per declaration, so a substitution map can only hit
//!   free occurrences, and the interned signature keeps its binders (the
//!   freshness invariant the function-subtype correlation relies on).
//! * `rebind_function_params` is the per-use counterpart — it mints fresh
//!   binder ids for every nested signature and renames their occurrences,
//!   so two uses of one alias template carry distinct binders and the
//!   caller can key each use's `ParamFacts.bound` separately.

use crate::ids::{TypeDeclId, TypeParamId};
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedFunction, UninternedType};

/// `substitute` leaves every signature's binder group untouched: the map
/// `{X -> int}` rewrites the free `X` occurrences but the declared `<Y>`
/// parameter keeps its own id — the binder is never a substitution key.
#[test]
fn substitute_slice_never_rewrites_function_binders() {
    let table = TypeTable::new();
    let int_decl = TypeDeclId::from_raw(1);
    let int = table.intern(
        UninternedType::Interface {
            decl: int_decl,
            args: Vec::new(),
        },
        false,
    );
    let x = TypeParamId::from_raw(10);
    let y = TypeParamId::from_raw(11);
    let x_ty = table.intern(UninternedType::TypeParameter { parameter: x }, false);
    let y_ty = table.intern(UninternedType::TypeParameter { parameter: y }, false);
    // `X Function<Y>(Y)` — `X` free, `Y` bound.
    let fn_ty = table
        .intern_function(
            UninternedFunction {
                return_type: x_ty,
                type_parameters: vec![y],
                positional: vec![y_ty],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("valid");

    let out = table.substitute(fn_ty, &[(x, int)]);
    let TypeKind::Function { signature } = table.get(out).kind() else {
        panic!("expected a function type");
    };
    let sig = table.signature(signature);
    // The declared binder kept its own id — `Subst::Slice` cannot touch it.
    assert_eq!(table.function_type_parameters(sig.type_parameters), vec![y]);
    // The free `X` return substituted to `int`; the `Y` formal did not.
    assert_eq!(sig.return_type, int);
    assert_eq!(table.arg(sig.positional.offset()), y_ty);
}

/// `rebind_function_params` mints fresh binder ids and renames their
/// occurrences: `X Function<Y>(Y)` rebounds under two different fresh ids
/// — the returned pairs key the per-use bound writes (#3383 route B).
#[test]
fn rebind_function_params_mints_fresh_binders_and_renames() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(10);
    let y = TypeParamId::from_raw(11);
    let x_ty = table.intern(UninternedType::TypeParameter { parameter: x }, false);
    let y_ty = table.intern(UninternedType::TypeParameter { parameter: y }, false);
    let fn_ty = table
        .intern_function(
            UninternedFunction {
                return_type: x_ty,
                type_parameters: vec![y],
                positional: vec![y_ty],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("valid");

    let mut next = 100_u32;
    let (rebound, pairs) = table.rebind_function_params(fn_ty, &mut || {
        let id = TypeParamId::from_raw(next);
        next += 1;
        id
    });
    assert_eq!(pairs.len(), 1, "one binder refreshed");
    let (old, fresh) = pairs[0];
    assert_eq!(old, y);
    assert_eq!(fresh, TypeParamId::from_raw(100));
    let TypeKind::Function { signature } = table.get(rebound).kind() else {
        panic!("expected a function type");
    };
    let sig = table.signature(signature);
    // The binder range carries the fresh id; the `Y` formal renamed to it.
    assert_eq!(
        table.function_type_parameters(sig.type_parameters),
        vec![fresh]
    );
    let fresh_ty = table.intern(UninternedType::TypeParameter { parameter: fresh }, false);
    assert_eq!(table.arg(sig.positional.offset()), fresh_ty);
    // The free `X` return is untouched — only binder occurrences rename.
    assert_eq!(sig.return_type, x_ty);

    // A binder-free type comes back unchanged with no pairs.
    let (same, empty) = table.rebind_function_params(x_ty, &mut || TypeParamId::from_raw(500));
    assert_eq!(same, x_ty);
    assert!(empty.is_empty());
}
