//! The interface hierarchy snapshot.
//!
//! The type system never reaches back into the semantic layer: class
//! declarations, their type parameters, and their direct supertypes are
//! injected as an immutable snapshot. Supertype edges are stored as
//! *instantiated interface types* whose arguments may reference the owning
//! declaration's own type parameters (e.g. `class MyList<T> implements
//! Iterable<T>`), which is exactly what the analyzer's
//! `allSupertypes`-plus-substitution walk consumes (`subtype.dart:211-221`).

use std::collections::BTreeSet;

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use crate::error::TypeError;
use crate::ids::{TypeDeclId, TypeId, TypeParamId};
use crate::table::TypeTable;
use crate::ty::{TypeKind, UninternedType};

/// The core declarations whose identity the subtype/LUB rules special-case.
///
/// `object`, `future`, and `function` must be declared in the snapshot:
/// `Object` with no type parameters, `Future<T>` with one, and bare
/// `Function` with none (`subtype.dart:206-209`, `subtype.dart:502-510`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CoreDecls {
    /// `Object` (`isDartCoreObject`): the Right Object rule
    /// (`subtype.dart:304-346`).
    pub object: TypeDeclId,
    /// `Future` (`isDartAsyncFuture`): synthesized for the `FutureOr` rules
    /// (`subtype.dart:372-385`, `418-449`).
    pub future: TypeDeclId,
    /// Bare `Function` (`isDartCoreFunction`): every function type is a
    /// subtype of it (`subtype.dart:502-510`); class types never subtype it
    /// (`subtype.dart:206-209`).
    pub function: TypeDeclId,
    /// The `Record` class (`isDartCoreRecord`), when the snapshot declares
    /// it: `UP((...), Record) = Record` (`least_upper_bound.dart:637-644`).
    /// `None` leaves every record pair on the record-vs-other fallback
    /// (`least_upper_bound.dart:652-659`).
    pub record: Option<TypeDeclId>,
}

/// One declared class: its type parameters and its direct supertypes
/// (excluding the auto-added `Object` edge is *not* done — the snapshot is
/// final, see [`TypeHierarchyBuilder::build`]).
#[derive(Clone, Debug)]
pub struct DeclInfo {
    /// The declaration's own type parameters, in declaration order.
    pub type_parameters: Vec<TypeParamId>,
    /// Direct supertypes, instantiated w.r.t. `type_parameters`.
    pub supertypes: Vec<TypeId>,
}

/// Pre-interned core types shared by the subtype/LUB algorithms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sentinels {
    pub(crate) object_none: TypeId,
    pub(crate) object_question: TypeId,
    pub(crate) null: TypeId,
}

/// Immutable hierarchy snapshot consumed by [`crate::TypeSystem`].
#[derive(Clone, Debug)]
pub struct TypeHierarchy {
    core: CoreDecls,
    decls: FxHashMap<TypeDeclId, DeclInfo>,
    sentinels: Sentinels,
    /// Longest supertype-chain depth to `Object` per declaration, counted
    /// over *all* direct supertype edges exactly like the oracle's
    /// superinterface walk, precomputed at build time for the LUB
    /// max-unique-depth rule (`least_upper_bound.dart:327-357`). Exact —
    /// computed topologically with no depth cap — so the tie-break matches
    /// the oracle's uncapped `computeLongestInheritancePathToObject` on
    /// chains of any depth.
    depths: FxHashMap<TypeDeclId, u32>,
    /// Declarations marked `sealed` (`ClassElementImpl.isSealed`): their
    /// subclass set is closed, which is the exhaustiveness checker's
    /// `isSealed` fact (static_type.dart:44-66). Enums are sealed
    /// independently of this set (their constants close the type).
    sealed: FxHashSet<TypeDeclId>,
    /// Enum declarations → their constant count. The exhaustiveness
    /// checker's `EnumStaticType` splits an enum over its constants
    /// (types/enum.dart:69-73).
    enum_constants: FxHashMap<TypeDeclId, u16>,
    /// Direct subclasses — the reversed final edge list — of every sealed
    /// or enum declaration with at least one edge, in edge-insertion order
    /// (the oracle's `directSubclassesOfSealed` covers extends, implements,
    /// and mixin faces alike, `AnalyzerSealedClassOperations`
    /// `.directSubtypesOfSealed`).
    subclasses: FxHashMap<TypeDeclId, Vec<TypeDeclId>>,
}

impl TypeHierarchy {
    /// Starts building a snapshot.
    #[must_use]
    pub fn builder(core: CoreDecls) -> TypeHierarchyBuilder {
        TypeHierarchyBuilder {
            core,
            decls: Vec::new(),
            edges: Vec::new(),
            sealed: Vec::new(),
            enums: Vec::new(),
        }
    }

    /// The core declaration handles.
    #[must_use]
    pub const fn core(&self) -> CoreDecls {
        self.core
    }

    /// Info for a declared class, if the snapshot declares it.
    #[must_use]
    pub fn decl(&self, decl: TypeDeclId) -> Option<&DeclInfo> {
        self.decls.get(&decl)
    }

    /// Direct supertypes of a declared class.
    ///
    /// # Panics
    /// Panics when `decl` is not declared — callers only walk declarations
    /// that came from the snapshot.
    #[must_use]
    pub fn supertypes(&self, decl: TypeDeclId) -> &[TypeId] {
        &self.decls[&decl].supertypes
    }

    /// The precomputed inheritance depth of a declared class.
    ///
    /// # Panics
    /// Panics when `decl` is not declared — callers only pass declarations
    /// that came from the snapshot.
    pub(crate) fn depth(&self, decl: TypeDeclId) -> u32 {
        self.depths[&decl]
    }

    pub(crate) const fn sentinels(&self) -> &Sentinels {
        &self.sentinels
    }

    /// Whether `decl` carries the `sealed` modifier.
    #[must_use]
    pub fn is_sealed(&self, decl: TypeDeclId) -> bool {
        self.sealed.contains(&decl)
    }

    /// The constant count of an enum declaration, or `None` when `decl` is
    /// not an enum.
    #[must_use]
    pub fn enum_constant_count(&self, decl: TypeDeclId) -> Option<u16> {
        self.enum_constants.get(&decl).copied()
    }

    /// Direct subclasses (extends/implements/mixin faces) of a sealed class,
    /// in declaration order, or `None` when `decl` is neither sealed nor an
    /// enum with edges.
    #[must_use]
    pub fn direct_subclasses(&self, decl: TypeDeclId) -> Option<&[TypeDeclId]> {
        self.subclasses.get(&decl).map(Vec::as_slice)
    }
}

/// A supertype edge [`TypeHierarchyBuilder::build_lenient`] left out: the
/// declaration that wrote it, the supertype it named, and why the edge is
/// not part of the snapshot. The owner's supertype closure is incomplete
/// from this edge on, which is what a consumer needs to know to keep an
/// over-report away from it (a subtype question through the missing edge
/// answers `false`, not "unknown").
#[derive(Clone, Debug)]
pub struct DroppedEdge {
    /// The declaration whose edge was dropped.
    pub owner: TypeDeclId,
    /// The supertype the edge named.
    pub supertype: TypeId,
    /// The validation failure.
    pub error: TypeError,
}

/// Builder for [`TypeHierarchy`]; validates every edge before the snapshot
/// becomes observable.
#[derive(Clone, Debug)]
pub struct TypeHierarchyBuilder {
    core: CoreDecls,
    decls: Vec<(TypeDeclId, Vec<TypeParamId>)>,
    edges: Vec<(TypeDeclId, TypeId)>,
    sealed: Vec<TypeDeclId>,
    enums: Vec<(TypeDeclId, u16)>,
}

impl TypeHierarchyBuilder {
    /// Declares a class with its type parameters.
    #[must_use]
    pub fn decl(mut self, decl: TypeDeclId, type_parameters: Vec<TypeParamId>) -> Self {
        self.decls.push((decl, type_parameters));
        self
    }

    /// Adds a direct supertype edge: `supertype` must be an interface type
    /// whose arguments reference only `decl`'s own type parameters.
    #[must_use]
    pub fn supertype(mut self, decl: TypeDeclId, supertype: TypeId) -> Self {
        self.edges.push((decl, supertype));
        self
    }

    /// Marks a class declaration `sealed` (`ClassElementImpl.isSealed`).
    #[must_use]
    pub fn sealed(mut self, decl: TypeDeclId) -> Self {
        self.sealed.push(decl);
        self
    }

    /// Registers an enum declaration with its constant count (the
    /// exhaustiveness checker splits the enum over its constants).
    #[must_use]
    pub fn enumeration(mut self, decl: TypeDeclId, constants: u16) -> Self {
        self.enums.push((decl, constants));
        self
    }

    /// Validates the graph and freezes the snapshot.
    ///
    /// Every declared class except `Object` implicitly receives an
    /// `Object` supertype edge (Dart's implicit inheritance), guaranteeing
    /// that LUB supertype-set intersections are never empty.
    ///
    /// # Errors
    /// [`TypeError::DuplicateDecl`], [`TypeError::UndeclaredDecl`],
    /// [`TypeError::SupertypeNotInterface`], [`TypeError::ObjectSupertype`],
    /// [`TypeError::NullableSupertype`], [`TypeError::ArityMismatch`],
    /// [`TypeError::ForeignTypeParameter`], [`TypeError::CyclicHierarchy`].
    pub fn build(self, table: &TypeTable) -> Result<TypeHierarchy, TypeError> {
        self.assemble(table, false).map(|(hierarchy, _)| hierarchy)
    }

    /// Validates the graph edge by edge and freezes the snapshot, keeping
    /// every declaration's well-formed edges: a supertype edge that fails
    /// validation is left out, and a cycle loses the back edge that closes
    /// it, while the owner keeps its other edges and the implicit `Object`
    /// edge. A graph assembled from source can carry both (`class A extends
    /// B {} class B extends A {}`, an `extends List<int, int>`), and the
    /// official analyzer keeps the rest of the hierarchy usable around them
    /// — one malformed edge must not blank every other declaration's
    /// supertype face. The dropped edges' errors come back in encounter
    /// order.
    ///
    /// # Errors
    /// Only the declaration-level contract violations the builder's caller
    /// controls: [`TypeError::DuplicateDecl`] and the core declarations'
    /// [`TypeError::UndeclaredDecl`] / [`TypeError::ArityMismatch`].
    pub fn build_lenient(
        self,
        table: &TypeTable,
    ) -> Result<(TypeHierarchy, Vec<DroppedEdge>), TypeError> {
        self.assemble(table, true)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one long pass mirrors the official hierarchy build; splitting would scatter \
                  the oracle mapping across helpers"
    )]
    fn assemble(
        self,
        table: &TypeTable,
        lenient: bool,
    ) -> Result<(TypeHierarchy, Vec<DroppedEdge>), TypeError> {
        let core = self.core;
        let sealed = self.sealed;
        let mut decls: FxHashMap<TypeDeclId, Vec<TypeParamId>> = FxHashMap::default();
        for (decl, params) in self.decls {
            if decls.insert(decl, params).is_some() {
                return Err(TypeError::DuplicateDecl(decl));
            }
        }
        for decl in &sealed {
            if !decls.contains_key(decl) {
                return Err(TypeError::UndeclaredDecl(*decl));
            }
        }
        let mut enum_constants: FxHashMap<TypeDeclId, u16> = FxHashMap::default();
        for (decl, constants) in self.enums {
            if !decls.contains_key(&decl) {
                return Err(TypeError::UndeclaredDecl(decl));
            }
            if enum_constants.insert(decl, constants).is_some() {
                return Err(TypeError::DuplicateDecl(decl));
            }
        }
        for (decl, arity) in [(core.object, 0), (core.future, 1), (core.function, 0)] {
            match decls.get(&decl) {
                None => return Err(TypeError::UndeclaredDecl(decl)),
                Some(params) if params.len() != arity => {
                    return Err(TypeError::ArityMismatch {
                        decl,
                        expected: arity,
                        actual: params.len(),
                    });
                }
                Some(_) => {}
            }
        }

        let mut dropped: Vec<DroppedEdge> = Vec::new();
        let mut edges: Vec<(TypeDeclId, TypeDeclId, TypeId)> = Vec::with_capacity(self.edges.len());
        for (owner, supertype) in self.edges {
            match validate_edge(table, &decls, core, owner, supertype) {
                Ok(target) => edges.push((owner, target, supertype)),
                Err(error) if lenient => dropped.push(DroppedEdge {
                    owner,
                    supertype,
                    error,
                }),
                Err(error) => return Err(error),
            }
        }

        // One adjacency index — `owner → positions in `edges`` — built once
        // and extended as implicit edges land; every pass below reads it
        // instead of rescanning the edge list, O(V+E) in total. Positions
        // are appended in edge-insertion order, so each consumer observes
        // exactly the per-owner order the former full scans produced.
        let mut adjacency: FxHashMap<TypeDeclId, Vec<usize>> =
            FxHashMap::with_capacity_and_hasher(decls.len(), FxBuildHasher);
        for (index, &(owner, _, _)) in edges.iter().enumerate() {
            adjacency.entry(owner).or_default().push(index);
        }

        // Implicit `Object` inheritance for every class except `Object`.
        let object_none = table.intern(
            UninternedType::Interface {
                decl: core.object,
                args: Vec::new(),
            },
            false,
        );
        for &decl in decls.keys() {
            if decl == core.object {
                continue;
            }
            let has_object_edge = adjacency
                .get(&decl)
                .is_some_and(|slots| slots.iter().any(|&index| edges[index].1 == core.object));
            if !has_object_edge {
                adjacency.entry(decl).or_default().push(edges.len());
                edges.push((decl, core.object, object_none));
            }
        }

        debug_assert!(
            edges.iter().all(|&(owner, _, supertype)| {
                owner != core.object && !table.get(supertype).is_nullable()
            }),
            "validated edges must keep `Object` uniquely at depth 1 for the LUB rule"
        );

        // A back edge closes a cycle: the strict build reports the first,
        // the lenient build drops every one from its owner's adjacency so
        // the depth sweep and the supertype faces below see a DAG. The
        // edge list keeps its positions (adjacency slots index into it);
        // the dropped slots are skipped wherever the list is read directly.
        let back = back_edges(&decls, &edges, &adjacency);
        let mut dropped_slots: FxHashSet<usize> = FxHashSet::default();
        if let Some((_, cycle)) = back.first() {
            if !lenient {
                return Err(TypeError::CyclicHierarchy(cycle.clone()));
            }
            dropped_slots.extend(back.iter().map(|&(slot, _)| slot));
            for slots in adjacency.values_mut() {
                slots.retain(|slot| !dropped_slots.contains(slot));
            }
            dropped.extend(back.into_iter().map(|(slot, cycle)| DroppedEdge {
                owner: edges[slot].0,
                supertype: edges[slot].2,
                error: TypeError::CyclicHierarchy(cycle),
            }));
        }

        let mut infos: FxHashMap<TypeDeclId, DeclInfo> = FxHashMap::default();
        for (decl, params) in &decls {
            let supertypes = adjacency
                .get(decl)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|&index| edges[index].2)
                .collect();
            infos.insert(
                *decl,
                DeclInfo {
                    type_parameters: params.clone(),
                    supertypes,
                },
            );
        }

        // Precompute inheritance depths (LUB max-unique-depth rule) exactly:
        // the graph is acyclic (validated above), so one iterative sweep in
        // topological order suffices — no recursion budget that could
        // memoize a truncated value for a node first reached deep in a
        // chain.
        let depths = class_depths(&decls, &edges, &adjacency);

        let object_question = table.intern(
            UninternedType::Interface {
                decl: core.object,
                args: Vec::new(),
            },
            true,
        );
        let null = table.intern(UninternedType::Null, false);
        // Direct subclasses: the reversed final edge list, kept for the
        // declarations whose subclass set is closed (sealed classes, enums)
        // — the exhaustiveness checker's expansion roots. Insertion order is
        // declaration order of the edges, which is the witness-choice order.
        let mut subclasses: FxHashMap<TypeDeclId, Vec<TypeDeclId>> = FxHashMap::default();
        for (slot, &(owner, target, _)) in edges.iter().enumerate() {
            if dropped_slots.contains(&slot) {
                continue;
            }
            if sealed.contains(&target) || enum_constants.contains_key(&target) {
                subclasses.entry(target).or_default().push(owner);
            }
        }
        Ok((
            TypeHierarchy {
                core,
                decls: infos,
                sentinels: Sentinels {
                    object_none,
                    object_question,
                    null,
                },
                depths,
                sealed: sealed.into_iter().collect(),
                enum_constants,
                subclasses,
            },
            dropped,
        ))
    }
}

/// Longest supertype-chain depth to `Object` over all direct supertype
/// edges (`computeLongestInheritancePathToObject`,
/// least_upper_bound.dart:204-323): every edge targets a validated interface
/// declaration, so the walk covers the oracle's superinterface walk
/// without kind filtering. Depths are computed exactly by a Kahn sweep
/// over the reversed edges: a declaration is resolved only after all of its
/// supertypes are, so `depth = 1 + max(target depths)` needs no recursion —
/// one O(V+E) pass, unconditionally stack-safe, with no depth budget that
/// could memoize a truncated value (the depths feed the LUB tie-break and
/// must match the oracle's uncapped walk; only the type-operation recursion
/// guards keep [`DEPTH_LIMIT`](crate::system::DEPTH_LIMIT)).
fn class_depths(
    decls: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    edges: &[(TypeDeclId, TypeDeclId, TypeId)],
    adjacency: &FxHashMap<TypeDeclId, Vec<usize>>,
) -> FxHashMap<TypeDeclId, u32> {
    // Unresolved supertypes per owner, and the reverse adjacency that
    // releases an owner each time one of its supertypes resolves. Both
    // read the adjacency (the live edge set) — a slot the lenient build
    // dropped must neither count as pending nor release its owner.
    let mut waiting: FxHashMap<TypeDeclId, usize> = decls
        .keys()
        .map(|&decl| (decl, adjacency.get(&decl).map_or(0, Vec::len)))
        .collect();
    let mut owners_of: FxHashMap<TypeDeclId, Vec<TypeDeclId>> = FxHashMap::default();
    for (&owner, slots) in adjacency {
        for &slot in slots {
            owners_of.entry(edges[slot].1).or_default().push(owner);
        }
    }

    let mut depths: FxHashMap<TypeDeclId, u32> =
        FxHashMap::with_capacity_and_hasher(decls.len(), FxBuildHasher);
    // Seeds: declarations with no supertypes — only `Object`, since every
    // other class received the implicit `Object` edge above.
    let mut ready: Vec<TypeDeclId> = waiting
        .iter()
        .filter(|&(_, &count)| count == 0)
        .map(|(&decl, _)| decl)
        .collect();
    while let Some(decl) = ready.pop() {
        let depth = 1 + adjacency
            .get(&decl)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&index| depths[&edges[index].1])
            .max()
            .unwrap_or(0);
        depths.insert(decl, depth);
        for &owner in owners_of.get(&decl).into_iter().flatten() {
            let count = waiting.get_mut(&owner).expect("owner is declared");
            *count -= 1;
            if *count == 0 {
                ready.push(owner);
            }
        }
    }
    depths
}

/// Validates one supertype edge and returns its target declaration.
fn validate_edge(
    table: &TypeTable,
    decls: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    core: CoreDecls,
    owner: TypeDeclId,
    supertype: TypeId,
) -> Result<TypeDeclId, TypeError> {
    let TypeKind::Interface { decl: target, args } = table.get(supertype).kind() else {
        return Err(TypeError::SupertypeNotInterface {
            decl: owner,
            actual: table.get(supertype).kind(),
        });
    };
    // `Object` is the hierarchy root: an edge above it would break the LUB
    // unique-depth-1 anchor (least_upper_bound.dart:327-357).
    if owner == core.object {
        return Err(TypeError::ObjectSupertype(owner));
    }
    // Edges feed the LUB supertype closures verbatim, so they must carry the
    // `?`-free projection (same invariant).
    if table.get(supertype).is_nullable() {
        return Err(TypeError::NullableSupertype(owner));
    }
    let Some(owner_params) = decls.get(&owner) else {
        return Err(TypeError::UndeclaredDecl(owner));
    };
    let Some(params) = decls.get(&target) else {
        return Err(TypeError::UndeclaredDecl(target));
    };
    if args.len() != params.len() {
        return Err(TypeError::ArityMismatch {
            decl: target,
            expected: params.len(),
            actual: args.len(),
        });
    }
    // A supertype template may only reference the owning declaration's own
    // type parameters (`class MyList<T> implements Iterable<T>`: the `T` in
    // `Iterable<T>` belongs to `MyList`).
    let mut referenced = BTreeSet::new();
    table.collect_type_parameters(supertype, &mut referenced);
    for param in referenced {
        if !owner_params.contains(&param) {
            return Err(TypeError::ForeignTypeParameter { decl: owner, param });
        }
    }
    Ok(target)
}

#[derive(Clone, Copy, PartialEq)]
enum Visit {
    Unvisited,
    InProgress,
    Done,
}

/// Depth-first back-edge detection over the declaration graph, on an
/// explicit `(declaration, next-edge)` frame stack: the `Unvisited`/
/// `InProgress`/`Done` coloring and the reported cycle paths are exactly
/// the recursive form's, but the walk cannot overflow the thread stack on
/// deep-but-valid hierarchies — the same guarantee the Kahn sweep in
/// [`class_depths`] was built for. Every back edge comes back as its
/// adjacency slot plus the path slice that closes it, in encounter order;
/// removing exactly those slots leaves a DAG (a DFS forest minus its back
/// edges is acyclic), which is what the lenient build does.
fn back_edges(
    decls: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    edges: &[(TypeDeclId, TypeDeclId, TypeId)],
    adjacency: &FxHashMap<TypeDeclId, Vec<usize>>,
) -> Vec<(usize, Vec<TypeDeclId>)> {
    let mut state: FxHashMap<TypeDeclId, Visit> =
        decls.keys().map(|&decl| (decl, Visit::Unvisited)).collect();
    let mut back: Vec<(usize, Vec<TypeDeclId>)> = Vec::new();
    // The DFS path, as frames of (declaration, index of the next unexplored
    // supertype edge).
    let mut path: Vec<TypeDeclId> = Vec::new();
    let mut frames: Vec<(TypeDeclId, usize)> = Vec::new();

    for &root in decls.keys() {
        if state[&root] != Visit::Unvisited {
            continue;
        }
        state.insert(root, Visit::InProgress);
        path.push(root);
        frames.push((root, 0));
        while let Some(&(decl, index)) = frames.last() {
            let slots = adjacency.get(&decl).map_or(&[][..], Vec::as_slice);
            let Some(&slot) = slots.get(index) else {
                // Every edge explored: the post-order exit.
                state.insert(decl, Visit::Done);
                path.pop();
                frames.pop();
                continue;
            };
            frames.last_mut().expect("peeked above").1 = index + 1;
            let target = edges[slot].1;
            match state[&target] {
                Visit::Done => {}
                Visit::InProgress => {
                    // A back edge: record the path slice that closes it and
                    // walk on without descending.
                    let start = path
                        .iter()
                        .position(|&d| d == target)
                        .expect("in-progress node is on the path");
                    let mut cycle: Vec<TypeDeclId> = path[start..].to_vec();
                    cycle.push(target);
                    back.push((slot, cycle));
                }
                Visit::Unvisited => {
                    state.insert(target, Visit::InProgress);
                    path.push(target);
                    frames.push((target, 0));
                }
            }
        }
    }
    back
}

#[cfg(test)]
mod depth_fallback {
    //! Termination pin for `pick_max_unique_depth` (lub.rs). The test lives
    //! here because only this module can corrupt a snapshot's private depth
    //! table, simulating the malformed snapshot the saturating fallback
    //! exists for (a frozen host incident made termination repo law). The
    //! precompute itself is exact (`class_depths`, see `depth_exactness`
    //! below), so the corruption here is purely synthetic.

    use super::*;
    use crate::system::TypeSystem;

    #[test]
    fn lub_terminates_when_no_depth_is_unique() {
        let table = TypeTable::new();
        let (object, future, function) = (
            TypeDeclId::from_raw(1),
            TypeDeclId::from_raw(2),
            TypeDeclId::from_raw(3),
        );
        let (m, a, b) = (
            TypeDeclId::from_raw(4),
            TypeDeclId::from_raw(5),
            TypeDeclId::from_raw(6),
        );
        let itf = |decl: TypeDeclId| {
            table.intern(
                UninternedType::Interface {
                    decl,
                    args: Vec::new(),
                },
                false,
            )
        };
        let mut hierarchy = TypeHierarchy::builder(CoreDecls {
            object,
            future,
            function,
            record: None,
        })
        .decl(object, Vec::new())
        .decl(future, vec![TypeParamId::from_raw(1)])
        .decl(function, Vec::new())
        .decl(m, Vec::new())
        .decl(a, Vec::new())
        .decl(b, Vec::new())
        .supertype(m, itf(object))
        .supertype(a, itf(m))
        .supertype(b, itf(m))
        .build(&table)
        .expect("fixture hierarchy is valid");

        // Sanity on the sound table: the shared supertype wins.
        assert_eq!(
            TypeSystem::new(&table, &hierarchy).least_upper_bound(itf(a), itf(b)),
            itf(m)
        );

        // Corrupt every depth to the same value: the supertype intersection
        // {m, Object} is tied at every level, so no candidate is ever unique
        // and the walk can only end in the saturating `Object` fallback.
        for depth in hierarchy.depths.values_mut() {
            *depth = 5;
        }
        assert_eq!(
            TypeSystem::new(&table, &hierarchy).least_upper_bound(itf(a), itf(b)),
            itf(object)
        );
    }
}

#[cfg(test)]
mod depth_exactness {
    //! Regression pin for the exact depth precompute: the former budgeted
    //! DFS memoized the saturating value for a node first reached with an
    //! exhausted budget, so a shallow node behind a deep first-visit path
    //! carried depth [`DEPTH_LIMIT`](crate::system::DEPTH_LIMIT) and the LUB
    //! tie-break diverged from the oracle's uncapped
    //! `computeLongestInheritancePathToObject`. The topological sweep must
    //! be exact on chains of any depth.

    use super::*;

    #[test]
    fn deep_chain_leaves_every_depth_exact() {
        let table = TypeTable::new();
        let (object, future, function) = (
            TypeDeclId::from_raw(1),
            TypeDeclId::from_raw(2),
            TypeDeclId::from_raw(3),
        );
        // `C1 -> C2 -> ... -> C101 -> Object`: `C101` is the shallow end
        // (depth 2) but sits 100 steps behind `C1`'s first visit, and `C1`
        // itself is deeper than the old saturation cap (depth 102 > 100).
        let chain = |i: u32| TypeDeclId::from_raw(3 + i);
        let itf = |table: &TypeTable, decl: TypeDeclId| {
            table.intern(
                UninternedType::Interface {
                    decl,
                    args: Vec::new(),
                },
                false,
            )
        };
        let mut builder = TypeHierarchy::builder(CoreDecls {
            object,
            future,
            function,
            record: None,
        })
        .decl(object, Vec::new())
        .decl(future, vec![TypeParamId::from_raw(1)])
        .decl(function, Vec::new());
        for i in 1..=101 {
            builder = builder.decl(chain(i), Vec::new());
        }
        builder = builder.supertype(chain(101), itf(&table, object));
        for i in 1..=100 {
            builder = builder.supertype(chain(i), itf(&table, chain(i + 1)));
        }
        let hierarchy = builder.build(&table).expect("deep chain is a valid DAG");

        assert_eq!(hierarchy.depth(object), 1);
        for i in 1..=101 {
            assert_eq!(hierarchy.depth(chain(i)), 103 - i, "depth of C{i}");
        }
    }
}

#[cfg(test)]
mod stack_safety {
    //! Regression pin for the iterative build passes: 50,000 declarations in
    //! one supertype chain must build without touching the thread stack —
    //! cycle detection runs the explicit-frame DFS (`detect_cycles`) and the
    //! depth precompute the Kahn sweep (`class_depths`) — where the former
    //! recursive cycle check overflowed on deep-but-valid hierarchies.

    use super::*;

    #[test]
    fn deep_acyclic_chain_builds_without_recursion() {
        let table = TypeTable::new();
        let (object, future, function) = (
            TypeDeclId::from_raw(1),
            TypeDeclId::from_raw(2),
            TypeDeclId::from_raw(3),
        );
        let chain = |i: u32| TypeDeclId::from_raw(10 + i);
        let itf = |table: &TypeTable, decl: TypeDeclId| {
            table.intern(
                UninternedType::Interface {
                    decl,
                    args: Vec::new(),
                },
                false,
            )
        };
        let mut builder = TypeHierarchy::builder(CoreDecls {
            object,
            future,
            function,
            record: None,
        })
        .decl(object, Vec::new())
        .decl(future, vec![TypeParamId::from_raw(1)])
        .decl(function, Vec::new());
        for i in 1..=50_000 {
            builder = builder.decl(chain(i), Vec::new());
        }
        builder = builder.supertype(chain(50_000), itf(&table, object));
        for i in 1..=49_999 {
            builder = builder.supertype(chain(i), itf(&table, chain(i + 1)));
        }
        let hierarchy = builder
            .build(&table)
            .expect("deep acyclic chain is a valid DAG");

        assert_eq!(hierarchy.depth(object), 1);
        assert_eq!(hierarchy.depth(chain(50_000)), 2);
        assert_eq!(hierarchy.depth(chain(25_000)), 25_002);
        assert_eq!(hierarchy.depth(chain(1)), 50_001);
    }
}
