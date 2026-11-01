//! Instantiate-to-bound: the type-parameter defaults computation.
//!
//! Direct port of the NDBD algorithm the oracle uses to compute
//! `TypeParameterElementImpl.defaultType` — `DefaultTypesBuilder.
//! _computeDefaultType` (`pkg/analyzer/lib/src/summary2/
//! default_types_builder.dart:128-187`), the same algorithm `calculateBounds`
//! spells out for the kernel side (`pkg/kernel/lib/src/bounds_checks.dart:
//! 213-295`), implementing the `-->m`-relation spec of
//! dart-lang/sdk#27526: start each parameter at its declared bound
//! (unbounded reads `dynamic`, default_types_builder.dart:136-140), build
//! the reference graph among the parameters, rewrite every strongly
//! connected component's own bounds through the substitution that maps the
//! component's members to `dynamic` (default_types_builder.dart:144-162),
//! then substitute each parameter's resolved bound through every bound in
//! declaration order so no parameter leaf survives
//! (default_types_builder.dart:164-180).
//!
//! The component rewrite is *scoped to the component*: a parameter outside
//! a cycle whose bound mentions a cycle member keeps its parameter leaf
//! through the SCC pass and completes in the grounding sweep with the
//! member's substituted default — `X extends A<X>, Y extends X` yields
//! `Y = A<dynamic>`, not `dynamic`. That is the difference from the
//! fixpoint-plus-Phi reading of `instantiateTypeFormalsToBounds`
//! (`type_system.dart:735-811`), whose fallback maps *every* uncompleted
//! parameter to `dynamic` (type_system.dart:793-804) and so loses the
//! dependent's bound entirely.
//!
//! The free-parameter test behind the `-->m` edges is `getFreeParameters`
//! (type_system.dart:567-610): a structure walk that respects function
//! types' binders — a parameter declared by a nested function type is
//! bound there, not free — and honors the optional candidate set.
//!
//! The interface entry point is `instantiateInterfaceToBounds`
//! (type_system.dart:687-698) with `_defaultTypeArguments`
//! (type_system.dart:1822-1829) reading each parameter's computed
//! `defaultType` — exactly the value this port produces.
//!
//! [`instantiate_type_formals_to_bounds_with`] takes the bound lookup as a
//! closure because a declaration's bounds do not always live in one facts
//! table: parameters are keyed by element upstream, while dartq's element
//! model spells them name-keyed in shared snapshots, so a consumer holding
//! the declaration's own bound list (a type alias's shape, for one) must ask
//! the algorithm directly instead of seeding a shared table with a value
//! another same-spelled parameter would then read.
//!
//! Alias contract: the algorithm never sees an alias. It is handed the
//! bound as the element model stores it — already expanded, `X extends F<X>`
//! over `typedef F<T> = List<T>` arriving as `List<X>` (upstream keeps the
//! alias only as a decoration on the expanded type; the bound read is
//! `typeParameter.bound`). [`crate::TypeKind`] carries no alias kind,
//! so a bound provider that cannot expand a typedef head must report the
//! parameter unbounded (`None`) and accept the `dynamic` default; feeding the
//! alias's own parameter leaves through instead would ground a foreign
//! parameter the algorithm has no candidates for.

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
    /// `true` when some parameter's bound reaches a cycle — the official
    /// `partials.isNotEmpty → hasError[0] = true` (type_system.dart:
    /// 785-788): cycle members and their dependents were completed only by
    /// the component rewrite and grounding sweep. The caller reports the
    /// malbounded-ness error; the substituted types are still returned.
    pub malbounded: bool,
}

/// `instantiateTypeFormalsToBounds` over the facts snapshot — the bound of
/// each parameter is its declared bound, or the unbounded `dynamic` face
/// (default_types_builder.dart:136-140). See the module docs for the
/// algorithm.
#[must_use]
pub fn instantiate_type_formals_to_bounds(
    table: &TypeTable,
    facts: &ParamFactsTable,
    parameters: &[TypeParamId],
) -> BoundsInstantiation {
    instantiate_type_formals_to_bounds_with(table, parameters, |parameter| {
        facts.get(parameter).bound
    })
}

/// The same instantiation with the bound lookup supplied by the caller —
/// `None` for an unbounded parameter (the implicit `dynamic`,
/// default_types_builder.dart:136-140). The algorithm is the oracle's NDBD
/// shape that computes `TypeParameterElementImpl.defaultType` — the value
/// `_defaultTypeArguments` reads for every raw-type instantiation
/// (`_computeDefaultType`, default_types_builder.dart:128-187; the kernel
/// twin is `calculateBounds`, bounds_checks.dart:213-295): materialize the
/// `-->m` relation, rewrite each strongly connected component's own bounds
/// through the component's dynamic substitution, then ground every bound by
/// substituting each parameter's resolved default in declaration order. A
/// parameter *outside* a cycle whose bound mentions a cycle member therefore
/// completes with the member's substituted default (`X extends A<X>`,
/// `Y extends X` yields `Y = A<dynamic>`) — not with `dynamic`, the
/// all-or-nothing Phi answer the fixpoint port used to give
/// (type_system.dart:793-804 maps every uncompleted parameter to
/// `dynamic`); see the module docs.
///
/// # Panics
///
/// Panics on the one ordering invariant of the `-->m` build — a free
/// parameter of a declared bound always came from the input list (the
/// candidate set is exactly the input), pinned by an `expect`.
#[must_use]
pub fn instantiate_type_formals_to_bounds_with(
    table: &TypeTable,
    parameters: &[TypeParamId],
    bound_of: impl Fn(TypeParamId) -> Option<TypeId>,
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
    let mut index_of: rustc_hash::FxHashMap<TypeParamId, usize> =
        rustc_hash::FxHashMap::default();
    for (i, &parameter) in parameters.iter().enumerate() {
        index_of.insert(parameter, i);
    }

    // `bounds[i]` is parameter `i`'s declared bound, or `dynamic` when
    // unbounded (default_types_builder.dart:136-140).
    let mut bounds: Vec<TypeId> = parameters
        .iter()
        .map(|&p| bound_of(p).unwrap_or(dynamic))
        .collect();

    // The `-->m` relation (default_types_builder.dart:375-428 —
    // `_TypeParametersGraph._collectReferencesFrom`): `dependents[i]` lists
    // the indices of the parameters whose bounds reference `parameters[i]`.
    // [`get_free_parameters`] already restricts occurrences to the input
    // set and honors nested function-type binders, the counterpart of the
    // official `TypeParameterType` arm restricted by `_parameterToIndex`;
    // a set dedupes repeated occurrences (`X` under two arg positions).
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (j, &bound) in bounds.iter().enumerate() {
        let Some(free) = get_free_parameters(table, &all, bound) else {
            continue;
        };
        let mut seen: FxHashSet<TypeParamId> = FxHashSet::default();
        for tp in free {
            if seen.insert(tp) {
                let i = index_of
                    .get(&tp)
                    .copied()
                    .expect("free parameter is a candidate");
                dependents[i].push(j);
            }
        }
    }

    // The component pass (default_types_builder.dart:142-162): each
    // strongly connected component's own bounds are rewritten through the
    // substitution mapping the component's members to `dynamic`. The
    // rewrite is scoped to the component — a peer bound that merely
    // mentions a member keeps its parameter leaf for the grounding sweep.
    let components = strong_components(&dependents);
    let mut cyclic = vec![false; count];
    for component in &components {
        let is_cycle = component.len() > 1
            || component
                .iter()
                .any(|&i| dependents[i].contains(&i));
        if is_cycle {
            for &i in component {
                cyclic[i] = true;
            }
        }
        let map: Vec<(TypeParamId, TypeId)> = component
            .iter()
            .map(|&i| (parameters[i], dynamic))
            .collect();
        for &i in component {
            bounds[i] = table.substitute(bounds[i], &map);
        }
    }

    // `malbounded` is the official `partials.isNotEmpty → hasError[0]`
    // (type_system.dart:785-788): a parameter is malbounded when it sits in
    // a cycle or depends on one — the dependents-walk forward from cyclic
    // members marks exactly the set the old fixpoint left ungrounded.
    let malbounded = cyclic.iter().any(|&c| c);

    // The grounding sweep (default_types_builder.dart:164-180): in
    // declaration order, substitute each parameter's resolved bound through
    // every bound. Every parameter leaf is eliminated exactly once, so
    // dependents of a cycle complete with the member's substituted default
    // transitively (`X extends A<X>`, `Y extends X`, `Z extends Y` grounds
    // all three to `A<dynamic>`).
    for i in 0..count {
        let map = [(parameters[i], bounds[i])];
        for bound in bounds.iter_mut() {
            *bound = table.substitute(*bound, &map);
        }
    }

    BoundsInstantiation {
        defaults: bounds,
        malbounded,
    }
}

/// `computeStrongComponents` (`pkg/kernel/lib/util/graph.dart:35-93`) — the
/// Dijkstra path-based strong-component algorithm the oracle's
/// `DefaultTypesBuilder` consumes (default_types_builder.dart:142-143).
/// `dependents[i]` are the out-neighbors of `i` (upstream `_edges`); the
/// component order is the official emission order, which the component pass
/// is indifferent to — each component's substitution is scoped to its own
/// members' bounds.
fn strong_components(dependents: &[Vec<usize>]) -> Vec<Vec<usize>> {
    fn visit(
        vertex: usize,
        dependents: &[Vec<usize>],
        count: &mut usize,
        preorder: &mut [usize],
        unassigned: &mut Vec<usize>,
        candidates: &mut Vec<usize>,
        assigned: &mut [bool],
        result: &mut Vec<Vec<usize>>,
    ) {
        preorder[vertex] = *count;
        *count += 1;
        unassigned.push(vertex);
        candidates.push(vertex);
        for &neighbor in &dependents[vertex] {
            if preorder[neighbor] == usize::MAX {
                visit(
                    neighbor, dependents, count, preorder, unassigned, candidates, assigned,
                    result,
                );
            } else if !assigned[neighbor] {
                while preorder[*candidates.last().expect("path is nonempty")]
                    > preorder[neighbor]
                {
                    candidates.pop();
                }
            }
        }
        if *candidates.last().expect("path is nonempty") == vertex {
            let mut component = Vec::new();
            while let Some(top) = unassigned.pop() {
                component.push(top);
                assigned[top] = true;
                if top == vertex {
                    break;
                }
            }
            result.push(component);
            candidates.pop();
        }
    }

    let mut result = Vec::new();
    let mut count = 0usize;
    let mut preorder = vec![usize::MAX; dependents.len()];
    let mut unassigned = Vec::new();
    let mut candidates = Vec::new();
    let mut assigned = vec![false; dependents.len()];
    for vertex in 0..dependents.len() {
        if preorder[vertex] == usize::MAX {
            visit(
                vertex,
                dependents,
                &mut count,
                &mut preorder,
                &mut unassigned,
                &mut candidates,
                &mut assigned,
                &mut result,
            );
        }
    }
    result
}

/// `getFreeParameters` (`type_system.dart:567-610`): the free type
/// parameters of `root`, restricted to `candidates`, or `None` when there
/// are none. Function types bind their own declared parameters
/// (type_system.dart:588-600); the official alias arm
/// (type_system.dart:596-598) has no counterpart — dartq types carry no
/// alias kinds.
#[expect(
    clippy::implicit_hasher,
    reason = "the workspace pins Fx hashing (rustc-hash); generalizing over \
              `BuildHasher` would add a parameter no caller can use differently"
)]
#[must_use]
pub fn get_free_parameters(
    table: &TypeTable,
    candidates: &FxHashSet<TypeParamId>,
    root: TypeId,
) -> Option<Vec<TypeParamId>> {
    let mut found: Vec<TypeParamId> = Vec::new();
    let mut visited: FxHashSet<TypeId> = FxHashSet::default();
    let mut bound: FxHashSet<TypeParamId> = FxHashSet::default();
    append_free_parameters(
        table,
        candidates,
        root,
        &mut visited,
        &mut bound,
        &mut found,
        0,
    );
    if found.is_empty() { None } else { Some(found) }
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
            append_free_parameters(
                table,
                candidates,
                sig.return_type,
                visited,
                bound,
                found,
                depth + 1,
            );
            for i in 0..sig.positional.len() {
                let p = table.arg(sig.positional.offset() + i);
                append_free_parameters(table, candidates, p, visited, bound, found, depth + 1);
            }
            for member in table.named_members(sig.named) {
                append_free_parameters(
                    table,
                    candidates,
                    member.ty,
                    visited,
                    bound,
                    found,
                    depth + 1,
                );
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
                append_free_parameters(
                    table,
                    candidates,
                    member.ty,
                    visited,
                    bound,
                    found,
                    depth + 1,
                );
            }
        }
        // Unit kinds carry no structure (the official `null`/fall-through).
        TypeKind::Never
        | TypeKind::Dynamic
        | TypeKind::Void
        | TypeKind::Invalid
        | TypeKind::Null => {}
    }
}

/// `instantiateInterfaceToBounds` (`type_system.dart:719-731`):
/// instantiates `decl` with its parameters' instantiate-to-bound defaults
/// — the official reads each `typeParameterImpl.defaultType!`
/// (`_defaultTypeArguments`, type_system.dart:1811-1820), which is exactly
/// the ITB default computed here.
///
/// # Panics
///
/// Panics if `decl` is missing from the hierarchy — impossible for a
/// well-formed interface type; the `expect` pins that invariant.
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
    /// (`type_system.dart:786`).
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
        assert!(matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
        assert!(matches!(
            table.get(result.defaults[1]).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
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
        assert!(matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Dynamic
        ));
        assert!(matches!(
            table.get(result.defaults[1]).kind(),
            TypeKind::Dynamic
        ));
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

    /// The explicit-bound entry, one-parameter cycle: `X extends A<X>`
    /// never grounds, so the SCC step's Phi substitution rewrites the
    /// occurrence of `X` in its own bound — the default is the substituted
    /// bound `A<dynamic>`, reached without recursing on `X`.
    #[test]
    fn explicit_self_bound_defaults_to_the_substituted_bound() {
        let table = fixture();
        let x = TypeParamId::from_raw(19);
        let x_ty = table.intern(UninternedType::TypeParameter { parameter: x }, false);
        let bound = a_of(&table, x_ty);
        let result = instantiate_type_formals_to_bounds_with(&table, &[x], |p| {
            (p == x).then_some(bound)
        });
        assert!(result.malbounded);
        assert!(
            matches!(table.get(result.defaults[0]).kind(), TypeKind::Interface { decl: A, args } if args.len() == 1 && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Dynamic)),
            "the default is the substituted bound `A<dynamic>`"
        );
    }

    /// The same cycle under `FutureOr` — the head survives
    /// (`FutureOr<dynamic>`); the answer is never the bare `dynamic` a
    /// declined self-bound leaves behind.
    #[test]
    fn explicit_future_or_self_bound_keeps_its_head() {
        let table = fixture();
        let x = TypeParamId::from_raw(20);
        let x_ty = table.intern(UninternedType::TypeParameter { parameter: x }, false);
        let bound = table.intern(UninternedType::FutureOr { base: x_ty }, false);
        let result = instantiate_type_formals_to_bounds_with(&table, &[x], |p| {
            (p == x).then_some(bound)
        });
        assert!(result.malbounded);
        assert!(
            matches!(table.get(result.defaults[0]).kind(), TypeKind::FutureOr { base } if matches!(table.get(base).kind(), TypeKind::Dynamic)),
            "the default is `FutureOr<dynamic>`"
        );
    }

    /// `T extends Comparable<T>` — the F-bound shape the co19 super-bounds
    /// family spells; the head and its argument position survive.
    #[test]
    fn explicit_comparable_self_bound_keeps_its_head() {
        let table = fixture();
        let comparable = TypeDeclId::from_raw(7);
        let t = TypeParamId::from_raw(21);
        let t_ty = table.intern(UninternedType::TypeParameter { parameter: t }, false);
        let bound = table.intern(
            UninternedType::Interface {
                decl: comparable,
                args: vec![t_ty],
            },
            false,
        );
        let result = instantiate_type_formals_to_bounds_with(&table, &[t], |p| {
            (p == t).then_some(bound)
        });
        assert!(result.malbounded);
        assert!(
            matches!(table.get(result.defaults[0]).kind(), TypeKind::Interface { decl, args } if decl == comparable && args.len() == 1 && matches!(table.get(table.arg(args.offset())).kind(), TypeKind::Dynamic)),
            "the default is `Comparable<dynamic>`"
        );
    }

    /// The peer-grounded regression for the explicit-bound entry: `X
    /// extends Y`, `Y extends int` grounds both to `int` and reports no
    /// malboundedness — the SCC step must not disturb the acyclic path.
    #[test]
    fn explicit_peer_bounds_ground_without_malboundedness() {
        let table = fixture();
        let (x, y) = (TypeParamId::from_raw(22), TypeParamId::from_raw(23));
        let y_ty = table.intern(UninternedType::TypeParameter { parameter: y }, false);
        let int = int_type(&table);
        let result = instantiate_type_formals_to_bounds_with(&table, &[x, y], |p| {
            if p == x {
                Some(y_ty)
            } else if p == y {
                Some(int)
            } else {
                None
            }
        });
        assert!(!result.malbounded);
        assert!(matches!(
            table.get(result.defaults[0]).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
        assert!(matches!(
            table.get(result.defaults[1]).kind(),
            TypeKind::Interface { decl: INT, .. }
        ));
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

// The #3311 pins (a formal's bound written through a `typedef` alias) live
// beside the algorithm they fix, registered here so the module resolves
// against this file's own imports.
#[cfg(test)]
#[path = "ticket_3311_tests.rs"]
mod ticket_3311_tests;
