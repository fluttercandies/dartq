//! Instantiate-to-bound: the type-parameter defaults fixpoint.
//!
//! Direct port of `instantiateTypeFormalsToBounds`
//! (`pkg/analyzer/lib/src/dart/element/type_system.dart:762-838`), the
//! algorithm of dart-lang/sdk#27526 (comment 260021397): start each
//! parameter at its declared bound (unbounded reads `dynamic`,
//! type_system.dart:786), then repeatedly substitute defaulted parameters
//! into the remaining "partial" defaults until a fixed point; parameters
//! whose defaults still mention free parameters at the fixed point are
//! *malbounded* and get the Phi substitution (each uncompleted parameter
//! replaced by `dynamic`, each completed one by its default,
//! type_system.dart:816-838).
//!
//! The free-parameter test is `getFreeParameters`
//! (type_system.dart:567-610): a structure walk that respects function
//! types' binders — a parameter declared by a nested function type is
//! bound there, not free — and honors the optional candidate set.
//!
//! Iteration-order note: the official progress loop scans
//! `partials.keys` (Dart insertion order) and restarts on the first
//! completion. The fixed point is order-independent — a parameter
//! completes exactly when every free parameter of its partial default is
//! already ground, and completed substitutions on disjoint parameters
//! commute — so the loop below scans the parameters in declaration order
//! and restarts identically; only the *completion schedule* could differ,
//! which nothing observes. The malbounded set is likewise order-independent
//! (it is exactly the set of defaults whose free parameters form a cycle
//! among non-ground values).
//!
//! The interface entry point is `instantiateInterfaceToBounds`
//! (type_system.dart:719-731) with `_defaultTypeArguments`
//! (type_system.dart:1811-1820) reading each parameter's computed default.

use rustc_hash::FxHashSet;

use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::system::TypeSystem;
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedType};
use crate::variance::ParamFactsTable;

/// The result of [`instantiate_type_formals_to_bounds`]: the defaults in
/// the input parameter order, and whether the fixed point left any
/// parameter malbounded (the official `hasError[0] = true`,
/// type_system.dart:820-822).
#[derive(Clone, Debug)]
pub struct BoundsInstantiation {
    /// One default per input parameter, in input order.
    pub defaults: Vec<TypeId>,
    /// `true` when some defaults were completed only by the Phi
    /// substitution (type_system.dart:816-838) — the caller reports the
    /// malbounded-ness error; the partial types are still returned.
    pub malbounded: bool,
}

/// `instantiateTypeFormalsToBounds` (`type_system.dart:762-838`). See the
/// module docs for the algorithm and the iteration-order note.
#[must_use]
pub fn instantiate_type_formals_to_bounds(
    table: &TypeTable,
    facts: &ParamFactsTable,
    parameters: &[TypeParamId],
) -> BoundsInstantiation {
    let count = parameters.len();
    if count == 0 {
        return BoundsInstantiation {
            defaults: Vec::new(),
            malbounded: false,
        };
    }

    let dynamic = table.intern(UninternedType::Dynamic, false);
    let all: FxHashSet<TypeParamId> = parameters.iter().copied().collect();
    // "all ground" — completed defaults, keyed like the official maps.
    let mut defaults: Vec<(TypeParamId, TypeId)> = Vec::with_capacity(count);
    // "not ground" — in declaration (insertion) order.
    let mut partials: Vec<(TypeParamId, TypeId)> = Vec::with_capacity(count);

    for &parameter in parameters {
        let bound = facts.get(parameter).bound.unwrap_or(dynamic);
        partials.push((parameter, bound));
    }

    // The progress loop (type_system.dart:791-812): scan in order; a
    // partial with no free parameters is ground, one whose free parameters
    // are all defaulted gets substituted; either way it completes and the
    // scan restarts.
    let mut has_progress = true;
    while has_progress {
        has_progress = false;
        for index in 0..partials.len() {
            let (parameter, value) = partials[index];
            let Some(free) = get_free_parameters(table, &all, value) else {
                defaults.push((parameter, value));
                partials.remove(index);
                has_progress = true;
                break;
            };
            if free.iter().all(|p| defaults.iter().any(|(d, _)| d == p)) {
                let map: Vec<(TypeParamId, TypeId)> = defaults.clone();
                let ground = table.substitute(value, &map);
                defaults.push((parameter, ground));
                partials.remove(index);
                has_progress = true;
                break;
            }
        }
    }

    // Malbounded fallback (type_system.dart:814-838): build the Phi
    // substitution — every uncompleted parameter to `dynamic`, every
    // completed one to its default — with the domain COMPLETE before any
    // partial is rewritten (the official fills domain/range first, then
    // substitutes).
    let malbounded = !partials.is_empty();
    if malbounded {
        let mut map: Vec<(TypeParamId, TypeId)> = defaults.clone();
        for (parameter, _) in &partials {
            map.push((*parameter, dynamic));
        }
        for (parameter, value) in &partials {
            let default = table.substitute(*value, &map);
            defaults.push((*parameter, default));
        }
    }

    let mut ordered: Vec<Option<TypeId>> = vec![None; count];
    for (parameter, default) in defaults {
        let slot = parameters.iter().position(|&p| p == parameter).expect("parameter came from the input");
        ordered[slot] = Some(default);
    }
    BoundsInstantiation {
        defaults: ordered.into_iter().map(|d| d.expect("every parameter completed")).collect(),
        malbounded,
    }
}

/// `getFreeParameters` (`type_system.dart:567-610`): the free type
/// parameters of `root`, restricted to `candidates`, or `None` when there
/// are none. Function types bind their own declared parameters
/// (type_system.dart:588-600); the official alias arm
/// (type_system.dart:596-598) has no counterpart — dartq types carry no
/// alias kinds.
#[must_use]
pub fn get_free_parameters(
    table: &TypeTable,
    candidates: &FxHashSet<TypeParamId>,
    root: TypeId,
) -> Option<Vec<TypeParamId>> {
    let mut found: Vec<TypeParamId> = Vec::new();
    let mut visited: FxHashSet<TypeId> = FxHashSet::default();
    let mut bound: FxHashSet<TypeParamId> = FxHashSet::default();
    append_free_parameters(table, candidates, root, &mut visited, &mut bound, &mut found, 0);
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// The `appendParameters` closure (`type_system.dart:579-609`), iterative
/// over the type spine via the depth budget (crate recursion law) —
/// `depth > DEPTH_LIMIT` answers "no free parameters found past here",
/// which only deepens the malbounded answer instead of crashing.
fn append_free_parameters(
    table: &TypeTable,
    candidates: &FxHashSet<TypeParamId>,
    ty: TypeId,
    visited: &mut FxHashSet<TypeId>,
    bound: &mut FxHashSet<TypeParamId>,
    found: &mut Vec<TypeParamId>,
    depth: u32,
) {
    const DEPTH_LIMIT: u32 = crate::system::DEPTH_LIMIT;
    if depth > DEPTH_LIMIT || !visited.insert(ty) {
        return;
    }
    match table.get(ty).kind() {
        TypeKind::TypeParameter { parameter } => {
            if candidates.contains(&parameter) && !bound.contains(&parameter) {
                found.push(parameter);
            }
        }
        TypeKind::Interface { args, .. } => {
            for i in 0..args.len() {
                let arg = table.arg(args.offset() + i);
                append_free_parameters(table, candidates, arg, visited, bound, found, depth + 1);
            }
        }
        TypeKind::FutureOr { base } => {
            append_free_parameters(table, candidates, base, visited, bound, found, depth + 1);
        }
        TypeKind::Function { signature } => {
            let sig = table.signature(signature);
            let params = table.function_type_parameters(sig.type_parameters);
            // The binder: the signature's own parameters are bound inside
            // (type_system.dart:588-600); the official asserts they are
            // fresh, so shadowing cannot hide an outer parameter here.
            let newly_bound: Vec<TypeParamId> = params
                .iter()
                .copied()
                .filter(|p| !bound.contains(p))
                .collect();
            for p in &newly_bound {
                bound.insert(*p);
            }
            append_free_parameters(table, candidates, sig.return_type, visited, bound, found, depth + 1);
            for i in 0..sig.positional.len() {
                let p = table.arg(sig.positional.offset() + i);
                append_free_parameters(table, candidates, p, visited, bound, found, depth + 1);
            }
            for member in table.named_members(sig.named) {
                append_free_parameters(table, candidates, member.ty, visited, bound, found, depth + 1);
            }
            for p in &newly_bound {
                bound.remove(p);
            }
        }
        TypeKind::Record { shape } => {
            let shape = table.record_shape(shape);
            for i in 0..shape.positional.len() {
                let p = table.arg(shape.positional.offset() + i);
                append_free_parameters(table, candidates, p, visited, bound, found, depth + 1);
            }
            for member in table.named_members(shape.named) {
                append_free_parameters(table, candidates, member.ty, visited, bound, found, depth + 1);
            }
        }
        // Unit kinds carry no structure (the official `null`/fall-through).
        TypeKind::Never | TypeKind::Dynamic | TypeKind::Void | TypeKind::Invalid | TypeKind::Null => {}
    }
}

/// `instantiateInterfaceToBounds` (`type_system.dart:719-731`):
/// instantiates `decl` with its parameters' instantiate-to-bound defaults
/// — the official reads each `typeParameterImpl.defaultType!`
/// (`_defaultTypeArguments`, type_system.dart:1811-1820), which is exactly
/// the ITB default computed here.
#[must_use]
pub fn instantiate_interface_to_bounds(
    sys: &TypeSystem<'_>,
    facts: &ParamFactsTable,
    decl: TypeDeclId,
    nullable: bool,
) -> TypeId {
    let parameters = sys
        .hierarchy()
        .decl(decl)
        .expect("declared owner")
        .type_parameters
        .clone();
    let BoundsInstantiation { defaults, .. } =
        instantiate_type_formals_to_bounds(sys.table(), facts, &parameters);
    sys.table().intern(
        UninternedType::Interface {
            decl,
            args: defaults,
        },
        nullable,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::{CoreDecls, TypeHierarchy};
    use crate::variance::ParamFacts;

    const OBJECT: TypeDeclId = TypeDeclId::from_raw(1);
    const FUTURE: TypeDeclId = TypeDeclId::from_raw(2);
    const FUNCTION: TypeDeclId = TypeDeclId::from_raw(3);
    const INT: TypeDeclId = TypeDeclId::from_raw(4);
    const A: TypeDeclId = TypeDeclId::from_raw(5);

    fn fixture() -> TypeTable {
        TypeTable::new()
    }

    fn int_type(table: &TypeTable) -> TypeId {
        table.intern(
            UninternedType::Interface {
                decl: INT,
                args: Vec::new(),
            },
            false,
        )
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

    /// Unbounded parameters default to `dynamic`
    /// (type_system.dart:786).
    #[test]
    fn unbounded_parameter_defaults_to_dynamic() {
        let table = fixture();
        let t = TypeParamId::from_raw(10);
        let result = instantiate_type_formals_to_bounds(&table, &ParamFactsTable::new(), &[t]);
        assert!(!result.malbounded);
        assert_eq!(result.defaults.len(), 1);
        assert!(matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Dynamic
        ));
    }

    /// Chain `T1 extends T2 extends int`: the fixpoint grounds both to
    /// `int` (type_system.dart:791-812 — T1 completes only once T2 is
    /// defaulted).
    #[test]
    fn chained_bounds_substitute_transitively() {
        let table = fixture();
        let (t1, t2) = (TypeParamId::from_raw(11), TypeParamId::from_raw(12));
        let mut facts = ParamFactsTable::new();
        facts.set(
            t2,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(int_type(&table)),
            },
        );
        let t2_ty = table.intern(UninternedType::TypeParameter { parameter: t2 }, false);
        facts.set(
            t1,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(t2_ty),
            },
        );
        let result = instantiate_type_formals_to_bounds(&table, &facts, &[t1, t2]);
        assert!(!result.malbounded);
        assert!(matches!(table.get(result.defaults[0]).kind(), TypeKind::Interface { decl: INT, .. }));
        assert!(matches!(table.get(result.defaults[1]).kind(), TypeKind::Interface { decl: INT, .. }));
    }

    /// The recursive-bound edge: `class A<T extends A<T>>` — `T`'s default
    /// `A<T>` mentions itself, the fixpoint cannot ground it, and the Phi
    /// substitution yields `A<dynamic>` with `malbounded = true`
    /// (type_system.dart:816-838).
    #[test]
    fn recursive_bound_is_malbounded_with_dynamic_phi() {
        let table = fixture();
        let t = TypeParamId::from_raw(13);
        let mut facts = ParamFactsTable::new();
        facts.set(
            t,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(a_of(&table, {
                    table.intern(UninternedType::TypeParameter { parameter: t }, false)
                })),
            },
        );
        let result = instantiate_type_formals_to_bounds(&table, &facts, &[t]);
        assert!(result.malbounded);
        assert!(
            matches!(table.get(result.defaults[0]).kind(), TypeKind::Interface { decl: A, args } if args.len() == 1 && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Dynamic)),
            "the Phi default must be A<dynamic>"
        );
    }

    /// Two parameters bound through each other (`T extends U`,
    /// `U extends List<T>`-shaped cycle): both are malbounded and both get
    /// the full-Phi rewrite — completed parameters to their defaults,
    /// stuck ones to `dynamic`.
    #[test]
    fn cyclic_bounds_are_malbounded_for_every_stuck_parameter() {
        let table = fixture();
        let (t, u) = (TypeParamId::from_raw(14), TypeParamId::from_raw(15));
        let t_ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        let u_ty = table.intern(UninternedType::TypeParameter { parameter: u }, false);
        let mut facts = ParamFactsTable::new();
        facts.set(
            t,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(u_ty),
            },
        );
        facts.set(
            u,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(t_ty),
            },
        );
        let result = instantiate_type_formals_to_bounds(&table, &facts, &[t, u]);
        assert!(result.malbounded);
        assert!(matches!(table.get(result.defaults[0]).kind(), TypeKind::Dynamic));
        assert!(matches!(table.get(result.defaults[1]).kind(), TypeKind::Dynamic));
    }

    /// `getFreeParameters` honors function-type binders
    /// (type_system.dart:588-600): a parameter used inside a generic
    /// function type that redeclares the same id slot is bound there, not
    /// free.
    #[test]
    fn function_binders_shadow_their_own_parameters() {
        let table = fixture();
        let outer = TypeParamId::from_raw(16);
        let inner = TypeParamId::from_raw(17);
        let outer_ty = table.intern(UninternedType::TypeParameter { parameter: outer }, false);
        let inner_ty = table.intern(UninternedType::TypeParameter { parameter: inner }, false);
        // `void Function<T2>(T2)` — mentions only the bound T2.
        let gft = table
            .intern_function(
                crate::ty::UninternedFunction {
                    return_type: table.intern(UninternedType::Void, false),
                    type_parameters: vec![inner],
                    positional: vec![inner_ty],
                    named: Vec::new(),
                    required_positional: 1,
                },
                false,
            )
            .expect("signature is well-formed");
        let mut candidates = FxHashSet::default();
        candidates.insert(outer);
        candidates.insert(inner);
        // A type mentioning the outer parameter *and* the GFT: only the
        // outer occurrence is free.
        let ty = table.intern(
            UninternedType::Interface {
                decl: A,
                args: vec![outer_ty, gft],
            },
            false,
        );
        let free = get_free_parameters(&table, &candidates, ty).expect("outer is free");
        assert_eq!(free, vec![outer]);
    }

    /// `instantiateInterfaceToBounds` builds `A<…defaults…>` from the
    /// declaration's own parameters (type_system.dart:719-731).
    #[test]
    fn interface_to_bounds_uses_the_declaration_parameters() {
        let table = fixture();
        let t = TypeParamId::from_raw(18);
        let mut facts = ParamFactsTable::new();
        facts.set(
            t,
            ParamFacts {
                variance: crate::variance::Variance::Covariant,
                bound: Some(int_type(&table)),
            },
        );
        // The fixture `A` has no parameters; declare a parameterized
        // declaration `A2<T>` for the entry point.
        let a2 = TypeDeclId::from_raw(6);
        let hierarchy = TypeHierarchy::builder(CoreDecls {
            object: OBJECT,
            future: FUTURE,
            function: FUNCTION,
            record: None,
        })
        .decl(OBJECT, Vec::new())
        .decl(FUTURE, vec![TypeParamId::from_raw(1)])
        .decl(FUNCTION, Vec::new())
        .decl(INT, Vec::new())
        .decl(A, Vec::new())
        .decl(a2, vec![t])
        .build(&table)
        .expect("fixture hierarchy is valid");
        let sys = TypeSystem::new(&table, &hierarchy);
        let instantiated = instantiate_interface_to_bounds(&sys, &facts, a2, false);
        assert!(matches!(
            table.get(instantiated).kind(),
            TypeKind::Interface { decl: d, args } if d == a2 && args.len() == 1 && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Interface { decl: INT, .. })
        ));
    }
}
