//! The resolved-type back-end for the flow layer's [`dartq_flow::TypeOps`]
//! seam (#2204, phase A): a per-library `dartq_types` snapshot plus the
//! adapter that maps the flow layer's `Ty`/`ArgTy` spellings onto it.
//!
//! # The snapshot
//!
//! [`LibraryTypes`] is built once per analyze beside the `ElementIndex`
//! (the engine's `semdiag::build_index` sibling) and shared by every
//! per-unit fold through `LibraryView`. It declares every named container
//! the diagnosing library can see — the library's own units plus every
//! container its namespace resolves through imports and re-exports —
//! over a curated `dart:core` face identical to [`crate::bounds`]'s
//! builtin table.
//!
//! The element model's supertype edges are name-level ([`Container`]'s
//! `superclass`/`with_clause`/`implements_clause` carry no written
//! arguments), so each edge instantiates its target to bounds (the
//! official raw-type posture, `instantiateToBounds`,
//! type_system.dart:719-731) — exact for the sealed-subtype closure
//! (`sealed class Resource` / `class File extends Resource`), one arity
//! step short for written clause arguments (`class A extends S<int>`
//! reads `A <: S<dynamic>`); carrying the real clause trees is the
//! `TypedModel` migration the diagnostic-layer lanes own. A clause name
//! that resolves to no registered declaration contributes no edge — the
//! open direction (a missing edge under-reports subtype answers, never a
//! false positive).
//!
//! # The adapter
//!
//! [`ResolvedTypeOps`] implements [`dartq_flow::TypeOps`]: a
//! `flow::Ty`'s interned name id resolves through the recorder's own
//! lexeme table to the shared `StringTable`, then through the
//! `StringId → TypeDeclId` registry to an interned `TypeId`. A name
//! absent from the registry maps to [`UninternedType::Invalid`] —
//! `subtype.dart`'s 0c arm suppresses both directions, matching the
//! name-level `unknown-is-top` posture.
//!
//! ## The `names` asymmetry
//!
//! The seam's `is_always_exhaustive` carries the recorder's `names`
//! table and `ArgTy` arena (a switch probe needs both to decode the
//! scrutinee's spelling); the other six ops take bare `Ty`s. So the
//! adapter resolves a `Base::Name` id two ways: inside
//! `is_always_exhaustive` it reads `names[id]` directly and caches the
//! `id → (decl, args)` binding; inside `is_subtype`/`factor`/`classify`/…
//! it reads the cache only. A name id the cache has not yet seen answers
//! through [`dartq_flow::ty::NameTypeOps`]'s name-level logic — the
//! pre-mount behavior — so the resolved face never answers *worse* than
//! today's, only better (cross-file decl identity) once a switch probe
//! has populated the cache.
//!
//! Single-threaded like [`TypeTable`] (its interior `RefCell` memo):
//! the flow replays on the fold's thread.

use std::cell::RefCell;

use dartq_flow::ty::{
    ArgTy, Base, Classification, EMPTY_ARGS, NameTypeOps, TypeOps, Ty as FlowTy,
};
use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use dartq_types::{
    CoreDecls, ParamFacts, ParamFactsTable, TypeDeclId, TypeHierarchy, TypeId, TypeKind,
    TypeParamId, TypeSystem, TypeTable, UninternedType, Variance,
    instantiate_type_formals_to_bounds,
};

use crate::bounds::TypeTree;
use crate::library::ElementIndex;
use crate::model::{ContainerKind, MicroKind};

/// Core-declaration raw ids (the hierarchy requires `Object`, `Future`,
/// and bare `Function`). Each snapshot owns its id space.
const OBJECT_RAW: u32 = 1;
const FUTURE_RAW: u32 = 2;
const FUNCTION_RAW: u32 = 3;
const FIRST_BUILTIN_RAW: u32 = 4;
const FIRST_CONTAINER_RAW: u32 = 100;
/// The opaque-declaration band for resolved-but-unmodeled names
/// ([`ResolvedTypeOps::opaque_decl`]) — disjoint from the registry's
/// allocation range (builtins + containers grow upward from 100).
const FIRST_OPAQUE_RAW: u32 = 0x4000_0000;

/// The curated `dart:core` face — the same fixed public-declaration table
/// [`crate::bounds`]'s snapshot carries (`int`/`double` subtype `num`,
/// `num` implements `Comparable<num>`, `String` implements
/// `Comparable<String>, Pattern`, the collection roots reach
/// `Iterable`), plus `bool` registered as a two-constant enum: its
/// `true`/`false` constants close the exhaustiveness arm
/// (`static_type.dart:67`'s closed-by-construction face), plus `Record`
/// and `Enum` — the two remaining dart:core classes an `is`-test
/// routinely names: the oracle resolves them to plain classes, so
/// `x is Record` promotes and keeps its else live where an `Invalid`
/// read vacuously deads it (subtype.dart's 0c arm). `FutureOr`
/// stays unregistered — it has a first-class [`TypeKind`] variant, not a
/// declaration. A name absent here is invisible to the resolved walk; the
/// adapter's `Invalid` fallback keeps the question open.
const BUILTINS: &[(&str, &[&str], &[&str])] = &[
    ("num", &[], &["Comparable<num>"]),
    ("int", &[], &["num"]),
    ("double", &[], &["num"]),
    ("String", &[], &["Comparable<String>", "Pattern"]),
    ("bool", &[], &[]),
    ("Pattern", &[], &[]),
    ("Comparable", &["T"], &[]),
    ("Iterable", &["E"], &[]),
    ("List", &["E"], &["Iterable<E>"]),
    ("Set", &["E"], &["Iterable<E>"]),
    ("Map", &["K", "V"], &[]),
    ("Record", &[], &[]),
    ("Enum", &[], &[]),
];

/// The per-library resolved-type world: one interned [`TypeTable`] plus
/// the [`TypeHierarchy`] over it, a `StringId → TypeDeclId` name registry
/// covering every reachable declaration (in-library and imported
/// containers, the `dart:core` builtins), and the [`ParamFactsTable`] the
/// instantiate-to-bounds defaults read. Built once per analyze over the
/// whole residency's element index (the same face `build_index`
/// indexes), shared by every per-unit fold.
pub struct LibraryTypes {
    table: TypeTable,
    hierarchy: TypeHierarchy,
    /// Written name → declaration. In-library and import-reachable
    /// containers register under their declared spelling; the builtins
    /// ride under their `dart:core` names and lose to a same-named
    /// registered declaration (the scope chain resolves the local first).
    registry: FxHashMap<StringId, TypeDeclId>,
    /// The parameter bounds face (`instantiateToBounds` reads it). The
    /// element model carries parameter *counts* only — the bound trees
    /// are the parse-time capture's (`crate::bounds`), which this
    /// library-scoped build does not see — so every parameter's bound
    /// registers `dynamic` (the always-satisfied default). Phase C's
    /// `TypedModel` wiring tightens the same field.
    facts: ParamFactsTable,
    /// Declaration → its type-parameter ids, in declaration order — the
    /// instantiate-to-bounds input (a raw `List` reads `List<dynamic>`).
    decl_params: FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    /// Resolved names that are not containers (a typedef, a top-level
    /// function or variable): a spelling that resolves to one maps
    /// `Invalid` rather than borrowing a same-named builtin.
    non_containers: FxHashSet<StringId>,
    /// `typedef` name → the aliased head name plus its own `?` flag —
    /// captured only when the element model carried the RHS as a bare
    /// `Ty::Named` with no written arguments (`typedef A = B`,
    /// `typedef A = B?`). A `typedef A = List<int>` or
    /// `typedef F = void Function()` RHS stays in `non_containers` only:
    /// its `ArgListId`/`FnShapeId` slots index the element model's
    /// `ArgPool` this snapshot does not hold, so the spelled use keeps
    /// the opaque face today (the under-report direction — oracle-true
    /// only for the bare-Named arm this map covers).
    typedef_targets: FxHashMap<StringId, (StringId, bool)>,
    /// Type-parameter spellings → a synthetic parameter id each
    /// (`MicroKind::TypeParam` declarations). Declaration-scoped in the
    /// official element model but name-keyed here exactly like the flow
    /// layer's own spelling-keyed model: a name declared as a type
    /// parameter maps to a [`TypeKind::TypeParameter`] instead of
    /// `Invalid` — `Response<T>`'s `T` keeps the subtype rules'
    /// parameter arms live (`dynamic <: T` fails, so the instantiated
    /// pair is not a subtype) rather than suppressing through `Invalid`
    /// (#2186 S4-C). Two same-spelled parameters in sibling scopes share
    /// the id — reflexive `T <: T` answers `true` either way, the only
    /// question identity changes.
    type_params: FxHashMap<StringId, TypeParamId>,
}

/// One container's parse-time supertype capture: the type-parameter
/// spellings in declaration order (the scope the clause trees resolve
/// their parameter arguments through) and the written supertype trees
/// (`extends`/`with`/`implements`, in clause order — the `on` constraints
/// of a mixin are constraints, not edges, and ride the name-level arm).
/// The capture's `open` bit never reaches here: an unfaithful capture
/// contributes no entry at all, so [`LibraryTypes`] keeps the name-level
/// edge set — the under-report direction.
pub(crate) struct ClauseCapture {
    /// The container's own type-parameter spellings, declaration order.
    pub(crate) params: Box<[StringId]>,
    /// The written supertype trees (extends/with/implements), clause order.
    pub(crate) supers: Vec<TypeTree>,
}

/// The real supertype-clause trees of the residency, keyed by declaring
/// file and container name — the argument-bearing faces the element
/// model's `Container` strips to names. Fed from the parse-time
/// `crate::bounds` capture once the engine threads it through (the engine
/// one-liner lands beside `semdiag::build_index`; until then
/// [`ClauseTrees::empty`] keeps the instantiate-to-bounds edges). Opaque
/// to the engine: the tree type stays crate-internal, so the transport
/// is construction-by-the-semantic-side only.
#[derive(Default)]
pub struct ClauseTrees {
    /// `(declaring file, container name)` → the captured clause facts.
    pub(crate) trees: FxHashMap<(FileId, StringId), ClauseCapture>,
}

impl ClauseTrees {
    /// An empty face: every container's edges instantiate to bounds, the
    /// pre-Phase-D behavior.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Records one container's captured clause facts. Insertion is
    /// first-wins on a shared `(file, name)` key — the duplicate-name
    /// discipline `BoundsIndex`'s own capture keeps. Currently no
    /// producer calls it — the engine-side wiring lands beside
    /// `semdiag::build_index` (Phase D's engine one-liner).
    #[allow(dead_code)] // the transport face; consumed once the engine threads captures through
    pub(crate) fn insert(
        &mut self,
        file: FileId,
        name: StringId,
        capture: ClauseCapture,
    ) {
        self.trees.entry((file, name)).or_insert(capture);
    }

    /// The captured clause facts of one declaring file's container.
    pub(crate) fn get(&self, file: FileId, name: StringId) -> Option<&ClauseCapture> {
        self.trees.get(&(file, name))
    }
}

impl LibraryTypes {
    /// [`LibraryTypes::build`] over the name-level clause names only —
    /// the pre-Phase-D face. Equivalent to [`Self::build_with_clauses`]
    /// over an empty [`ClauseTrees`].
    pub fn build(
        graph: &LibraryGraph,
        namespaces: &Namespaces,
        index: &ElementIndex,
        strings: &StringTable,
        lib: LibraryId,
    ) -> Self {
        Self::build_with_clauses(
            graph,
            namespaces,
            index,
            strings,
            lib,
            &ClauseTrees::empty(),
        )
    }

    /// Builds the snapshot for one library. `index` is the analyze-wide
    /// element index (`build_index`'s product); `graph`/`namespaces` the
    /// resolution pair the unit's own resolves run on. The registered
    /// names are the library's own containers plus every container its
    /// namespace can resolve a name to (import edges' targets' export
    /// scopes, transitively through re-exports), keyed on the shared
    /// `StringTable` so the adapter's lookups cost one hash probe.
    /// `clauses`, when populated, carries the real written supertype
    /// trees — `class A extends S<int>` edges as `A → S<int>` rather
    /// than the to-bounds `A → S<dynamic>` the name-level arm produces.
    /// A container absent from `clauses` keeps the name-level edges
    /// (the open direction: missing edges only ever under-report).
    pub fn build_with_clauses(
        graph: &LibraryGraph,
        namespaces: &Namespaces,
        index: &ElementIndex,
        strings: &StringTable,
        lib: LibraryId,
        clauses: &ClauseTrees,
    ) -> Self {
        let table = TypeTable::new();
        let object = TypeDeclId::from_raw(OBJECT_RAW);
        let future = TypeDeclId::from_raw(FUTURE_RAW);
        let function = TypeDeclId::from_raw(FUNCTION_RAW);
        // `Record`'s builtin decl — the raw id the loop below mints for
        // the `"Record"` row, computed positionally so the table's order
        // stays the single source. `CoreDecls.record` is what lets the
        // record-class arms (`isDartCoreRecord`) answer over this
        // snapshot instead of falling back to `None`.
        let record = TypeDeclId::from_raw(
            FIRST_BUILTIN_RAW
                + BUILTINS
                    .iter()
                    .position(|&(name, ..)| name == "Record")
                    .unwrap() as u32,
        );
        let mut builder = TypeHierarchy::builder(CoreDecls {
            object,
            future,
            function,
            record: Some(record),
        })
        .decl(object, Vec::new())
        .decl(future, vec![TypeParamId::from_raw(1)])
        .decl(function, Vec::new());
        let mut facts = ParamFactsTable::new();
        facts.set(
            TypeParamId::from_raw(1),
            ParamFacts {
                variance: Variance::Covariant,
                bound: None,
            },
        );

        let mut registry: FxHashMap<StringId, TypeDeclId> = FxHashMap::default();
        let mut decl_params: FxHashMap<TypeDeclId, Vec<TypeParamId>> = FxHashMap::default();
        let decl_owner: FxHashMap<TypeDeclId, (FileId, LibraryId)> = FxHashMap::default();
        let non_containers: FxHashSet<StringId> = FxHashSet::default();
        let typedef_targets: FxHashMap<StringId, (StringId, bool)> = FxHashMap::default();
        let type_params: FxHashMap<StringId, TypeParamId> = FxHashMap::default();
        let mut next_decl = FIRST_BUILTIN_RAW;
        let mut next_param = 2_u32;

        // —— the builtin face ————————————————————————————————————————
        // Fresh parameter ids per builtin; `Object`'s edge arrives
        // implicitly at `build`, and `bool` is the only enum-shaped one
        // (its constants close `true`/`false` — the exhaustiveness arm
        // reads `enum_constant_count`). The `Comparable<num>`/`Iterable<E>`
        // edges intern below once every decl exists (a forward reference
        // like `Comparable` must already be registered).
        let mut builtin_edges: Vec<(TypeDeclId, &str)> = Vec::new();
        for &(name, params, supers) in BUILTINS {
            let decl = TypeDeclId::from_raw(next_decl);
            next_decl += 1;
            let mut param_ids = Vec::with_capacity(params.len());
            for _ in params {
                let id = TypeParamId::from_raw(next_param);
                next_param += 1;
                param_ids.push(id);
                facts.set(
                    id,
                    ParamFacts {
                        variance: Variance::Covariant,
                        bound: None,
                    },
                );
            }
            registry.insert(strings.intern(name), decl);
            builder = builder.decl(decl, param_ids.clone());
            decl_params.insert(decl, param_ids);
            for &super_text in supers {
                builtin_edges.push((decl, super_text));
            }
        }
        let bool_decl = registry[&strings.intern("bool")];
        builder = builder.enumeration(bool_decl, 2);
        // dart:core's `Enum` class — the implicit supertype every user
        // enum declaration carries (`E <: Enum` holds in the oracle). The
        // builtin row stays edge-free: `bool` is a plain class in the
        // analyzer's element model, and the oracle keeps `b is Enum`
        // decidable (probed: dart analyze 3.13.1, no dead_code), so only
        // the user-declared enum actions synthesize the edge below.
        let core_enum = registry[&strings.intern("Enum")];

        // The `CoreDecls` spellings: `Object`, `Function` and `Future` are
        // declared in the hierarchy but were never keyed into `registry`,
        // so a spelled `Object`/`Function`/`Future` (a declared parameter
        // type, an `is`-tested name) read `Invalid` where the oracle
        // resolves the dart:core class. With `Invalid` now answering
        // through `TypeSystem::is_subtype` — the oracle's own 0c arm —
        // an unregistered `Object` receiver made every `factor` vacuous
        // and deaded every `is` arm (the `Object x` regression the
        // pre-mount `is_invalid` bail had masked). `decl_params` rides
        // beside so `instantiate_to_bounds` answers `Object`/`Function`
        // (arity 0) and `Future<dynamic>` (the unbounded default).
        registry.insert(strings.intern("Object"), object);
        decl_params.insert(object, Vec::new());
        registry.insert(strings.intern("Function"), function);
        decl_params.insert(function, Vec::new());
        registry.insert(strings.intern("Future"), future);
        decl_params.insert(future, vec![TypeParamId::from_raw(1)]);

        // —— the reachable containers ————————————————————————————————
        // `Registry` carries the mutable build state; `register_file`
        // collects the decl actions (plain/sealed/enum) into `actions`
        // rather than touching the move-only builder, then a single pass
        // applies them all — the same information, in build order.
        let mut registry = Registry {
            registry,
            decl_params,
            decl_owner,
            non_containers,
            typedef_targets,
            type_params,
            facts,
            next_decl: FIRST_CONTAINER_RAW,
            next_param,
        };
        let mut actions: Vec<DeclAction> = Vec::new();
        for &file in graph.unit_files(lib) {
            registry.register_file(index, file, lib, &mut actions);
            for action in actions.drain(..) {
                builder = action.apply(builder, &table, core_enum);
            }
        }


        // The import reachability sweep: BFS over import edges from
        // `lib`, each target's export scope contributing the names the
        // unit's resolves see (re-exports ride the same edges — the
        // scope the names resolve through is exactly `export_scope`'s).
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        seen.insert(lib.index());
        let mut queue: Vec<LibraryId> = vec![lib];
        while let Some(current) = queue.pop() {
            for edge in graph.import_edges(current) {
                let Some(target) = edge.target else {
                    continue;
                };
                if !seen.insert(target.index()) {
                    continue;
                }
                queue.push(target);
                for (name, entry) in graph.export_scope(target) {
                    if registry.registry.contains_key(&name)
                        || registry.non_containers.contains(&name)
                    {
                        continue;
                    }
                    match index.get(entry.file).map(|els| els.container(name)) {
                        Some(Some(_)) => {
                            registry.register_file(index, entry.file, target, &mut actions);
                            for action in actions.drain(..) {
                                builder = action.apply(builder, &table, core_enum);
                            }
                        }
                        Some(None) | None => {
                            // The name resolves but its declaring file
                            // carries no model, or the model holds no
                            // container under it — suppress, never
                            // borrow a same-named decl.
                            registry.non_containers.insert(name);
                        }
                    }
                }
            }
        }
        let Registry {
            registry,
            decl_params,
            decl_owner,
            non_containers,
            typedef_targets,
            type_params,
            facts,
            ..
        } = registry;

        // —— supertype edges —————————————————————————————————————————
        // The builtins' written edges (`Comparable<num>`, `Iterable<E>`)
        // intern by hand: the argument `E` is the owner's own parameter
        // (a `List<E> implements Iterable<E>` edge reads `E` as `List`'s
        // parameter); ground names resolve through the registry.
        for (decl, super_text) in &builtin_edges {
            if let Some(ty) =
                builtin_super(&table, &registry, &decl_params, strings, *decl, super_text)
            {
                builder = builder.supertype(*decl, ty);
            }
        }
        // The model's name-level clauses: `superclass`/`with`/
        // `implements` instantiate to bounds (no written arguments exist
        // to map); a mixin's `on` constraints join the walk — a
        // constraint bounds the application's supertype face. A clause
        // name resolving to no registered container contributes
        // nothing — the declaration keeps its remaining edges plus the
        // implicit `Object` the builder adds (the open direction).
        //
        // `decl_owner` pairs each decl back to its declaring file and
        // library: an imported container's `extends` resolves in its
        // own scope, not `lib`'s (the same discipline `resolve_container`
        // climbs with).
        for (&decl, &(file, owner)) in &decl_owner {
            let Some(elements) = index.get(file) else {
                continue;
            };
            // Recover the decl's name from the registry (the only
            // name→decl pairing the snapshot carries; a build-time scan
            // pays once per decl).
            let Some(name) = registry
                .iter()
                .find_map(|(&n, &d)| (d == decl).then_some(n))
            else {
                continue;
            };
            let Some(container) = elements.container(name) else {
                continue;
            };
            // The real clause trees when the capture reaches this build —
            // `class A extends S<int>` maps the written `S<int>` against
            // `A`'s own parameter scope, replacing the to-bounds
            // `S<dynamic>` the name-level arm produces. A container with
            // no capture keeps the clause-name fallback verbatim; a
            // capture that fails to map contributes no edge (the open
            // direction — a dropped edge only ever under-reports).
            if let Some(capture) = clauses.get(file, name) {
                let params = decl_params.get(&decl).map_or(&[][..], Vec::as_slice);
                let scope: FxHashMap<StringId, TypeParamId> = capture
                    .params
                    .iter()
                    .copied()
                    .zip(params.iter().copied())
                    .collect();
                for tree in &capture.supers {
                    if let Some(ty) =
                        clause_tree_ty(&table, &registry, &decl_params, &facts, &scope, tree)
                        && matches!(
                            table.get(ty).kind(),
                            TypeKind::Interface { .. }
                        )
                    {
                        builder = builder.supertype(decl, ty);
                    }
                }
                // A mixin's `on` constraints still join at name level —
                // the capture's `On` trees are discarded upstream of
                // `ClauseCapture::supers`, exactly like today's clause-name
                // arm keeps them.
                if container.kind == ContainerKind::Mixin {
                    for &target_name in &container.on_clause {
                        if let Some(ty) = clause_head_instantiation(
                            &table,
                            &registry,
                            &decl_params,
                            &facts,
                            namespaces,
                            graph,
                            index,
                            owner,
                            target_name,
                        ) {
                            builder = builder.supertype(decl, ty);
                        }
                    }
                }
                continue;
            }
            let clause_names = container
                .superclass
                .iter()
                .chain(container.with_clause.iter())
                .chain(container.implements_clause.iter())
                .chain(
                    (container.kind == ContainerKind::Mixin)
                        .then(|| container.on_clause.iter())
                        .into_iter()
                        .flatten(),
                )
                .copied();
            for target_name in clause_names {
                if let Some(ty) = clause_head_instantiation(
                    &table,
                    &registry,
                    &decl_params,
                    &facts,
                    namespaces,
                    graph,
                    index,
                    owner,
                    target_name,
                ) {
                    builder = builder.supertype(decl, ty);
                }
            }
        }

        // Freeze: an invalid captured graph (a cycle, an arity-mismatched
        // edge) falls back to the bare core — declarations stay
        // registered but carry no hierarchy face, the same under-report
        // posture `crate::bounds` takes.
        let hierarchy = match builder.build(&table) {
            Ok(hierarchy) => hierarchy,
            Err(_) => TypeHierarchy::builder(CoreDecls {
                object,
                future,
                function,
                record: None,
            })
            .decl(object, Vec::new())
            .decl(future, vec![TypeParamId::from_raw(1)])
            .decl(function, Vec::new())
            .build(&table)
            .expect("the bare core hierarchy is always valid"),
        };

        Self {
            table,
            hierarchy,
            registry,
            facts,
            decl_params,
            non_containers,
            typedef_targets,
            type_params,
        }
    }
}

/// One hierarchy declaration step `register_file` emits — the move-only
/// `TypeHierarchyBuilder` applies them in order after each file's names
/// are registered (the builder's `decl`/`sealed`/`enumeration` calls
/// consume `self`, so they ride out through `apply`).
enum DeclAction {
    /// `builder.decl(decl, params)`.
    Decl(TypeDeclId, Vec<TypeParamId>),
    /// `builder.sealed(decl)`.
    Sealed(TypeDeclId),
    /// `builder.enumeration(decl, constants)`.
    Enum(TypeDeclId, u16),
}

impl DeclAction {
    fn apply(
        self,
        builder: dartq_types::TypeHierarchyBuilder,
        table: &TypeTable,
        core_enum: TypeDeclId,
    ) -> dartq_types::TypeHierarchyBuilder {
        match self {
            Self::Decl(decl, params) => builder.decl(decl, params),
            Self::Sealed(decl) => builder.sealed(decl),
            // An enum declaration implicitly implements dart:core's
            // `Enum` (the oracle's `E <: Enum` — `e is Enum` is a
            // guaranteed-true test and the else branch deads; #2370).
            // Synthesized where the enum enters the snapshot, so the
            // whole resolved walk sees the edge; class declarations
            // never grow it.
            Self::Enum(decl, constants) => {
                let enum_itf = table.intern(
                    UninternedType::Interface {
                        decl: core_enum,
                        args: Vec::new(),
                    },
                    false,
                );
                builder.enumeration(decl, constants).supertype(decl, enum_itf)
            }
        }
    }
}

/// The mutable state `LibraryTypes::build`'s container-registration pass
/// carries: the name→decl registry, the decl→params/owner side tables,
/// the parameter facts, and the fresh-id cursors. Struct-scoped so
/// `register_file` can take `&mut self` without fighting the borrow
/// checker.
struct Registry {
    registry: FxHashMap<StringId, TypeDeclId>,
    decl_params: FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    decl_owner: FxHashMap<TypeDeclId, (FileId, LibraryId)>,
    non_containers: FxHashSet<StringId>,
    /// Bare `Ty::Named` typedef right-hand sides keyed on the alias's
    /// own spelling — the only alias shape the snapshot can resolve
    /// without the element model's `ArgPool` (see [`LibraryTypes::typedef_targets`]).
    typedef_targets: FxHashMap<StringId, (StringId, bool)>,
    /// Type-parameter spellings → their synthetic parameter ids: the
    /// `TypeKind::TypeParameter` map the adapter answers instead of
    /// `Invalid` for a `T`-shaped name (#2186 S4-C).
    type_params: FxHashMap<StringId, TypeParamId>,
    facts: ParamFactsTable,
    next_decl: u32,
    next_param: u32,
}

impl Registry {
    /// Declares every named container of one unit file into the snapshot
    /// (declaration order; first-wins on a shared name — a local shadows
    /// an import exactly like the scope chain), plus the unit's
    /// non-container top-level declarations (marked so a spelling that
    /// resolves to a typedef/function/variable never borrows a same-named
    /// registered decl or builtin). The hierarchy steps append to
    /// `actions` in file order — the caller applies them to the builder.
    fn register_file(
        &mut self,
        index: &ElementIndex,
        file: FileId,
        owner: LibraryId,
        actions: &mut Vec<DeclAction>,
    ) {
        let Some(elements) = index.get(file) else {
            return;
        };
        for &name in elements.container_order() {
            let Some(container) = elements.container(name) else {
                continue;
            };
            if self.registry.contains_key(&name) {
                continue;
            }
            let decl = TypeDeclId::from_raw(self.next_decl);
            self.next_decl += 1;
            let mut param_ids = Vec::with_capacity(usize::from(container.type_params));
            for _ in 0..container.type_params {
                let id = TypeParamId::from_raw(self.next_param);
                self.next_param += 1;
                param_ids.push(id);
                self.facts.set(
                    id,
                    ParamFacts {
                        variance: Variance::Covariant,
                        bound: None,
                    },
                );
            }
            self.registry.insert(name, decl);
            self.decl_params.insert(decl, param_ids.clone());
            self.decl_owner.insert(decl, (file, owner));
            actions.push(DeclAction::Decl(decl, param_ids));
            if container.sealed {
                actions.push(DeclAction::Sealed(decl));
            }
            if container.kind == ContainerKind::Enum {
                let constants =
                    u16::try_from(container.enum_constants.len()).unwrap_or(u16::MAX);
                actions.push(DeclAction::Enum(decl, constants));
            }
        }
        // A non-container top-level declaration of a name stays out of
        // the registry — mark it so the adapter never borrows a
        // same-named builtin or co-declared container. Type parameters
        // stay out of both sets: their spellings map to the synthetic
        // `TypeParameter` ids below, one per name (declaration-scoped in
        // the element model, name-keyed here like the flow layer).
        for (name, decl) in elements.declarations() {
            if self.registry.contains_key(&name) {
                continue;
            }
            if decl.kind == MicroKind::TypeParam {
                if !self.type_params.contains_key(&name) {
                    let param = TypeParamId::from_raw(self.next_param);
                    self.next_param += 1;
                    self.facts.set(param, ParamFacts::LEGACY);
                    self.type_params.insert(name, param);
                }
                continue;
            }
            let container_like = matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin | MicroKind::Extension
            );
            if !container_like {
                self.non_containers.insert(name);
                // A bare `typedef A = B;` resolves `A` to `B` at the oracle's
                // face — capture the RHS head so `map_name` chases it instead
                // of minting an opaque declaration (subtype.dart's `C c = a`
                // arm would otherwise under-report through `Invalid`).
                // Argument-carrying and `Function`/`Record` RHS shapes index
                // the element model's `ArgPool`, which this snapshot does
                // not hold — they keep the opaque face (under-report).
                if decl.kind == MicroKind::TypeAlias
                    && let crate::model::Ty::Named {
                        name: rhs_name,
                        nullable: rhs_nullable,
                        args,
                    } = decl.var_ty
                    && args == crate::model::ArgListId::EMPTY
                {
                    self.typedef_targets.insert(name, (rhs_name, rhs_nullable));
                }
            }
        }
    }
}

/// One builtin supertype text (`Comparable<num>`, `Iterable<E>`, or a
/// bare `num`) interned: the head resolves through the registry; a
/// written argument resolves as the OWNER's own type parameter when the
/// head declares one and the text is not a registered name (the fixed
/// table's generic edges are all `Name<Param>` — `List<E>`'s `E` is
/// `List`'s sole parameter) or as a ground type through the registry
/// (`Comparable<num>`'s `num`). `None` on an unresolvable head or
/// argument.
fn builtin_super(
    table: &TypeTable,
    registry: &FxHashMap<StringId, TypeDeclId>,
    decl_params: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    strings: &StringTable,
    owner: TypeDeclId,
    text: &str,
) -> Option<TypeId> {
    let (head, args_text) = match text.split_once('<') {
        Some((head, rest)) => (head.trim(), rest.strip_suffix('>').map(str::trim)),
        None => (text.trim(), None),
    };
    let &decl = registry.get(&strings.intern(head))?;
    let args = match args_text {
        None => Vec::new(),
        Some(inner) if inner.is_empty() => Vec::new(),
        Some(inner) => {
            let owner_params = decl_params.get(&owner)?;
            let &param = owner_params.first()?;
            let mut args = Vec::new();
            for arg_text in inner.split(',') {
                let arg_text = arg_text.trim();
                // A registered name (`num` in `Comparable<num>`) is a
                // ground argument; anything else in the fixed table is
                // the owner's own parameter spelling.
                if let Some(&arg_decl) = registry.get(&strings.intern(arg_text)) {
                    args.push(instantiate_to_bounds(
                        table,
                        &ParamFactsTable::new(),
                        decl_params,
                        arg_decl,
                    )?);
                    continue;
                }
                args.push(table.intern(
                    UninternedType::TypeParameter { parameter: param },
                    false,
                ));
            }
            args
        }
    };
    Some(table.intern(UninternedType::Interface { decl, args }, false))
}

/// The name-level edge of a `superclass`/`with`/`implements`/`on` clause:
/// the head resolves through the registry, then the owner library's own
/// scope chain (an imported container's `extends` resolves in its own
/// scope — the discipline `resolve_container` climbs with); a resolved
/// head instantiates to bounds. `None` on an unresolvable name — the
/// declaration keeps its remaining edges plus the implicit `Object` the
/// builder adds (the open direction).
#[expect(clippy::too_many_arguments)] // one operand per snapshot face the resolution reads
fn clause_head_instantiation(
    table: &TypeTable,
    registry: &FxHashMap<StringId, TypeDeclId>,
    decl_params: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    facts: &ParamFactsTable,
    namespaces: &Namespaces,
    graph: &LibraryGraph,
    index: &ElementIndex,
    owner: LibraryId,
    target_name: StringId,
) -> Option<TypeId> {
    let target_decl = registry.get(&target_name).copied().or_else(|| {
        match namespaces.lookup(graph, owner, target_name) {
            Lookup::Found(NamespaceEntry::Decl(owner_file)) => index
                .get(owner_file)
                .and_then(|els| els.container(target_name))
                .and_then(|_| registry.get(&target_name).copied()),
            _ => None,
        }
    })?;
    instantiate_to_bounds(table, facts, decl_params, target_decl)
}

/// Maps one captured supertype tree onto the snapshot: the written
/// head resolves through the registry, its written arguments recurse,
/// and a leaf inside `scope` (the declaring container's own parameters
/// by spelling) interns a [`TypeKind::TypeParameter`]. `None` on a
/// poisoned tree, a nullable root (hierarchy edges are non-nullable),
/// an unresolvable head, an arity mismatch, or a record/function shape —
/// the clause then contributes no edge (the open direction, matching
/// `crate::bounds`'s unmodelable-face suppression). A bare generic head
/// (`extends S` under `S<T>`) instantiates to bounds exactly like the
/// clause-name arm.
fn clause_tree_ty(
    table: &TypeTable,
    registry: &FxHashMap<StringId, TypeDeclId>,
    decl_params: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    facts: &ParamFactsTable,
    scope: &FxHashMap<StringId, TypeParamId>,
    tree: &TypeTree,
) -> Option<TypeId> {
    if tree.poisoned || tree.nullable || tree.record.is_some() || tree.function.is_some() {
        return None;
    }
    // A type-parameter leaf: the declaring container's own spelling —
    // `class B<T> extends S<T>` carries `T` into the edge, the official
    // `InterfaceType(T)` face.
    if tree.args.is_empty()
        && let Some(&parameter) = scope.get(&tree.head)
    {
        return Some(
            table.intern(UninternedType::TypeParameter { parameter }, false),
        );
    }
    let &decl = registry.get(&tree.head)?;
    let args = if tree.args.is_empty() {
        instantiate_type_formals_to_bounds(
            table,
            facts,
            decl_params.get(&decl).map_or(&[][..], Vec::as_slice),
        )
        .defaults
    } else {
        let params = decl_params.get(&decl).map_or(&[][..], Vec::as_slice);
        if tree.args.len() != params.len() {
            return None;
        }
        let mut args = Vec::with_capacity(tree.args.len());
        for arg in &tree.args {
            // Clause arguments keep their own `?` faces — an argument
            // tree nullable at ITS root maps nullable (subtype.dart's
            // argument nullability is per-node), so the `tree.nullable`
            // gate above is root-only.
            args.push(clause_arg_ty(table, registry, decl_params, facts, scope, arg)?);
        }
        args
    };
    Some(table.intern(UninternedType::Interface { decl, args }, false))
}

/// The argument-node face of [`clause_tree_ty`]: identical mapping but
/// `nullable` honored per node (a `T?` argument is legal where the
/// supertype root is not).
fn clause_arg_ty(
    table: &TypeTable,
    registry: &FxHashMap<StringId, TypeDeclId>,
    decl_params: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    facts: &ParamFactsTable,
    scope: &FxHashMap<StringId, TypeParamId>,
    tree: &TypeTree,
) -> Option<TypeId> {
    if tree.poisoned || tree.record.is_some() || tree.function.is_some() {
        return None;
    }
    if tree.args.is_empty()
        && let Some(&parameter) = scope.get(&tree.head)
    {
        return Some(
            table.intern(UninternedType::TypeParameter { parameter }, tree.nullable),
        );
    }
    let &decl = registry.get(&tree.head)?;
    let args = if tree.args.is_empty() {
        instantiate_type_formals_to_bounds(
            table,
            facts,
            decl_params.get(&decl).map_or(&[][..], Vec::as_slice),
        )
        .defaults
    } else {
        let params = decl_params.get(&decl).map_or(&[][..], Vec::as_slice);
        if tree.args.len() != params.len() {
            return None;
        }
        let mut args = Vec::with_capacity(tree.args.len());
        for arg in &tree.args {
            args.push(clause_arg_ty(table, registry, decl_params, facts, scope, arg)?);
        }
        args
    };
    Some(table.intern(UninternedType::Interface { decl, args }, tree.nullable))
}

/// Instantiates a registered declaration to its parameter bounds —
/// `List` reads `List<dynamic>` while the snapshot carries no bounds, a
/// bounded head would read its bound. `None` for an undeclared decl (the
/// registry and `decl_params` are built together, so a miss is the bail).
fn instantiate_to_bounds(
    table: &TypeTable,
    facts: &ParamFactsTable,
    decl_params: &FxHashMap<TypeDeclId, Vec<TypeParamId>>,
    decl: TypeDeclId,
) -> Option<TypeId> {
    let params = decl_params.get(&decl)?;
    if params.is_empty() {
        return Some(
            table.intern(
                UninternedType::Interface {
                    decl,
                    args: Vec::new(),
                },
                false,
            ),
        );
    }
    let instantiation = instantiate_type_formals_to_bounds(table, facts, params);
    Some(table.intern(
        UninternedType::Interface {
            decl,
            args: instantiation.defaults,
        },
        false,
    ))
}

/// The adapter: `dartq_flow`'s `Ty`/`ArgTy` spellings mapped onto the
/// snapshot's interned `TypeId`s, [`TypeOps`] answered through
/// [`TypeSystem`]. One instance per `fold_unit` call — the `name_ty`
/// cache keys on the unit recorder's interned ids, which are only
/// meaningful inside that unit's spelling universe.
pub struct ResolvedTypeOps<'a> {
    snapshot: &'a LibraryTypes,
    strings: &'a StringTable,
    system: TypeSystem<'a>,
    /// `name_id → (non-null TypeId, nullable TypeId)`, lazily extended:
    /// `is_always_exhaustive` is the only op carrying the recorder's
    /// `names` table, so each probe's `names` slice grows the cache in
    /// one pass (the recorder's table never shrinks — a cached binding
    /// stays valid). A name the registry misses caches the `invalid`
    /// pair — the base ops read `None` only for an *unobserved* id and
    /// fall back to `NameTypeOps`; an observed-but-unresolved name
    /// answers `Invalid` (the suppress face), matching today's
    /// unknown-is-top posture.
    name_ty: RefCell<Vec<(TypeId, TypeId)>>,
    /// `TypeDeclId` → memoized "sealed or enum" answer — the
    /// `is_always_exhaustive` per-decl verdict, stable across the
    /// corpus's switch-end probes.
    closed: RefCell<FxHashMap<TypeDeclId, bool>>,
    /// `(name, written arity)` → its opaque declaration. The import
    /// scopes resolve the spelling (it landed in `non_containers`), but
    /// no element model carries its class — a `dart:` SDK class outside
    /// the builtin table (`Uint8List`, `File`), an unanalyzed dependency.
    /// One synthetic declaration per spelling and written arity, disjoint
    /// from every registered one, so the type questions answer "unrelated"
    /// (the oracle's resolved-class posture) instead of collapsing through
    /// `Invalid`'s vacuous arms; the written arguments ride along on the
    /// `Interface`, so `Stream<List<int>>` and `Stream<Uint8List>` read as
    /// two instantiations of one opaque `Stream` (covariance decides)
    /// rather than one shared argument-less declaration (#2288's spurious
    /// `is`-check dead rows).
    opaque: RefCell<FxHashMap<(StringId, usize), TypeDeclId>>,
    dynamic: TypeId,
    null: TypeId,
    void: TypeId,
    invalid: TypeId,
}

impl<'a> ResolvedTypeOps<'a> {
    /// Mounts the adapter over `snapshot`: `strings` is the shared name
    /// table the recorder's lexemes intern into (the same one the
    /// snapshot's registry keyed its `StringId`s on).
    pub fn new(snapshot: &'a LibraryTypes, strings: &'a StringTable) -> Self {
        let table = &snapshot.table;
        Self {
            snapshot,
            strings,
            system: TypeSystem::new(&snapshot.table, &snapshot.hierarchy),
            name_ty: RefCell::new(Vec::new()),
            closed: RefCell::new(FxHashMap::default()),
            opaque: RefCell::new(FxHashMap::default()),
            dynamic: table.intern(UninternedType::Dynamic, false),
            null: table.intern(UninternedType::Null, false),
            void: table.intern(UninternedType::Void, false),
            invalid: table.intern(UninternedType::Invalid, false),
        }
    }

    /// The opaque declaration of one resolved-but-unmodeled name at one
    /// written arity, minted once per key in a raw range no registered
    /// declaration can reach (the registry's counter starts at
    /// [`FIRST_CONTAINER_RAW`] and grows by declaration count; an analyze
    /// would need 2^30 containers to collide). The arity is part of the
    /// key because a declaration's type-parameter count is part of its
    /// identity: two spellings of one name with different written arities
    /// are not instantiations of the same class.
    fn opaque_decl(&self, name: StringId, arity: usize) -> TypeDeclId {
        let mut cache = self.opaque.borrow_mut();
        if let Some(&decl) = cache.get(&(name, arity)) {
            return decl;
        }
        let decl = TypeDeclId::from_raw(FIRST_OPAQUE_RAW + cache.len() as u32);
        cache.insert((name, arity), decl);
        decl
    }

    /// The opaque `Interface` for `name` with its written `args` and
    /// nullability face — the single mint every map arm shares, so a name
    /// reads as the same declaration at the base, scrutinee and nested
    /// argument faces.
    fn opaque_ty(&self, name: StringId, args: Vec<TypeId>, nullable: bool) -> TypeId {
        let decl = self.opaque_decl(name, args.len());
        self.snapshot
            .table
            .intern(UninternedType::Interface { decl, args }, nullable)
    }

    /// Extends the `name_id → (non-null, nullable)` binding through
    /// `names` — one forward scan covering the ids seen since the last
    /// call. A `Base::Name` spelling whose decl is registered maps to its
    /// to-bounds interface (interned once per nullability face); a
    /// type-parameter spelling maps to its synthetic [`TypeParameter`]
    /// id; `FutureOr` maps to `FutureOr<dynamic>` (the raw-type face
    /// `map_future_or`'s argument-less arm produces); a
    /// resolved-but-unmodeled spelling (`non_containers`) caches its
    /// argument-less opaque pair; everything else (a typedef, a local
    /// function, a synthetic `#<handle>` token, an unindexed import)
    /// caches the `Invalid` pair.
    fn observe_names(&self, names: &[Box<str>]) {
        let mut cache = self.name_ty.borrow_mut();
        let start = cache.len();
        if names.len() <= start {
            return;
        }
        cache.reserve(names.len() - start);
        for lexeme in &names[start..] {
            let name = self.strings.intern(lexeme);
            let pair = if let Some(&param) = self.snapshot.type_params.get(&name) {
                (
                    self.snapshot.table.intern(
                        UninternedType::TypeParameter { parameter: param },
                        false,
                    ),
                    self.snapshot.table.intern(
                        UninternedType::TypeParameter { parameter: param },
                        true,
                    ),
                )
            } else if lexeme.as_ref() == "FutureOr" {
                let non_null = self.snapshot.table.intern(
                    UninternedType::FutureOr { base: self.dynamic },
                    false,
                );
                let nullable = self.snapshot.table.intern(
                    UninternedType::FutureOr { base: self.dynamic },
                    true,
                );
                (non_null, nullable)
            } else {
                self.snapshot
                    .registry
                    .get(&name)
                    .filter(|_| !self.snapshot.non_containers.contains(&name))
                    .and_then(|&decl| {
                        let params = self
                            .snapshot
                            .decl_params
                            .get(&decl)
                            .map_or(&[][..], Vec::as_slice);
                        let args = if params.is_empty() {
                            Vec::new()
                        } else {
                            instantiate_type_formals_to_bounds(
                                &self.snapshot.table,
                                &self.snapshot.facts,
                                params,
                            )
                            .defaults
                        };
                        Some((decl, args))
                    })
                    .map_or_else(
                        || {
                            self.builtin_spelling(lexeme, false).map_or_else(
                                || self.observed_opaque_pair(name),
                                |non_null| {
                                    (
                                        non_null,
                                        self.builtin_spelling(lexeme, true)
                                            .unwrap_or(non_null),
                                    )
                                },
                            )
                        },
                        |(decl, args)| {
                            let non_null = self.snapshot.table.intern(
                                UninternedType::Interface {
                                    decl,
                                    args: args.clone(),
                                },
                                false,
                            );
                            let nullable = self.snapshot.table.intern(
                                UninternedType::Interface { decl, args },
                                true,
                            );
                            (non_null, nullable)
                        },
                    )
            };
            cache.push(pair);
        }
    }

    /// The observed `(non-null, nullable)` pair of a name the registry
    /// and the builtin table both miss: a resolved-but-unmodeled spelling
    /// (`non_containers`) mints its argument-less opaque face, so the base
    /// ops answer "unrelated" exactly like `map_scrutinee`'s arm, and
    /// every other miss is the oracle's `InvalidType`. A typedef name
    /// first chases its captured `Ty::Named` head — `typedef A = B` reads
    /// the `B` pair, so `is_subtype(A, B)` answers through the real
    /// `B <: B` edge rather than the opaque face's "unrelated" arm
    /// (the oracle resolves the alias wholesale: `C c = a` over
    /// `A a` still reports `invalid_assignment`).
    fn observed_opaque_pair(&self, name: StringId) -> (TypeId, TypeId) {
        if !self.snapshot.non_containers.contains(&name) {
            return (self.invalid, self.invalid);
        }
        let (chased, chased_nullable) = self.chase_typedef(name);
        if chased != name && !self.snapshot.non_containers.contains(&chased) {
            // The chase landed on a registered container or builtin:
            // intern the same `Interface`/builtin pair the `registry`
            // arm above produces, nullability composed. A terminal name
            // that declares nowhere answers the `Invalid` pair.
            let lexeme = self.strings.resolve(chased);
            if let Some(&decl) = self.snapshot.registry.get(&chased) {
                let params = self
                    .snapshot
                    .decl_params
                    .get(&decl)
                    .map_or(&[][..], Vec::as_slice);
                let args = if params.is_empty() {
                    Vec::new()
                } else {
                    instantiate_type_formals_to_bounds(
                        &self.snapshot.table,
                        &self.snapshot.facts,
                        params,
                    )
                    .defaults
                };
                return (
                    self.snapshot.table.intern(
                        UninternedType::Interface {
                            decl,
                            args: args.clone(),
                        },
                        chased_nullable,
                    ),
                    self.snapshot
                        .table
                        .intern(UninternedType::Interface { decl, args }, true),
                );
            }
            if let Some(non_null) = self.builtin_spelling(lexeme, chased_nullable) {
                let nullable = self
                    .builtin_spelling(lexeme, true)
                    .unwrap_or(non_null);
                return (non_null, nullable);
            }
            return (self.invalid, self.invalid);
        }
        (
            self.opaque_ty(chased, Vec::new(), chased_nullable),
            self.opaque_ty(chased, Vec::new(), true),
        )
    }

    /// The bare-`Named` typedef chase shared by every name-resolution
    /// arm: walks `typedef_targets` from `name` while each hop stays
    /// inside `non_containers` (a terminal name that resolves to a
    /// container or builtin leaves the set and the caller re-enters the
    /// normal precedence on it). Returns the terminal name plus the
    /// accumulated `?` flag — each hop's own `nullable` composes into
    /// the alias's face exactly like the oracle's `A?`-for-`B?` read.
    /// Bounded by `typedef_targets.len()` — a cyclic chain
    /// (`typedef A = B; typedef B = A;`) exhausts the bound and the
    /// terminal name keeps the opaque face rather than looping.
    fn chase_typedef(&self, name: StringId) -> (StringId, bool) {
        let mut chased = name;
        let mut nullable = false;
        for _ in 0..=self.snapshot.typedef_targets.len() {
            let Some(&(rhs_name, rhs_nullable)) = self.snapshot.typedef_targets.get(&chased)
            else {
                break;
            };
            chased = rhs_name;
            nullable |= rhs_nullable;
            if !self.snapshot.non_containers.contains(&chased) {
                break;
            }
        }
        (chased, nullable)
    }

    /// One written name's resolved mapping — the shared core of
    /// [`ResolvedTypeOps::map_scrutinee`] (a probe/typed-op `Base::Name`
    /// with its own written argument list) and
    /// [`ResolvedTypeOps::map_node`] (a nested argument tree). Precedence
    /// mirrors the official scope chain: a type parameter, the
    /// `FutureOr` pseudo type, a registered container, the
    /// `Null`/`Never` kind spellings, the resolved-but-unmodeled opaque
    /// declaration, and finally `Invalid`.
    fn map_name(
        &self,
        lexeme: &str,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
        nullable: bool,
    ) -> TypeId {
        let name = self.strings.intern(lexeme);
        if let Some(&param) = self.snapshot.type_params.get(&name) {
            return self
                .snapshot
                .table
                .intern(UninternedType::TypeParameter { parameter: param }, nullable);
        }
        if lexeme == "FutureOr" {
            return self.map_future_or(args, arena, names, nullable);
        }
        let registered = self
            .snapshot
            .registry
            .get(&name)
            .filter(|_| !self.snapshot.non_containers.contains(&name));
        if let Some(&decl) = registered {
            let Some(mapped_args) = self.map_args(args, arena, names, decl) else {
                return self.invalid;
            };
            return self
                .snapshot
                .table
                .intern(UninternedType::Interface { decl, args: mapped_args }, nullable);
        }
        if let Some(builtin) = self.builtin_spelling(lexeme, nullable) {
            return builtin;
        }
        if self.snapshot.non_containers.contains(&name) {
            // A bare `typedef A = B` chases its aliased head before the
            // opaque face — the oracle resolves `A` to `B` wholesale
            // (`B f(A a) => a` is silent, `C c = a` still reports
            // `invalid_assignment`). Argument-carrying and non-Named RHS
            // shapes are not modeled here and fall through to opaque
            // (the under-report direction).
            let (chased, chased_nullable) = self.chase_typedef(name);
            let nullable = nullable || chased_nullable;
            if chased != name && !self.snapshot.non_containers.contains(&chased) {
                // The chase left `non_containers` — the terminal resolves
                // through the full precedence chain (a container, a
                // builtin spelling, or `Invalid` when it declares
                // nowhere).
                return self.map_name(
                    self.strings.resolve(chased),
                    args,
                    arena,
                    names,
                    nullable,
                );
            }
            let written = arena.get(args as usize).map_or(&[][..], Vec::as_slice);
            let mapped = self.opaque_args(written, arena, names);
            return self.opaque_ty(chased, mapped, nullable);
        }
        self.invalid
    }

    /// The written argument list of an opaque instantiation: each node
    /// maps through [`ResolvedTypeOps::map_node`], and an `Invalid`
    /// argument reads `dynamic` — the top instantiation the official
    /// `instantiateToBounds` substitutes for an unconstrained parameter —
    /// so an unresolved nested name keeps the enclosing opaque
    /// `Interface` comparable instead of suppressing it through
    /// `map_args`'s arity veto. (Registered containers keep the `Invalid`
    /// argument itself — [`ResolvedTypeOps::map_args`] —; the opaque face
    /// has no real declaration behind it, so its instantiation has
    /// nothing for the argument to sit in.) Without that substitution the
    /// residual `Stream<List<int>>` vs `Stream<Uint8List>` comparison
    /// would read both arguments as `Invalid` and answer "same type".
    fn opaque_args(
        &self,
        written: &[ArgTy],
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Vec<TypeId> {
        written
            .iter()
            .map(|arg| {
                let ty = self.map_node(arg, arena, names);
                match self.snapshot.table.get(ty).kind() {
                    TypeKind::Invalid => self.dynamic,
                    _ => ty,
                }
            })
            .collect()
    }

    /// `Null` and `Never` spelled as types: kind-level spellings the
    /// registry cannot carry (they are not declarations) but the oracle's
    /// scope chain resolves from dart:core — `x is Null` keeps its else
    /// (`T <: Null` only for `Null`), where an `Invalid` read would
    /// vacuously dead it under the 0c arm. A user-declared `class Null`
    /// still shadows: the caller asks only on a registry miss, the same
    /// precedence `bounds.rs`'s `shadowed` check keeps.
    fn builtin_spelling(&self, lexeme: &str, nullable: bool) -> Option<TypeId> {
        match lexeme {
            "Null" => Some(self.null),
            "Never" => Some(
                self.snapshot
                    .table
                    .intern(UninternedType::Never, nullable),
            ),
            _ => None,
        }
    }

    /// The `name_id → (non-null, nullable)` map the base ops read:
    /// `Some` when the id was observed (a `names` table reached it),
    /// `None` while still unobserved — a `var`-inferred or
    /// never-switched spelling answers through the `NameTypeOps`
    /// fallback, preserving the pre-mount behavior exactly.
    fn mapped_name(&self, id: u32) -> Option<(TypeId, TypeId)> {
        self.name_ty.borrow().get(id as usize).copied()
    }

    /// Maps a `Ty` to a `TypeId` for the base ops — the `names`-free
    /// arm. `Base::Name` resolves through the observed cache;
    /// `None` where the name is unobserved means the caller falls back
    /// to `NameTypeOps`.
    fn map_base(&self, ty: FlowTy) -> Option<TypeId> {
        match ty.base {
            Base::Dynamic => Some(self.dynamic),
            Base::Void => Some(self.void),
            Base::Never => Some(
                self.snapshot
                    .table
                    .intern(UninternedType::Never, ty.nullable),
            ),
            Base::Null => Some(self.null),
            // The unknown type is the asymmetric top `dynamic` at the
            // resolved face exactly as at the name level
            // (`NameTypeOps::is_subtype`: everything <: unknown, unknown
            // <: only tops) — `Invalid` would make every question
            // vacuously true (subtype.dart's 0c arm), deading the `else`
            // branch of `var x; x is int` and vetoing every `var`
            // promotion.
            Base::Unknown => Some(self.dynamic),
            Base::Function => Some(
                self.snapshot
                    .table
                    .intern(UninternedType::Interface {
                        decl: self.snapshot.hierarchy.core().function,
                        args: Vec::new(),
                    }, ty.nullable),
            ),
            Base::Name(id) => {
                let (non_null, nullable) = self.mapped_name(id)?;
                Some(if ty.nullable { nullable } else { non_null })
            }
        }
    }

    /// Maps one `ArgTy` node (recursion for `FutureOr`); `names` and
    /// `arena` are the live faces `is_always_exhaustive` carries. A node
    /// whose name resolves to nothing maps `invalid` — the argument the
    /// enclosing `Interface` arm carries in place of a resolvable type.
    fn map_node(&self, node: &ArgTy, arena: &[Vec<ArgTy>], names: &[Box<str>]) -> TypeId {
        match node.base {
            Base::Dynamic => self.dynamic,
            Base::Void => self.void,
            Base::Never => self
                .snapshot
                .table
                .intern(UninternedType::Never, node.nullable),
            Base::Null => self.null,
            // As in `map_base`: the asymmetric top, not `Invalid` — the
            // unknown argument's true answer is `dynamic` (a `var`-inferred
            // or unreduced argument reads as the top instantiation), and a
            // resolved `Invalid` argument would vacuum every subtype
            // question the instantiated face enters (subtype.dart's 0c
            // arm).
            Base::Unknown => self.dynamic,
            Base::Function => self
                .snapshot
                .table
                .intern(UninternedType::Interface {
                    decl: self.snapshot.hierarchy.core().function,
                    args: Vec::new(),
                }, node.nullable),
            Base::Name(id) => {
                let Some(lexeme) = names.get(id as usize) else {
                    return self.invalid;
                };
                // A parameter named `FutureOr` shadows the core type — the
                // same precedence `observe_names` uses (inside `map_name`).
                self.map_name(lexeme, node.args, arena, names, node.nullable)
            }
        }
    }

    /// `FutureOr<T>`: the tree arm `is_always_exhaustive` recurses into.
    /// A bare `FutureOr` (no args) reads `FutureOr<dynamic>` — the
    /// raw-type face every unparameterized generic takes.
    fn map_future_or(
        &self,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
        nullable: bool,
    ) -> TypeId {
        let base = match arena.get(args as usize).map(Vec::as_slice) {
            Some([arg]) => self.map_node(arg, arena, names),
            _ => self.dynamic,
        };
        self.snapshot
            .table
            .intern(UninternedType::FutureOr { base }, nullable)
    }

    /// The `Interface` arm's written argument list; an argument-less
    /// head instantiates to bounds (a bare `List` reads `List<dynamic>`
    /// — the official `instantiateToBounds` on a resolved element).
    /// `None` on arity mismatch (the written count disagrees with the
    /// declaration's) — the whole `Interface` suppresses through
    /// `Invalid`. An `Invalid` argument is *not* a veto: it keeps its
    /// position inside the instantiation, exactly the oracle's
    /// `Response<InvalidType>` — still an `InterfaceType` carrying the
    /// invalid argument, whose subtype questions answer through the 0c
    /// arm instead of erasing the whole face.
    fn map_args(
        &self,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
        decl: TypeDeclId,
    ) -> Option<Vec<TypeId>> {
        let params = self
            .snapshot
            .decl_params
            .get(&decl)
            .map_or(&[][..], Vec::as_slice);
        let row = arena.get(args as usize).map_or(&[][..], Vec::as_slice);
        if row.is_empty() {
            let ty = instantiate_to_bounds(
                &self.snapshot.table,
                &self.snapshot.facts,
                &self.snapshot.decl_params,
                decl,
            )?;
            return match self.snapshot.table.get(ty).kind() {
                TypeKind::Interface { args, .. } => Some(self.snapshot.table.args(args)),
                _ => Some(Vec::new()),
            };
        }
        if row.len() != params.len() {
            return None;
        }
        Some(
            row.iter()
                .map(|arg| self.map_node(arg, arena, names))
                .collect(),
        )
    }

    /// The scrutinee map `is_always_exhaustive` uses: `Base::Name`
    /// resolves through `names` (fresh every probe — a new spelling
    /// never needs a prior observation), everything else through the
    /// base map.
    fn map_scrutinee(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> TypeId {
        match ty.base {
            Base::Name(id) => {
                let Some(lexeme) = names.get(id as usize) else {
                    return self.invalid;
                };
                // The full chain — including the resolved-but-unmodeled
                // opaque face — is shared with `map_node`, so a name reads
                // as the same declaration at every depth.
                self.map_name(lexeme, args, arena, names, ty.nullable)
            }
            _ => self.map_base(ty).unwrap_or(self.invalid),
        }
    }

    /// The is-test face's tested-side map: [`Self::map_scrutinee`], except
    /// an arity-vetoed registered spelling recovers to its declared
    /// interface carrying one `Invalid` per type parameter. The official
    /// builds that exact shape for a wrong-arity type annotation —
    /// `_buildTypeArguments` (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart`)
    /// reports `wrong_number_of_type_arguments` and fills `InvalidType`
    /// into every parameter position, so the checked type is
    /// `Response<InvalidType>`, still an `InterfaceType`, NOT the
    /// whole-face `InvalidType`: `Object is Response<int, int>` answers
    /// `Object <: Response<Invalid>` = false and keeps both branches live,
    /// while `Response<int> is Response<int, int>` admits through the
    /// carried argument's 0c arm and deads the else — both
    /// oracle-probed (dart analyze 3.13.1). The bare `Invalid` read
    /// instead answered vacuously true in both directions (subtype.dart's
    /// 0c arm) and deaded a live else — the residual this recovery
    /// removes (#2370). The pattern faces keep the whole-face `Invalid`
    /// posture: `is_invalid_typed` reads `map_scrutinee` directly, and a
    /// wrong-arity pattern skips its type test exactly as before.
    ///
    /// Only the registered-container spelling recovers. An unresolved head
    /// (`x is Missing`) stays the whole-face `Invalid` whose 0c arm deads
    /// the else exactly as the analyzer reports it, and a
    /// resolved-but-unmodeled name keeps its opaque face; a bare spelling
    /// instantiates to bounds instead of vetoing, so `row.is_empty()` and
    /// the equal-count case never reach the recovery.
    fn map_tested(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> TypeId {
        let mapped = self.map_scrutinee(ty, args, arena, names);
        if !matches!(self.snapshot.table.get(mapped).kind(), TypeKind::Invalid) {
            return mapped;
        }
        let Base::Name(id) = ty.base else {
            return mapped;
        };
        let Some(lexeme) = names.get(id as usize) else {
            return mapped;
        };
        let name = self.strings.intern(lexeme);
        let Some(&decl) = self
            .snapshot
            .registry
            .get(&name)
            .filter(|_| !self.snapshot.non_containers.contains(&name))
        else {
            return mapped;
        };
        let params = self
            .snapshot
            .decl_params
            .get(&decl)
            .map_or(&[][..], Vec::as_slice);
        let row = arena.get(args as usize).map_or(&[][..], Vec::as_slice);
        if row.is_empty() || row.len() == params.len() {
            return mapped;
        }
        self.snapshot.table.intern(
            UninternedType::Interface {
                decl,
                args: vec![self.invalid; params.len()],
            },
            ty.nullable,
        )
    }
}

impl TypeOps for ResolvedTypeOps<'_> {
    /// `is_subtype` over the resolved world where both sides map; either
    /// side unmapped answers through [`NameTypeOps`] — today's name-level
    /// logic — so a `var`-inferred or unresolvable spelling keeps its
    /// pre-mount answer and a resolved pair gains the real hierarchy
    /// (`Resource` <: `Object`, `File` <: `Resource` cross-file).
    fn is_subtype(&self, left: FlowTy, right: FlowTy) -> bool {
        match (self.map_base(left), self.map_base(right)) {
            (Some(l), Some(r)) => self.system.is_subtype(l, r),
            _ => NameTypeOps.is_subtype(left, right),
        }
    }

    /// `factor`: the remainder of `from` after `is what` removes it.
    /// Mapped arms answer through `TypeSystem::is_subtype` plus the
    /// name-level remainder shape; an unmapped arm falls back to
    /// `NameTypeOps::factor` verbatim — the pre-mount answer.
    fn factor(&self, from: FlowTy, what: FlowTy) -> FlowTy {
        match (self.map_base(from), self.map_base(what)) {
            (Some(f), Some(w)) if self.system.is_subtype(f, w) => FlowTy::NEVER,
            (Some(f), Some(w)) => {
                let _ = (f, w);
                // Rule 2 of factorType: `R?` factors as
                // `factor(R, what)` when `Null <: what` — the resolved
                // face answers the null-half question through
                // `is_subtype(Null, what)`.
                if from.nullable
                    && !matches!(
                        from.base,
                        Base::Dynamic | Base::Unknown | Base::Void | Base::Null | Base::Never
                    )
                {
                    if self.system.is_subtype(self.null, w) {
                        return self.factor(from.non_null(), what);
                    }
                    return self.factor(from.non_null(), what).make_nullable();
                }
                from
            }
            _ => NameTypeOps.factor(from, what),
        }
    }

    /// `tryPromoteToType`: the strict-subtype gate verbatim
    /// (`isValidPromotionStep`, flow_analysis.dart:7229-7247) — `to <:
    /// from` and not the reverse (mutual subtypes, identical types
    /// included, are no step). Phase D tightened this from the bare
    /// `is_subtype(t, f)`-else-fallback shape: the model.rs migration
    /// moved every production caller to the typed seam, and the
    /// `promoted_is` dedup in `add_promoted` already stands down on
    /// `to == from`, so no consumer needed the non-strict answer.
    /// Unmapped arms keep `NameTypeOps`'s pre-mount posture.
    fn try_promote_to(&self, to: FlowTy, from: FlowTy) -> Option<FlowTy> {
        if let (Some(t), Some(f)) = (self.map_base(to), self.map_base(from)) {
            return (self.system.is_subtype(t, f) && !self.system.is_subtype(f, t))
                .then_some(to);
        }
        NameTypeOps.try_promote_to(to, from)
    }

    /// `promoteToNonNull`: strip `?` / `Null`→`Never` / `dynamic` stays —
    /// resolved where the name maps, name-level otherwise.
    fn promote_to_non_null(&self, t: FlowTy) -> FlowTy {
        if let Some(mapped) = self.map_base(t) {
            return match self.system.is_bottom(mapped) {
                true => FlowTy::NEVER,
                false if self.system.is_nullable(mapped) => t.non_null(),
                false => t,
            };
        }
        NameTypeOps.promote_to_non_null(t)
    }

    /// `classifyType`: the resolved face answers through `TypeSystem`'s
    /// nullability predicates; an unmapped arm keeps `NameTypeOps`'s
    /// syntactic answer.
    fn classify(&self, t: FlowTy) -> Classification {
        match self.map_base(t) {
            Some(mapped) if self.system.is_null(mapped) => Classification::NullOrEquivalent,
            Some(mapped) if self.system.is_nullable(mapped) => {
                Classification::PotentiallyNullable
            }
            Some(_) => Classification::NonNullable,
            None => NameTypeOps.classify(t),
        }
    }

    /// `is_bottom`: resolved `Never`/`Null`/`Invalid` answers through
    /// `TypeSystem::is_bottom`; an unmapped arm keeps the name-level
    /// `Never` check.
    fn is_bottom(&self, t: FlowTy) -> bool {
        match self.map_base(t) {
            Some(mapped) => self.system.is_bottom(mapped),
            None => NameTypeOps.is_bottom(t),
        }
    }

    /// `TypeSystem.isAlwaysExhaustive` (type_system.dart:846-888) over
    /// the resolved hierarchy: `Null`/`bool` split completely, enums and
    /// sealed classes close by their element faces, `FutureOr<T>`
    /// recurses into `T`. A `Base::Name` spelling resolves through
    /// `names` (populating the `name_ty` cache for the base ops); an
    /// unresolvable spelling is the under-report `false`.
    fn is_always_exhaustive(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        self.observe_names(names);
        let mapped = self.map_scrutinee(ty, args, arena, names);
        let is_closed = |decl: TypeDeclId| -> bool {
            *self.closed.borrow_mut().entry(decl).or_insert_with(|| {
                self.snapshot.hierarchy.is_sealed(decl)
                    || self.snapshot.hierarchy.enum_constant_count(decl).is_some()
            })
        };
        match self.snapshot.table.get(mapped).kind() {
            // `Null` splits completely — the official's `isDartCoreNull`
            // arm. `Never?` interned as `Null` lands here too.
            TypeKind::Null => true,
            TypeKind::Interface { decl, .. } => is_closed(decl),
            TypeKind::FutureOr { base } => match self.snapshot.table.get(base).kind() {
                // `FutureOr<T>` recurses into `T` (type_system.dart:865):
                // a closed base closes the `FutureOr` — `Null` splits
                // completely, a sealed/enum base's decl answers.
                TypeKind::Null => true,
                TypeKind::Interface { decl, .. } => is_closed(decl),
                // `FutureOr<FutureOr<…>>` unrolls one arm deeper than
                // this single-level match reaches — under-report.
                _ => false,
            },
            // `Never`, `dynamic`, `void`, `Invalid`, `Function`,
            // `TypeParameter`, `Record` — the official's undecidable or
            // open arms: `false` keeps the post-switch state live.
            _ => false,
        }
    }

    /// Whether a pattern's written type resolves to `Invalid` (official
    /// `promoteForPattern`'s invalid arm, `flow_analysis.dart:7720-7723`):
    /// the spelling maps through [`ResolvedTypeOps::map_scrutinee`], the
    /// same resolution the typed subtype ops use, so an unresolvable head
    /// and a registered container with a wrong written arity both answer
    /// `true` — the analyzer's `InvalidType` for the pattern's required
    /// type. The flow layer then skips the type test and joins the live
    /// state into the unmatched chain instead of letting
    /// `factor(any, Invalid)` dead the body.
    fn is_invalid_typed(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        self.observe_names(names);
        matches!(
            self.snapshot
                .table
                .get(self.map_scrutinee(ty, args, arena, names))
                .kind(),
            TypeKind::Invalid
        )
    }

    /// The name-observation hook `Interp::run` calls once per replay: a
    /// forward scan extends the `name_ty` bindings to every name the unit
    /// has interned so far, so the base ops resolve through the hierarchy
    /// even when no switch probe runs (#2204 phase B — the ops mounting
    /// alone widened `is_always_exhaustive`; this widens the six base
    /// ops to the same world).
    fn observe_names(&self, names: &[Box<str>]) {
        ResolvedTypeOps::observe_names(self, names);
    }

    /// [`NameTypeOps::factor`] over the instantiated forms: both operand
    /// spellings map with their written arguments (`Response` reads
    /// `Response<dynamic>`, `Response<T>` reads the parameter) and the
    /// remainder verdict answers through [`TypeSystem::is_subtype`] —
    /// `factor(Response, Response<T>)` keeps `Response` where the bare
    /// `Ty`s' name equality would answer `Never` and dead the else
    /// branch (#2186 S4-C). An `Invalid` side answers through
    /// `TypeSystem::is_subtype` like every other kind — the oracle's 0c
    /// arm (subtype.dart:274-276) makes `X <: InvalidType` and
    /// `InvalidType <: X` vacuously true, so `factor(T, InvalidType)` is
    /// `Never` and the `x is Missing` else branch deads exactly as the
    /// analyzer reports it (the broken-import corpus's dominant
    /// dead_code root); a name the registry misses AND the import scopes
    /// cannot resolve IS the oracle's `InvalidType` — a name the scopes
    /// do resolve (`non_containers`) maps opaque instead
    /// ([`ResolvedTypeOps::opaque_decl`]), keeping the remainder live
    /// like the oracle's resolved class. The tested side's arity veto
    /// recovers instead ([`Self::map_tested`]) — the official's
    /// `Response<InvalidType>` keeps the test decidable.
    fn factor_typed(
        &self,
        from: FlowTy,
        from_args: u32,
        what: FlowTy,
        what_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> FlowTy {
        self.observe_names(names);
        let f = self.map_scrutinee(from, from_args, arena, names);
        let w = self.map_tested(what, what_args, arena, names);
        // The signature-less function pair (see `is_subtype_typed`) skips
        // only the `Never` shortcut — no relation is provable between two
        // unknown signatures. The nullability rule below still splits the
        // `Null` half (`factor(Sig?, Sig2)` keeps `Sig?`), and its
        // recursion re-enters past this point with the non-null face,
        // keeping the unchanged `from` the official factor's fallthrough
        // keeps.
        let unsignatured_pair = from.base == Base::Function && what.base == Base::Function;
        if !unsignatured_pair && self.system.is_subtype(f, w) {
            return FlowTy::NEVER;
        }
        // Rule 2 of factorType (factor_type.dart:25-31): `R?` factors as
        // `factor(R, what)` when `Null <: what`, else `factor(R, what)?`
        // — the resolved face answers the null-half question through
        // `is_subtype(Null, what)` exactly like the arg-less arm.
        if from.nullable
            && !matches!(
                from.base,
                Base::Dynamic | Base::Unknown | Base::Void | Base::Null | Base::Never
            )
        {
            if self.system.is_subtype(self.null, w) {
                return self.factor_typed(from.non_null(), from_args, what, what_args, arena, names);
            }
            return self
                .factor_typed(from.non_null(), from_args, what, what_args, arena, names)
                .make_nullable();
        }
        from
    }

    /// `is_subtype` over the instantiated spellings: both sides map
    /// through `map_scrutinee` (the written-argument face — `List<int>`
    /// reads `Interface{List, [int]}`, a bare `List` its to-bounds
    /// instantiation) and the verdict answers through
    /// [`TypeSystem::is_subtype`]. An `Invalid` arm (an unresolved head)
    /// answers through the same call — subtype.dart's 0c arm makes it a
    /// vacuous `true` in both directions, the oracle's own `InvalidType`
    /// verdict, so the resolved face reports exactly what the analyzer
    /// reports for a name it cannot resolve. The tested side's arity veto
    /// recovers instead ([`Self::map_tested`]) — the official's
    /// `Response<InvalidType>` keeps the question decidable (#2370).
    fn is_subtype_typed(
        &self,
        left: FlowTy,
        left_args: u32,
        right: FlowTy,
        right_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        self.observe_names(names);
        // Two signature-less function spellings: the flow layer records
        // every function type as the bare `Base::Function` head (no
        // signature — ev.rs's `FunctionType` carries the nullability
        // only), so the pair may spell two different signatures — a
        // `FutureOr<R> Function(T)` operand tested `is R Function(T)`
        // (drift async.dart) — between which no relation is provable.
        // Mapping both to the bare `Function` class answers `true` on
        // spelling identity: the vacuous-true that deads a live else
        // branch and reports a false constant-true test. The official
        // never sees this shape — its operands resolve to real signatures
        // and subtype.dart's Function Type/Function rule decides
        // (subtype.dart:502-510, return covariant / formals contravariant
        // via `_isFunctionSubtypeOf`); a bare `Function` left side has no
        // admitting arm at all. At this seam nothing is provable, so
        // `false` keeps both branches live — the under-report direction.
        if left.base == Base::Function && right.base == Base::Function {
            return false;
        }
        let l = self.map_scrutinee(left, left_args, arena, names);
        let r = self.map_tested(right, right_args, arena, names);
        self.system.is_subtype(l, r)
    }

    /// `tryPromoteToType` over the instantiated spellings — the oracle
    /// gate verbatim: `Some(to)` exactly when `to <: from`
    /// (type_system.dart:1739-1741), `None` otherwise. Both sides map
    /// with their written arguments, so a strict narrowing
    /// (`x is Response<T>` after `x is Response` — `Response<T> <:
    /// Response<dynamic>` holds) promotes, where the bare-`Ty` coverage
    /// test would veto it. An `Invalid` arm (an unresolved head) answers
    /// through the same gate — subtype.dart's 0c arm makes both
    /// directions vacuously true, so the strict gate refuses the step
    /// exactly as the oracle's `isValidPromotionStep` refuses an
    /// `InvalidType` promotion (mutual subtypes are no step). The tested
    /// side's arity veto recovers instead ([`Self::map_tested`]) — the
    /// official promotes to its `Response<InvalidType>` face (probed:
    /// `x.data` reads in the then branch, dart analyze 3.13.1).
    fn try_promote_to_typed(
        &self,
        to: FlowTy,
        to_args: u32,
        from: FlowTy,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<FlowTy> {
        self.observe_names(names);
        let t = self.map_tested(to, to_args, arena, names);
        let f = self.map_scrutinee(from, from_args, arena, names);
        // The strict-subtype gate (`isValidPromotionStep`,
        // flow_analysis.dart:7229-7247): `to <: from` is required, and a
        // `to` the current type already satisfies (`from <: to` —
        // identical types included, `x is int` on `int x`) is no step.
        if self.system.is_subtype(t, f) && !self.system.is_subtype(f, t) {
            Some(to)
        } else {
            None
        }
    }

    /// The downward-inferred object-pattern required type: a RAW pattern
    /// spelling solves its own parameters along its superclass chain (the
    /// official `analyzeObjectPattern` inference — shared
    /// type_analyzer.dart:1749-1772 — with the matched value's static type
    /// as the context). `case AL():` over `AV<int>` finds the hop
    /// `AL<T> extends AV<T>` spelling the matched declaration; the hop's
    /// arguments are the tested declaration's own parameter positions,
    /// and the matched spelling's written arguments substitute them
    /// positionally — so the inferred spelling is `AL<int>`, carried
    /// as `(tested, from_args)`: the hop's parameter positions line up
    /// with the matched arity exactly when the inferred tree IS the
    /// matched tree, and any other hop shape (a partial or compound
    /// parameter mapping) is undecidable here and refuses. An `Invalid`
    /// arm (either head unresolved) refuses the same way — the raw face's
    /// posture stands.
    ///
    /// Dormant under the name-level edge set: the erased hop cannot
    /// decide the inferred spelling a strict-subtype step (the promotion
    /// gate's `is_subtype` walk refuses the dynamic-erased argument), so
    /// the answer changes nothing until the written clause trees land
    /// (the TypedModel migration) — this body is that migration's
    /// consumer, final-form on both faces.
    fn downcast_infer(
        &self,
        tested: FlowTy,
        tested_args: u32,
        from: FlowTy,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<(FlowTy, u32)> {
        self.observe_names(names);
        // Inference is the raw-pattern face only: a written argument list
        // already spells the required type.
        if tested_args != EMPTY_ARGS || from_args == EMPTY_ARGS {
            return None;
        }
        let (Base::Name(tested_id), Base::Name(from_id)) = (tested.base, from.base) else {
            return None;
        };
        let tested_name = self.strings.intern(names.get(tested_id as usize)?);
        let from_name = self.strings.intern(names.get(from_id as usize)?);
        let tested_decl = *self
            .snapshot
            .registry
            .get(&tested_name)
            .filter(|_| !self.snapshot.non_containers.contains(&tested_name))?;
        let from_decl = *self
            .snapshot
            .registry
            .get(&from_name)
            .filter(|_| !self.snapshot.non_containers.contains(&from_name))?;
        // The superclass hop spelling the matched declaration. The
        // declared-ness gate keeps `supertypes`'s panic contract.
        if self.snapshot.hierarchy.decl(tested_decl).is_none() {
            return None;
        }
        let hop = self
            .snapshot
            .hierarchy
            .supertypes(tested_decl)
            .iter()
            .copied()
            .find(|ty| {
                matches!(
                    self.snapshot.table.get(*ty).kind(),
                    TypeKind::Interface { decl, .. } if decl == from_decl
                )
            })?;
        let TypeKind::Interface { args: hop_range, .. } = self.snapshot.table.get(hop).kind()
        else {
            return None;
        };
        let hop_args = self.snapshot.table.args(hop_range);
        // Every hop argument is the tested declaration's own parameter
        // position, in declaration order, and the matched spelling writes
        // exactly that many — the inferred tree is then the matched tree
        // verbatim. The name-level edge set erases every clause argument
        // to `dynamic` today (the written clause trees are the TypedModel
        // migration's transport), so the placeholder face accepts the
        // erased `dynamic` beside the sharpened parameter reference; a
        // sharpened CONCRETE argument (`class AL<T> extends AV<int>`)
        // refuses — that hop constrains nothing and the inference must
        // not read the matched arguments into the pattern's parameters.
        let params = self.snapshot.decl_params.get(&tested_decl)?;
        if params.len() != hop_args.len() {
            return None;
        }
        if !hop_args.iter().zip(params.iter()).all(|(arg, param)| {
            match self.snapshot.table.get(*arg).kind() {
                TypeKind::Dynamic => true,
                TypeKind::TypeParameter { parameter } => parameter == *param,
                _ => false,
            }
        }) {
            return None;
        }
        let written_len = arena.get(from_args as usize).map_or(0, Vec::len);
        if written_len != hop_args.len() {
            return None;
        }
        Some((tested, from_args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use crate::diagnose::{StrictModes, fold_unit_with_ops};
    use dartq_flow::ty::{EMPTY_ARGS, Ty};
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
    };
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::FileTable;
    use std::path::Path;

    /// Scans + collects one unit, interning its names into `strings`.
    /// Mirrors `library.rs`'s `unit` helper — kept private so the pin's
    /// dependency set stays visible.
    fn unit(
        source: &str,
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
    ) -> crate::model::FileElements {
        let file = files.intern(Path::new(path));
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
            LanguageVersion::new(3, 15),
        ));
        collect(source, tokens, features, strings)
    }

    /// The unit's flow-driven diagnostic code names — the same harness
    /// `diagnose/tests.rs` runs, with the ops adapter mounted.
    fn diagnose_codes(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    ) -> Vec<&'static str> {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source:?}");
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        if let Some(edge) = make_edge(&strings) {
            builder.import(lib, edge);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);
        let fold = fold_unit_with_ops(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            Some(&ops),
        );
        fold.report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect()
    }

    /// `diagnose_codes` with `names` declared in an imported library whose
    /// file carries no element model: the names resolve through the
    /// import scope but no declaration backs them, so the adapter files
    /// them under `non_containers` and maps their spellings to opaque
    /// declarations — the exact face a `dart:` SDK class outside the
    /// builtin table presents in the real pipeline.
    fn diagnose_codes_unmodeled(source: &str, names: &[&str]) -> Vec<&'static str> {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let main_file = files.intern(Path::new("main.dart"));
        let other_file = files.intern(Path::new("other.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            main_file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source:?}");
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_other = builder.intern("package:t/other.dart");
        let lib_main = builder.intern("package:t/main.dart");
        builder.unit(lib_other, other_file);
        builder.unit(lib_main, main_file);
        let mut index = ElementIndex::default();
        index.insert(main_file, elements.clone());
        for name in names {
            builder.declare(lib_other, strings.intern(name), other_file);
        }
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_main, name, main_file);
        }
        builder.import(
            lib_main,
            ImportEdge {
                target: Some(lib_other),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib_main);
        let ops = ResolvedTypeOps::new(&library_types, &strings);
        let fold = fold_unit_with_ops(
            source,
            &scanned.tokens,
            features,
            &strings,
            main_file,
            lib_main,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            Some(&ops),
        );
        fold.report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect()
    }

    /// A sealed-scrutinee switch closes: no `body_might_complete_normally`
    /// The raw object pattern's downward-inferred promotion (#2293 rider):
    /// `case AL():` over an `AV<Num>` scrutinee solves `AL`'s own parameter
    /// along the superclass hop `AL<T> extends AV<T>` — the matched arm
    /// promotes to `AL<Num>` (official `analyzeObjectPattern` inference +
    /// `promoteForPattern`) and the clause body's call accepts the promoted
    /// read. The oracle reports nothing.
    ///
    /// Ignored pending the hierarchy sharpening: the inference seam
    /// ([`TypeOps::downcast_infer`] + the `matched_object_pattern`
    /// consumption) is wired, but the name-level edge set erases every
    /// clause argument to `dynamic`, so the promotion gate's subtype walk
    /// refuses the inferred spelling (`AL<Num> <: AV<Num>` walks the
    /// dynamic-erased hop and refuses). The written clause trees are the
    /// TypedModel migration's transport; revive this pin when they land.
    #[ignore = "dormant: erased hierarchy edges refuse the inferred subtype (TypedModel clause-tree transport pending)"]
    #[test]
    fn raw_object_pattern_downcast_promotes() {
        let codes = diagnose_codes(
            "sealed class AV<T> {}\n\
             class AL<T> extends AV<T> {}\n\
             class Num {}\n\
             void h(AL<Num> v) {}\n\
             void f(AV<Num> value) {\n\
             \x20 switch (value) {\n\
             \x20   case AL():\n\
             \x20     h(value);\n\
             \x20 }\n\
             }",
            |_| None,
        );
        assert!(
            codes.is_empty(),
            "raw pattern should downcast-promote: {codes:?}"
        );
    }

    /// The declared-arity control: a pattern that WRITES its arguments
    /// (`case AL<Num>():`) needs no inference — the written spelling is
    /// the required type and the promotion stands as before.
    ///
    /// Same dormancy as [`Self::raw_object_pattern_downcast_promotes`]:
    /// the promotion gate's subtype walk refuses over the dynamic-erased
    /// hop even for the written spelling.
    #[ignore = "dormant: erased hierarchy edges refuse the inferred subtype (TypedModel clause-tree transport pending)"]
    #[test]
    fn written_object_pattern_args_promote() {
        let codes = diagnose_codes(
            "sealed class AV<T> {}\n\
             class AL<T> extends AV<T> {}\n\
             class Num {}\n\
             void h(AL<Num> v) {}\n\
             void f(AV<Num> value) {\n\
             \x20 switch (value) {\n\
             \x20   case AL<Num>():\n\
             \x20     h(value);\n\
             \x20 }\n\
             }",
            |_| None,
        );
        assert!(
            codes.is_empty(),
            "written pattern args should promote: {codes:?}"
        );
    }

    /// The sealed-scrutinee switch stays silent
    /// on the non-void function — the exhaustive arms keep the
    /// post-switch state unreachable, so the missing tail `return` never
    /// reports. The resolved `is_always_exhaustive` answers `true`
    /// through `is_sealed`, where the name-level `ClosedNames` only saw
    /// same-file sealeds that parsed before the executable (the resolved
    /// registry is complete at fold time, so a sealed declared *after*
    /// `f` still closes).
    #[test]
    fn sealed_scrutinee_switch_stays_silent() {
        let codes = diagnose_codes(
            "sealed class S {} \
             class A extends S {} \
             class B extends S {} \
             int f(S s) { \
               switch (s) { case A(): return 0; case B(): return 1; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"body_might_complete_normally"),
            "sealed switch should close: {codes:?}"
        );
        // The sealed decl follows its consumer in source order below —
        // the parse-time `ClosedNames` capture would answer `false`
        // (declaration unseen at record time); the resolved registry
        // answers `true` either way.
        let codes = diagnose_codes(
            "int f(S s) { \
               switch (s) { case A(): return 0; case B(): return 1; } \
             } \
             sealed class S {} \
             class A extends S {} \
             class B extends S {}",
            |_| None,
        );
        assert!(
            !codes.contains(&"body_might_complete_normally"),
            "later-declared sealed switch should still close: {codes:?}"
        );
    }

    /// The non-sealed control: a switch over an open class still reports
    /// `body_might_complete_normally` on the non-void function — the
    /// post-switch state stays live (the case list is incomplete), so the
    /// missing tail `return` reports.
    #[test]
    fn open_scrutinee_switch_still_reports() {
        let codes = diagnose_codes(
            "class S {} \
             class A extends S {} \
             int f(S s) { \
               switch (s) { case A(): return 0; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"body_might_complete_normally"),
            "open switch should keep reporting: {codes:?}"
        );
    }

    /// The archive-corpus DEAD_CODE cascade pins (2026-10-19). A name the
    /// import scopes resolve but no element model carries (a `dart:` SDK
    /// class outside the builtin table) maps to an unrelated opaque
    /// declaration, so an `is`-test against it keeps both branches live —
    /// where the registry-miss `Invalid` read answered every subtype
    /// question vacuously (subtype.dart's 0c arm) and deaded the else.
    ///
    /// The harness names the class in an imported library whose file
    /// carries no element model — the exact `non_containers` face the
    /// synthetic `dart:` nodes produce in the real pipeline.
    #[test]
    fn resolved_but_unmodeled_is_test_keeps_both_branches_live() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let main_file = files.intern(Path::new("main.dart"));
        let other_file = files.intern(Path::new("other.dart"));
        let main_source = "int f(int x) => x is Widget ? 1 : 2;";
        let scanned = dartq_scanner::scan(
            main_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            main_file,
        );
        assert!(scanned.diagnostics.is_empty());
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let main_elements = collect(main_source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_other = builder.intern("package:t/other.dart");
        let lib_main = builder.intern("package:t/main.dart");
        builder.unit(lib_other, other_file);
        builder.unit(lib_main, main_file);
        let mut index = ElementIndex::default();
        index.insert(main_file, main_elements.clone());
        // `Widget` resolves through the import scope, but `other_file`
        // carries no element model — the sweep files the name under
        // `non_containers`.
        builder.declare(lib_other, strings.intern("Widget"), other_file);
        for (name, decl) in main_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_main, name, main_file);
        }
        builder.import(
            lib_main,
            ImportEdge {
                target: Some(lib_other),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib_main);
        let ops = ResolvedTypeOps::new(&library_types, &strings);
        let fold = fold_unit_with_ops(
            main_source,
            &scanned.tokens,
            features,
            &strings,
            main_file,
            lib_main,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            Some(&ops),
        );
        let codes: Vec<&'static str> = fold
            .report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
        assert!(
            !codes.contains(&"dead_code"),
            "an unmodeled-but-resolved is-test keeps the else live: {codes:?}"
        );
    }

    /// The cascade's `else if` face: after a negative `is Shape`
    /// refinement, `is Circle` over the same receiver kept deading the
    /// whole else-if clause. Both tests map opaque now, so neither branch
    /// collapses — the same model-less-import rig as the pin above, with
    /// both names declared in it.
    #[test]
    fn resolved_but_unmodeled_else_if_chain_keeps_the_second_clause() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let main_file = files.intern(Path::new("main.dart"));
        let other_file = files.intern(Path::new("other.dart"));
        let main_source = "int g(e) { \
               if (e is Shape) { return 1; } else if (e is Circle) { return 2; } \
               return 0; \
             }";
        let scanned = dartq_scanner::scan(
            main_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            main_file,
        );
        assert!(scanned.diagnostics.is_empty());
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let main_elements = collect(main_source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_other = builder.intern("package:t/other.dart");
        let lib_main = builder.intern("package:t/main.dart");
        builder.unit(lib_other, other_file);
        builder.unit(lib_main, main_file);
        let mut index = ElementIndex::default();
        index.insert(main_file, main_elements.clone());
        // `Shape`/`Circle` resolve through the import scope, but
        // `other_file` carries no element model — the sweep files both
        // names under `non_containers`.
        builder.declare(lib_other, strings.intern("Shape"), other_file);
        builder.declare(lib_other, strings.intern("Circle"), other_file);
        for (name, decl) in main_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_main, name, main_file);
        }
        builder.import(
            lib_main,
            ImportEdge {
                target: Some(lib_other),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib_main);
        let ops = ResolvedTypeOps::new(&library_types, &strings);
        let fold = fold_unit_with_ops(
            main_source,
            &scanned.tokens,
            features,
            &strings,
            main_file,
            lib_main,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            Some(&ops),
        );
        let codes: Vec<&'static str> = fold
            .report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
        assert!(
            !codes.contains(&"dead_code"),
            "the else-if chain stays live over unmodeled tests: {codes:?}"
        );
    }

    /// #2288: an unmodeled `is`-test compares the written instantiations,
    /// not just the heads. `Stream<List<int>>` and `Stream<Uint8List>` are
    /// two instantiations of one opaque `Stream` (covariance decides), so
    /// the test can fail and the else branch stays live — the dio
    /// `progress_stream/io_progress_stream.dart:13` face. With the written
    /// arguments dropped, both sides shared one argument-less declaration,
    /// the test read as guaranteed true, and the else body deaded.
    #[test]
    fn unmodeled_generic_is_test_distinguishes_written_arguments() {
        let codes = diagnose_codes_unmodeled(
            "int f(Stream<List<int>> stream) { \
               if (stream is Stream<Uint8List>) { return 1; } else { return 2; } \
             }",
            &["Stream", "Uint8List"],
        );
        assert!(
            !codes.contains(&"dead_code"),
            "distinct instantiations keep the else live: {codes:?}"
        );
    }

    /// The positive face of the same rule: identical written arguments are
    /// the same instantiation, so the `is`-test is guaranteed true and the
    /// else branch is genuinely dead. The arity split must not turn a
    /// decidable test into a vacuous one.
    #[test]
    fn unmodeled_generic_same_instantiation_deads_the_else() {
        let codes = diagnose_codes_unmodeled(
            "int f(Stream<Uint8List> stream) { \
               if (stream is Stream<Uint8List>) { return 1; } else { return 2; } \
             }",
            &["Stream", "Uint8List"],
        );
        assert!(
            codes.contains(&"dead_code"),
            "the same instantiation deads the else: {codes:?}"
        );
    }

    /// #2288: a pattern whose written type resolves to `Invalid` is not a
    /// guaranteed mismatch. Official `promoteForPattern`'s invalid arm
    /// (`flow_analysis.dart:7720-7723`) joins the live state into the
    /// unmatched chain and skips the type test; without it
    /// `factor(any, Invalid)` answered `Never` and deaded the body.
    #[test]
    fn invalid_pattern_type_keeps_the_body_live() {
        let codes = diagnose_codes(
            "void f(Object o) { \
               if (o case Missing()) { print(1); } else { print(2); } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "an unresolvable pattern type keeps both branches live: {codes:?}"
        );
    }

    /// The cascade's `??`-chain face: `a ?? b` with a nullable right side
    /// types nullable, so the next arm stays live (`int? ?? int?` is
    /// `int?`, and `6` runs whenever both operands are null).
    #[test]
    fn ifnull_chain_with_nullable_right_keeps_the_next_arm() {
        let codes = diagnose_codes("int h(int? a, int? b) => a ?? b ?? 6;", |_| None);
        assert!(
            !codes.contains(&"dead_code"),
            "a nullable ??-chain keeps its last arm: {codes:?}"
        );
    }

    /// The cascade's dead-null-chain face: a variable written inside an
    /// if's then branch is possibly-assigned after the join, so the
    /// definite-unassignment gate (the fact's `definitely_unassigned` bit)
    /// keeps the `?.` chain's dead row silent.
    #[test]
    fn if_then_write_keeps_the_null_aware_chain_live() {
        let codes = diagnose_codes(
            "class L {} L? mk() => null; \
             int f(bool c) { L? v; if (c) { v = mk(); } return v?.hashCode ?? 0; }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "a possibly-assigned local's ?. chain stays live: {codes:?}"
        );
    }

    /// The adapter's own verdict, exercised directly: a `Base::Name` id
    /// resolving through `names` to a sealed decl answers `true`, a
    /// registered non-sealed decl `false`, an unregistered spelling
    /// `false` — and the `FutureOr<sealed>` arm recurses through the
    /// `args` arena.
    #[test]
    fn ops_is_always_exhaustive_verdicts() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit(
            "sealed class S {} class A extends S {} class Open {}",
            &strings,
            &mut files,
            "u.dart",
        );
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        // `names` is the recorder's table — ids index into it.
        let names: Vec<Box<str>> =
            vec!["S".into(), "A".into(), "Open".into(), "Missing".into(), "FutureOr".into()];
        let empty_arena: Vec<Vec<ArgTy>> = vec![Vec::new()];
        // The `FutureOr` row: one arg node pointing at `S` (name id 0,
        // EMPTY_ARGS row of its own).
        let future_or_arena: Vec<Vec<ArgTy>> = vec![
            Vec::new(),
            vec![ArgTy {
                base: Base::Name(0),
                nullable: false,
                args: EMPTY_ARGS,
            }],
        ];

        assert!(ops.is_always_exhaustive(
            Ty { base: Base::Name(0), nullable: false },
            EMPTY_ARGS,
            &empty_arena,
            &names,
        ));
        assert!(!ops.is_always_exhaustive(
            Ty { base: Base::Name(2), nullable: false },
            EMPTY_ARGS,
            &empty_arena,
            &names,
        ));
        assert!(!ops.is_always_exhaustive(
            Ty { base: Base::Name(3), nullable: false },
            EMPTY_ARGS,
            &empty_arena,
            &names,
        ));
        // `FutureOr<S>` over a sealed base closes.
        assert!(ops.is_always_exhaustive(
            Ty { base: Base::Name(4), nullable: false },
            1,
            &future_or_arena,
            &names,
        ));
        // `Null` and `bool` split completely.
        assert!(ops.is_always_exhaustive(
            Ty::NULL,
            EMPTY_ARGS,
            &empty_arena,
            &names,
        ));
    }

    /// Cross-file sealed closure: `S` declared in `s.dart`, diagnosed in
    /// `main.dart` — the registry reaches the import's export scope, so
    /// a `switch` over `S` in `main` still answers `true`. The name-level
    /// `ClosedNames` never saw the imported decl.
    #[test]
    fn sealed_scrutinee_closes_across_an_import() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let s_elements = unit(
            "sealed class S {} class A extends S {} class B extends S {}",
            &strings,
            &mut files,
            "s.dart",
        );
        let s_file = files.try_file_id(Path::new("s.dart")).unwrap();
        let main_file = files.intern(Path::new("main.dart"));
        let main_source = "import 's.dart'; \
                           void f(S s) { \
                             switch (s) { case A(): return; case B(): return; } \
                           }";
        let scanned = dartq_scanner::scan(
            main_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            main_file,
        );
        assert!(scanned.diagnostics.is_empty());
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let main_elements = collect(main_source, &scanned.tokens, features, &strings);

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_s = builder.intern("package:t/s.dart");
        let lib_main = builder.intern("package:t/main.dart");
        builder.unit(lib_s, s_file);
        builder.unit(lib_main, main_file);
        let mut index = ElementIndex::default();
        index.insert(s_file, s_elements.clone());
        index.insert(main_file, main_elements.clone());
        for (name, decl) in s_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_s, name, s_file);
        }
        for (name, decl) in main_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_main, name, main_file);
        }
        builder.import(
            lib_main,
            ImportEdge {
                target: Some(lib_s),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types =
            LibraryTypes::build(&graph, &namespaces, &index, &strings, lib_main);
        let ops = ResolvedTypeOps::new(&library_types, &strings);
        let fold = fold_unit_with_ops(
            main_source,
            &scanned.tokens,
            features,
            &strings,
            main_file,
            lib_main,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            Some(&ops),
        );
        let codes: Vec<&'static str> = fold
            .report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
        assert!(
            !codes.contains(&"body_might_complete_normally"),
            "imported sealed switch should close: {codes:?}"
        );
    }

    /// Phase B widened base ops: `observe_names` lets `is_subtype`,
    /// `classify`, `is_bottom`, `promote_to_non_null`, `try_promote_to`,
    /// and `factor` answer through the hierarchy for every interned name,
    /// and the observed-but-unregistered face keeps the `Invalid`
    /// suppress posture rather than the name-level answer — while a
    /// never-observed `Base::Name` id still falls back to
    /// `NameTypeOps` verbatim.
    #[test]
    fn widened_ops_resolve_observed_names_and_fall_back_unobserved() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit(
            "class Base {} class Derived extends Base {} class Open {} typedef Alias = Base;",
            &strings,
            &mut files,
            "u.dart",
        );
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        // `names` is the recorder's table — ids index into it.
        let names: Vec<Box<str>> = vec![
            "Base".into(),    // 0
            "Derived".into(), // 1
            "Open".into(),    // 2
            "Alias".into(),   // 3 — a typedef: registered non-container
            "Missing".into(), // 4 — declared nowhere
        ];
        ops.observe_names(&names);
        let base = Ty {
            base: Base::Name(0),
            nullable: false,
        };
        let derived = Ty {
            base: Base::Name(1),
            nullable: false,
        };
        let open = Ty {
            base: Base::Name(2),
            nullable: false,
        };
        let alias = Ty {
            base: Base::Name(3),
            nullable: false,
        };
        let missing = Ty {
            base: Base::Name(4),
            nullable: false,
        };
        // An id past the observed table — the unobserved arm.
        let unobserved = Ty {
            base: Base::Name(9),
            nullable: false,
        };

        // `is_subtype`: the resolved face answers the real hierarchy
        // (`Derived <: Base`, never `Base <: Derived`); the name level
        // refuses both directions on distinct ids.
        assert!(ops.is_subtype(derived, base));
        assert!(!ops.is_subtype(base, derived));
        assert!(ops.is_subtype(base, base));
        // `classify`: a registered non-null interface is non-nullable.
        assert_eq!(ops.classify(base), Classification::NonNullable);
        assert_eq!(
            ops.classify(Ty {
                base: Base::Name(0),
                nullable: true,
            }),
            Classification::PotentiallyNullable
        );
        // `is_bottom`: a registered decl is never bottom; `Never` is.
        assert!(!ops.is_bottom(base));
        assert!(ops.is_bottom(Ty::NEVER));
        // `promote_to_non_null`: `Base?` strips to `Base`, `dynamic` stays.
        assert_eq!(
            ops.promote_to_non_null(Ty {
                base: Base::Name(0),
                nullable: true,
            }),
            base,
        );
        assert_eq!(ops.promote_to_non_null(Ty::DYNAMIC), Ty::DYNAMIC);
        // `try_promote_to`: `Derived` is a valid promotion target under
        // `Base` (a real subtype), `Base` under `Derived` is not — the
        // resolved subtype gate, where the name level can only compare
        // ids.
        assert_eq!(ops.try_promote_to(derived, base), Some(derived));
        assert_eq!(ops.try_promote_to(base, derived), None);
        // `factor`: `factor(Derived, Base)` is `Never` (Derived <: Base),
        // `factor(Base, Derived)` keeps `Base`.
        assert_eq!(ops.factor(derived, base), Ty::NEVER);
        assert_eq!(ops.factor(base, derived), base);

        // The unobserved arm falls back to `NameTypeOps` verbatim:
        // distinct ids are never subtypes, the spelling classifies
        // non-nullable, and the promotion gate answers name-equality only.
        assert!(!ops.is_subtype(unobserved, base));
        assert!(!ops.is_subtype(base, unobserved));
        assert_eq!(ops.classify(unobserved), Classification::NonNullable);
        assert!(!ops.is_bottom(unobserved));
        assert_eq!(
            ops.try_promote_to(unobserved, base),
            NameTypeOps.try_promote_to(unobserved, base)
        );
        assert_eq!(
            ops.factor(unobserved, base),
            NameTypeOps.factor(unobserved, base)
        );

        // A typedef name resolves through its captured RHS — `Alias`
        // stands for `Base`, so `is_subtype(alias, base)` answers `true`
        // via the real `B <: B` edge, matching the oracle
        // (`B f(A a) => a` is silent; `C c = a` reports
        // `invalid_assignment`). `Missing` still maps `Invalid` — its
        // `is_subtype(missing, open)` answers `true` through
        // subtype.dart's 0c arm where `NameTypeOps` says `false`.
        assert!(ops.is_subtype(alias, base));
        assert!(ops.is_subtype(missing, open));
    }

    /// S4-C (#2186): a negated type test whose tested type carries
    /// written arguments must not dead the else branch. `factor(Response,
    /// Response<T>)` compares equal on bare `Ty`s (same name) but the
    /// instantiated forms are not mutual subtypes, so the unmatched
    /// remainder keeps `Response` — the promoted receiver's member read
    /// resolves and no `undefined_getter`/`undefined_method` reports.
    #[test]
    fn is_not_promotion_keeps_the_remainder_under_generic_tested() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; int get statusCode => 0; } \
             void f<T>(Object response) { \
               if (response is! Response) { return; } \
               if (response is! Response<T>) { response.data; response.statusCode; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"undefined_getter")
                && !codes.contains(&"undefined_method")
                && !codes.contains(&"dead_code"),
            "the is! remainder must keep Response: {codes:?}"
        );
        // The deliberate divergence (#2204 C): the unmounted name-level
        // fold still deads the else branch — `factor(Response,
        // Response<T>)` on bare `Ty`s compares equal, so the remainder
        // is `Never` and the read on the `Object` remainder reports
        // `undefined_getter`. The resolved face's argument-penetrating
        // `factor_typed` is exactly what Phase C adds; pinning the
        // name-level answer keeps the boundary honest rather than
        // pretending parity. (The `undefined_class` pair reports in both
        // arms — an arm-independent resolution row outside this fix.)
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let source = "class Response<T> { T? data; int get statusCode => 0; } \
                      void f<T>(Object response) { \
                        if (response is! Response) { return; } \
                        if (response is! Response<T>) { response.data; } \
                      }";
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty());
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit_with_ops(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
            None,
        );
        let codes: Vec<&'static str> = fold
            .report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
        assert!(
            codes.contains(&"dead_code") && codes.contains(&"undefined_getter"),
            "the name-level fold keeps the pre-C posture (dead else + \
             Object member miss): {codes:?}"
        );
    }

    /// Phase C acceptance pin: the argument-penetrating subtype verdict.
    /// `List<int> <: List<num>` answers `true` through the instantiated
    /// forms — the written-argument tree reaches `arguments_subtype`'s
    /// covariant pairwise compare and `int <: num` through the builtin
    /// `int extends num` edge — where the bare `Ty`s can only compare
    /// names. The reverse answers `false` (`num <: int` fails), and the
    /// promotion gate follows the same verdict: `List<int>` promotes a
    /// `List<num>` variable, `List<num>` does not promote a `List<int>`
    /// one.
    #[test]
    fn typed_subtype_penetrates_the_written_arguments() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit("class Placeholder {}", &strings, &mut files, "u.dart");
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        // `names` is the recorder's table — ids index into it.
        let names: Vec<Box<str>> = vec!["List".into(), "int".into(), "num".into()];
        // Arena row 1 = `[int]`, row 2 = `[num]` — `List<int>` reads
        // `Ty{base: Name(0), args: 1}`, `List<num>` args: 2.
        let arena: Vec<Vec<ArgTy>> = vec![
            Vec::new(),
            vec![ArgTy {
                base: Base::Name(1),
                nullable: false,
                args: EMPTY_ARGS,
            }],
            vec![ArgTy {
                base: Base::Name(2),
                nullable: false,
                args: EMPTY_ARGS,
            }],
        ];
        let list = Ty {
            base: Base::Name(0),
            nullable: false,
        };

        // The acceptance pin: `List<int> <: List<num>` — the
        // argument-penetrating answer the bare `Ty`s could not give.
        assert!(ops.is_subtype_typed(list, 1, list, 2, &arena, &names));
        // And the reverse fails — `num` is no subtype of `int`.
        assert!(!ops.is_subtype_typed(list, 2, list, 1, &arena, &names));
        // The promotion gate follows the same verdict: `to <: from`.
        assert_eq!(
            ops.try_promote_to_typed(list, 1, list, 2, &arena, &names),
            Some(list)
        );
        assert_eq!(
            ops.try_promote_to_typed(list, 2, list, 1, &arena, &names),
            None
        );
    }

    /// The drift `async.dart` shape (#2355), hermetic form: an
    /// `A Function(S)` operand tested `is R Function(S)` — two different
    /// signatures the flow layer spells as the same signature-less
    /// `Base::Function` head (`A <: R` fails on the return type, so the
    /// official verdict is not constant and the oracle is silent). Before
    /// the signature-less guard the mapped `Interface{Function}` pair
    /// answered `true` on spelling identity: the else branch deaded
    /// (`dead_code`) and the check reported a false constant-true
    /// `unnecessary_type_check`. The flow face burns through the
    /// signature-less guard; the check face splits because the formal's
    /// captured shape and the body check's own captured shape (#2359 —
    /// `type_capture.rs` composes body-position spellings into real
    /// shapes instead of the shapeless sentinel) key to different
    /// sentinels.
    #[test]
    fn function_is_test_between_unrelated_signatures_stays_live() {
        let codes = diagnose_codes(
            "class R {}\nclass S {}\nclass A {}\n\
             void t(A Function(S) p) {\n\
             \x20 if (p is R Function(S)) {\n\
             \x20   p;\n\
             \x20 } else {\n\
             \x20   p;\n\
             \x20 }\n\
             }",
            |_| None,
        );
        assert!(codes.is_empty(), "{codes:?}");
    }

    /// The identical-spelling control: both sides of `f is F` fold the
    /// alias to the same canonically-interned function shape, so the
    /// constant-true `unnecessary_type_check` the oracle emits stays
    /// reported — the shape-keyed sentinels keep the self-equality the
    /// guard must not over-suppress, while the unrelated pin above splits
    /// the two distinct shapes.
    #[test]
    fn identical_function_signature_test_stays_constant_true() {
        let codes = diagnose_codes(
            "class S {}\nclass A {}\n\
             typedef F = A Function(S);\n\
             void t(F f) {\n\
             \x20 if (f is F) {\n\
             \x20   f;\n\
             \x20 }\n\
             }",
            |_| None,
        );
        assert_eq!(codes, ["unnecessary_type_check"], "{codes:?}");
    }

    /// The signature-less function pair claims no relation at the name
    /// level: `is_subtype_typed` answers `false` on the pair and on
    /// every function-vs-interface mix (the official's bare `Function`
    /// face has no admitting arm toward a function type — upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/element/subtype.dart:502-510
    /// — and two unknown signatures prove nothing), while a real edge
    /// still answers directionally (`D <: B`, not the reverse) — the
    /// swap-back controls prove the verdicts are decided, not vacuous.
    /// `factor` keeps the operand instead of collapsing to `Never`, the
    /// nullable face riding the nullability rule.
    #[test]
    fn signatureless_function_pair_claims_no_relation() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit("class B {}\nclass D extends B {}", &strings, &mut files, "u.dart");
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        let names: Vec<Box<str>> = vec!["B".into(), "D".into()];
        let sup = Ty {
            base: Base::Name(0),
            nullable: false,
        };
        let sub = Ty {
            base: Base::Name(1),
            nullable: false,
        };
        let function = Ty {
            base: Base::Function,
            nullable: false,
        };
        let nullable_function = Ty {
            base: Base::Function,
            nullable: true,
        };

        // Non-vacuity control: the harness decides a one-directional
        // relation (`D <: B` holds, the swap refuses).
        assert!(ops.is_subtype_typed(sub, EMPTY_ARGS, sup, EMPTY_ARGS, &[], &names));
        assert!(!ops.is_subtype_typed(sup, EMPTY_ARGS, sub, EMPTY_ARGS, &[], &names));
        // The signature-less function pair: both directions refuse.
        assert!(!ops.is_subtype_typed(
            function, EMPTY_ARGS, function, EMPTY_ARGS, &[], &names
        ));
        assert!(!ops.is_subtype_typed(
            nullable_function,
            EMPTY_ARGS,
            function,
            EMPTY_ARGS,
            &[],
            &names
        ));
        // Function-vs-interface mixes refuse both ways.
        assert!(!ops.is_subtype_typed(
            function, EMPTY_ARGS, sup, EMPTY_ARGS, &[], &names
        ));
        assert!(!ops.is_subtype_typed(
            sup, EMPTY_ARGS, function, EMPTY_ARGS, &[], &names
        ));
        // `factor(Sig, Sig2)` keeps the operand — never the `Never`
        // collapse; `factor(Sig?, Sig2)` keeps the nullable face.
        assert_eq!(
            ops.factor_typed(function, EMPTY_ARGS, function, EMPTY_ARGS, &[], &names),
            function
        );
        assert_eq!(
            ops.factor_typed(
                nullable_function,
                EMPTY_ARGS,
                function,
                EMPTY_ARGS,
                &[],
                &names
            ),
            nullable_function
        );
    }

    /// Phase C acceptance pin: a switch over a generic sealed scrutinee
    /// still closes — `S<num>`'s written arguments reach `map_scrutinee`
    /// and the `is_closed` verdict answers on the declaration alone
    /// (`is_sealed(S)` is arg-agnostic). The post-switch state goes
    /// unreachable, so the missing tail `return` never reports — the
    /// `body_might_complete_normally` silence the sealed DEAD rows stand
    /// on.
    #[test]
    fn sealed_generic_scrutinee_switch_stays_silent() {
        let codes = diagnose_codes(
            "sealed class S<T> {} \
             class A extends S<int> {} \
             class B<T> extends S<T> {} \
             int f(S<num> s) { \
               switch (s) { case A(): return 0; case B<num>(): return 1; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"body_might_complete_normally"),
            "generic sealed switch should close: {codes:?}"
        );
    }

    /// Phase C leftover pin: a strict generic narrowing is a real
    /// promotion step. `x is Response` records the raw head; `x is
    /// Response<T>` then promotes — `Response<T> <: Response<dynamic>`
    /// holds through the instantiated forms — where the bare-`Ty`
    /// coverage test factored the tested type to `Never` and vetoed the
    /// step. The promoted read resolves `data`, so no `undefined_*`
    /// reports.
    #[test]
    fn strict_generic_narrowing_promotes_after_the_raw_test() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f<T>(Object response) { \
               if (response is Response) {} \
               if (response is Response<T>) { response.data; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"undefined_getter")
                && !codes.contains(&"undefined_method"),
            "the strict narrowing must promote: {codes:?}"
        );
    }

    /// Phase C regression pin: `var`-typed reads must map to the
    /// asymmetric top `dynamic`, not `Invalid` — `factor(Invalid, int)`
    /// answered `Never` through subtype.dart's 0c arm and deaded the
    /// else branch of `var x; x is int`, a false `dead_code` row. With
    /// `Unknown → dynamic` the unmatched remainder keeps `dynamic` and
    /// the else branch stays live.
    #[test]
    fn var_tested_else_branch_stays_live() {
        let codes = diagnose_codes(
            "void g() {} \
             void f() { \
               var x; \
               if (x is int) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "the var else branch must stay live: {codes:?}"
        );
    }

    /// #2250: `Record` and `Enum` are plain dart:core classes, so
    /// `x is Record` / `x is Enum` resolve to real interfaces — the true
    /// branch promotes and the else branch stays live. The pre-fix face
    /// had no declaration behind the spellings, mapped them `Invalid`,
    /// and subtype.dart's 0c arm (`X <: InvalidType` vacuously true)
    /// answered `factor(any, Invalid)` = `Never` — a false `dead_code`
    /// row on every `x is Record` else branch (the oracle probe reports
    /// nothing).
    #[test]
    fn ticket_2250_record_and_enum_tests_keep_the_else_live() {
        let codes = diagnose_codes(
            "void g() {} \
             void f(dynamic x) { \
               if (x is Record) { return; } \
               g(); \
               if (x is Enum) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "x is Record/Enum must keep the else branches live: {codes:?}"
        );
    }

    /// #2250's object face: the same test on a typed receiver — `Object`
    /// is not a `Record`, so the factor keeps `Object` instead of the
    /// pre-fix `Invalid`-vacuous `Never`. Pins the exact question the
    /// type-factor seam answers (`factor_typed`), independent of the
    /// `dynamic`-scrutinee probe above.
    #[test]
    fn ticket_2250_record_test_on_object_keeps_the_remainder() {
        let codes = diagnose_codes(
            "void g() {} \
             void f(Object x) { \
               if (x is Record) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "x is Record on Object must keep the else branch live: {codes:?}"
        );
    }

    /// #2252: an unresolved type argument keeps its position inside the
    /// instantiation — the oracle's `Response<InvalidType>` is still an
    /// `InterfaceType` carrying the invalid argument (the resolution pass
    /// reports `non_type_as_type_argument` and the flow face stays live),
    /// so neither branch of `x is Response<Missing>` deads. The pre-fix
    /// face vetoed the whole instantiation to `Invalid`, and the 0c arm
    /// made `factor(any, Invalid)` = `Never` — a false `dead_code` row
    /// the oracle probe does not report.
    #[test]
    fn ticket_2252_invalid_argument_keeps_the_interface_face() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void g() {} \
             void f(dynamic x) { \
               if (x is Response<Missing>) { return; } \
               g(); \
               if (x is! Response<Missing>) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "Response<Missing> must keep its Interface face: {codes:?}"
        );
    }

    /// #2252's comparability face: the interface-with-invalid-argument
    /// stays a first-class `Interface`, so it answers its own head's
    /// question — after `x is Response`, `x is Response<Missing>` is
    /// always true (`Response<dynamic> <: Response<Invalid>` through the
    /// carried argument's 0c arm), the inner else deads, and the inner
    /// `x.data` read resolves. The oracle probe reports exactly
    /// `non_type_as_type_argument` + `unnecessary_type_check` +
    /// `dead_code`; the pre-fix face collapsed the instantiation to bare
    /// `Invalid` and the same rows fell out of the erased face instead.
    #[test]
    fn ticket_2252_invalid_argument_stays_comparable_to_its_head() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Object x) { \
               if (x is Response) { \
                 if (x is Response<Missing>) { x.data; } else { g(); } \
               } \
             } \
             void g() {}",
            |_| None,
        );
        assert!(
            codes.contains(&"dead_code")
                && !codes.contains(&"undefined_getter")
                && !codes.contains(&"undefined_method"),
            "the invalid-argument face must stay comparable and readable: {codes:?}"
        );
    }

    /// #2370 form A: a wrong-arity is-test is not the whole-face
    /// `InvalidType` — the official's `_buildTypeArguments` fills
    /// `InvalidType` into the declared parameter positions and keeps the
    /// interface, so `Object is Response<int, int>` is a decidable
    /// question (`Object <: Response<Invalid>` = false) and the else
    /// branch stays live. The pre-fix face vetoed the spelling to bare
    /// `Invalid`, whose 0c arm answered vacuously true and deaded a live
    /// else — the oracle probe reports only
    /// `wrong_number_of_type_arguments` (dart analyze 3.13.1).
    #[test]
    fn ticket_2370_wrong_arity_is_test_keeps_the_else_live() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void g() {} \
             void f(Object x) { \
               if (x is Response<int, int>) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "a wrong-arity is-test must keep the else branch live: {codes:?}"
        );
    }

    /// #2370 form A's decidable face: the recovery keeps the interface
    /// (one `Invalid` per declared parameter), so a typed receiver
    /// answers its own head's question — `Response<int> <:
    /// Response<Invalid>` admits through the carried argument's 0c arm,
    /// the test is guaranteed true, and the else branch deads exactly as
    /// the oracle reports (`wrong_number_of_type_arguments` +
    /// `dead_code`; dart analyze 3.13.1). Pinning the dead direction
    /// guards against an over-correction that would treat every
    /// arity-vetoed spelling as undecidable.
    #[test]
    fn ticket_2370_wrong_arity_is_test_deads_on_typed_receiver() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void g() {} \
             void f(Response<int> x) { \
               if (x is Response<int, int>) { return; } else { g(); } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"dead_code"),
            "the typed receiver's wrong-arity test must dead the else: {codes:?}"
        );
    }

    /// #2370 form B: a user enum declaration implicitly implements
    /// dart:core's `Enum`, so `c is Enum` on an `E` receiver is a
    /// guaranteed-true test and the else branch deads (`E <: Enum` holds
    /// in the oracle — probed dart analyze 3.13.1, which reports
    /// `dead_code` on the else; the accompanying
    /// `unnecessary_type_check` row is the static-warning tier, not this
    /// flow face). The pre-fix face grew no edge and kept the else live.
    #[test]
    fn ticket_2370_user_enum_is_enum_deads_the_else() {
        let codes = diagnose_codes(
            "enum Color { r } \
             void f(Color c) { \
               if (c is Enum) { c; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"dead_code"),
            "E is Enum must read as guaranteed true and dead the else: {codes:?}"
        );
    }

    /// #2370 form B's negative face: the edge belongs to enum
    /// declarations only — a class receiver keeps `c is Enum` decidable
    /// (both branches live; probed dart analyze 3.13.1), and so does the
    /// `bool` builtin, which the analyzer's element model declares a
    /// plain class (its `b is Enum` probe reports nothing).
    #[test]
    fn ticket_2370_class_receiver_is_enum_keeps_both_branches_live() {
        let codes = diagnose_codes(
            "enum Color { r } \
             class C {} \
             void g() {} \
             void f(C c) { \
               if (c is Enum) { return; } \
               g(); \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"dead_code"),
            "a class receiver must keep the is-Enum test decidable: {codes:?}"
        );
    }

    // —— #2380: the `unnecessary_type_check` static-warning face over the
    // same is-test shapes — the check tier (`check_type_check`) beside the
    // flow's dead-arm rows above. The oracle reports the code on every
    // provably-constant test at `warning` severity, the span at the `is`
    // operator, polarity in the message ('true' for `is`, 'false' for
    // `is!`); all shapes probed (dart analyze 3.13.1).

    /// The enum receiver's guaranteed-true `is Enum` reports the warning
    /// beside the else's dead row (`enum_recv.dart` probe).
    #[test]
    fn ticket_2380_enum_receiver_is_enum_reports_the_check_warning() {
        let codes = diagnose_codes(
            "enum Color { r } \
             void f(Color c) { \
               if (c is Enum) { c; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "E is Enum must report unnecessary_type_check: {codes:?}"
        );
    }

    /// The subclass/superclass and implements faces report the same
    /// warning (`class_super.dart` / `iface_impl.dart` probes).
    #[test]
    fn ticket_2380_supertype_is_test_reports_the_check_warning() {
        let codes = diagnose_codes(
            "class A {} \
             class B extends A {} \
             void f(B b) { \
               if (b is A) { b; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "B is A must report unnecessary_type_check: {codes:?}"
        );
        let codes = diagnose_codes(
            "abstract class I {} \
             class C implements I {} \
             void f(C c) { \
               if (c is I) { c; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "C is I must report unnecessary_type_check: {codes:?}"
        );
    }

    /// The typed receiver's wrong-arity test reports the warning beside
    /// the arity error and the dead row (`recv_arity.dart` probe) — the
    /// same recovered `Response<Invalid>` interface the flow face reads.
    #[test]
    fn ticket_2380_wrong_arity_test_reports_the_check_warning() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Response<int> x) { \
               if (x is Response<int, int>) { x; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "the typed receiver's wrong-arity test must report the warning: {codes:?}"
        );
    }

    /// The `is!` polarity reports the complement row — the recovered
    /// interface makes the test constant-false (`recv_arity_isnot.dart`
    /// probe), so the warning fires on `is!` too.
    #[test]
    fn ticket_2380_wrong_arity_isnot_reports_the_check_warning() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Response<int> x) { \
               if (x is! Response<int, int>) { 1; } else { 2; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "the constant-false is! must report unnecessary_type_check: {codes:?}"
        );
    }

    /// The undecidable face stays silent: `Object is Response<int, int>`
    /// answers false, not constant (`recv_arity_obj.dart` probe reports
    /// only `wrong_number_of_type_arguments`).
    #[test]
    fn ticket_2380_object_wrong_arity_test_stays_silent() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Object o) { \
               if (o is Response<int, int>) { o; } else { 1; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"unnecessary_type_check"),
            "a decidable-false test must not report the warning: {codes:?}"
        );
    }

    /// #2380 face 2: the RECEIVER's own wrong-arity annotation recovers
    /// to the same `Response<Invalid>` interface, so `x is String`
    /// answers a decidable false — the is-test face stays silent, only
    /// the arity error reports (`recv_self_arity.dart` probe).
    #[test]
    fn ticket_2380_wrong_arity_receiver_is_string_stays_silent() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Response<int, int> x) { \
               if (x is String) { 1; } else { 2; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"unnecessary_type_check") && !codes.contains(&"dead_code"),
            "a wrong-arity receiver's is String must stay silent: {codes:?}"
        );
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Response<int, int> x) { \
               if (x is! String) { 1; } else { 2; } \
             }",
            |_| None,
        );
        assert!(
            !codes.contains(&"unnecessary_type_check") && !codes.contains(&"dead_code"),
            "a wrong-arity receiver's is! String must stay silent: {codes:?}"
        );
    }

    /// #2380 face 2's admitting control: the same wrong-arity receiver
    /// against a bare right-arity `Response` target — any instantiation
    /// of the recovered interface is a `Response`, so the test is
    /// constant-true and the warning fires beside the dead row
    /// (`recv_self_arity_same.dart` probe).
    #[test]
    fn ticket_2380_wrong_arity_receiver_is_bare_response_reports() {
        let codes = diagnose_codes(
            "class Response<T> { T? data; } \
             void f(Response<int, int> x) { \
               if (x is Response) { x; } else { 2; } \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"unnecessary_type_check"),
            "the wrong-arity receiver's bare-head test must report: {codes:?}"
        );
    }

    /// Phase D acceptance pin: a real supertype clause tree reaches the
    /// hierarchy. `class A extends S<int>` edges `A → S<int>` (not the
    /// to-bounds `S<dynamic>` the name-level arm produces), so the
    /// cross-declaration questions answer on the written `int`:
    /// `A <: S<num>` admits through the covariant `int <: num` (the
    /// `S<dynamic>` edge refuses — `dynamic <: num` is not a top-arm
    /// hit); `S<num> <: A` refuses (`num <: int` fails) where the
    /// `S<dynamic>` edge admitted it (`num <: dynamic`, right-top).
    /// `A <: S<String>` refuses either way (`int <: String` and
    /// `dynamic <: String` both fail) — pinned for completeness, not as
    /// the discriminator. The `ClauseTrees` map is the transport face
    /// Phase D routes the `BoundsIndex` captures through.
    #[test]
    fn clause_trees_carry_the_written_supertype_arguments() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit(
            "sealed class S<T> {} class A extends S<int> {} class B<T> extends S<T> {}",
            &strings,
            &mut files,
            "u.dart",
        );
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);

        // The `A extends S<int>` capture, hand-shaped the way the
        // `Bounds` header sink produces it: one supertype tree `S<int>`
        // and `A`'s own (empty) parameter list.
        let range = dartq_source::SourceRange {
            offset: 0,
            length: 0,
        };
        let mut s_of_int = TypeTree::leaf(strings.intern("S"), false, range);
        s_of_int
            .args
            .push(TypeTree::leaf(strings.intern("int"), false, range));
        let mut clauses = ClauseTrees::empty();
        clauses.insert(
            file,
            strings.intern("A"),
            ClauseCapture {
                params: Vec::new().into_boxed_slice(),
                supers: vec![s_of_int],
            },
        );
        let library_types = LibraryTypes::build_with_clauses(
            &graph,
            &namespaces,
            &index,
            &strings,
            lib,
            &clauses,
        );
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        // `names` is the recorder's table — ids index into it.
        let names: Vec<Box<str>> = vec![
            "A".into(), // 0
            "S".into(), // 1
            "num".into(), // 2
            "String".into(), // 3
        ];
        // Arena rows: [num] = 1, [String] = 2 — `S<num>`/`S<String>`
        // spellings the questions write.
        let arena: Vec<Vec<ArgTy>> = vec![
            Vec::new(),
            vec![ArgTy {
                base: Base::Name(2),
                nullable: false,
                args: EMPTY_ARGS,
            }],
            vec![ArgTy {
                base: Base::Name(3),
                nullable: false,
                args: EMPTY_ARGS,
            }],
        ];
        let a_ty = Ty {
            base: Base::Name(0),
            nullable: false,
        };
        let s_ty = Ty {
            base: Base::Name(1),
            nullable: false,
        };

        // Admit: `A <: S<num>` through `A → S<int>` + `int <: num`.
        assert!(ops.is_subtype_typed(a_ty, EMPTY_ARGS, s_ty, 1, &arena, &names));
        // Reject: `A <: S<String>` — `int <: String` fails; the
        // to-bounds `S<dynamic>` edge would have refused it too
        // (`dynamic <: String` is not a top-arm hit), so this direction
        // alone does not distinguish the two edges. The direction that
        // DOES: `S<num> <: A` is refused (`num <: int` fails) where the
        // `S<dynamic>` edge admitted it (`num <: dynamic`, right-top).
        assert!(!ops.is_subtype_typed(a_ty, EMPTY_ARGS, s_ty, 2, &arena, &names));
        assert!(!ops.is_subtype_typed(s_ty, 1, a_ty, EMPTY_ARGS, &arena, &names));
        // The promotion gate follows the strict subtype rule:
        // `A <: S<num>` through the real `S<int>` edge promotes; the
        // reverse `S<num> <: A` has no step (`S<num>` is wider); and
        // `S<String> <: A` is refused — the to-bounds edge would have
        // admitted `S<dynamic> <: A` and promoted the impossibility.
        assert_eq!(
            ops.try_promote_to_typed(a_ty, EMPTY_ARGS, s_ty, 1, &arena, &names),
            Some(a_ty),
        );
        assert_eq!(
            ops.try_promote_to_typed(s_ty, 1, a_ty, EMPTY_ARGS, &arena, &names),
            None,
        );
        assert_eq!(
            ops.try_promote_to_typed(s_ty, 2, a_ty, EMPTY_ARGS, &arena, &names),
            None,
        );
    }

    /// Phase D acceptance pin (name-level arm): the same `A extends
    /// S<int>` declaration WITHOUT a `ClauseTrees` entry keeps the
    /// to-bounds `A → S<dynamic>` edge — and the `dynamic` argument
    /// loses the declaration's own supertype face: `A <: S<int>` itself
    /// refuses (`dynamic <: int` fails, the Left-Top arm reducing to
    /// `Object? <: int`), where the captured `int` argument answers
    /// true. `A <: S<num>` refuses too (`dynamic <: num` fails where
    /// `int <: num` holds).
    #[test]
    fn missing_clause_capture_keeps_the_to_bounds_edge() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements = unit(
            "sealed class S<T> {} class A extends S<int> {}",
            &strings,
            &mut files,
            "u.dart",
        );
        let file = files.try_file_id(Path::new("u.dart")).unwrap();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/u.dart");
        builder.unit(lib, file);
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        let ops = ResolvedTypeOps::new(&library_types, &strings);

        let names: Vec<Box<str>> = vec!["A".into(), "S".into(), "num".into(), "int".into()];
        let arena: Vec<Vec<ArgTy>> = vec![
            Vec::new(),
            vec![ArgTy {
                base: Base::Name(2),
                nullable: false,
                args: EMPTY_ARGS,
            }],
            vec![ArgTy {
                base: Base::Name(3),
                nullable: false,
                args: EMPTY_ARGS,
            }],
        ];
        let a_ty = Ty {
            base: Base::Name(0),
            nullable: false,
        };
        let s_ty = Ty {
            base: Base::Name(1),
            nullable: false,
        };
        // `A → S<dynamic>` refuses `A <: S<num>` (`dynamic <: num` fails)
        // and refuses `A <: S<int>` itself (`dynamic <: int` fails) —
        // the declaration's own supertype is unrecoverable at name
        // level, where the real `int` argument answers both correctly.
        assert!(!ops.is_subtype_typed(a_ty, EMPTY_ARGS, s_ty, 1, &arena, &names));
        assert!(!ops.is_subtype_typed(a_ty, EMPTY_ARGS, s_ty, 2, &arena, &names));
    }

    /// Phase D acceptance pin (model.rs write-demotion): a write whose
    /// written type no longer covers the standing promotion drops it —
    /// `o is List<int>` promotes, then `o = <num>[]` demotes back to the
    /// declared `Object` (`List<num> <: List<int>` fails on the
    /// instantiated spellings where the bare `Ty`s compare equal), so
    /// `o.length` reads `Object`'s missing member and reports
    /// `undefined_getter`. The name-level demotion kept `List<int>` —
    /// `num` and `int` share the `List` base — and the read resolved.
    #[test]
    fn write_demotion_drops_a_generic_promotion() {
        let codes = diagnose_codes(
            "void f() { \
               Object? o = <int>[]; \
               if (o is List<int>) {} \
               o = <num>[]; \
               o.first; \
             }",
            |_| None,
        );
        assert!(
            codes.contains(&"undefined_getter"),
            "the demoted Object read must miss `length`: {codes:?}"
        );
    }
}
