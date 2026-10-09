//! The interface hierarchy snapshot (spec §6, R5).
//!
//! The type system never reaches back into the semantic layer: class
//! declarations, their type parameters, and their direct supertypes are
//! injected as an immutable snapshot. Supertype edges are stored as
//! *instantiated interface types* whose arguments may reference the owning
//! declaration's own type parameters (e.g. `class MyList<T> implements
//! Iterable<T>`), which is exactly what the analyzer's
//! `allSupertypes`-plus-substitution walk consumes (`subtype.dart:211-221`).

use std::collections::BTreeSet;

use rustc_hash::{FxBuildHasher, FxHashMap};

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
    /// Longest supertype-chain depth to `Object` per declaration (class
    /// edges only), precomputed at build time for the LUB
    /// max-unique-depth rule (`least_upper_bound.dart:327-357`).
    depths: FxHashMap<TypeDeclId, u32>,
}

impl TypeHierarchy {
    /// Starts building a snapshot.
    #[must_use]
    pub fn builder(core: CoreDecls) -> TypeHierarchyBuilder {
        TypeHierarchyBuilder {
            core,
            decls: Vec::new(),
            edges: Vec::new(),
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
}

/// Builder for [`TypeHierarchy`]; validates every edge before the snapshot
/// becomes observable.
#[derive(Clone, Debug)]
pub struct TypeHierarchyBuilder {
    core: CoreDecls,
    decls: Vec<(TypeDeclId, Vec<TypeParamId>)>,
    edges: Vec<(TypeDeclId, TypeId)>,
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

    /// Validates the graph and freezes the snapshot.
    ///
    /// Every declared class except `Object` implicitly receives an
    /// `Object` supertype edge (Dart's implicit inheritance), guaranteeing
    /// that LUB supertype-set intersections are never empty.
    ///
    /// # Errors
    /// [`TypeError::DuplicateDecl`], [`TypeError::UndeclaredDecl`],
    /// [`TypeError::SupertypeNotInterface`], [`TypeError::ArityMismatch`],
    /// [`TypeError::ForeignTypeParameter`], [`TypeError::CyclicHierarchy`].
    pub fn build(self, table: &TypeTable) -> Result<TypeHierarchy, TypeError> {
        let core = self.core;
        let mut decls: FxHashMap<TypeDeclId, Vec<TypeParamId>> = FxHashMap::default();
        for (decl, params) in self.decls {
            if decls.insert(decl, params).is_some() {
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

        let mut edges: Vec<(TypeDeclId, TypeDeclId, TypeId)> = Vec::with_capacity(self.edges.len());
        for (owner, supertype) in self.edges {
            let target = validate_edge(table, &decls, owner, supertype)?;
            edges.push((owner, target, supertype));
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
            let has_object_edge = edges
                .iter()
                .any(|&(owner, target, _)| owner == decl && target == core.object);
            if !has_object_edge {
                edges.push((decl, core.object, object_none));
            }
        }

        detect_cycles(&decls, &edges)?;

        let mut infos: FxHashMap<TypeDeclId, DeclInfo> = FxHashMap::default();
        for (decl, params) in &decls {
            infos.insert(
                *decl,
                DeclInfo {
                    type_parameters: params.clone(),
                    supertypes: edges
                        .iter()
                        .filter(|&&(owner, _, _)| owner == *decl)
                        .map(|&(_, _, supertype)| supertype)
                        .collect(),
                },
            );
        }

        // Precompute inheritance depths (LUB max-unique-depth rule). The
        // graph is acyclic (validated above), so the memoized DFS terminates.
        let mut targets: FxHashMap<TypeDeclId, Vec<TypeDeclId>> =
            FxHashMap::with_capacity_and_hasher(decls.len(), FxBuildHasher);
        for &(owner, target, _) in &edges {
            targets.entry(owner).or_default().push(target);
        }
        let mut depths: FxHashMap<TypeDeclId, u32> =
            FxHashMap::with_capacity_and_hasher(decls.len(), FxBuildHasher);
        for &decl in decls.keys() {
            class_depth(decl, &targets, &mut depths);
        }

        let object_question = table.intern(
            UninternedType::Interface {
                decl: core.object,
                args: Vec::new(),
            },
            true,
        );
        let null = table.intern(UninternedType::Null, false);
        Ok(TypeHierarchy {
            core,
            decls: infos,
            sentinels: Sentinels {
                object_none,
                object_question,
                null,
            },
            depths,
        })
    }
}

/// Longest supertype-chain depth to `Object` over the declared class edges
/// (`computeLongestInheritancePathToObject`, least_upper_bound.dart:204-323).
/// Every edge targets a validated interface declaration, so the recursion
/// mirrors the LUB helper's filter over supertype kinds; the memoized DFS
/// expects an acyclic graph.
fn class_depth(
    decl: TypeDeclId,
    targets: &FxHashMap<TypeDeclId, Vec<TypeDeclId>>,
    memo: &mut FxHashMap<TypeDeclId, u32>,
) -> u32 {
    if let Some(&depth) = memo.get(&decl) {
        return depth;
    }
    let depth = 1 + targets.get(&decl).map_or(0, |children| {
        children
            .iter()
            .map(|&child| class_depth(child, targets, memo))
            .max()
            .unwrap_or(0)
    });
    memo.insert(decl, depth);
    depth
}

/// Validates one supertype edge and returns its target declaration.
fn validate_edge(
    table: &TypeTable,
    decls: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    owner: TypeDeclId,
    supertype: TypeId,
) -> Result<TypeDeclId, TypeError> {
    let TypeKind::Interface { decl: target, args } = table.get(supertype).kind() else {
        return Err(TypeError::SupertypeNotInterface {
            decl: owner,
            actual: table.get(supertype).kind(),
        });
    };
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

/// Depth-first cycle detection over the declaration graph.
fn detect_cycles(
    decls: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    edges: &[(TypeDeclId, TypeDeclId, TypeId)],
) -> Result<(), TypeError> {
    let mut state: FxHashMap<TypeDeclId, Visit> =
        decls.keys().map(|&decl| (decl, Visit::Unvisited)).collect();
    let mut path: Vec<TypeDeclId> = Vec::new();

    for &root in decls.keys() {
        dfs(root, edges, &mut state, &mut path)?;
    }
    Ok(())
}

/// Visits `decl`, reporting a cycle as the path slice that closes it.
fn dfs(
    decl: TypeDeclId,
    edges: &[(TypeDeclId, TypeDeclId, TypeId)],
    state: &mut FxHashMap<TypeDeclId, Visit>,
    path: &mut Vec<TypeDeclId>,
) -> Result<(), TypeError> {
    match state[&decl] {
        Visit::Done => return Ok(()),
        Visit::InProgress => {
            let start = path
                .iter()
                .position(|&d| d == decl)
                .expect("in-progress node is on the path");
            let mut cycle: Vec<TypeDeclId> = path[start..].to_vec();
            cycle.push(decl);
            return Err(TypeError::CyclicHierarchy(cycle));
        }
        Visit::Unvisited => {}
    }
    state.insert(decl, Visit::InProgress);
    path.push(decl);
    for &(owner, target, _) in edges {
        if owner != decl {
            continue;
        }
        dfs(target, edges, state, path)?;
    }
    state.insert(decl, Visit::Done);
    path.pop();
    Ok(())
}
