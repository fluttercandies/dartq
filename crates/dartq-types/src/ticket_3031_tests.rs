//! Pins for #3031: the two `system.rs` subtype faces root-located after
//! the #3030 I2B exoneration.
//!
//! **Face A — the bounds-invariance leg of the generic-function
//! correlation.** Upstream `relateTypeParameters`
//! (type_system.dart:1581-1635) rejects the correlation when any
//! correlated parameter pair's bounds — each missing bound reading
//! `dynamic` (`DynamicTypeImpl.instance`, type_system.dart:1610-1611) —
//! are not `isEqualTo` after substitution onto one shared fresh set
//! (`isEqualTo` = mutual subtyping, type_system.dart:924-926); a null
//! answer makes `_isFunctionSubtypeOf` answer `false` before any body
//! leg runs (subtype.dart:338-344). The `==` face carries the same leg
//! through `relateTypeFormals` with plain type equality as the relation
//! (`(t, s) => t == s`, type.dart:271-273; missing bound ≡ `dynamic`,
//! type.dart:459-460). dartq's correlation renames `f`'s parameters onto
//! `g`'s — `g`'s parameters play the official's fresh variables — so the
//! leg renames `f`'s bound and reads `g`'s as-is.
//!
//! **Face B — `dynamic <: Never` verdict: already oracle-correct at this
//! layer, pinned here against regression.** `subtype_impl` has no
//! `Dynamic`-from short-circuit: the Left Top rule accepts `dynamic <: T1`
//! only when `Object? <: T1` (subtype.dart:292-296), and `Object? <:
//! Never` fails (`Object <: Never` and `Null <: Never` both fail), so the
//! walk reaches the fall-through `false`. `dynamic` is a subtype of
//! `dynamic`/`void`/`Object?` only. The `List<dynamic> → List<Never>`
//! acceptance #3030 observed routes through the semantic layer's two
//! remaining faces (the tearoff type-parameter drop and the call
//! return-type inference collapse, see `ticket_3030_tests.rs`), not
//! through this crate. `is_assignable(dynamic, Never)` staying `true`
//! under non-strict casts is oracle parity, not a bug
//! (`isAssignableTo`'s `fromType is DynamicType` downcast arm,
//! type_system.dart:919-923).

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
const INT: TypeDeclId = TypeDeclId::from_raw(5);
const STRING: TypeDeclId = TypeDeclId::from_raw(6);
const ITERABLE: TypeDeclId = TypeDeclId::from_raw(7);
const LIST: TypeDeclId = TypeDeclId::from_raw(8);

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
    let (iterable_e, list_e) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
    let list_of_e = table.intern(UninternedType::TypeParameter { parameter: list_e }, false);
    let hierarchy = TypeHierarchy::builder(CoreDecls {
        object: OBJECT,
        future: FUTURE,
        function: FUNCTION,
        record: None,
    })
    .decl(OBJECT, Vec::new())
    .decl(FUTURE, vec![iterable_e])
    .decl(FUNCTION, Vec::new())
    .decl(NUM, Vec::new())
    .decl(INT, Vec::new())
    .decl(STRING, Vec::new())
    .decl(ITERABLE, vec![iterable_e])
    .decl(LIST, vec![list_e])
    .supertype(NUM, itf(&table, OBJECT, &[]))
    .supertype(INT, itf(&table, NUM, &[]))
    .supertype(STRING, itf(&table, OBJECT, &[]))
    .supertype(ITERABLE, itf(&table, OBJECT, &[]))
    .supertype(LIST, itf(&table, ITERABLE, &[list_of_e]))
    .build(&table)
    .expect("fixture hierarchy is valid");
    (table, hierarchy)
}

/// `void Function<P...>()` with the given declared parameters.
fn generic_fn(table: &TypeTable, params: &[TypeParamId]) -> TypeId {
    table
        .intern_function(
            UninternedFunction {
                return_type: table.intern(UninternedType::Void, false),
                type_parameters: params.to_vec(),
                positional: Vec::new(),
                named: Vec::new(),
                required_positional: 0,
            },
            false,
        )
        .expect("valid signature")
}

/// Face A, the #3030 probes 2/3 shape (`typedef F<X> = void
/// Function<Y extends X>()` instantiated at `int` and `num`): `int` and
/// `num` are related one way (`int <: num`) but not `isEqualTo`, so
/// neither instantiation subtypes the other — both directions reject.
/// Without the bounds snapshot the leg is inert and both accept (the
/// documented gate, mirroring how the left-type-variable-bound rule
/// arms).
#[test]
fn generic_function_bounds_are_invariant() {
    let (table, hierarchy) = fixture();
    let (int, num) = (itf(&table, INT, &[]), itf(&table, NUM, &[]));
    let (y, z) = (TypeParamId::from_raw(30), TypeParamId::from_raw(31));
    let mut facts = ParamFactsTable::new();
    facts.set(y, ParamFacts::LEGACY.with_bound(int));
    facts.set(z, ParamFacts::LEGACY.with_bound(num));
    let f_int = generic_fn(&table, &[y]);
    let f_num = generic_fn(&table, &[z]);

    let plain = TypeSystem::new(&table, &hierarchy);
    assert!(plain.is_subtype(f_int, f_num), "inert without the snapshot");
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);
    assert!(
        !sys.is_subtype(f_int, f_num),
        "int bound <: num bound rejected"
    );
    assert!(
        !sys.is_subtype(f_num, f_int),
        "num bound <: int bound rejected"
    );
    assert!(sys.is_subtype(f_int, f_int), "reflexivity untouched");
}

/// Face A, matching bounds: alpha-renamed equal bounds (`num` vs `num`,
/// the F-bounded `List<T>` vs `List<U>`) keep the correlation, a bound
/// referencing a sibling parameter renames with it, and a mismatched
/// F-bound rejects. The missing-≡-`dynamic` reading follows the official
/// `?? DynamicTypeImpl.instance`: unbounded pairs with an `Object?` bound
/// (both tops, mutual) still correlate, while `dynamic` vs `int` does not.
#[test]
fn generic_function_matching_bounds_still_subtype() {
    let (table, hierarchy) = fixture();
    let (int, num) = (itf(&table, INT, &[]), itf(&table, NUM, &[]));
    let t_of = |p: TypeParamId| table.intern(UninternedType::TypeParameter { parameter: p }, false);
    // `<U extends num>` / `<V extends num>`; the mismatched F-bound `W`;
    // the F-bounded pair `<U2 extends List<U2>>` / `<V2 extends List<V2>>`;
    // the sibling-referencing pair `<A extends B, B extends num>` /
    // `<X extends Y, Y extends num>`; and the missing-≡-dynamic pairings.
    let (u, v, w) = (
        TypeParamId::from_raw(32),
        TypeParamId::from_raw(33),
        TypeParamId::from_raw(34),
    );
    let (u2, v2) = (TypeParamId::from_raw(35), TypeParamId::from_raw(36));
    let (a, b, x, y) = (
        TypeParamId::from_raw(37),
        TypeParamId::from_raw(38),
        TypeParamId::from_raw(39),
        TypeParamId::from_raw(40),
    );
    let (free, obj_q_bound, int_bound) = (
        TypeParamId::from_raw(41),
        TypeParamId::from_raw(42),
        TypeParamId::from_raw(43),
    );
    let object_q = table.intern(
        UninternedType::Interface {
            decl: OBJECT,
            args: Vec::new(),
        },
        true,
    );
    let mut facts = ParamFactsTable::new();
    facts.set(u, ParamFacts::LEGACY.with_bound(num));
    facts.set(v, ParamFacts::LEGACY.with_bound(num));
    facts.set(w, ParamFacts::LEGACY.with_bound(itf(&table, LIST, &[int])));
    facts.set(
        u2,
        ParamFacts::LEGACY.with_bound(itf(&table, LIST, &[t_of(u2)])),
    );
    facts.set(
        v2,
        ParamFacts::LEGACY.with_bound(itf(&table, LIST, &[t_of(v2)])),
    );
    facts.set(a, ParamFacts::LEGACY.with_bound(t_of(b)));
    facts.set(b, ParamFacts::LEGACY.with_bound(num));
    facts.set(x, ParamFacts::LEGACY.with_bound(t_of(y)));
    facts.set(y, ParamFacts::LEGACY.with_bound(num));
    facts.set(obj_q_bound, ParamFacts::LEGACY.with_bound(object_q));
    facts.set(int_bound, ParamFacts::LEGACY.with_bound(int));
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);

    // `<U extends num>` vs `<V extends num>`: renamed bounds are id-equal.
    assert!(sys.is_subtype(generic_fn(&table, &[u]), generic_fn(&table, &[v])));
    // F-bounded self-reference renames with the parameter; a concrete
    // mismatch against it rejects.
    assert!(sys.is_subtype(generic_fn(&table, &[u2]), generic_fn(&table, &[v2])));
    assert!(!sys.is_subtype(generic_fn(&table, &[u2]), generic_fn(&table, &[w])));
    // Sibling reference: the bound's `B` renames onto `Y`.
    assert!(sys.is_subtype(generic_fn(&table, &[a, b]), generic_fn(&table, &[x, y])));

    // Missing ≡ dynamic: unbounded vs `Object?`-bound correlate (both
    // tops, mutual subtyping); unbounded vs `int`-bound does not
    // (`dynamic <: int` fails through Left Top's `Object? <: T1`).
    assert!(sys.is_subtype(
        generic_fn(&table, &[free]),
        generic_fn(&table, &[obj_q_bound])
    ));
    assert!(!sys.is_subtype(
        generic_fn(&table, &[free]),
        generic_fn(&table, &[int_bound])
    ));
}

/// Face A: the bounds leg rejects before the body legs but never
/// overrides them — matching bounds with a contravariance-violating
/// positional parameter still rejects.
#[test]
fn generic_function_body_legs_still_enforced_with_bounds() {
    let (table, hierarchy) = fixture();
    let (int, string) = (itf(&table, INT, &[]), itf(&table, STRING, &[]));
    let (y, z) = (TypeParamId::from_raw(44), TypeParamId::from_raw(45));
    let mut facts = ParamFactsTable::new();
    facts.set(y, ParamFacts::LEGACY.with_bound(int));
    facts.set(z, ParamFacts::LEGACY.with_bound(int));
    let f = table
        .intern_function(
            UninternedFunction {
                return_type: table.intern(UninternedType::Void, false),
                type_parameters: vec![y],
                positional: vec![int],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("valid signature");
    let g = table
        .intern_function(
            UninternedFunction {
                return_type: table.intern(UninternedType::Void, false),
                type_parameters: vec![z],
                positional: vec![string],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("valid signature");
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);
    assert!(
        !sys.is_subtype(g, f),
        "String is not a subtype of int: contravariance rejects"
    );
}

/// Face A on the `==` grade (`FunctionTypeImpl.==` via
/// `relateTypeFormals`, relation `(t, s) => t == s`, type.dart:271-273):
/// differing bounds break structural equality under the snapshot; equal
/// bounds keep it; without the snapshot the leg is inert (both read
/// `dynamic`, `==` to itself).
#[test]
fn structural_equality_honors_bounds() {
    let (table, hierarchy) = fixture();
    let (int, num) = (itf(&table, INT, &[]), itf(&table, NUM, &[]));
    let (y, z, v) = (
        TypeParamId::from_raw(46),
        TypeParamId::from_raw(47),
        TypeParamId::from_raw(48),
    );
    let mut facts = ParamFactsTable::new();
    facts.set(y, ParamFacts::LEGACY.with_bound(int));
    facts.set(z, ParamFacts::LEGACY.with_bound(num));
    facts.set(v, ParamFacts::LEGACY.with_bound(int));
    let (f_int, f_num, f_v) = (
        generic_fn(&table, &[y]),
        generic_fn(&table, &[z]),
        generic_fn(&table, &[v]),
    );

    let plain = TypeSystem::new(&table, &hierarchy);
    assert!(plain.is_structurally_equal(f_int, f_num), "inert gate");
    let sys = TypeSystem::new(&table, &hierarchy).with_param_facts(&facts);
    assert!(!sys.is_structurally_equal(f_int, f_num), "int != num");
    assert!(sys.is_structurally_equal(f_int, f_v), "int == int");
}

/// Face B: `dynamic` subtypes only the tops — never `Never`, directly or
/// through a class argument (the #3030 probe 7/8 shapes). The `Never`-
/// to-`dynamic` direction stays covariantly true (Left Bottom), and
/// `Object <: Never` rejects too.
#[test]
fn dynamic_is_not_a_subtype_of_never() {
    let (table, hierarchy) = fixture();
    let sys = TypeSystem::new(&table, &hierarchy);
    let never = table.intern(UninternedType::Never, false);
    let dynamic = table.intern(UninternedType::Dynamic, false);
    let void_ty = table.intern(UninternedType::Void, false);
    let object = itf(&table, OBJECT, &[]);
    let object_q = table.intern(
        UninternedType::Interface {
            decl: OBJECT,
            args: Vec::new(),
        },
        true,
    );

    assert!(!sys.is_subtype(dynamic, never), "the face itself");
    assert!(sys.is_subtype(dynamic, dynamic));
    assert!(sys.is_subtype(dynamic, void_ty));
    assert!(sys.is_subtype(dynamic, object_q));
    assert!(!sys.is_subtype(object, never));
    assert!(sys.is_subtype(never, dynamic), "Left Bottom keeps it true");

    // The class-argument face: `List<dynamic> <: List<Never>` rejects
    // both through same-decl argument checks; `List<Never> <:
    // List<dynamic>` accepts.
    let list_dynamic = itf(&table, LIST, &[dynamic]);
    let list_never = itf(&table, LIST, &[never]);
    assert!(!sys.is_subtype(list_dynamic, list_never));
    assert!(sys.is_subtype(list_never, list_dynamic));
}

/// Face B, assignability parity (`isAssignableTo`,
/// type_system.dart:891-925): the `dynamic`-from downcast arm fires only
/// when `from` itself is `dynamic` — `Never n = d` is accepted by the
/// oracle under default casts — while `List<dynamic> → List<Never>`
/// (from is `List`, not `dynamic`) rejects, and `strict-casts` closes
/// the downcast entirely.
#[test]
fn dynamic_downcast_arm_matches_the_oracle() {
    let (table, hierarchy) = fixture();
    let sys = TypeSystem::new(&table, &hierarchy);
    let never = table.intern(UninternedType::Never, false);
    let dynamic = table.intern(UninternedType::Dynamic, false);
    let list_dynamic = itf(&table, LIST, &[dynamic]);
    let list_never = itf(&table, LIST, &[never]);

    assert!(sys.is_assignable(dynamic, never, false), "oracle accepts");
    assert!(!sys.is_assignable(dynamic, never, true), "strict-casts");
    assert!(
        !sys.is_assignable(list_dynamic, list_never, false),
        "from is not dynamic: rejected"
    );
}
