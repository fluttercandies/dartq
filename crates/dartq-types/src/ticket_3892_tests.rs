//! Pins for #3892: the binder-count gate and the verifier-side integration
//! of the generic-function bounds leg — the census family `function_A01`
//! t02-t05 plus the gfta/mismatch shapes.
//!
//! The bounds leg itself (`relateTypeParameters`,
//! type_system.dart:1581-1635: per-pair `isEqualTo` — mutual subtyping,
//! type_system.dart:924-926 — with a missing bound reading `dynamic`,
//! type_system.dart:1618-1619, rejecting before any body leg,
//! subtype.dart:338-344) is pinned by `ticket_3031_tests.rs` on the bare
//! `is_subtype` grade. Three faces of the same family are pinned here:
//!
//! * The **count gate** (`relateTypeParameters` answers `null` on any
//!   length mismatch before a bound is read, type_system.dart:1585-1587;
//!   two empty groups relate vacuously, :1588-1590) — the oracle's
//!   `void Function()` against `void Function<T>()` rejection.
//! * The **body correlation under equal bounds**: after the leg accepts,
//!   `_isFunctionSubtypeOf` instantiates both signatures with one shared
//!   fresh set (subtype.dart:347-348), so a body leaf spelling the source
//!   binder compares against the target's correlated binder — `T
//!   Function<T extends num>(T)` against `U Function<U extends num>(U)`
//!   is reflexive, never a name-lattice question.
//! * The **verifier integration**: `TypeBoundedHelper`'s regular pass
//!   (well_bounded.dart:74-119) asks `argument <: bound` through
//!   [`crate::system::TypeSystem::is_subtype`], so a generic-function
//!   argument whose violation lives ONLY in the binders' bounds reports
//!   exactly when BOTH sides' binder bounds ride the facts snapshot — the
//!   contract the semantic layer's merged-facts face fulfills. The
//!   super-bounded re-check (well_bounded.dart:121-129) inverts the whole
//!   type but keeps the binders verbatim
//!   (`replace_top_bottom_visitor.dart:56`), so a binder-only violation
//!   survives the inverted pass and reports `NotWellBounded` — never
//!   `SuperBounded`.

use crate::generic_function::{Boundedness, is_well_bounded, regular_bounded_issues};
use crate::hierarchy::{CoreDecls, TypeHierarchy};
use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::system::TypeSystem;
use crate::table::TypeTable;
use crate::ty::{UninternedFunction, UninternedType};
use crate::variance::{ParamFacts, ParamFactsTable};

const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
const NUM: TypeDeclId = TypeDeclId::from_raw(4);
const STRING: TypeDeclId = TypeDeclId::from_raw(5);
const INT: TypeDeclId = TypeDeclId::from_raw(6);
const G: TypeDeclId = TypeDeclId::from_raw(7);

fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: args.to_vec(),
        },
        false,
    )
}

fn fixture() -> (TypeTable, TypeHierarchy) {
    let table = TypeTable::new();
    let g_param = TypeParamId::from_raw(1);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object: OBJECT,
        future: FUTURE,
        function: FUNCTION,
        record: None,
    })
    .decl(OBJECT, Vec::new())
    .decl(FUTURE, vec![TypeParamId::from_raw(9)])
    .decl(FUNCTION, Vec::new())
    .decl(NUM, Vec::new())
    .decl(STRING, Vec::new())
    .decl(INT, Vec::new())
    .decl(G, vec![g_param])
    .supertype(NUM, itf(&table, OBJECT, &[]))
    .supertype(STRING, itf(&table, OBJECT, &[]))
    .supertype(INT, itf(&table, NUM, &[]))
    .build(&table)
    .expect("fixture hierarchy is valid");
    (table, hierarchy)
}

/// `R Function<P...>()` — the census family's zero-formal signature over
/// any return face.
fn generic_fn(table: &TypeTable, params: &[TypeParamId], return_ty: TypeId) -> TypeId {
    table
        .intern_function(
            UninternedFunction {
                return_type: return_ty,
                type_parameters: params.to_vec(),
                positional: Vec::new(),
                named: Vec::new(),
                required_positional: 0,
            },
            false,
        )
        .expect("valid signature")
}

/// `void Function<P...>()` — the census family's zero-formal signature.
fn generic_void_fn(table: &TypeTable, params: &[TypeParamId]) -> TypeId {
    generic_fn(table, params, table.intern(UninternedType::Void, false))
}

/// The count gate (type_system.dart:1585-1590): any length mismatch
/// rejects before a bound is read — even when the shared prefix's bounds
/// are equal — and two empty groups relate vacuously, so the bodies alone
/// decide. The oracle's t05 face: `C<void Function()>()` against
/// `void Function<T>()` fires while `C<void Function<T>()>()` against the
/// same bound does not (whatever the binders' bounds spell).
#[test]
fn binder_count_gate_rejects_before_any_bound_ask() {
    let (table, hierarchy) = fixture();
    let (num, string) = (itf(&table, NUM, &[]), itf(&table, STRING, &[]));
    let (t, u, v) = (
        TypeParamId::from_raw(30),
        TypeParamId::from_raw(31),
        TypeParamId::from_raw(32),
    );
    let mut facts = ParamFactsTable::new();
    // `t` unbounded on the one side; `u`/`v` both `extends num` — the
    // shared prefix's bounds coincide, so only the count can decide.
    facts.set(u, ParamFacts::LEGACY.with_bound(num));
    facts.set(v, ParamFacts::LEGACY.with_bound(num));
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);

    // 0 vs 1 rejects both ways, an unbounded binder against a bounded one.
    assert!(!sys.is_subtype(generic_void_fn(&table, &[]), generic_void_fn(&table, &[t])));
    assert!(!sys.is_subtype(generic_void_fn(&table, &[t]), generic_void_fn(&table, &[])));
    // 1 vs 2 rejects even with the first pair's bounds equal.
    assert!(!sys.is_subtype(
        generic_void_fn(&table, &[u]),
        generic_void_fn(&table, &[u, v])
    ));
    // 0 vs 0 passes — no group to relate, no bound to ask.
    assert!(sys.is_subtype(generic_void_fn(&table, &[]), generic_void_fn(&table, &[])));
    // The gate never overrides the body: an equal-count pair with equal
    // bounds still decides on its bodies (`String` vs `num` return —
    // `String <: num` is false; `void`, a top, would accept anything).
    assert!(!sys.is_subtype(
        generic_fn(&table, &[u], string),
        generic_fn(&table, &[v], num)
    ));
}

/// After the bounds leg accepts, the bodies compare over the correlated
/// binders (subtype.dart:347-348's shared instantiation): `T Function<T
/// extends num>(T)` subtypes `U Function<U extends num>(U)` — the source's
/// formal `T` renames onto the target's `U` and the contravariant ask is
/// reflexive — while a concrete formal against the binder asks `int <: U`,
/// which only the binder's own bound could license and `num <: int`
/// refuses. This is the leg the census t02 `exp1`/`exp4` split rides: the
/// passing shape needs the correlation, the failing one the body face over
/// the correlated binder.
#[test]
fn correlated_binders_rename_body_leaves_under_equal_bounds() {
    let (table, hierarchy) = fixture();
    let (num, int) = (itf(&table, NUM, &[]), itf(&table, INT, &[]));
    let (t, u) = (TypeParamId::from_raw(33), TypeParamId::from_raw(34));
    let t_of = |p: TypeParamId| table.intern(UninternedType::TypeParameter { parameter: p }, false);
    let mut facts = ParamFactsTable::new();
    facts.set(t, ParamFacts::LEGACY.with_bound(num));
    facts.set(u, ParamFacts::LEGACY.with_bound(num));
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);

    let identity = |binder: TypeParamId, formal: TypeId| {
        table
            .intern_function(
                UninternedFunction {
                    return_type: t_of(binder),
                    type_parameters: vec![binder],
                    positional: vec![formal],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("valid signature")
    };
    // `T Function<T extends num>(T)` <: `U Function<U extends num>(U)`:
    // the renamed bodies are id-equal.
    assert!(sys.is_subtype(identity(t, t_of(t)), identity(u, t_of(u))));
    // A concrete formal against the binder: contravariance asks
    // `int <: U`, whose only route is `bound(U) = num <: int` — false.
    assert!(!sys.is_subtype(identity(t, int), identity(u, t_of(u))));
}

/// The verifier integration the census family rides at this layer: the
/// regular pass asks `argument <: bound` through [`TypeSystem::is_subtype`],
/// so a generic-function argument whose violation lives ONLY in the
/// binders' bounds reports exactly one issue when both sides' binder
/// bounds ride the snapshot — and the super-bounded re-check, which
/// inverts the whole type but keeps the binders verbatim, still reports
/// `NotWellBounded` (the violation survives the inversion; `SuperBounded`
/// would silence the oracle's row). Equal binder bounds keep the same
/// instantiation `RegularBounded`.
#[test]
fn well_bounded_reports_a_binder_only_bound_violation() {
    let (table, hierarchy) = fixture();
    let (num, string) = (itf(&table, NUM, &[]), itf(&table, STRING, &[]));
    let g_param = TypeParamId::from_raw(1);
    // The bound's own binder `z extends num`; the argument's binder
    // `y extends String` — the violation lives only in the binders.
    let (y, z) = (TypeParamId::from_raw(35), TypeParamId::from_raw(36));
    let mut facts = ParamFactsTable::new();
    let bound_fn = generic_void_fn(&table, &[z]);
    facts.set(g_param, ParamFacts::LEGACY.with_bound(bound_fn));
    facts.set(y, ParamFacts::LEGACY.with_bound(string));
    facts.set(z, ParamFacts::LEGACY.with_bound(num));
    let arg_fn = generic_void_fn(&table, &[y]);
    let bad = itf(&table, G, &[arg_fn]);
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);

    let issues = regular_bounded_issues(&sys, &facts, bad);
    assert_eq!(issues.len(), 1, "the binder-only violation reports");
    assert_eq!(issues[0].parameter, g_param);
    assert_eq!(issues[0].argument, arg_fn);
    assert_eq!(issues[0].bound, bound_fn);

    // The super pass inverts `void` to `Never` in the argument but keeps
    // the binders (and their bounds) verbatim, so the re-check re-asks the
    // same binder pair and stays `NotWellBounded` — never `SuperBounded`.
    assert!(matches!(
        is_well_bounded(&sys, &facts, bad, true),
        Boundedness::NotWellBounded(_)
    ));

    // Equal binder bounds: the same shape is `RegularBounded` — the leg
    // fires only on a genuine bound mismatch (the additive contract).
    let mut equal_facts = ParamFactsTable::new();
    let equal_bound = generic_void_fn(&table, &[z]);
    equal_facts.set(g_param, ParamFacts::LEGACY.with_bound(equal_bound));
    equal_facts.set(y, ParamFacts::LEGACY.with_bound(num));
    equal_facts.set(z, ParamFacts::LEGACY.with_bound(num));
    let equal_sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&equal_facts);
    let good = itf(&table, G, &[generic_void_fn(&table, &[y])]);
    assert!(matches!(
        is_well_bounded(&equal_sys, &equal_facts, good, true),
        Boundedness::RegularBounded
    ));
}
