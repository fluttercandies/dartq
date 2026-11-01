//! Pins for #3468: instantiate-to-bound is *variance-directed* — the
//! oracle's `_UpperLowerReplacementVisitor`
//! (`default_types_builder.dart:431-479`) substitutes `upper`/`lower`
//! pairs seeded by each parameter's stored variance, not a `dynamic`-only
//! map. For a typedef that stored variance is computed from the aliased
//! body (`VarianceBuilder`, `variance_builder.dart:153-225`), so
//! `typedef G<X extends A<X>, Y extends X> = Y Function(X)` grounds X —
//! contravariant in the body — to `A<Never>`, the oracle's
//! `G<A<Never>, A<Never>>`; the class half (covariant seeds) keeps the
//! `A<dynamic>` answer unchanged.
//!
//! Every pin also carries the *measured baseline*: the same bounds through
//! the pre-change `dynamic`-only entry
//! ([`instantiate_type_formals_to_bounds_with`]), which answers
//! `A<dynamic>` for pin 1's bounds — the divergence these tests pin down.

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::instantiate_to_bound::{
    instantiate_type_formals_to_bounds, instantiate_type_formals_to_bounds_directed,
    instantiate_type_formals_to_bounds_with,
};
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedFunction, UninternedType};
use crate::variance::{
    ParamFacts, ParamFactsTable, Variance, typedef_parameter_variances,
};

const A: TypeDeclId = TypeDeclId::from_raw(5);
const NUM: TypeDeclId = TypeDeclId::from_raw(7);
const INT: TypeDeclId = TypeDeclId::from_raw(8);
const LIST: TypeDeclId = TypeDeclId::from_raw(9);

fn param(table: &TypeTable, parameter: TypeParamId) -> TypeId {
    table.intern(UninternedType::TypeParameter { parameter }, false)
}

fn a_of(table: &TypeTable, arg: TypeId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl: A,
            args: vec![arg],
        },
        false,
    )
}

fn list_of(table: &TypeTable, arg: TypeId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl: LIST,
            args: vec![arg],
        },
        false,
    )
}

fn plain(table: &TypeTable, decl: TypeDeclId) -> TypeId {
    table.intern(
        UninternedType::Interface {
            decl,
            args: Vec::new(),
        },
        false,
    )
}

fn fn_type(table: &TypeTable, return_type: TypeId, positional: Vec<TypeId>) -> TypeId {
    let required_positional = positional.len() as u32;
    table
        .intern_function(
            UninternedFunction {
                return_type,
                type_parameters: Vec::new(),
                positional,
                named: Vec::new(),
                required_positional,
            },
            false,
        )
        .expect("fixture signature is well formed")
}

/// The oracle's interface-argument lookup adapted to the test fixtures:
/// `A`, `List`, `num`, `int` each declare a single legacy-covariant
/// parameter (or none). `None` for an unmodelled declaration — the same
/// under-report answer the semantic layer gives a decl outside its map.
fn arg_variances(_: TypeDeclId, _: usize) -> Option<Variance> {
    Some(Variance::Covariant)
}

/// Renders a type back to its source-ish spelling so assertions read the
/// completion strings the oracle prints (`A<Never>`, `A<dynamic>`,
/// `void Function(Never)`).
fn show(table: &TypeTable, ty: TypeId) -> String {
    let node = table.get(ty);
    let suffix = if node.is_nullable() { "?" } else { "" };
    match node.kind() {
        TypeKind::Never => format!("Never{suffix}"),
        TypeKind::Dynamic => "dynamic".to_string(),
        TypeKind::Void => "void".to_string(),
        TypeKind::Invalid => "Invalid".to_string(),
        TypeKind::Null => "Null".to_string(),
        TypeKind::TypeParameter { parameter } => format!("T#{}", parameter.raw()),
        TypeKind::Interface { decl, args } => {
            let name = match decl {
                A => "A",
                NUM => "num",
                INT => "int",
                LIST => "List",
                _ => "?",
            };
            if args.is_empty() {
                format!("{name}{suffix}")
            } else {
                let inner = (0..args.len())
                    .map(|i| show(table, table.arg(args.offset() + i)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}<{inner}>{suffix}")
            }
        }
        TypeKind::FutureOr { base } => format!("FutureOr<{}>{suffix}", show(table, base)),
        TypeKind::Function { signature } => {
            let sig = table.signature(signature);
            let positional = (0..sig.positional.len())
                .map(|i| show(table, table.arg(sig.positional.offset() + i)))
                .collect::<Vec<_>>()
                .join(", ");
            let named = table
                .named_members(sig.named)
                .iter()
                .map(|member| {
                    format!(
                        "{}{} {}",
                        if member.required { "required " } else { "" },
                        show(table, member.ty),
                        member.name
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let formals = [positional, named]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} Function({formals}){suffix}", show(table, sig.return_type))
        }
        TypeKind::Record { shape } => {
            let shape = table.record_shape(shape);
            let positional = (0..shape.positional.len())
                .map(|i| show(table, table.arg(shape.positional.offset() + i)))
                .collect::<Vec<_>>()
                .join(", ");
            let named = table
                .named_members(shape.named)
                .iter()
                .map(|member| format!("{} {}", show(table, member.ty), member.name))
                .collect::<Vec<_>>()
                .join(", ");
            let fields = [positional, named]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({fields}){suffix}")
        }
    }
}

fn shows(table: &TypeTable, tys: &[TypeId]) -> Vec<String> {
    tys.iter().map(|&ty| show(table, ty)).collect()
}

// —— pin 1: the ticket's headline case ————————————————————————————————

/// `typedef G<X extends A<X>, Y extends X> = Y Function(X)` — the
/// variance derivation itself is pinned: `variance_in_type` over the
/// flattened body reads X contravariant (a formal position) and Y
/// covariant (the return), exactly `VarianceBuilder._genericTypeAlias`
/// (variance_builder.dart:188-225 → :121-151).
///
/// Baseline (measured): the same bounds through the pre-change entry
/// complete as `A<dynamic>, A<dynamic>` — the first `assert_eq!` below
/// runs on the unchanged code path and passes before and after this
/// ticket.
#[test]
fn typedef_g_contravariant_x_grounds_to_a_never() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(10), TypeParamId::from_raw(11));
    let body = fn_type(&table, param(&table, y), vec![param(&table, x)]);
    let facts = ParamFactsTable::new();
    let variances =
        typedef_parameter_variances(&table, &facts, &[x, y], body, arg_variances);
    assert_eq!(
        variances,
        vec![Variance::Contravariant, Variance::Covariant],
        "X is contravariant (formal), Y covariant (return) in `Y Function(X)`"
    );

    let x_bound = a_of(&table, param(&table, x));
    let y_bound = param(&table, x);
    let bound_of = |p: TypeParamId| {
        if p == x {
            Some(x_bound)
        } else if p == y {
            Some(y_bound)
        } else {
            None
        }
    };
    // Baseline: the unchanged dynamic-only entry answers `A<dynamic>` for
    // the same bounds — the divergence this ticket introduces is exactly
    // the directed substitution below.
    let baseline = instantiate_type_formals_to_bounds_with(&table, &[x, y], bound_of);
    assert_eq!(
        shows(&table, &baseline.defaults),
        vec!["A<dynamic>", "A<dynamic>"],
        "baseline: the pre-change port substituted `dynamic` everywhere"
    );

    let directed = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x, y],
        bound_of,
        |p| if p == x { variances[0] } else { variances[1] },
        arg_variances,
    );
    assert!(directed.malbounded, "the self-cycle is still malbounded");
    assert_eq!(
        shows(&table, &directed.defaults),
        vec!["A<Never>", "A<Never>"],
        "the oracle renders `G<A<Never>, A<Never>>`: X grounds through the \
         contravariant lower map; Y — covariant — copies X's grounded \
         default through the upper map"
    );
}

// —— pin 2: the same parameter in a covariant position —————————————————

/// `typedef G<X extends A<X>> = X Function()` — X covariant in the body
/// (the return) seeds the visitor covariant, so its `A<X>` bound grounds
/// to `A<dynamic>` — the identical shape the class half produces.
#[test]
fn covariant_position_keeps_the_dynamic_answer() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(20);
    let body = fn_type(&table, param(&table, x), Vec::new());
    let facts = ParamFactsTable::new();
    let variances = typedef_parameter_variances(&table, &facts, &[x], body, arg_variances);
    assert_eq!(variances, vec![Variance::Covariant]);

    let bound = a_of(&table, param(&table, x));
    let directed = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(bound),
        |p| {
            assert_eq!(p, x);
            variances[0]
        },
        arg_variances,
    );
    assert!(directed.malbounded);
    assert_eq!(shows(&table, &directed.defaults), vec!["A<dynamic>"]);
}

// —— pin 3: the class half must not move ——————————————————————————————

/// `class C<X extends A<X>>` — the facts-snapshot entry with a
/// legacy-covariant parameter: the directed visitor seeded covariant reads
/// the upper map everywhere, so the answer is the unchanged `A<dynamic>`
/// the class half has always produced.
#[test]
fn class_cycle_stays_dynamic_through_the_facts_entry() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(30);
    let mut facts = ParamFactsTable::new();
    facts.set(
        x,
        ParamFacts {
            variance: Variance::Covariant,
            bound: Some(a_of(&table, param(&table, x))),
        },
    );
    let result = instantiate_type_formals_to_bounds(&table, &facts, &[x]);
    assert!(result.malbounded);
    assert_eq!(
        shows(&table, &result.defaults),
        vec!["A<dynamic>"],
        "covariant seed → upper map → the class answer is untouched"
    );
}

// —— pin 4: mixed position ————————————————————————————————————————————

/// `typedef M<X extends A<X>> = X Function(X)` — X is covariant in the
/// return *and* contravariant in the formal: the meet over positions is
/// `invariant` (shared_type.dart:129-134 — `index | index`), and an
/// invariant seed is not `contravariant`, so every occurrence reads the
/// upper map → `A<dynamic>`.
///
/// Per-position, the same seed can still reach `Never`: with a covariant
/// seed and a bound that itself mixes positions — `A<X> Function(X)` —
/// the return argument grounds to `A<dynamic>` while the formal grounds
/// to `Never` (`A<dynamic> Function(Never)`); a contravariant seed swaps
/// them (`A<Never> Function(dynamic)`).
#[test]
fn mixed_position_meets_to_invariant_and_splits_per_position() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(40);
    let facts = ParamFactsTable::new();

    // The union result: body `X Function(X)` → invariant → `A<dynamic>`.
    let body = fn_type(&table, param(&table, x), vec![param(&table, x)]);
    let variances = typedef_parameter_variances(&table, &facts, &[x], body, arg_variances);
    assert_eq!(
        variances,
        vec![Variance::Invariant],
        "covariant.meet(contravariant) over the body is invariant"
    );
    let bound = a_of(&table, param(&table, x));
    let directed = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(bound),
        |p| {
            assert_eq!(p, x);
            variances[0]
        },
        arg_variances,
    );
    assert_eq!(
        shows(&table, &directed.defaults),
        vec!["A<dynamic>"],
        "invariant seeds read the upper map everywhere"
    );

    // The per-position split: bound `A<X> Function(X)`.
    let mixed_bound = fn_type(
        &table,
        a_of(&table, param(&table, x)),
        vec![param(&table, x)],
    );
    let covariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(mixed_bound),
        |_| Variance::Covariant,
        arg_variances,
    );
    assert_eq!(
        shows(&table, &covariant.defaults),
        vec!["A<dynamic> Function(Never)"],
        "covariant seed: return arg → upper, formal → lower"
    );
    let contravariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(mixed_bound),
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert_eq!(
        shows(&table, &contravariant.defaults),
        vec!["A<Never> Function(dynamic)"],
        "contravariant seed: return arg → lower, formal → upper"
    );
}

// —— pin 5: nested function types flip once per level ——————————————————

/// `X extends void Function(void Function(X))` — `changeVariance`
/// (:444-451) flips once around each formal list, so the seed parity
/// decides the innermost leaf: a contravariant seed flips to covariant
/// around the outer formals, then flips to contravariant around the
/// inner ones → `void Function(void Function(Never))`; a covariant seed
/// lands on the inner leaf covariant → `void Function(void
/// Function(dynamic))`.
#[test]
fn nested_function_type_flips_once_per_level() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(50);
    let void = table.intern(UninternedType::Void, false);
    let bound = fn_type(
        &table,
        void,
        vec![fn_type(&table, void, vec![param(&table, x)])],
    );
    let contravariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(bound),
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert_eq!(
        shows(&table, &contravariant.defaults),
        vec!["void Function(void Function(Never))"],
        "contra → (flip) co → (flip) contra → Never"
    );
    let covariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(bound),
        |_| Variance::Covariant,
        arg_variances,
    );
    assert_eq!(
        shows(&table, &covariant.defaults),
        vec!["void Function(void Function(dynamic))"],
        "co → (flip) contra → (flip) co → dynamic"
    );
}

// —— pin 6: the degradation contract ———————————————————————————————————

/// A bound provider that cannot expand a typedef head still reports the
/// parameter unbounded (`None` → `dynamic`), *under any seed* — the
/// module's alias contract preserved: a contravariant seed cannot turn a
/// missing bound into `Never` (an under-report is acceptable; a
/// fabricated `Never` is not).
#[test]
fn unmodelled_head_keeps_the_dynamic_degradation() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(60);
    for seed in [Variance::Covariant, Variance::Contravariant] {
        let result = instantiate_type_formals_to_bounds_directed(
            &table,
            &[x],
            |_| None,
            |_| seed,
            arg_variances,
        );
        assert!(!result.malbounded);
        assert_eq!(
            shows(&table, &result.defaults),
            vec!["dynamic"],
            "unbounded reads `dynamic` even under a {seed:?} seed"
        );
    }
}

// —— pin 7: a non-generic bound ignores variance ———————————————————————

/// `X extends num` under a contravariant seed — the bound carries no
/// parameter leaves, so the visitor leaves it verbatim: `num`, not
/// `Never`.
#[test]
fn non_generic_bound_ignores_the_seed() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(70);
    let num = plain(&table, NUM);
    let result = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(num),
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert!(!result.malbounded);
    assert_eq!(shows(&table, &result.defaults), vec!["num"]);
}

// —— pin 8: a genuinely cyclic SCC stays malbounded ————————————————————

/// `X extends Y, Y extends X` — the two-member SCC still reports
/// `malbounded`; the directed rewrite answers per seed: covariant members
/// → `dynamic` (the class answer), contravariant members → `Never` (the
/// oracle's lower map, default_types_builder.dart:150/160).
#[test]
fn mutual_cycle_stays_malbounded_and_answers_per_seed() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(80), TypeParamId::from_raw(81));
    let bound_of = |p: TypeParamId| {
        if p == x {
            Some(param(&table, y))
        } else if p == y {
            Some(param(&table, x))
        } else {
            None
        }
    };
    let covariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x, y],
        bound_of,
        |_| Variance::Covariant,
        arg_variances,
    );
    assert!(covariant.malbounded);
    assert_eq!(
        shows(&table, &covariant.defaults),
        vec!["dynamic", "dynamic"],
        "covariant cycle → the unchanged `dynamic` answer"
    );
    let contravariant = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x, y],
        bound_of,
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert!(contravariant.malbounded);
    assert_eq!(
        shows(&table, &contravariant.defaults),
        vec!["Never", "Never"],
        "contravariant cycle → the oracle's lower map `Never`"
    );
}

// —— the variance derivation ——————————————————————————————————————————

/// `VarianceBuilder` itself: `typedef_parameter_variances` answers per
/// body — covariant in an interface argument (`List<X>`), contravariant
/// in a formal (`void Function(X)`), invariant through the bound-invariance
/// rule (a nested `void Function<T extends X>()`), unrelated when the
/// parameter never occurs (`int`).
#[test]
fn typedef_variances_come_from_the_aliased_body() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(90);
    let mut facts = ParamFactsTable::new();

    // `typedef F<X> = List<X>` — covariant through the covariant arg.
    let body = list_of(&table, param(&table, x));
    assert_eq!(
        typedef_parameter_variances(&table, &facts, &[x], body, arg_variances),
        vec![Variance::Covariant]
    );

    // `typedef F<X> = void Function(X)` — contravariant (formal column).
    let void = table.intern(UninternedType::Void, false);
    let body = fn_type(&table, void, vec![param(&table, x)]);
    assert_eq!(
        typedef_parameter_variances(&table, &facts, &[x], body, arg_variances),
        vec![Variance::Contravariant]
    );

    // `typedef F<X> = void Function<T extends X>()` — the bound-invariance
    // rule (variance_builder.dart:131-140) forces invariant.
    let t = TypeParamId::from_raw(91);
    facts.set(
        t,
        ParamFacts {
            variance: Variance::Covariant,
            bound: Some(param(&table, x)),
        },
    );
    let body = table
        .intern_function(
            UninternedFunction {
                return_type: void,
                type_parameters: vec![t],
                positional: Vec::new(),
                named: Vec::new(),
                required_positional: 0,
            },
            false,
        )
        .expect("fixture signature is well formed");
    assert_eq!(
        typedef_parameter_variances(&table, &facts, &[x], body, arg_variances),
        vec![Variance::Invariant]
    );

    // `typedef F<X> = int` — X never occurs → unrelated.
    let body = plain(&table, INT);
    assert_eq!(
        typedef_parameter_variances(&table, &facts, &[x], body, arg_variances),
        vec![Variance::Unrelated]
    );
}

// —— the grounding sweep is directed per bound owner ———————————————————

/// `X extends A<X>, Y extends X` with *Y* contravariant — the grounding
/// sweep seeds the visitor with the bound owner's variance
/// (default_types_builder.dart:175-176), so a contravariant dependent
/// grounds to `Never`, not to the peer's `A<Never>` default.
#[test]
fn contravariant_dependent_grounds_to_never_not_the_peer_default() {
    let table = TypeTable::new();
    let (x, y) = (TypeParamId::from_raw(100), TypeParamId::from_raw(101));
    let x_bound = a_of(&table, param(&table, x));
    let y_bound = param(&table, x);
    let result = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x, y],
        |p| {
            if p == x {
                Some(x_bound)
            } else if p == y {
                Some(y_bound)
            } else {
                None
            }
        },
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert!(result.malbounded);
    assert_eq!(
        shows(&table, &result.defaults),
        vec!["A<Never>", "Never"],
        "X = A<Never> (its own bound rewritten); Y — contravariant — reads \
         the lower map where its peer `X` maps to `Never`"
    );
}

// —— verbatim leaf substitution (no nullability union) —————————————————

/// `X extends A<X?>` under a contravariant seed — the leaf is `X?`, and
/// `_UpperLowerReplacementVisitor.visitTypeParameterType` (:472-478)
/// returns the map value *verbatim*: `A<Never>`, not a `?`-unioned
/// `A<Never?>`.
#[test]
fn nullable_leaf_takes_the_map_value_verbatim() {
    let table = TypeTable::new();
    let x = TypeParamId::from_raw(110);
    let x_nullable = table.intern(UninternedType::TypeParameter { parameter: x }, true);
    let bound = a_of(&table, x_nullable);
    let result = instantiate_type_formals_to_bounds_directed(
        &table,
        &[x],
        |p| (p == x).then_some(bound),
        |_| Variance::Contravariant,
        arg_variances,
    );
    assert!(result.malbounded);
    assert_eq!(
        shows(&table, &result.defaults),
        vec!["A<Never>"],
        "the official returns `_lower[type.element]` unwrapped — no `?` union"
    );
}
