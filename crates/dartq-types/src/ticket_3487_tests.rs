//! Pins for #3487: the analyzer's two instantiate-to-bound faces answer the
//! same bounds differently, and neither entry may stand in for the other.
//! Every pin runs one bound list through both entries and pins both
//! answers:
//!
//! * the to-bounds face — [`instantiate_type_formals_to_bounds_fixpoint`],
//!   `instantiateTypeFormalsToBounds` (fixpoint plus `Phi`, plain
//!   nullability-uniting substitution);
//! * the default-type face of a class parameter —
//!   [`instantiate_type_formals_to_bounds_with`], `defaultType` (covariant
//!   seed, variance-directed upper `dynamic` / lower `Never`).
//!
//! Oracle verification (Dart SDK 3.13.7 `dart analyze --format=machine`):
//! each pin's fixture observes both faces over the same bounds.
//!
//! * The to-bounds face through extension specificity: `E1` declares the
//!   pinned parameters, `E2` declares parameters whose to-bounds type is the
//!   expected answer, and the receiver instantiates both to one type, so
//!   `_isMoreSpecific` (`extension_member_resolver.dart:466-506`) reaches
//!   step 5 and compares the two `_instantiateToBounds` types — equal types
//!   report `AMBIGUOUS_EXTENSION_MEMBER_ACCESS`, a strictly more specific
//!   `E2` resolves the member.
//! * The default-type face through a raw type: `class C<…same bounds…> {}`
//!   with `void g(C c) { int i = c; }`, whose `INVALID_ASSIGNMENT` message
//!   prints the raw instantiation.
//!
//! Each pin's fixture is its doc block verbatim. Its parenthetical
//! counterfactual was run too: rewriting `E1`'s bounds, acyclically, so its
//! to-bounds type is the other answer flips the diagnostic as stated.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::instantiate_to_bound::{
    BoundsInstantiation, instantiate_type_formals_to_bounds_fixpoint,
    instantiate_type_formals_to_bounds_with,
};
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedFunction, UninternedType};

const A: TypeDeclId = TypeDeclId::from_raw(5);
const LIST: TypeDeclId = TypeDeclId::from_raw(6);
const NUM: TypeDeclId = TypeDeclId::from_raw(7);
const INT: TypeDeclId = TypeDeclId::from_raw(8);

fn param(table: &TypeTable, parameter: TypeParamId, nullable: bool) -> TypeId {
    table.intern(UninternedType::TypeParameter { parameter }, nullable)
}

fn itf(table: &TypeTable, decl: TypeDeclId, args: &[TypeId]) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: args.to_vec(),
        },
        false,
    )
}

/// `void Function(formal)`.
fn void_fn(table: &TypeTable, formal: TypeId) -> TypeId {
    table
        .intern_function(
            UninternedFunction {
                return_type: table.intern(UninternedType::Void, false),
                type_parameters: Vec::new(),
                positional: vec![formal],
                named: Vec::new(),
                required_positional: 1,
            },
            false,
        )
        .expect("fixture signature is well formed")
}

/// Renders a type in the oracle's spelling (`List<int?>`,
/// `void Function(Never)`); a surviving parameter leaf renders `T#<id>` so
/// a failed grounding is visible in the assertion message.
fn show(table: &TypeTable, ty: TypeId) -> String {
    let node = table.get(ty);
    let suffix = if node.is_nullable() { "?" } else { "" };
    match node.kind() {
        TypeKind::Dynamic => "dynamic".to_string(),
        TypeKind::Void => "void".to_string(),
        TypeKind::Never => format!("Never{suffix}"),
        TypeKind::TypeParameter { parameter } => format!("T#{}{suffix}", parameter.raw()),
        TypeKind::Interface { decl, args } => {
            let name = match decl {
                A => "A",
                LIST => "List",
                NUM => "num",
                INT => "int",
                _ => "?",
            };
            let args = table.args(args);
            if args.is_empty() {
                format!("{name}{suffix}")
            } else {
                format!("{name}<{}>{suffix}", shows(table, &args).join(", "))
            }
        }
        TypeKind::Function { signature } => {
            let sig = table.signature(signature);
            format!(
                "{} Function({}){suffix}",
                show(table, sig.return_type),
                shows(table, &table.args(sig.positional)).join(", ")
            )
        }
        other => format!("{other:?}"),
    }
}

fn shows(table: &TypeTable, tys: &[TypeId]) -> Vec<String> {
    tys.iter().map(|&ty| show(table, ty)).collect()
}

/// Runs one bound list (`None` = unbounded) through the to-bounds face and
/// the class-parameter default-type face, in that order.
fn faces(
    table: &TypeTable,
    parameters: &[TypeParamId],
    bounds: &[Option<TypeId>],
) -> (BoundsInstantiation, BoundsInstantiation) {
    let bound_of = |p: TypeParamId| {
        parameters
            .iter()
            .position(|&q| q == p)
            .and_then(|i| bounds[i])
    };
    (
        instantiate_type_formals_to_bounds_fixpoint(table, parameters, bound_of),
        instantiate_type_formals_to_bounds_with(table, parameters, bound_of),
    )
}

/// `X extends void Function(X)` — a self cycle through a function-type
/// formal. To-bounds: `X` never completes and `Phi` maps it to `dynamic`
/// everywhere → `void Function(dynamic)`. Default: the formal column flips
/// the covariant seed to contravariant and reads the lower map →
/// `void Function(Never)`.
///
/// Oracle (3.13.7) over
///
/// ```dart
/// class A<X> {}
/// extension E1<X extends void Function(X)> on A<X> { int get foo => 1; }
/// extension E2 on A<void Function(dynamic)> { int get foo => 2; }
/// void main() { A<void Function(dynamic)> a = A(); a.foo; }
/// class C<X extends void Function(X)> {}
/// void g(C c) { int i = c; print(i); }
/// ```
///
/// reports `AMBIGUOUS_EXTENSION_MEMBER_ACCESS` at `a.foo` — `E1`'s
/// to-bounds type equals `E2`'s `A<void Function(dynamic)>` (the default
/// face's `A<void Function(Never)>` would make `E2` strictly more specific
/// and silent) — and `INVALID_ASSIGNMENT` "A value of type
/// 'C<void Function(Never)>' can't be assigned to a variable of type
/// 'int'."
#[test]
fn function_formal_cycle_reads_dynamic_in_the_to_bounds_face() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(10);
    let bound = void_fn(&table, param(&table, x, false));
    let (to_bounds, default) = faces(&table, &[x], &[Some(bound)]);
    assert!(to_bounds.malbounded, "the self cycle never completes");
    assert_eq!(
        shows(&table, &to_bounds.defaults),
        vec!["void Function(dynamic)"],
        "to-bounds: Phi maps the stuck `X` to `dynamic`, position-blind"
    );
    assert!(default.malbounded);
    assert_eq!(
        shows(&table, &default.defaults),
        vec!["void Function(Never)"],
        "default: the flipped formal column reads the lower map"
    );
}

/// `X extends A<X>, Y extends List<X>` — `Y` only depends on the cycle.
/// To-bounds: both stay incomplete, so `Phi` maps both to `dynamic` and `Y`
/// completes as `Phi(List<X>)` = `List<dynamic>`. Default: the component
/// rewrite is scoped to `X`'s self cycle and the grounding sweep hands `Y`
/// the member's default → `List<A<dynamic>>`.
///
/// Oracle (3.13.7) over
///
/// ```dart
/// class A<X> {}
/// class D extends A<D> {}
/// class B<S, T> {}
/// extension E1<X extends A<X>, Y extends List<X>> on B<X, Y> {
///   int get foo => 1;
/// }
/// extension E2<P extends A<dynamic>, Q extends List<A<dynamic>>> on B<P, Q> {
///   String get foo => '';
/// }
/// void main() { B<D, List<D>> b = B(); int i = b.foo; print(i); }
/// class C<X extends A<X>, Y extends List<X>> {}
/// void g(C c) { int i = c; print(i); }
/// ```
///
/// reports `INVALID_ASSIGNMENT` "A value of type 'String' can't be assigned
/// to a variable of type 'int'." at `b.foo` — `E2` is strictly more
/// specific, so `E1`'s to-bounds type is `B<A<dynamic>, List<dynamic>>`
/// (the default face's `List<A<dynamic>>` would tie and report ambiguity)
/// — and `INVALID_ASSIGNMENT` "A value of type
/// 'C<A<dynamic>, List<A<dynamic>>>' can't be assigned to a variable of
/// type 'int'."
#[test]
fn cycle_dependent_reads_phi_dynamic_in_the_to_bounds_face() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(20), TypeParamId::from_raw(21));
    let x_leaf = param(&table, x, false);
    let bounds = [
        Some(itf(&table, A, &[x_leaf])),
        Some(itf(&table, LIST, &[x_leaf])),
    ];
    let (to_bounds, default) = faces(&table, &[x, y], &bounds);
    assert!(to_bounds.malbounded);
    assert_eq!(
        shows(&table, &to_bounds.defaults),
        vec!["A<dynamic>", "List<dynamic>"],
        "to-bounds: `Y` reads the stuck `X` through Phi as `dynamic`"
    );
    assert!(default.malbounded);
    assert_eq!(
        shows(&table, &default.defaults),
        vec!["A<dynamic>", "List<A<dynamic>>"],
        "default: the grounding sweep hands `Y` the member's default"
    );
}

/// `X extends int, Y extends List<X?>` — no cycle; the faces split on the
/// `X?` leaf. To-bounds: `Substitution.fromMap` unites the leaf's `?` →
/// `List<int?>`. Default: `_UpperLowerReplacementVisitor` returns the
/// replacement verbatim → `List<int>`.
///
/// Oracle (3.13.7) over
///
/// ```dart
/// class B<S, T> {}
/// extension E1<X extends int, Y extends List<X?>> on B<X, Y> {
///   int get foo => 1;
/// }
/// extension E2<P extends int, Q extends List<int?>> on B<P, Q> {
///   int get foo => 2;
/// }
/// void main() { B<int, List<int>> b = B(); b.foo; }
/// class C<X extends int, Y extends List<X?>> {}
/// void g(C c) { int i = c; print(i); }
/// ```
///
/// reports `AMBIGUOUS_EXTENSION_MEMBER_ACCESS` at `b.foo` — `E1`'s
/// to-bounds type equals `E2`'s `B<int, List<int?>>` (a verbatim leaf's
/// `B<int, List<int>>` would make `E1` strictly more specific and silent) —
/// and `INVALID_ASSIGNMENT` "A value of type 'C<int, List<int>>' can't be
/// assigned to a variable of type 'int'."
#[test]
fn nullable_leaf_unites_in_the_to_bounds_face() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(30), TypeParamId::from_raw(31));
    let int = itf(&table, INT, &[]);
    let bounds = [
        Some(int),
        Some(itf(&table, LIST, &[param(&table, x, true)])),
    ];
    let (to_bounds, default) = faces(&table, &[x, y], &bounds);
    assert!(!to_bounds.malbounded);
    assert_eq!(
        shows(&table, &to_bounds.defaults),
        vec!["int", "List<int?>"],
        "to-bounds: the substitution unites the leaf's `?`"
    );
    assert!(!default.malbounded);
    assert_eq!(
        shows(&table, &default.defaults),
        vec!["int", "List<int>"],
        "default: the replacement is taken verbatim"
    );
}

/// `X extends num, Y extends void Function(X)` — no cycle; the faces split
/// at the formal. To-bounds: the completed `X` substitutes position-blind →
/// `void Function(num)`. Default: the grounding sweep's formal column reads
/// the lower map even without a cycle → `void Function(Never)`.
///
/// Oracle (3.13.7) over
///
/// ```dart
/// class B<S, T> {}
/// extension E1<X extends num, Y extends void Function(X)> on B<X, Y> {
///   int get foo => 1;
/// }
/// extension E2<P extends num, Q extends void Function(num)> on B<P, Q> {
///   int get foo => 2;
/// }
/// void main() { B<int, void Function(num)> b = B(); b.foo; }
/// class C<X extends num, Y extends void Function(X)> {}
/// void g(C c) { int i = c; print(i); }
/// ```
///
/// reports `AMBIGUOUS_EXTENSION_MEMBER_ACCESS` at `b.foo` — `E1`'s
/// to-bounds type equals `E2`'s `B<num, void Function(num)>` (the default
/// face's `void Function(Never)` would make `E2` strictly more specific and
/// silent) — and `INVALID_ASSIGNMENT` "A value of type
/// 'C<num, void Function(Never)>' can't be assigned to a variable of type
/// 'int'."
#[test]
fn acyclic_formal_substitutes_position_blind_in_the_to_bounds_face() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(40), TypeParamId::from_raw(41));
    let bounds = [
        Some(itf(&table, NUM, &[])),
        Some(void_fn(&table, param(&table, x, false))),
    ];
    let (to_bounds, default) = faces(&table, &[x, y], &bounds);
    assert!(!to_bounds.malbounded);
    assert_eq!(
        shows(&table, &to_bounds.defaults),
        vec!["num", "void Function(num)"],
        "to-bounds: the completed `X` substitutes into the formal as is"
    );
    assert!(!default.malbounded);
    assert_eq!(
        shows(&table, &default.defaults),
        vec!["num", "void Function(Never)"],
        "default: the flipped formal column reads the lower map"
    );
}

/// `X extends Y, Y extends num, Z` — an acyclic chain declared dependent
/// first, plus an unbounded parameter. To-bounds: `X` completes only on a
/// sweep after `Y` does, `Z` reads `dynamic`; nothing is malbounded. Both
/// faces agree here — the pin guards the progress loop: a single sweep
/// would leave `X` incomplete and `Phi` it to `dynamic`.
///
/// Oracle (3.13.7) over
///
/// ```dart
/// class B3<S, T, U> {}
/// extension E1<X extends Y, Y extends num, Z> on B3<X, Y, Z> {
///   int get foo => 1;
/// }
/// extension E2<P extends num, Q extends num, R> on B3<P, Q, R> {
///   int get foo => 2;
/// }
/// void main() { B3<int, int, String> b = B3(); b.foo; }
/// class C<X extends Y, Y extends num, Z> {}
/// void g(C c) { int i = c; print(i); }
/// ```
///
/// reports `AMBIGUOUS_EXTENSION_MEMBER_ACCESS` at `b.foo` — `E1`'s
/// to-bounds type equals `E2`'s `B3<num, num, dynamic>` (an incomplete `X`
/// read as `dynamic` would make `E2` strictly more specific and silent) —
/// and `INVALID_ASSIGNMENT` "A value of type 'C<num, num, dynamic>' can't
/// be assigned to a variable of type 'int'."
#[test]
fn out_of_order_chain_completes_in_the_to_bounds_face() {
    let table = TypeTable::new();
    let (x, y, z) = (
        TypeParamId::from_raw(50),
        TypeParamId::from_raw(51),
        TypeParamId::from_raw(52),
    );
    let bounds = [
        Some(param(&table, y, false)),
        Some(itf(&table, NUM, &[])),
        None,
    ];
    let (to_bounds, default) = faces(&table, &[x, y, z], &bounds);
    assert!(!to_bounds.malbounded);
    assert_eq!(
        shows(&table, &to_bounds.defaults),
        vec!["num", "num", "dynamic"],
        "to-bounds: `X` completes through `Y` on a later sweep"
    );
    assert!(!default.malbounded);
    assert_eq!(
        shows(&table, &default.defaults),
        vec!["num", "num", "dynamic"]
    );
}
