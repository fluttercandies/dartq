//! Pins for #3030: the recursive instantiate-to-bound fixed point on
//! `class A<X extends FutureOr<A<X>>>` and kin.
//!
//! Audit outcome (2026-10-30, live oracle comparison, SDK 3.13.x pin):
//! dartq's fixed point **matches the oracle** on every probed shape — the
//! malbounded Phi substitution (upstream
//! `type_system.dart:735-811`, `instantiateTypeFormalsToBounds`) grounds
//! the cyclic bound exactly as the analyzer does. The "divergent bound"
//! text seen in the substrate audit
//! (`'FutureOr<A<Never>?>' doesn't conform to the bound
//! 'FutureOr<A<FutureOr<A<Never>?>>>'`) is emitted **verbatim by the
//! oracle** (conformance `expected.tsv`, case
//! `Instantiate-to-bound/class/static/class_FutureOr_l1_t02`); it comes
//! from the well-boundedness/inverted-type face
//! (`replace_top_bottom` + `regular_bounded_issues`), not from I2B.
//!
//! The rows actually missing on that case are the four `invalid_assignment`
//! rows through `typedef F<X> = void Function<Y extends X>()`, and they
//! stem from three unrelated faces (none in this file's algorithm):
//! 1. the tearoff of a generic function declaration drops its type
//!    parameters (`void g<Y extends int>()` typed `void Function()`);
//! 2. the return-type inference of a call whose return type is a
//!    generic-function typedef (`F<X> toF<X>(X x)`) collapses to
//!    dynamic/unknown, silently accepting every later assignment;
//! 3. `function_subtype` (system.rs) omits the bounds leg of the official
//!    `relateTypeParameters` (type_system.dart:1581-1635: per parameter,
//!    both bounds substituted onto a shared fresh set — unbounded reading
//!    `dynamic` — must be `isEqualTo`).
//!
//! These pins lock the oracle-verified defaults so the fixed point cannot
//! regress while those faces are fixed elsewhere.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::instantiate_to_bound::instantiate_type_formals_to_bounds;
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedType};
use crate::variance::{ParamFacts, ParamFactsTable, Variance};

const A: TypeDeclId = TypeDeclId::from_raw(5);
const LIST: TypeDeclId = TypeDeclId::from_raw(6);
const NUM: TypeDeclId = TypeDeclId::from_raw(7);

fn param(table: &TypeTable, p: TypeParamId) -> TypeId {
    table.intern(UninternedType::TypeParameter { parameter: p }, false)
}

fn interface(table: &TypeTable, decl: TypeDeclId, args: Vec<TypeId>) -> TypeId {
    table.intern(UninternedType::Interface { decl, args }, false)
}

fn facts_with(p: TypeParamId, bound: TypeId) -> ParamFactsTable {
    let mut facts = ParamFactsTable::new();
    facts.set(
        p,
        ParamFacts {
            variance: Variance::Covariant,
            bound: Some(bound),
        },
    );
    facts
}

/// `class A<X extends FutureOr<A<X>>>` — the FutureOr-headed self-cycle.
/// The progress loop cannot ground `X` (its own partial default mentions
/// it), so the Phi substitution maps `X -> dynamic`, giving the default
/// `FutureOr<A<dynamic>>` with `malbounded = true`. Oracle-verified: the
/// co19 case types the raw `A?` as `A<FutureOr<A<dynamic>>>?`
/// (class_FutureOr_l1_t02, `invalid_assignment` rows naming
/// `F<A<FutureOr<A<dynamic>>>?>`).
#[test]
fn future_or_headed_self_cycle_defaults_to_phi_future_or_a_dynamic() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(20);
    let a_of_x = interface(&table, A, vec![param(&table, x)]);
    let bound = table.intern(UninternedType::FutureOr { base: a_of_x }, false);
    let result = instantiate_type_formals_to_bounds(&table, &facts_with(x, bound), &[x]);

    assert!(result.malbounded, "the cycle cannot ground: malbounded");
    let default = table.get(result.defaults[0]);
    match default.kind() {
        TypeKind::FutureOr { base } => {
            assert!(
                matches!(
                    table.get(base).kind(),
                    TypeKind::Interface { decl: A, args } if args.len() == 1
                        && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Dynamic)
                ),
                "the Phi default must be FutureOr<A<dynamic>>"
            );
        }
        other => panic!("expected FutureOr<..>, got {other:?}"),
    }
    assert!(!default.is_nullable());
}

/// `class C<T extends C<T>>` — the plain (non-FutureOr) self-cycle. The
/// default is the Phi rewrite `C<dynamic>`, `malbounded = true`.
/// Oracle-verified: the raw `C?` reads back as `C<C<dynamic>>?`.
#[test]
fn plain_class_self_cycle_defaults_to_c_of_dynamic() {
    let table = TypeTable::new();
    let t = TypeParamId::from_raw(21);
    let bound = interface(&table, A, vec![param(&table, t)]);
    let result = instantiate_type_formals_to_bounds(&table, &facts_with(t, bound), &[t]);

    assert!(result.malbounded);
    assert!(
        matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Interface { decl: A, args } if args.len() == 1
                && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Dynamic)
        ),
        "the Phi default must be C<dynamic>"
    );
}

/// `class Z<T extends T>` — the degenerate cycle where the bound is the
/// parameter itself. Stuck at the fixed point; Phi gives plain `dynamic`.
/// Oracle-verified: the raw `Z?` reads back as `Z<dynamic>?` (the oracle
/// separately reports `type_parameter_supertype_of_its_bound` at the
/// declaration — a verifier diagnostic, not an I2B concern).
#[test]
fn degenerate_self_bound_defaults_to_dynamic() {
    let table = TypeTable::new();
    let t = TypeParamId::from_raw(22);
    let bound = param(&table, t);
    let result = instantiate_type_formals_to_bounds(&table, &facts_with(t, bound), &[t]);

    assert!(result.malbounded);
    assert!(matches!(
        table.get(result.defaults[0]).kind(),
        TypeKind::Dynamic
    ));
}

/// `class W<T extends List<num>>` — a normal bound with no free
/// parameters: it is ground on the first scan, no malboundedness, and the
/// default is the bound itself. Oracle-verified: the raw `W?` reads back
/// as `W<List<num>>?`.
#[test]
fn normal_interface_bound_grounds_unchanged() {
    let table = TypeTable::new();
    let t = TypeParamId::from_raw(23);
    let num = interface(&table, NUM, Vec::new());
    let bound = interface(&table, LIST, vec![num]);
    let result = instantiate_type_formals_to_bounds(&table, &facts_with(t, bound), &[t]);

    assert!(!result.malbounded);
    assert_eq!(result.defaults[0], bound);
}
