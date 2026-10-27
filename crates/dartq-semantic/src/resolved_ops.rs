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

use std::cell::{Cell, RefCell};

use dartq_flow::ty::{
    ArgTy, Base, Classification, EMPTY_ARGS, NameTypeOps, TypeOps, Ty as FlowTy,
};
use dartq_resolution::{LibraryGraph, LibraryId, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use dartq_types::{
    CoreDecls, NamedMember, ParamFacts, ParamFactsTable, TypeDeclId, TypeHierarchy, TypeId,
    TypeKind, TypeParamId, TypeSystem, TypeTable, UninternedFunction, UninternedRecord,
    UninternedType, Variance, instantiate_type_formals_to_bounds,
};

use crate::bounds::TypeTree;
use crate::library::ElementIndex;
use crate::model::{ArgListId, ArgPool, ContainerKind, MemberKind, MicroKind, Ty};

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
    /// Spellings a PREFIXED import target exports: the flow layer's
    /// bare-head type question cannot say which class the source named
    /// (`io.File` beside — or in place of — a bare-registered `File`),
    /// so the adapter's verdict gates keep the name-level default's
    /// answer over them ([`ResolvedTypeOps::head_overloaded`]).
    lossy_overloads: FxHashSet<StringId>,
    /// `typedef` name → the aliased head name plus its own `?` flag —
    /// captured only when the element model carried the RHS as a bare
    /// `Ty::Named` with no written arguments and the alias declares no
    /// type parameters (`typedef A = B`, `typedef A = B?`). The chase
    /// re-enters the full name precedence on the terminal, so a chain or
    /// a builtin-spelled terminal resolves exactly as before.
    typedef_targets: FxHashMap<StringId, (StringId, bool)>,
    /// `typedef` name → the RHS flattened into this snapshot's lattice —
    /// the shapes the bare-Named chase cannot carry: an argument-bearing
    /// `Named` (`typedef A = List<int>`), a function shape
    /// (`typedef F = int Function(String)`), a record shape
    /// (`typedef R = (int, String)`), and every parameterized alias's
    /// captured RHS (its parameter leaves ride the alias's own synthetic
    /// ids; the chase substitutes the use-site written arguments
    /// positionally). Built once after registration by translating the
    /// element model's RHS (`Ty` plus the unit's `ArgPool`) into interned
    /// `TypeId`s — the snapshot needs no pool back-reference at chase
    /// time. An alias whose RHS names something the snapshot cannot
    /// resolve registers no entry and keeps the opaque face (the
    /// under-report direction).
    typedef_shapes: FxHashMap<StringId, TypedefShape>,
    /// Each registered container's own parameter ids → their declared
    /// spellings, in declaration order (`List`'s first id → `E`): the
    /// receiver-instantiation environment ([`ResolvedTypeOps::instance_env`])
    /// spells an instantiated supertype's arguments back onto the names
    /// the declaring container's member signatures use.
    param_names: FxHashMap<TypeParamId, StringId>,
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
    /// The `(file, spelling)` pairs of every registered type parameter —
    /// the scope face of [`Self::type_params`]. Type parameters are never
    /// visible across files: a body sees only the parameters of the
    /// declarations that enclose it, all in its own file. The name-keyed
    /// map above stays global for the build-time filters (every spelling
    /// registered anywhere), but every query-time resolution — the
    /// adapter's `map_name` face — requires the pair here, so an SDK
    /// type-parameter spelling (`List<E>`'s `E`) can never capture a
    /// same-named user class in another file (#2510).
    type_param_scope: FxHashSet<(FileId, StringId)>,
    /// Every member name of every registered container — the
    /// bare-head-invocation over-approximation
    /// ([`TypeOps::call_head_unresolved`]): a spelling here may resolve as
    /// an instance or static member somewhere the library can see, so it
    /// is NOT an unresolved-call face. Deliberately coarse (name-keyed,
    /// not scope-keyed): a same-named member elsewhere only costs the
    /// under-report direction — the call keeps `Unknown` and its
    /// `is`-tests stay decidable-agnostic, never a false dead arm.
    invokable_members: FxHashSet<StringId>,
    /// Own-declared `(container, member)` pairs whose property's required
    /// type head — a field's declared type, a getter's return type —
    /// resolves nowhere in this snapshot's universe (registry, alias, or
    /// type parameter; the builtins register first, so a core head always
    /// resolves). The official's `promoteForPattern` refuses an invalid
    /// known type (`flow_analysis.dart:7720-7723`), so an object pattern's
    /// property sub-pattern over such a member may fail and its case
    /// cannot cover vacuously.
    invalid_property_tys: FxHashSet<(StringId, StringId)>,
}

/// One container's parse-time supertype capture: the type-parameter
/// spellings in declaration order (the scope the clause trees resolve
/// their parameter arguments through), each parameter's written bound
/// tree in the same order (`None` for an unbounded parameter — the
/// implicit `Object?`), and the written supertype trees
/// (`extends`/`with`/`implements`, in clause order — the `on` constraints
/// of a mixin are constraints, not edges, and ride the name-level arm).
/// The capture's `open` bit never reaches here: an unfaithful capture
/// contributes no entry at all, so [`LibraryTypes`] keeps the name-level
/// edge set — the under-report direction.
pub(crate) struct ClauseCapture {
    /// The container's own type-parameter spellings, declaration order.
    pub(crate) params: Box<[StringId]>,
    /// Each parameter's written bound tree, parallel to
    /// [`Self::params`]; `None` for an unbounded parameter. The trees
    /// resolve inside the container's own parameter scope (a bound may
    /// name a sibling parameter — `T extends Comparable<T>`).
    pub(crate) param_bounds: Box<[Option<TypeTree>]>,
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

/// The `dart:` SDK libraries' public type names — the flat fallback the
/// lossy-spelling overload set reads for a PREFIXED SDK import whose
/// target carries no element model (the SDK is not part of the census's
/// element index, so `export_scope` answers nothing for it). The names
/// matter exactly as overload markers: the flow layer's bare-head type
/// question can never say the source meant `io.File` rather than the
/// bare-registered `File`, so the verdict gates keep the name-level
/// answer over these spellings. Extend per library as the corpora
/// prefix-import more of the SDK.

const SDK_LIBRARY_TYPE_NAMES: &[(&str, &[&str])] = &[(
    "dart:io",
    &[
        "CompressionOptions", "ContentType", "Cookie", "Directory", "File",
        "FileLock", "FileMode", "FileStat", "FileSystemEntity",
        "FileSystemEvent", "FileSystemException", "GZipCodec", "HttpBody",
        "HttpCompressionOptions", "HttpConnectionsInfo", "HttpDate",
        "HttpHeaders", "HttpClient", "HttpClientBadResponseCertificate",
        "HttpClientBasicCredentials", "HttpClientCredentials",
        "HttpClientDigestCredentials", "HttpClientRequest",
        "HttpClientResponse", "HttpOverrides", "HttpRequest", "HttpResponse",
        "HttpServer", "HttpSession", "HttpStatus", "InternetAddress",
        "InternetAddressType", "IOException", "IOSink", "Link",
        "NetworkInterface", "OSError", "Process", "ProcessException",
        "ProcessInfo", "ProcessMode", "ProcessResult", "ProcessSignal",
        "RandomAccessFile", "RawDatagramSocket", "RawSecureServerSocket",
        "RawSecureSocket", "RawSocket", "RawSocketEvent",
        "RawSynchronousSocket", "SecureServerSocket", "SecurityContext",
        "ServerSocket", "Socket", "SocketDirection", "SocketException",
        "SocketOption", "Stdin", "StdioType", "Stdout", "WebSocket",
        "WebSocketException", "WebSocketStatus", "WebSocketTransformer",
        "X509Certificate", "ZLibCodec", "ZLibOption",
    ],
)];

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
        let mut param_names: FxHashMap<TypeParamId, StringId> = FxHashMap::default();

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
            for &param in params {
                let id = TypeParamId::from_raw(next_param);
                next_param += 1;
                param_ids.push(id);
                param_names.insert(id, strings.intern(param));
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
            typedef_shapes: FxHashMap::default(),
            aliases: Vec::new(),
            type_params,
            type_param_scope: FxHashSet::default(),
            param_names,
            invokable_members: FxHashSet::default(),
            property_ty_heads: Vec::new(),
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

        // The direct-import registration: the unit's OWN import edges'
        // targets contribute their export-scope names — the graph's
        // export fixpoint has already folded re-exports into that scope,
        // so one hop is the exact visibility. A PREFIXED import
        // contributes nothing to the bare namespace (its names live only
        // under the `prefix.Name` aliases below), and the directive's
        // show/hide filters what it does contribute. Deeper import edges
        // never leak: a library's private imports are invisible to its
        // importers (the official namespace build,
        // `library_analyzer.dart`'s import scopes) — walking them
        // transitively once let a prefixed `dart:io as io` register the
        // SDK's `File` bare and shadow the imported one, refusing the
        // `is File` promotion the oracle makes (the census's
        // blaze_watcher rows: first-writer-wins made the wrong `File`
        // answer `is_subtype(File, Resource)` false).
        for edge in graph.import_edges(lib) {
            let Some(target) = edge.target else {
                continue;
            };
            if edge.prefix.is_some() {
                continue;
            }
            for (name, entry) in graph.export_scope(target) {
                if !edge.combinators.allows(name) {
                    continue;
                }
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
        // The prefixed spellings' joined heads (`io.File`): the diagnose
        // side arms import-qualified annotations dot-joined (the intern
        // table's one-head form, the collector's own convention), so the
        // registry answers those spellings too — every DIRECT prefixed
        // import's exported container aliases its bare-keyed decl under
        // `prefix.Name`. Combinator-hidden names never alias (the written
        // spelling cannot legally name them; an alias would over-resolve
        // the hide/show boundary).
        for edge in graph.import_edges(lib) {
            let (Some(target), Some(prefix)) = (edge.target, edge.prefix) else {
                continue;
            };
            for (name, entry) in graph.export_scope(target) {
                if !edge.combinators.allows(name) {
                    continue;
                }
                // A prefixed import never publishes its target's names
                // bare, so the bare key exists only when another import
                // (or the unit itself) already registered the container.
                // A prefixed-only target still needs its declaration for
                // the alias — register the declaring file's containers
                // (the same probe the unprefixed pass runs: no model or
                // no container under the name suppresses, never borrows a
                // same-named decl), then alias the joined spelling.
                if !registry.registry.contains_key(&name)
                    && !registry.non_containers.contains(&name)
                {
                    match index.get(entry.file).map(|els| els.container(name)) {
                        Some(Some(_)) => {
                            registry.register_file(index, entry.file, target, &mut actions);
                            for action in actions.drain(..) {
                                builder = action.apply(builder, &table, core_enum);
                            }
                        }
                        Some(None) | None => {
                            registry.non_containers.insert(name);
                        }
                    }
                }
                let Some(&decl) = registry.registry.get(&name) else {
                    continue;
                };
                let joined = format!(
                    "{}.{}",
                    strings.resolve(prefix),
                    strings.resolve(name)
                );
                registry
                    .registry
                    .entry(strings.intern(&joined))
                    .or_insert(decl);
            }
        }
        // —— the supertype closure ————————————————————————————————————————
        // Name visibility above is exact (own units + direct unprefixed
        // imports), but the HIERARCHY answers over chains through
        // containers the unit cannot spell — an imported file's private
        // import of the interface its class implements (`XPathMap`
        // reaching `XPathItem` through `XPathFunctionItem`). Every
        // registered declaration's clause spellings must therefore
        // resolve — in the DECLARING library's own scope, the same
        // `namespaces.lookup` the edge pass consults — or the edge pass
        // drops the hop and `is_subtype` breaks mid-chain. The worklist
        // registers each file a clause spelling names; its own clauses
        // join the next round until a pass adds nothing. First-writer
        // -wins keeps the visible spelling's declaration authoritative:
        // a closure entry only ever fills an absent slot, never retypes
        // a name the unit itself resolves.
        loop {
            let mut grew = false;
            let mut scan: Vec<(FileId, LibraryId)> = Vec::new();
            let mut scanned: FxHashSet<(FileId, LibraryId)> = FxHashSet::default();
            for &(file, owner) in registry.decl_owner.values() {
                if scanned.insert((file, owner)) {
                    scan.push((file, owner));
                }
            }
            for (file, owner) in scan {
                let Some(elements) = index.get(file) else {
                    continue;
                };
                // The clause spellings of every container in the file:
                // the written trees' heads (recursively — a written
                // argument's head rides the same edge) beside the
                // element model's name-level clause lists. A member's
                // declared type head rides the same closure: the
                // recorder spells a member-typed variable's type bare,
                // and the oracle resolves that spelling in the
                // DECLARING file's context — a name the reading unit
                // never imports (`InternalVariableElement`, spelled by
                // src/dart/ast/ast.dart's property while the reader
                // imports no element-model src file) still answers
                // through its real declaration instead of collapsing
                // to `Invalid`'s vacuous arms.
                let mut heads: Vec<StringId> = Vec::new();
                for &name in elements.container_order() {
                    if let Some(capture) = clauses.get(file, name) {
                        for tree in &capture.supers {
                            collect_tree_heads(tree, &mut heads);
                        }
                    }
                    if let Some(container) = elements.container(name) {
                        heads.extend(
                            container
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
                                .copied(),
                        );
                        for entry in container.members.values() {
                            let ty = match entry.kind {
                                MemberKind::Field => entry.var_ty,
                                MemberKind::Getter | MemberKind::Method => entry.return_ty,
                                MemberKind::Setter => continue,
                            };
                            if let Ty::Named { name: head, .. } = ty {
                                heads.push(head);
                            }
                        }
                    }
                }
                for head in heads {
                    if registry.registry.contains_key(&head)
                        || registry.non_containers.contains(&head)
                        || registry.type_params.contains_key(&head)
                    {
                        continue;
                    }
                    let Lookup::Found(NamespaceEntry::Decl(target_file)) =
                        namespaces.lookup(graph, owner, head)
                    else {
                        continue;
                    };
                    if !index
                        .get(target_file)
                        .is_some_and(|els| els.container(head).is_some())
                    {
                        continue;
                    }
                    let target_lib = graph.library_of_file(target_file).unwrap_or(owner);
                    registry.register_file(index, target_file, target_lib, &mut actions);
                    for action in actions.drain(..) {
                        builder = action.apply(builder, &table, core_enum);
                    }
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        // —— the lossy-spelling overloads —————————————————————————————————
        // The flow layer spells every qualified type reference by its bare
        // head (`io.File` reads `File`), so a spelling a PREFIXED import
        // target exports names a class the bare question can never point
        // at — whether the unit also resolves it bare (`File` beside
        // `file_system.File`) or resolves it nowhere the snapshot sees
        // (`io.FileSystemEntity` mapping `Invalid`, whose vacuous subtype
        // arm would dead the `is io.File` arm the oracle keeps live).
        // Those spellings carry no resolved verdict: the adapter's
        // verdict gates keep the name-level default's answer over them
        // ([`ResolvedTypeOps::head_overloaded`]).
        let mut lossy_overloads: FxHashSet<StringId> = FxHashSet::default();
        for edge in graph.import_edges(lib) {
            let (Some(target), Some(_)) = (edge.target, edge.prefix) else {
                continue;
            };
            for (name, _) in graph.export_scope(target) {
                if edge.combinators.allows(name) {
                    lossy_overloads.insert(name);
                }
            }
            // A prefixed SDK import carries no element model, so its
            // export scope answers nothing — the static table stands in
            // ([`SDK_LIBRARY_TYPE_NAMES`]).
            let uri = graph.uri(target);
            for (sdk_uri, names) in SDK_LIBRARY_TYPE_NAMES {
                if uri == *sdk_uri {
                    lossy_overloads.extend(names.iter().map(|n| strings.intern(n)));
                }
            }
        }
        // The alias RHS shapes flatten after every registration pass (the
        // library's units and the import sweep) — a co-alias chain needs
        // its target's registration first.
        registry.resolve_typedef_shapes(&table, index, strings);
        let Registry {
            registry,
            decl_params,
            decl_owner,
            non_containers,
            typedef_targets,
            typedef_shapes,
            type_params,
            type_param_scope,
            param_names,
            invokable_members,
            property_ty_heads,
            mut facts,
            ..
        } = registry;
        // The object-pattern property gate's set: a collected member's
        // type head is the official's invalid face exactly when no
        // registered container, alias, or type parameter spells it (the
        // builtins registered first, so core heads always resolve).
        let invalid_property_tys: FxHashSet<(StringId, StringId)> = property_ty_heads
            .into_iter()
            .filter(|&(_, _, head)| {
                !registry.contains_key(&head)
                    && !non_containers.contains(&head)
                    && !typedef_targets.contains_key(&head)
                    && !typedef_shapes.contains_key(&head)
                    && !type_params.contains_key(&head)
            })
            .map(|(owner, member, _)| (owner, member))
            .collect();

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
        // climbs with). The name of each decl comes from the inverted
        // registry, built once (the registry is the snapshot's only
        // name→decl pairing).
        // The joined alias keys (`file_state.X`) must not invert: the
        // decl's declaration name (dot-free, the container index keys on
        // it) is the only spelling that reaches `elements.container` —
        // an inverted alias would strand the decl's supertype edges.
        let name_of_decl: FxHashMap<TypeDeclId, StringId> = registry
            .iter()
            .filter(|kv| !strings.resolve(*kv.0).contains('.'))
            .map(|(&name, &decl)| (decl, name))
            .collect();
        let scope_of = |capture: &ClauseCapture, decl: TypeDeclId| {
            let params = decl_params.get(&decl).map_or(&[][..], Vec::as_slice);
            capture
                .params
                .iter()
                .copied()
                .zip(params.iter().copied())
                .collect::<FxHashMap<StringId, TypeParamId>>()
        };
        // Pass 1 — the written parameter bounds of every captured
        // container feed `ParamFacts.bound` before any edge maps:
        // `instantiate_type_formals_to_bounds` and the bound-projecting
        // subtype arms read them, and a raw head inside another
        // container's clause (`extends Base` for `class Base<T extends
        // num>`) instantiates to the bound only when the bound is already
        // set — a single interleaved pass made that read order-dependent.
        // A bound resolves inside the container's own parameter scope
        // (`T extends Comparable<T>` names a sibling — the scope is
        // complete before any bound maps), and a bound that fails to map
        // keeps the `None` face (the open direction — an unmodeled bound
        // only ever under-reports through `instantiate_to_bounds`).
        for (&decl, &(file, _)) in &decl_owner {
            let Some(&name) = name_of_decl.get(&decl) else {
                continue;
            };
            let Some(capture) = clauses.get(file, name) else {
                continue;
            };
            let params = decl_params.get(&decl).map_or(&[][..], Vec::as_slice);
            let scope = scope_of(capture, decl);
            for (&param_id, bound_tree) in params.iter().zip(capture.param_bounds.iter()) {
                let Some(tree) = bound_tree else {
                    continue;
                };
                let bound =
                    clause_arg_ty(strings, &table, &registry, &decl_params, &facts, &scope, tree);
                facts.set(
                    param_id,
                    ParamFacts {
                        variance: Variance::Covariant,
                        bound,
                    },
                );
            }
        }
        // Pass 2 — the supertype edges.
        for (&decl, &(file, owner)) in &decl_owner {
            let Some(elements) = index.get(file) else {
                continue;
            };
            let Some(&name) = name_of_decl.get(&decl) else {
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
                let scope = scope_of(capture, decl);
                for tree in &capture.supers {
                    if let Some(ty) =
                        clause_tree_ty(strings, &table, &registry, &decl_params, &facts, &scope, tree)
                        && matches!(
                            table.get(ty).kind(),
                            TypeKind::Interface { .. }
                        )
                    {
                        builder = builder.supertype(decl, ty);
                    }
                }
                // A mixin's `on` constraints ride the capture too (the
                // listener claims them as supertype roots), so no
                // name-level join repeats them: the to-bounds
                // instantiation it would add beside the written
                // `TableInfo<TableDsl, D>` made every `TableInfo<_, _>`
                // a supertype of the mixin — the over-admitting duplicate
                // edge.
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

        // Freeze edge by edge: a malformed captured edge (an arity-mismatched
        // instantiation, a foreign parameter, a back edge of a cyclic
        // `extends` pair) drops out alone and its owner keeps the rest of
        // its face — the official hierarchy stays usable around such a
        // declaration, and a whole-snapshot fallback would blank every
        // other declaration's supertype face in the unit (a lost `is`
        // promotion in an unrelated file, #2508's drift regression). The
        // declaration-level contract never fails from source shape: every
        // registered decl is unique and the core decls are the fixed
        // sentinels, so the error arm is unreachable by construction and
        // only the bare core answers it.
        let hierarchy = match builder.build_lenient(&table) {
            Ok((hierarchy, _dropped)) => {
                hierarchy
            }
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
            lossy_overloads,
            typedef_targets,
            typedef_shapes,
            param_names,
            type_params,
            type_param_scope,
            invokable_members,
            invalid_property_tys,
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
    /// The argument-bearing/function/record/parameterized alias RHS
    /// shapes, resolved after every file registers (see
    /// [`LibraryTypes::typedef_shapes`]). Populated by
    /// [`Registry::resolve_typedef_shapes`].
    typedef_shapes: FxHashMap<StringId, TypedefShape>,
    /// Every `TypeAlias` declaration as `(name, file)` — the worklist
    /// [`Registry::resolve_typedef_shapes`] iterates to its fixpoint
    /// (a co-alias chain resolves once its target's shape lands).
    aliases: Vec<(StringId, FileId)>,
    /// Type-parameter spellings → their synthetic parameter ids: the
    /// `TypeKind::TypeParameter` map the adapter answers instead of
    /// `Invalid` for a `T`-shaped name (#2186 S4-C).
    type_params: FxHashMap<StringId, TypeParamId>,
    /// See [`LibraryTypes::type_param_scope`].
    type_param_scope: FxHashSet<(FileId, StringId)>,
    /// See [`LibraryTypes::param_names`].
    param_names: FxHashMap<TypeParamId, StringId>,
    /// See [`LibraryTypes::invokable_members`].
    invokable_members: FxHashSet<StringId>,
    /// Own-declared `(container, member, type-head)` triples collected
    /// during registration — filtered into
    /// [`LibraryTypes::invalid_property_tys`] once every name has
    /// registered (a member typed by a later-registered container must
    /// not read unresolved mid-build).
    property_ty_heads: Vec<(StringId, StringId, StringId)>,
    facts: ParamFactsTable,
    next_decl: u32,
    next_param: u32,
}

/// One alias's flattened right-hand side in the snapshot's lattice
/// ([`LibraryTypes::typedef_shapes`]). The shape interns with its own `?`
/// stripped into `nullable` — the same composition the bare chase keeps —
/// and a parameterized alias's `shape` carries its own parameters as
/// [`TypeKind::TypeParameter`] leaves: the chase zips `params` with the
/// use-site written arguments and substitutes.
struct TypedefShape {
    /// The RHS shape, interned non-null (a parameterized alias keeps its
    /// parameter leaves unsubstituted).
    shape: TypeId,
    /// The RHS's own trailing `?`.
    nullable: bool,
    /// The alias's own parameters in declaration order — the positional
    /// zip the written arguments substitute against. Empty for a
    /// non-parameterized alias.
    params: Box<[TypeParamId]>,
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
            for position in 0..usize::from(container.type_params) {
                let id = TypeParamId::from_raw(self.next_param);
                self.next_param += 1;
                param_ids.push(id);
                if let Some(&spelling) = container.type_param_names.get(position) {
                    self.param_names.insert(id, spelling);
                }
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
            // Every registered container's member names join the
            // invokable face as a whole — the over-approximation
            // `call_head_unresolved` refuses on (an instance or static
            // member spelling somewhere beats the unresolved-call face).
            self.invokable_members.extend(container.members.keys().copied());
            // The property-type heads for the object-pattern coverage
            // gate: a field's declared type / a getter's return type
            // whose head resolves nowhere makes the property sub-pattern
            // may-fail (see `LibraryTypes::invalid_property_tys`).
            for (member, entry) in container.members.iter() {
                let ty = match entry.kind {
                    MemberKind::Field => entry.var_ty,
                    MemberKind::Getter | MemberKind::Method => entry.return_ty,
                    MemberKind::Setter => continue,
                };
                if let Ty::Named { name: head, .. } = ty {
                    self.property_ty_heads.push((name, *member, head));
                }
            }
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
                // The scope face keys on the declaring file: a spelling is
                // a type parameter only inside the file that declares it.
                self.type_param_scope.insert((file, name));
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
                // Argument-carrying and `Function`/`Record` RHS shapes land
                // in `typedef_shapes` after registration; a parameterized
                // alias never chases by head (its RHS leaves name its own
                // parameters, not a terminal head).
                if decl.kind == MicroKind::TypeAlias {
                    if decl.type_params == 0
                        && let crate::model::Ty::Named {
                            name: rhs_name,
                            nullable: rhs_nullable,
                            args,
                        } = decl.var_ty
                        && args == crate::model::ArgListId::EMPTY
                    {
                        self.typedef_targets.insert(name, (rhs_name, rhs_nullable));
                    }
                    self.aliases.push((name, file));
                }
            }
        }
    }

    /// Resolves every registered alias's RHS shape to its fixpoint: a
    /// co-alias chain (`typedef B = A<int>` over `A = List<int>`)
    /// resolves once its target's shape lands, so the loop retries the
    /// unresolved entries until a pass makes no progress — bounded by the
    /// alias count, with a cyclic chain never resolving (the under-report
    /// direction, the same posture a missing shape keeps). A
    /// non-parameterized alias whose RHS is a bare `Named` registers
    /// nothing here — `typedef_targets` owns that shape and its chase
    /// re-enters the full name precedence on the terminal.
    fn resolve_typedef_shapes(
        &mut self,
        table: &TypeTable,
        index: &ElementIndex,
        strings: &StringTable,
    ) {
        for _ in 0..=self.aliases.len() {
            let mut progress = false;
            for at in 0..self.aliases.len() {
                let (name, file) = self.aliases[at];
                if self.typedef_shapes.contains_key(&name) {
                    continue;
                }
                let Some(elements) = index.get(file) else {
                    continue;
                };
                let Some(decl) = elements.decl(name) else {
                    continue;
                };
                if decl.kind != MicroKind::TypeAlias || decl.var_ty == Ty::UNKNOWN {
                    continue;
                }
                let shape_worthy = decl.type_params != 0
                    || match decl.var_ty {
                        Ty::Named { args, .. } => args != ArgListId::EMPTY,
                        Ty::Record { .. } | Ty::Function { .. } => true,
                        _ => false,
                    };
                if !shape_worthy {
                    continue;
                }
                let Some((shape, nullable)) =
                    self.flatten_rhs(table, elements.type_args(), strings, decl.var_ty, name, &decl.type_param_names)
                else {
                    continue;
                };
                let params = decl
                    .type_param_names
                    .iter()
                    .filter_map(|spelling| self.type_params.get(spelling).copied())
                    .collect();
                self.typedef_shapes.insert(
                    name,
                    TypedefShape {
                        shape,
                        nullable,
                        params,
                    },
                );
                progress = true;
            }
            if !progress {
                break;
            }
        }
    }

    /// Translates one element-model RHS (`Ty` over the declaring unit's
    /// [`ArgPool`]) into the snapshot's lattice, `?` stripped. `alias` is
    /// the alias being registered (the cycle guard — an RHS naming its own
    /// alias registers nothing) and `own_params` its parameter spellings
    /// (a bare leaf naming one reads the alias's own
    /// [`TypeKind::TypeParameter`]). `None` when some head the tree names
    /// resolves to nothing this snapshot registers: the alias keeps no
    /// shape and its uses stay opaque (the under-report direction).
    fn flatten_rhs(
        &mut self,
        table: &TypeTable,
        pool: &ArgPool,
        strings: &StringTable,
        ty: Ty,
        alias: StringId,
        own_params: &[StringId],
    ) -> Option<(TypeId, bool)> {
        match ty {
            // A bare unit face (`void`/`dynamic`/`Never`) is not a shape
            // the chase stands for — the use keeps today's opaque read.
            Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Void | Ty::Never => None,
            Ty::Record { shape, nullable } => {
                let shape = pool.record_shape(shape);
                let mut positional = Vec::with_capacity(shape.positional.len());
                for &field in &shape.positional {
                    positional.push(self.flatten_rhs(table, pool, strings, field, alias, own_params)?.0);
                }
                let mut named = Vec::with_capacity(shape.named.len());
                for &(field_name, field) in &shape.named {
                    named.push(NamedMember {
                        name: strings.resolve(field_name).to_string(),
                        ty: self.flatten_rhs(table, pool, strings, field, alias, own_params)?.0,
                        required: false,
                    });
                }
                Some((
                    table
                        .intern_record(UninternedRecord { positional, named }, false)
                        .ok()?,
                    nullable,
                ))
            }
            Ty::Function { shape, nullable } => {
                let shape = pool.function_shape(shape);
                let return_ty =
                    self.flatten_rhs(table, pool, strings, shape.return_ty, alias, own_params)?.0;
                let mut positional = Vec::with_capacity(shape.positional.len());
                for &formal in &shape.positional {
                    positional.push(self.flatten_rhs(table, pool, strings, formal, alias, own_params)?.0);
                }
                let mut named = Vec::with_capacity(shape.named.len());
                for &(formal_name, formal) in &shape.named {
                    named.push(NamedMember {
                        name: strings.resolve(formal_name).to_string(),
                        ty: self.flatten_rhs(table, pool, strings, formal, alias, own_params)?.0,
                        required: false,
                    });
                }
                // The RHS's own `<T>` group: fresh parameter ids in this
                // snapshot's space (globally unique per interned signature,
                // so no binder confusion — the same ids the function-subtype
                // correlation renames positionally).
                let mut type_parameters = Vec::new();
                for _ in 0..shape.type_params {
                    let id = TypeParamId::from_raw(self.next_param);
                    self.next_param += 1;
                    self.facts.set(
                        id,
                        ParamFacts {
                            variance: Variance::Covariant,
                            bound: None,
                        },
                    );
                    type_parameters.push(id);
                }
                let required_positional =
                    u32::try_from(positional.len()).unwrap_or(u32::MAX);
                Some((
                    table
                        .intern_function(
                            UninternedFunction {
                                return_type: return_ty,
                                type_parameters,
                                positional,
                                named,
                                required_positional,
                            },
                            false,
                        )
                        .ok()?,
                    nullable,
                ))
            }
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                // The alias's own parameter as a bare leaf: the shape keeps
                // the parameter unsubstituted (the chase zips the use-site
                // arguments against `params`).
                if args == ArgListId::EMPTY
                    && own_params.contains(&name)
                    && let Some(&parameter) = self.type_params.get(&name)
                {
                    return Some((
                        table.intern(UninternedType::TypeParameter { parameter }, false),
                        nullable,
                    ));
                }
                let mut flattened = Vec::with_capacity(pool.get(args).len());
                for &arg in pool.get(args) {
                    flattened.push(self.flatten_rhs(table, pool, strings, arg, alias, own_params)?.0);
                }
                self.flatten_named_head(table, strings, name, nullable, &flattened, alias)
            }
        }
    }

    /// Resolves one flattened RHS head: another alias (through its
    /// registered shape, or the bare-Named chase when it has none), a
    /// registered container, or a `Null`/`Never` spelling — each
    /// instantiated with the flattened written arguments. `None` keeps
    /// the enclosing alias shapeless.
    fn flatten_named_head(
        &mut self,
        table: &TypeTable,
        strings: &StringTable,
        name: StringId,
        nullable: bool,
        args: &[TypeId],
        alias: StringId,
    ) -> Option<(TypeId, bool)> {
        if name == alias {
            // A self-referential RHS never resolves.
            return None;
        }
        if self.non_containers.contains(&name) {
            if let Some(entry) = self.typedef_shapes.get(&name) {
                let (shape, nullable) = (entry.shape, entry.nullable | nullable);
                if entry.params.is_empty() {
                    return args.is_empty().then_some((shape, nullable));
                }
                if args.len() != entry.params.len() {
                    return None;
                }
                let map: Vec<(TypeParamId, TypeId)> = entry
                    .params
                    .iter()
                    .copied()
                    .zip(args.iter().copied())
                    .collect();
                return Some((table.substitute(shape, &map), nullable));
            }
            // A bare-Named alias chain (`typedef B = A;` with `A` still
            // bare-Named): chase to its terminal and instantiate that —
            // the same walk `chase_typedef` keeps at query time.
            let mut chased = name;
            let mut chased_nullable = nullable;
            for _ in 0..=self.typedef_targets.len() {
                let Some(&(next, next_nullable)) = self.typedef_targets.get(&chased) else {
                    break;
                };
                chased = next;
                chased_nullable |= next_nullable;
            }
            return self.registered_instantiation(table, chased, chased_nullable, args);
        }
        match strings.resolve(name) {
            // The two kind-level spellings the registry cannot carry (the
            // query-time `builtin_spelling` face).
            "Null" => Some((table.intern(UninternedType::Null, false), true)),
            "Never" => Some((table.intern(UninternedType::Never, nullable), false)),
            _ => self.registered_instantiation(table, name, nullable, args),
        }
    }

    /// A registered container's instantiation: the written arguments stand
    /// as given; without any, a generic head instantiates to bounds (the
    /// official `instantiateToBounds` posture the query-time bare read
    /// keeps). `None` when the name registers nowhere.
    fn registered_instantiation(
        &self,
        table: &TypeTable,
        name: StringId,
        nullable: bool,
        args: &[TypeId],
    ) -> Option<(TypeId, bool)> {
        let &decl = self.registry.get(&name)?;
        let args = if args.is_empty() {
            let params = self
                .decl_params
                .get(&decl)
                .map_or(&[][..], Vec::as_slice);
            if params.is_empty() {
                Vec::new()
            } else {
                instantiate_type_formals_to_bounds(table, &self.facts, params).defaults
            }
        } else {
            args.to_vec()
        };
        Some((
            table.intern(UninternedType::Interface { decl, args }, false),
            nullable,
        ))
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
/// Collects every named head a written supertype tree mentions — the
/// root head beside each written argument's — feeding the supertype
/// closure's registration worklist. A record/function-shaped node can
/// never map to an edge (`clause_tree_ty` refuses the shape wholesale),
/// so its subtrees carry no edge to protect.
fn collect_tree_heads(tree: &crate::capture::TypeTree, heads: &mut Vec<StringId>) {
    if tree.record.is_some() || tree.function.is_some() {
        return;
    }
    heads.push(tree.head);
    for arg in &tree.args {
        collect_tree_heads(arg, heads);
    }
}

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
/// A clause leaf's builtin spelling: `Never`, `Null`, `void` and
/// `dynamic` intern as their unit kinds, not registry declarations —
/// `class Failure extends Result<Never>` rides its `Never` argument, and
/// an unregistered-name miss would drop the whole edge where the oracle
/// carries `Result<Never>` (making `Failure <: S` undecidable). The
/// per-node `?` is honored for `Never` only (`Never?` interns as `Null`,
/// type.dart:1207-1209; `Null`/`void`/`dynamic` are always non-`?`).
fn clause_builtin_ty(table: &TypeTable, head: &str, nullable: bool) -> Option<TypeId> {
    let uninterned = match head {
        "Never" => {
            return Some(table.intern(UninternedType::Never, nullable));
        }
        "Null" => UninternedType::Null,
        "void" => UninternedType::Void,
        "dynamic" => UninternedType::Dynamic,
        _ => return None,
    };
    Some(table.intern(uninterned, false))
}

fn clause_tree_ty(
    strings: &StringTable,
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
    if tree.args.is_empty()
        && let Some(builtin) = clause_builtin_ty(table, strings.resolve(tree.head), false)
    {
        return Some(builtin);
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
            args.push(clause_arg_ty(strings, table, registry, decl_params, facts, scope, arg)?);
        }
        args
    };
    Some(table.intern(UninternedType::Interface { decl, args }, false))
}

/// The argument-node face of [`clause_tree_ty`]: identical mapping but
/// `nullable` honored per node (a `T?` argument is legal where the
/// supertype root is not).
fn clause_arg_ty(
    strings: &StringTable,
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
    if tree.args.is_empty()
        && let Some(builtin) = clause_builtin_ty(table, strings.resolve(tree.head), tree.nullable)
    {
        return Some(builtin);
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
            args.push(clause_arg_ty(strings, table, registry, decl_params, facts, scope, arg)?);
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

/// Work budget per armed type-ops call (#2636): the structural
/// termination invariant of the mapping faces, mirroring
/// `dartq_flow`'s [`STEP_BUDGET_PER_EVENT`] posture — the flow replay's
/// step budget bounds the interpreter's steps but cannot bound the work
/// inside ONE type-ops call (the 4h53m incident's trunk:
/// `expr_step` → `is_invalid_typed` → `map_name*` spinning on the
/// mapping faces). Every armed entry below — `map_name`,
/// `map_named_strict`, `is_subtype`, `is_invalid_typed`,
/// `observe_names`, `extension_applies_strict` — spends this many work
/// units (one per charged node visit or loop iteration; the structurally
/// finite scans — `observe_names`' name table, `chase_typedef`'s
/// length-bounded hops — spend one per call) before its conservative
/// bail; nested entries share the outer arming and never re-arm.
/// Measured worst legit consumption: 9 work units across the full
/// 447-file `analyzer` corpus folded file-by-file through the
/// ops-mounted harness in debug (286 s wall, 52 MB peak RSS, zero
/// bails; the worst body's nested written-type trees) — the constant
/// keeps a >110× margin, pinned by
/// `legit_mapping_work_stays_far_under_budget` (no fixture may come
/// within 10× of the budget). The budget also bounds the `map_name` →
/// `map_node` recursion depth (one nested frame pair per two units), so
/// a tripping call unwinds inside a 2 MiB thread stack long before
/// overflow: a divergent written arena bottoms out in `Invalid` (see
/// `fuel_trips_on_a_cyclic_written_arena`), never a crash.
///
/// [`STEP_BUDGET_PER_EVENT`]: dartq_flow::interp::STEP_BUDGET_PER_EVENT
const WORK_BUDGET_PER_CALL: u32 = 1_024;

/// The adapter: `dartq_flow`'s `Ty`/`ArgTy` spellings mapped onto the
/// snapshot's interned `TypeId`s, [`TypeOps`] answered through
/// [`TypeSystem`]. One instance per `fold_unit` call — the `name_ty`
/// cache keys on the unit recorder's interned ids, which are only
/// meaningful inside that unit's spelling universe.
pub struct ResolvedTypeOps<'a> {
    snapshot: &'a LibraryTypes,
    strings: &'a StringTable,
    /// The unit file the replayed member lives in — the scope face of the
    /// snapshot's type parameters ([`LibraryTypes::type_param_scope`]): a
    /// type-parameter spelling resolves only inside the file that declares
    /// it, so an SDK `List<E>`'s `E` never captures a same-named class of
    /// this unit's file (#2510).
    file: FileId,
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
    /// Flow-layer name ids whose spelling is a lossy overload — extended
    /// beside the `name_ty` cache in `observe_names`: the arg-less ops
    /// (`is_subtype`, `factor`, `try_promote_to`) carry no `names`
    /// slice, so this set is the only overload face they can read.
    overload_ids: RefCell<FxHashSet<u32>>,
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
    /// The registry inverted (`TypeDeclId` → declared spelling), built on
    /// the first structural answer that maps back into the diagnosing
    /// pool ([`Self::instance_env`]).
    decl_names: RefCell<Option<FxHashMap<TypeDeclId, StringId>>>,
    /// The parameter registry inverted (`TypeParamId` → spelling); the
    /// registry is name-keyed, so the inversion is exact.
    param_names: RefCell<Option<FxHashMap<TypeParamId, StringId>>>,
    /// Whether a per-call work budget is armed (#2636): the outermost
    /// armed entry owns it, nested entries share the same budget (see
    /// [`WORK_BUDGET_PER_CALL`]).
    work_armed: Cell<bool>,
    /// Work units the armed call has spent (one per
    /// [`Self::work_tick`]).
    work_spent: Cell<u32>,
    /// The armed call's budget in work units.
    work_budget: Cell<u32>,
    /// Armed calls that exhausted their budget so far (monotone; read by
    /// [`Self::ops_bailed`] — a stability metric, never a diagnostic).
    work_bailed: Cell<u32>,
    /// The most work units one armed call spent (read by
    /// [`Self::ops_peak_work`]).
    work_peak: Cell<u32>,
    /// Test-only per-call budget override (the trip-path tests drive a
    /// legit mapping past an artificially small budget; production never
    /// sets it).
    #[cfg(test)]
    work_budget_override: Cell<Option<u32>>,
    dynamic: TypeId,
    null: TypeId,
    void: TypeId,
    invalid: TypeId,
}

impl<'a> ResolvedTypeOps<'a> {
    /// Mounts the adapter over `snapshot`: `strings` is the shared name
    /// table the recorder's lexemes intern into (the same one the
    /// snapshot's registry keyed its `StringId`s on).
    pub fn new(snapshot: &'a LibraryTypes, strings: &'a StringTable, file: FileId) -> Self {
        let table = &snapshot.table;
        Self {
            snapshot,
            strings,
            file,
            system: TypeSystem::new(&snapshot.table, &snapshot.hierarchy),
            name_ty: RefCell::new(Vec::new()),
            overload_ids: RefCell::new(FxHashSet::default()),
            closed: RefCell::new(FxHashMap::default()),
            opaque: RefCell::new(FxHashMap::default()),
            decl_names: RefCell::new(None),
            param_names: RefCell::new(None),
            work_armed: Cell::new(false),
            work_spent: Cell::new(0),
            work_budget: Cell::new(0),
            work_bailed: Cell::new(0),
            work_peak: Cell::new(0),
            #[cfg(test)]
            work_budget_override: Cell::new(None),
            dynamic: table.intern(UninternedType::Dynamic, false),
            null: table.intern(UninternedType::Null, false),
            void: table.intern(UninternedType::Void, false),
            invalid: table.intern(UninternedType::Invalid, false),
        }
    }

    // —— #2636: per-call work fuel ———————————————————————————————————

    /// Arms the per-call work budget; the outermost entry owns it — a
    /// nested entry answers `false` and shares the caller's budget (an
    /// arming never nests; see [`WORK_BUDGET_PER_CALL`]).
    #[inline]
    fn work_enter(&self) -> bool {
        if self.work_armed.get() {
            return false;
        }
        self.work_armed.set(true);
        self.work_spent.set(0);
        #[cfg(test)]
        if let Some(budget) = self.work_budget_override.get() {
            self.work_budget.set(budget);
            return true;
        }
        self.work_budget.set(WORK_BUDGET_PER_CALL);
        true
    }

    /// Spends one work unit. `false` = the armed call's budget is
    /// exhausted (a trip — counted once per armed call): the caller
    /// takes its conservative answer, exactly the posture the flow
    /// layer's bail takes on its step budget. Every tick site sits
    /// under an armed entry, so an unarmed tick (none today) proceeds
    /// unbounded.
    #[inline]
    fn work_tick(&self) -> bool {
        if !self.work_armed.get() {
            return true;
        }
        let spent = self.work_spent.get() + 1;
        self.work_spent.set(spent);
        if spent <= self.work_budget.get() {
            return true;
        }
        if spent == self.work_budget.get() + 1 {
            self.work_bailed.set(self.work_bailed.get() + 1);
        }
        false
    }

    /// Releases the budget the owning entry armed, tracking the call's
    /// spend against [`Self::work_peak`] — the budget-sizing telemetry
    /// ([`Self::ops_peak_work`]).
    fn work_exit(&self, owner: bool) {
        if !owner {
            return;
        }
        let spent = self.work_spent.get();
        if spent > self.work_peak.get() {
            self.work_peak.set(spent);
        }
        self.work_armed.set(false);
    }

    /// The fuel trip count to compare a guarded answer against after its
    /// walk: a nested entry that tripped mid-walk mapped `Invalid`, and
    /// the vacuous 0c subtype arms over-report through it — the guarded
    /// typed ops flip conservative instead ([`Self::work_tripped_since`]).
    #[inline]
    fn work_mark(&self) -> u32 {
        self.work_bailed.get()
    }

    /// Whether a trip was counted since `mark` (see [`Self::work_mark`]).
    #[inline]
    fn work_tripped_since(&self, mark: u32) -> bool {
        self.work_bailed.get() > mark
    }

    /// Armed calls that exhausted their work budget so far (monotone).
    /// Zero after every converging analyze; a nonzero value marks a
    /// structural spin the fuel converted into a conservative answer —
    /// the engine's stability metrics face, no diagnostic rides it.
    pub fn ops_bailed(&self) -> u32 {
        self.work_bailed.get()
    }

    /// The most work units one armed call spent — the budget-sizing
    /// telemetry behind [`WORK_BUDGET_PER_CALL`]'s measured peak.
    pub fn ops_peak_work(&self) -> u32 {
        self.work_peak.get()
    }

    /// Test-only budget override (see [`Self::work_budget_override`]).
    #[cfg(test)]
    fn override_work_budget_for_test(&self, budget: u32) {
        self.work_budget_override.set(Some(budget));
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
    /// call — under the per-call work fuel (#2636): the call spends ONE
    /// unit, not one per name — the scan is structurally finite (bounded
    /// by the recorded table's length), so a name-heavy unit can neither
    /// hang here nor trip the fuel and lose its observations (a spurious
    /// trip would under-report parity); a tripped budget skips the scan
    /// wholesale (the ids stay unobserved, the base ops answer them
    /// name-level — the pre-mount posture, the under-report direction).
    /// A `Base::Name` spelling whose decl is registered maps to its
    /// to-bounds interface (interned once per nullability face); a
    /// type-parameter spelling maps to its synthetic [`TypeParameter`]
    /// id; `FutureOr` maps to `FutureOr<dynamic>` (the raw-type face
    /// `map_future_or`'s argument-less arm produces); a
    /// resolved-but-unmodeled spelling (`non_containers`) caches its
    /// argument-less opaque pair; everything else (a typedef, a local
    /// function, a synthetic `#<handle>` token, an unindexed import)
    /// caches the `Invalid` pair.
    fn observe_names(&self, names: &[Box<str>]) {
        let owner = self.work_enter();
        if self.work_tick() {
            self.observe_names_walk(names);
        }
        self.work_exit(owner);
    }

    /// [`Self::observe_names`]'s scan (structurally finite; see its fuel
    /// note).
    fn observe_names_walk(&self, names: &[Box<str>]) {
        let mut cache = self.name_ty.borrow_mut();
        let start = cache.len();
        if names.len() <= start {
            return;
        }
        cache.reserve(names.len() - start);
        for (offset, lexeme) in names[start..].iter().enumerate() {
            if self.snapshot.lossy_overloads.contains(&self.strings.intern(lexeme)) {
                self.overload_ids.borrow_mut().insert((start + offset) as u32);
            }
            let name = self.strings.intern(lexeme);
            let pair = if let Some(param) = self.scoped_param(name) {
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
                            // The foreign member-signature parameter leaf —
                            // see the same arm in [`Self::map_name`]: after
                            // this unit's scope and the visible registry
                            // both miss, the parameter face is the
                            // conservative under-report posture (#2510's
                            // precedence flip).
                            if let Some(&param) = self.snapshot.type_params.get(&name) {
                                let non_null = self.snapshot.table.intern(
                                    UninternedType::TypeParameter { parameter: param },
                                    false,
                                );
                                let nullable = self.snapshot.table.intern(
                                    UninternedType::TypeParameter { parameter: param },
                                    true,
                                );
                                return (non_null, nullable);
                            }
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
    /// first answers through its flattened RHS shape (the argument-bearing,
    /// function, record, or parameterized faces — the raw-parameter
    /// reading instantiates every parameter to its to-bounds default, the
    /// official's raw-alias posture), then chases its captured `Ty::Named`
    /// head — `typedef A = B` reads the `B` pair, so `is_subtype(A, B)`
    /// answers through the real `B <: B` edge rather than the opaque
    /// face's "unrelated" arm (the oracle resolves the alias wholesale:
    /// `C c = a` over `A a` still reports `invalid_assignment`).
    fn observed_opaque_pair(&self, name: StringId) -> (TypeId, TypeId) {
        if !self.snapshot.non_containers.contains(&name) {
            return (self.invalid, self.invalid);
        }
        if let Some(entry) = self.snapshot.typedef_shapes.get(&name) {
            // No written arguments stand at this face: every parameter
            // reads its to-bounds default (the unbounded `dynamic`).
            let map: Vec<(TypeParamId, TypeId)> = entry
                .params
                .iter()
                .map(|&parameter| (parameter, self.dynamic))
                .collect();
            let shape = self.snapshot.table.substitute(entry.shape, &map);
            let non_null = self.with_question(shape, entry.nullable);
            return (non_null, self.with_question(shape, true));
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

    /// Re-interns a snapshot type with the `?` projection composed on top
    /// (`withNullability`, type.dart:1558) — the alias chase composes the
    /// RHS's own suffix with the use site's. `false` returns the type
    /// as-is (the flattened shapes intern non-null); a unit kind
    /// normalizes at the intern (`Never?` reads `Null`).
    fn with_question(&self, ty: TypeId, nullable: bool) -> TypeId {
        if !nullable {
            return ty;
        }
        // The signature and record payloads rebuild before the kind match —
        // both intern through their own entries, not `UninternedType`.
        if let TypeKind::Function { signature } = self.snapshot.table.get(ty).kind() {
            let sig = self.snapshot.table.signature(signature);
            return self
                .snapshot
                .table
                .intern_function(
                    UninternedFunction {
                        return_type: sig.return_type,
                        type_parameters: self
                            .snapshot
                            .table
                            .function_type_parameters(sig.type_parameters),
                        positional: self.snapshot.table.args(sig.positional),
                        named: self.snapshot.table.named_members(sig.named),
                        required_positional: sig.required_positional,
                    },
                    true,
                )
                .expect("a pooled signature re-interns");
        }
        if let TypeKind::Record { shape } = self.snapshot.table.get(ty).kind() {
            let shape = self.snapshot.table.record_shape(shape);
            return self
                .snapshot
                .table
                .intern_record(
                    UninternedRecord {
                        positional: self.snapshot.table.args(shape.positional),
                        named: self.snapshot.table.named_members(shape.named),
                    },
                    true,
                )
                .expect("a pooled record shape re-interns");
        }
        let un = match self.snapshot.table.get(ty).kind() {
            TypeKind::Never => UninternedType::Never,
            TypeKind::Dynamic => UninternedType::Dynamic,
            TypeKind::Void => UninternedType::Void,
            TypeKind::Invalid => UninternedType::Invalid,
            TypeKind::Null => UninternedType::Null,
            TypeKind::Interface { decl, args } => UninternedType::Interface {
                decl,
                args: self.snapshot.table.args(args),
            },
            TypeKind::TypeParameter { parameter } => UninternedType::TypeParameter { parameter },
            TypeKind::FutureOr { base } => UninternedType::FutureOr { base },
            TypeKind::Function { .. } | TypeKind::Record { .. } => {
                unreachable!("handled above")
            }
        };
        self.snapshot.table.intern(un, true)
    }

    /// One written name's resolved mapping — the fueled entry (#2636):
    /// arms the per-call work budget ([`WORK_BUDGET_PER_CALL`]) around
    /// [`Self::map_name_walk`]. A tripped budget answers the `Invalid`
    /// face — the same type an unresolved head maps to today, the
    /// unknown-is-top posture's suppress arm — never a wrong resolved
    /// fact; nested entries (the chase terminal's re-entry, every
    /// `map_node` visit) share this budget, so a divergent mapping
    /// bottoms out here instead of spinning.
    fn map_name(
        &self,
        lexeme: &str,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
        nullable: bool,
    ) -> TypeId {
        let owner = self.work_enter();
        let mapped = if self.work_tick() {
            self.map_name_walk(lexeme, args, arena, names, nullable)
        } else {
            self.invalid
        };
        self.work_exit(owner);
        mapped
    }

    /// [`Self::map_name`]'s resolution walk (the shared core of
    /// [`ResolvedTypeOps::map_scrutinee`] — a probe/typed-op `Base::Name`
    /// with its own written argument list — and
    /// [`ResolvedTypeOps::map_node`] — a nested argument tree). Precedence
    /// mirrors the official scope chain: a type parameter, the
    /// `FutureOr` pseudo type, a registered container, the
    /// `Null`/`Never` kind spellings, the resolved-but-unmodeled opaque
    /// declaration, and finally `Invalid`.
    fn map_name_walk(
        &self,
        lexeme: &str,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
        nullable: bool,
    ) -> TypeId {
        let name = self.strings.intern(lexeme);
        if let Some(param) = self.scoped_param(name) {
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
        if registered.is_none() && let Some(&param) = self.snapshot.type_params.get(&name) {
            // A spelling no container registers but SOME file declares as a
            // type parameter — the member-signature leaf of a foreign
            // generic (`Iterable<E>`'s `E` reaching the flow layer through
            // a member type the receiver instantiation never substituted).
            // It resolves nowhere in this unit's scope, so the parameter
            // face — conservative in every subtype question — is the
            // under-report posture that keeps the oracle's `is`-arms live.
            // The registry arm below runs FIRST precisely so a same-spelled
            // visible class wins over a foreign parameter (dart:core's
            // `List<E>` must not capture a user class named `E`, #2510).
            return self
                .snapshot
                .table
                .intern(UninternedType::TypeParameter { parameter: param }, nullable);
        }
        if let Some(&decl) = registered {
            let Some(mapped_args) = self.map_args(args, arena, names, decl) else {
                // The arity veto recovers instead of erasing: the official
                // `_buildTypeArguments` (upstream Dart SDK source,
                // `pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart`)
                // reports `wrong_number_of_type_arguments` and fills
                // `InvalidType` into every parameter position, so the
                // annotation reads `Response<InvalidType>` — still an
                // `InterfaceType`, not the whole-face `Invalid` whose 0c
                // arm vacuums every subtype question (the oracle keeps
                // `Response<int, int> x is Missing` dead-else but decidable
                // against a resolved head, probed 3.47.1). The tested side
                // carries the same recovery ([`Self::map_tested`]).
                let params = self
                    .snapshot
                    .decl_params
                    .get(&decl)
                    .map_or(&[][..], Vec::as_slice);
                return self.snapshot.table.intern(
                    UninternedType::Interface {
                        decl,
                        args: vec![self.invalid; params.len()],
                    },
                    nullable,
                );
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
            // The flattened RHS shapes land first: an argument-bearing,
            // function, record, or parameterized alias use reads its RHS
            // whole — the written arguments substitute the alias's own
            // parameters positionally (`typedef P<T> = Map<String, T>`
            // spelled `P<int>` reads `Map<String, int>`), a missing tail
            // argument reads its to-bounds default, and the use-site `?`
            // composes with the RHS's own.
            if let Some(entry) = self.snapshot.typedef_shapes.get(&name) {
                let written = arena.get(args as usize).map_or(&[][..], Vec::as_slice);
                let mut mapped: Vec<TypeId> = written
                    .iter()
                    .map(|arg| self.map_node(arg, arena, names))
                    .collect();
                mapped.resize(entry.params.len(), self.dynamic);
                let map: Vec<(TypeParamId, TypeId)> = entry
                    .params
                    .iter()
                    .copied()
                    .zip(mapped)
                    .collect();
                let shape = self.snapshot.table.substitute(entry.shape, &map);
                return self.with_question(shape, entry.nullable || nullable);
            }
            // A bare `typedef A = B` chases its aliased head before the
            // opaque face — the oracle resolves `A` to `B` wholesale
            // (`B f(A a) => a` is silent, `C c = a` still reports
            // `invalid_assignment`).
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

    // —— #2508: the strict `Ty` → `TypeId` map —————————————————————————
    // The extension application's structural face. Unlike `map_name`'s
    // every-name-answers contract — which resolves unknown spellings to
    // `Invalid` or the opaque decl so flow probes keep an answer — this
    // map answers `Some` only where the snapshot carries a REAL fact:
    // registered containers and builtins, declared type parameters, the
    // unit kinds. Anything else (the `Ty::Unknown` sentinel, a
    // `non_containers` spelling, an unresolvable head, a record or
    // function shape, an arity mismatch) maps `None` and the caller
    // degrades to the name-level logic — the `Invalid` arm's vacuous
    // `true` must never leak into the applicability/specificity gates.

    /// Maps a diagnosing `Ty` to the snapshot's `TypeId`, `None` where the
    /// structural world carries no fact for it.
    ///
    /// `pool` is the boundary pool the `Ty`'s argument lists index — the
    /// caller projects foreign `Ty`s through [`Boundary::ty`] first, so a
    /// list here always resolves in the diagnosing universe.
    pub(crate) fn map_ty_strict(
        &self,
        ty: Ty,
        pool: &crate::model::ArgPool,
    ) -> Option<TypeId> {
        match ty {
            Ty::Unknown => None,
            Ty::Null => Some(self.null),
            Ty::Dynamic => Some(self.dynamic),
            Ty::Never => Some(
                self.snapshot
                    .table
                    .intern(UninternedType::Never, false),
            ),
            Ty::Void => Some(self.void),
            Ty::Named {
                name,
                nullable,
                args,
            } => self.map_named_strict(name, nullable, args, pool),
            // The structural snapshot models records and functions, but
            // the diagnosing `Ty` shapes live in a different pool — the
            // shape translation is M2 work and the caller's name-level
            // read answers for them today (the fallback discipline).
            Ty::Record { .. } | Ty::Function { .. } => None,
        }
    }

    /// The `Ty::Named` arm of [`Self::map_ty_strict`]: a type parameter, the
    /// `FutureOr` pseudo type, a registered container, or the `Null`/`Never`
    /// spellings — precedence mirroring [`Self::map_name`]. The fueled
    /// entry (#2636): arms the per-call work budget around
    /// [`Self::map_named_strict_walk`], and a tripped budget answers
    /// `None` — no structural fact, the caller keeps the name-level
    /// verdict (the under-report direction).
    fn map_named_strict(
        &self,
        name: StringId,
        nullable: bool,
        args: crate::model::ArgListId,
        pool: &crate::model::ArgPool,
    ) -> Option<TypeId> {
        let owner = self.work_enter();
        let mapped = if self.work_tick() {
            self.map_named_strict_walk(name, nullable, args, pool)
        } else {
            None
        };
        self.work_exit(owner);
        mapped
    }

    /// [`Self::map_named_strict`]'s resolution walk.
    fn map_named_strict_walk(
        &self,
        name: StringId,
        nullable: bool,
        args: crate::model::ArgListId,
        pool: &crate::model::ArgPool,
    ) -> Option<TypeId> {
        if let Some(param) = self.scoped_param(name) {
            return Some(
                self.snapshot
                    .table
                    .intern(UninternedType::TypeParameter { parameter: param }, nullable),
            );
        }
        let lexeme = self.strings.resolve(name);
        if lexeme == "FutureOr" {
            let written = pool.get(args);
            let base = if written.is_empty() {
                self.dynamic
            } else if written.len() == 1 {
                self.map_ty_strict(written[0], pool)?
            } else {
                return None;
            };
            return Some(
                self.snapshot
                    .table
                    .intern(UninternedType::FutureOr { base }, nullable),
            );
        }
        let registered = self
            .snapshot
            .registry
            .get(&name)
            .filter(|_| !self.snapshot.non_containers.contains(&name));
        if registered.is_none() && let Some(&param) = self.snapshot.type_params.get(&name) {
            // The foreign member-signature parameter leaf — see the same
            // arm in [`Self::map_name`]: after this unit's scope and the
            // visible registry both miss, the parameter face is the
            // conservative under-report posture (#2510's precedence flip).
            return Some(
                self.snapshot
                    .table
                    .intern(UninternedType::TypeParameter { parameter: param }, nullable),
            );
        }
        if let Some(&decl) = registered {
            let params = self
                .snapshot
                .decl_params
                .get(&decl)
                .map_or(&[][..], Vec::as_slice);
            let mapped: Vec<TypeId> = if args == crate::model::ArgListId::EMPTY {
                // A bare head instantiates to bounds exactly like the
                // `map_name` registered arm (`Map` reads `Map<dynamic,
                // dynamic>` under the unbounded defaults).
                if params.is_empty() {
                    Vec::new()
                } else {
                    instantiate_type_formals_to_bounds(
                        &self.snapshot.table,
                        &self.snapshot.facts,
                        params,
                    )
                    .defaults
                }
            } else {
                let written = pool.get(args);
                if written.len() != params.len() {
                    return None;
                }
                let mut mapped = Vec::with_capacity(written.len());
                for &arg in written {
                    mapped.push(self.map_ty_strict(arg, pool)?);
                }
                mapped
            };
            return Some(
                self.snapshot
                    .table
                    .intern(UninternedType::Interface { decl, args: mapped }, nullable),
            );
        }
        self.builtin_spelling(lexeme, nullable)
    }

    /// The structural subtype answer over two diagnosing `Ty`s: `Some`
    /// where both sides map into the snapshot, `None` where either carries
    /// no structural fact (the caller's name-level fallback keeps the
    /// question).
    pub(crate) fn try_subtype_strict(
        &self,
        sub: Ty,
        sup: Ty,
        pool: &crate::model::ArgPool,
    ) -> Option<bool> {
        Some(
            self.system
                .is_subtype(self.map_ty_strict(sub, pool)?, self.map_ty_strict(sup, pool)?),
        )
    }

    /// Whether `ty` mentions a `TypeParameter` leaf anywhere in its
    /// structure — the strict path's decline signal: `is_subtype`'s
    /// M1 parameter arms answer `false` where the oracle reads the
    /// parameter's own bound, so a pair carrying one keeps the
    /// name-level verdict (the under-report direction).
    pub(crate) fn mentions_param(&self, ty: TypeId) -> bool {
        let table = &self.snapshot.table;
        match table.get(ty).kind() {
            TypeKind::TypeParameter { .. } => true,
            TypeKind::Interface { args, .. } => {
                table.args(args).iter().any(|&a| self.mentions_param(a))
            }
            TypeKind::FutureOr { base } => self.mentions_param(base),
            TypeKind::Function { signature } => {
                let sig = table.signature(signature);
                self.mentions_param(sig.return_type)
                    || table
                        .args(sig.positional)
                        .iter()
                        .any(|&a| self.mentions_param(a))
                    || table
                        .named_members(sig.named)
                        .iter()
                        .any(|member| self.mentions_param(member.ty))
            }
            TypeKind::Record { shape } => {
                let shape = table.record_shape(shape);
                table
                    .args(shape.positional)
                    .iter()
                    .any(|&a| self.mentions_param(a))
                    || table
                        .named_members(shape.named)
                        .iter()
                        .any(|member| self.mentions_param(member.ty))
            }
            _ => false,
        }
    }

    /// `is_subtype` over already-mapped ids — the caller's own guard ran
    /// first, so the question stands on mapped facts alone.
    pub(crate) fn subtype_ids(&self, sub: TypeId, sup: TypeId) -> bool {
        self.system.is_subtype(sub, sup)
    }

    /// `Substitution.fromPairs` over the snapshot's interned ids — the
    /// bound-check's `bound[T := arg]` instantiation.
    pub(crate) fn substitute(&self, ty: TypeId, map: &[(TypeParamId, TypeId)]) -> TypeId {
        self.snapshot.table.substitute(ty, map)
    }

    /// The `TypeParamId` a written parameter name lands on in this
    /// snapshot, scoped to `file` — the file that declares the parameter
    /// (`type_params` is name-keyed across the library, but a spelling is
    /// a parameter only inside its declaring file, #2510). `None` for a
    /// name no declaration of `file` registers.
    pub(crate) fn param_id(&self, name: StringId, file: FileId) -> Option<TypeParamId> {
        self.snapshot
            .type_params
            .get(&name)
            .filter(|_| self.snapshot.type_param_scope.contains(&(file, name)))
            .copied()
    }

    /// [`Self::param_id`] against the replayed unit's own file — the
    /// scope every bare spelling the flow layer interns resolves in.
    fn scoped_param(&self, name: StringId) -> Option<TypeParamId> {
        self.param_id(name, self.file)
    }

    /// Step-5's instantiate-to-bounds projection
    /// (`extension_member_resolver.dart:459-466` `_instantiateToBounds`)
    /// over a mapped `on` type: every bare face-parameter leaf is replaced
    /// by that parameter's declared bound — an unbounded parameter by
    /// `dynamic` (the Dart reading of unbounded), a written bound as the
    /// FACE declares it (`face_params` carries each parameter's own mapped
    /// bound: the snapshot's `type_params` registry is name-keyed, so two
    /// faces spelling the same `DT` would otherwise share one
    /// `ParamFacts.bound` — `BaseAggregate<DT extends Object>` and
    /// `ArithmeticAggregates<DT extends num>` both `on Expression<DT>`
    /// must project to `Expression<Object>` and `Expression<num>`
    /// respectively, never to one shared face). `T extends Comparable<T>`
    /// substitutes as written, the residual parameter riding as the
    /// unmodeled shape the subtype arm leaves unordered. `None` when
    /// `on_ty` maps no structural face — the caller keeps the name-level
    /// projection.
    pub(crate) fn bounds_projection(
        &self,
        on_ty: Ty,
        face_params: &[(TypeParamId, Option<TypeId>)],
        pool: &crate::model::ArgPool,
    ) -> Option<TypeId> {
        if face_params.is_empty() {
            return None;
        }
        let on = self.map_ty_strict(on_ty, pool)?;
        let mut facts = ParamFactsTable::new();
        for &(id, bound) in face_params {
            facts.set(
                id,
                ParamFacts {
                    variance: Variance::Covariant,
                    bound,
                },
            );
        }
        let ids: Vec<TypeParamId> = face_params.iter().map(|(id, _)| *id).collect();
        let instantiation =
            instantiate_type_formals_to_bounds(&self.snapshot.table, &facts, &ids);
        let map: Vec<(TypeParamId, TypeId)> = ids
            .iter()
            .copied()
            .zip(instantiation.defaults.iter().copied())
            .collect();
        Some(self.snapshot.table.substitute(on, &map))
    }

    /// The structural answer over two instantiate-to-bounds projections —
    /// step 5's pair test: `(sub <: sup, sup <: sub)` computed over the
    /// two mapped projections in one pass. `None` where either side
    /// carries no structural fact.
    pub(crate) fn try_bounds_subtype(
        &self,
        sub_on_ty: Ty,
        sub_face_params: &[(TypeParamId, Option<TypeId>)],
        sup_on_ty: Ty,
        sup_face_params: &[(TypeParamId, Option<TypeId>)],
        pool: &crate::model::ArgPool,
    ) -> Option<(bool, bool)> {
        let sub = self.bounds_projection(sub_on_ty, sub_face_params, pool)?;
        let sup = self.bounds_projection(sup_on_ty, sup_face_params, pool)?;
        Some((
            self.system.is_subtype(sub, sup),
            self.system.is_subtype(sup, sub),
        ))
    }

    /// Instantiates an extension's `on` type against the receiver's
    /// resolved instantiation of the `on` declaration
    /// (`applicable_extensions.dart:223-240`: `constrainArgument` binds
    /// the extended type's fresh parameters to the receiver's
    /// projection), gates every bound instantiation through its declared
    /// bound (the inference's `tryChooseFinalTypes` fails a face whose
    /// argument violates the parameter's bound — `List<String>` never
    /// applies `ListToJSArray<T extends JSAny?>`), then answers whether
    /// the receiver is a subtype of the instantiated `on` type.
    /// `face_params` are the extension's own type parameters (the ids
    /// [`Self::param_id`] resolves) with their mapped bounds — `None` for
    /// an unbounded parameter.
    ///
    /// `None` where the projection or the subtype question has no
    /// structural fact: `receiver`'s supertype closure never reaches the
    /// `on` declaration (the `reaches` gate already rejected it — this
    /// arm under-reports the same direction), a shape inside the `on`
    /// type maps no parameter, or a bound question still mentions a
    /// parameter after the substitution. The fueled entry (#2636): a
    /// tripped budget answers `None` the same way — including the
    /// binding walk's documented divergent arm, which the fuel makes
    /// real (a hierarchy whose instantiations keep minting fresh ids
    /// declines instead of expanding).
    pub(crate) fn extension_applies_strict(
        &self,
        receiver: Ty,
        on_ty: Ty,
        face_params: &[(TypeParamId, Option<TypeId>)],
        pool: &crate::model::ArgPool,
    ) -> Option<bool> {
        let owner = self.work_enter();
        let applies = if self.work_tick() {
            self.extension_applies_strict_walk(receiver, on_ty, face_params, pool)
        } else {
            None
        };
        self.work_exit(owner);
        applies
    }

    /// [`Self::extension_applies_strict`]'s walk.
    fn extension_applies_strict_walk(
        &self,
        receiver: Ty,
        on_ty: Ty,
        face_params: &[(TypeParamId, Option<TypeId>)],
        pool: &crate::model::ArgPool,
    ) -> Option<bool> {
        let receiver = self.map_ty_strict(receiver, pool)?;
        let on = self.map_ty_strict(on_ty, pool)?;
        let ids: Vec<TypeParamId> = face_params.iter().map(|(id, _)| *id).collect();
        let (instantiated, map) = self.unify_extended(receiver, on, &ids)?;
        for (param, bound) in face_params {
            let Some(bound) = bound else { continue };
            let Some(&(_, instantiation)) =
                map.iter().find(|(bound_param, _)| bound_param == param)
            else {
                continue;
            };
            let bound = self.substitute(*bound, &map);
            if self.mentions_param(bound) || self.mentions_param(instantiation) {
                return None;
            }
            if !self.system.is_subtype(instantiation, bound) {
                return Some(false);
            }
        }
        Some(self.system.is_subtype(receiver, instantiated))
    }

    /// `constrainArgument` over the `on` type's parameter leaves: walks
    /// `receiver`'s instantiated supertype closure to the `on`
    /// declaration, then binds each `TypeParameter` leaf in the `on`
    /// type's argument positions to the receiver's projected argument —
    /// `Box<int>` reaching `Iterable<Wrapper<int>>` against `on
    /// Iterable<T>` binds `T := Wrapper<int>` (the hop's argument
    /// projection the name-level read cannot express).
    ///
    /// Answers the instantiated `on` type beside the binding map it was
    /// built from (the bound gate reads the map). `None` when `receiver`
    /// is not an `Interface`, the `on` type is not an `Interface` with the
    /// receiver in its closure, or the binding walk diverges — the caller
    /// keeps the name-level answer.
    fn unify_extended(
        &self,
        receiver: TypeId,
        on: TypeId,
        face_params: &[TypeParamId],
    ) -> Option<(TypeId, Vec<(TypeParamId, TypeId)>)> {
        let table = &self.snapshot.table;
        let TypeKind::Interface { decl: on_decl, args: on_args } =
            table.get(on).kind()
        else {
            // A non-interface `on` type has no parameter positions to
            // bind — the subtype answer stands on it as written.
            return Some((on, Vec::new()));
        };
        let TypeKind::Interface {
            decl: recv_decl,
            args: recv_args,
        } = table.get(receiver).kind()
        else {
            return None;
        };
        // The receiver's instantiation of `on_decl`: a depth-first walk
        // over instantiated supertype edges mirroring `interface_walk`'s
        // order (LIFO pops, declaration-order pushes — first match decides,
        // sound under Dart's single-instantiation invariant). Each edge is
        // a template over its OWNER's parameters; `table.substitute`
        // instantiates it with the frontier entry's concrete arguments.
        let projected: Vec<TypeId> = if recv_decl == on_decl {
            table.args(recv_args)
        } else {
            let mut seen: FxHashSet<TypeId> = FxHashSet::default();
            let mut frontier: Vec<TypeId> = Vec::new();
            if let Some(info) = self.system.hierarchy().decl(recv_decl) {
                let map: Vec<(TypeParamId, TypeId)> = info
                    .type_parameters
                    .iter()
                    .copied()
                    .zip(table.args(recv_args))
                    .collect();
                for &edge in &info.supertypes {
                    frontier.push(table.substitute(edge, &map));
                }
            }
            let mut found = None;
            while let Some(current) = frontier.pop() {
                if !self.work_tick() {
                    // Fuel trip (#2636): the walk's `seen` guard bounds
                    // revisits, not growth — a hierarchy whose instantiations
                    // keep minting fresh ids would expand forever. Decline
                    // (the caller keeps the name-level answer).
                    return None;
                }
                if !seen.insert(current) {
                    continue;
                }
                let TypeKind::Interface { decl, args } = table.get(current).kind() else {
                    continue;
                };
                if decl == on_decl {
                    found = Some(table.args(args));
                    break;
                }
                let Some(info) = self.system.hierarchy().decl(decl) else {
                    continue;
                };
                let map: Vec<(TypeParamId, TypeId)> = info
                    .type_parameters
                    .iter()
                    .copied()
                    .zip(table.args(args))
                    .collect();
                for &edge in &info.supertypes {
                    frontier.push(table.substitute(edge, &map));
                }
            }
            found?
        };
        // Positional unification: each `on` argument's face-parameter leaf
        // binds the receiver's projected argument at the same position
        // (`on Map<K, List<V>>` vs `Map<K, List<RV>>` descends into the
        // nested group — `constrainArgument`'s interface shape).
        let on_args = table.args(on_args);
        if on_args.len() != projected.len() {
            return None;
        }
        let mut map: Vec<(TypeParamId, TypeId)> = Vec::new();
        for (&on_arg, &proj) in on_args.iter().zip(projected.iter()) {
            Self::bind_face_params(table, on_arg, proj, face_params, &mut map);
        }
        Some((table.substitute(on, &map), map))
    }

    /// Descends `on_arg` binding every face-parameter leaf to `proj` —
    /// `constrainArgument`'s parameter-position arm: a bare `TypeParameter`
    /// binds directly, a nested interface group descends argument-wise
    /// (same decl, matching arity), every other shape binds nothing.
    fn bind_face_params(
        table: &TypeTable,
        on_arg: TypeId,
        proj: TypeId,
        face_params: &[TypeParamId],
        map: &mut Vec<(TypeParamId, TypeId)>,
    ) {
        if let TypeKind::TypeParameter { parameter } = table.get(on_arg).kind() {
            if face_params.contains(&parameter) {
                // `constrainArgument`'s nullable absorption (upstream Dart
                // SDK source, extension_member_resolver.dart: `P? :> A?`
                // constrains `P` against `A`'s non-null face — `on
                // Iterable<T?>` over `Iterable<String?>` binds `T := String`,
                // so `T extends Object` holds and the face applies). A
                // nullable parameter leaf strips the projected argument's
                // own `?` before binding; a non-null leaf binds verbatim.
                let binding = if table.get(on_arg).is_nullable() {
                    non_nullable_of(table, proj)
                } else {
                    proj
                };
                map.push((parameter, binding));
            }
            return;
        }
        let (
            TypeKind::Interface {
                decl: on_decl,
                args: on_args,
            },
            TypeKind::Interface {
                decl: proj_decl,
                args: proj_args,
            },
        ) = (table.get(on_arg).kind(), table.get(proj).kind())
        else {
            return;
        };
        if on_decl != proj_decl {
            return;
        }
        let on_args = table.args(on_args);
        let proj_args = table.args(proj_args);
        if on_args.len() != proj_args.len() {
            return;
        }
        for (&on_arg, &proj) in on_args.iter().zip(proj_args.iter()) {
            Self::bind_face_params(table, on_arg, proj, face_params, map);
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

    /// Whether one spelled head is a lossy overload — a spelling a
    /// prefixed import target exports, which the flow layer's bare-head
    /// type question can never attribute (`io.File` reads `File`). Such
    /// spellings carry no resolved verdict: whichever declaration the
    /// bare registry holds (or none — the vacuous `Invalid`), the
    /// source may have meant the prefixed class, so the verdict gates
    /// keep the name-level default's answer instead.
    fn head_overloaded(&self, ty: FlowTy, names: &[Box<str>]) -> bool {
        let Base::Name(id) = ty.base else {
            return false;
        };
        names.get(id as usize).is_some_and(|lexeme| {
            self.snapshot
                .lossy_overloads
                .contains(&self.strings.intern(lexeme))
        })
    }

    /// [`Self::head_overloaded`]'s id-keyed face for the arg-less ops:
    /// the id set `observe_names` extended beside the name cache.
    fn id_overloaded(&self, ty: FlowTy) -> bool {
        let Base::Name(id) = ty.base else {
            return false;
        };
        self.overload_ids.borrow().contains(&id)
    }

    /// [`ResolvedTypeOps::is_subtype`]'s walk (#2636): a lossy-overload id
    /// answers name-level ([`Self::overload_ids`]) — the resolved verdict
    /// would ride whichever class registered bare, or the vacuous
    /// `Invalid` of an unregistered spelling, and the source may have
    /// meant the prefixed class.
    fn is_subtype_walk(&self, left: FlowTy, right: FlowTy) -> bool {
        if self.id_overloaded(left) || self.id_overloaded(right) {
            return NameTypeOps.is_subtype(left, right);
        }
        match (self.map_base(left), self.map_base(right)) {
            (Some(l), Some(r)) => self.system.is_subtype(l, r),
            _ => NameTypeOps.is_subtype(left, right),
        }
    }

    /// [`ResolvedTypeOps::is_invalid_typed`]'s walk (#2636 — see the
    /// trait method's doc).
    fn is_invalid_typed_walk(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        self.observe_names(names);
        if self.id_overloaded(ty) {
            return false;
        }
        matches!(
            self.snapshot
                .table
                .get(self.map_scrutinee(ty, args, arena, names))
                .kind(),
            TypeKind::Invalid
        )
    }

    /// [`ResolvedTypeOps::factor_typed`]'s walk (#2636): the
    /// fuel-guarded remainder rules — the outer call flips `from` whole
    /// on any trip, so the rules below run only over facts a converging
    /// mapping produced.
    fn factor_typed_walk(
        &self,
        from: FlowTy,
        from_args: u32,
        what: FlowTy,
        what_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> FlowTy {
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
        // Rule 3 of factorType (factor_type.dart:33-41): `FutureOr<R>`
        // factors as `Future<R>` when `R <: what`, and as `R` when
        // `Future<R> <: what`. The `Future<R>` remainder reuses the
        // FutureOr node's own argument list, which is exactly `[R]`.
        if let Base::Name(id) = from.base
            && names.get(id as usize).is_some_and(|n| &**n == "FutureOr")
            && let Some(&arg) = arena.get(from_args as usize).and_then(|a| a.first())
        {
            let inner = FlowTy {
                base: arg.base,
                nullable: arg.nullable,
            };
            let r = self.map_scrutinee(inner, arg.args, arena, names);
            let future = names.iter().position(|n| &**n == "Future");
            if self.system.is_subtype(r, w)
                && let Some(future) = future
            {
                return FlowTy {
                    base: Base::Name(future as u32),
                    nullable: false,
                };
            }
            if let Some(future) = future {
                let future_ty = FlowTy {
                    base: Base::Name(future as u32),
                    nullable: false,
                };
                if self.system.is_subtype(self.map_scrutinee(future_ty, from_args, arena, names), w) {
                    return inner;
                }
            }
        }
        from
    }

    /// Maps one `ArgTy` node (recursion for `FutureOr`); `names` and
    /// `arena` are the live faces `is_always_exhaustive` carries. A node
    /// whose name resolves to nothing maps `invalid` — the argument the
    /// enclosing `Interface` arm carries in place of a resolvable type.
    /// Each visit spends one work unit against the enclosing armed
    /// entry's budget (#2636): a written-argument arena that references
    /// itself (the recorder-bug class) bottoms out in `invalid` here
    /// instead of recursing forever.
    fn map_node(&self, node: &ArgTy, arena: &[Vec<ArgTy>], names: &[Box<str>]) -> TypeId {
        if !self.work_tick() {
            return self.invalid;
        }
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
    /// Only the registered-container spelling recovers. A
    /// resolved-but-unmodeled name keeps its opaque face; a bare spelling
    /// instantiates to bounds instead of vetoing, so `row.is_empty()` and
    /// the equal-count case never reach the recovery.
    ///
    /// An unresolved head (`x is Missing`) maps to the whole-face `Invalid`
    /// for the mapping's shape, but the is-check FOLDS refuse it before any
    /// subtype question runs ([`Self::tested_head_undefined`] — #2450: the
    /// oracle reports `type_test_with_undefined_name` on the spelling and
    /// keeps both branches live; the 0c vacuous answer never fires).
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

    /// Whether the is-check's tested spelling heads with a name that
    /// resolves nowhere — the oracle's undefined-name face. The same
    /// resolution chain [`Self::map_name`] walks decides: a type
    /// parameter, the `FutureOr` pseudo type, a registered container, the
    /// `Null`/`Never` kind spellings, or a resolved-but-unmodeled
    /// (`non_containers`) name all resolve; anything else is the
    /// undefined head the official's `named_type_resolver.dart` reports
    /// `type_test_with_undefined_name` on.
    ///
    /// #2450 counter-finding: an is-check whose tested type is that face
    /// never folds. The oracle's flow bails — no constant verdict, no
    /// promotion, `factorType` keeps the operand whole (probe pair
    /// `x is Missing` / `x is! Missing`: the code row fires on the
    /// spelling and neither branch reports dead). The pre-#2450 posture
    /// let the mapped `Invalid` reach the 0c arm (subtype.dart:274-276)
    /// and answer vacuously, over-reporting the dead branch.
    fn tested_head_undefined(&self, ty: FlowTy, names: &[Box<str>]) -> bool {
        let Base::Name(id) = ty.base else {
            return false;
        };
        let Some(lexeme) = names.get(id as usize) else {
            return false;
        };
        if matches!(&**lexeme, "FutureOr" | "Null" | "Never") {
            return false;
        }
        let name = self.strings.intern(lexeme);
        if self.scoped_param(name).is_some()
            || self.snapshot.non_containers.contains(&name)
        {
            return false;
        }
        !self.snapshot.registry.contains_key(&name)
    }
}

impl TypeOps for ResolvedTypeOps<'_> {
    fn is_resolved_seam(&self) -> bool {
        true
    }

    /// `is_subtype` over the resolved world where both sides map; either
    /// side unmapped answers through [`NameTypeOps`] — today's name-level
    /// logic — so a `var`-inferred or unresolvable spelling keeps its
    /// pre-mount answer and a resolved pair gains the real hierarchy
    /// (`Resource` <: `Object`, `File` <: `Resource` cross-file). The
    /// fueled entry (#2636): a tripped budget answers `false` — "not
    /// provable" — which every consumer reads conservatively: the join
    /// answers `UNKNOWN`, the non-promotion filter keeps its rows, no
    /// fold or promotion fires; a check gating an error withholds the
    /// error, one gating an "unnecessary" hint withholds the hint
    /// (under-report both ways, exactly the flow bail's posture).
    fn is_subtype(&self, left: FlowTy, right: FlowTy) -> bool {
        let owner = self.work_enter();
        let verdict = if self.work_tick() {
            self.is_subtype_walk(left, right)
        } else {
            false
        };
        self.work_exit(owner);
        verdict
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
    ///
    /// A lossy-overload head never answers `Invalid`: the bare spelling
    /// names a class a PREFIXED import target exports (the graph carries
    /// no element model for it), so the oracle's world resolves it — the
    /// "resolves nowhere" claim needs the bare spelling to be the
    /// resolved one, and the name-level default (`false`) stands. The
    /// fueled entry (#2636): the trait's own trip answers `false` ("not
    /// invalid" — the pattern's type test proceeds as the name-level
    /// default), and a trip anywhere inside the walk flips a computed
    /// `true` the same way — a tripped mapping's `Invalid` face must not
    /// masquerade as the oracle's `InvalidType` and drop the test's
    /// facts.
    fn is_invalid_typed(
        &self,
        ty: FlowTy,
        args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> bool {
        let owner = self.work_enter();
        let mark = self.work_mark();
        let verdict = if self.work_tick() {
            self.is_invalid_typed_walk(ty, args, arena, names)
        } else {
            false
        };
        self.work_exit(owner);
        verdict && !self.work_tripped_since(mark)
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

    /// The bare-head invocation face ([`TypeOps::call_head_unresolved`]):
    /// a spelling resolves as nothing invokable when it names no
    /// registered container (the constructor face), no resolved
    /// non-container declaration (a top-level function, variable or
    /// getter — own unit or import-swept), and no member of any
    /// registered container (the instance/static member face, name-keyed
    /// over-approximation — [`LibraryTypes::invokable_members`]). A
    /// dart:core top-level facade name (`print`) refuses the same way:
    /// a call the official types through core is never the invalid face.
    fn call_head_unresolved(&self, name: u32, names: &[Box<str>]) -> bool {
        self.observe_names(names);
        let Some(lexeme) = names.get(name as usize) else {
            return false;
        };
        if crate::diagnose::types::CORE_TOP_LEVEL_FNS.contains(&lexeme.as_ref()) {
            return false;
        }
        let name = self.strings.intern(lexeme);
        !(self.snapshot.registry.contains_key(&name)
            || self.snapshot.non_containers.contains(&name)
            || self.snapshot.invokable_members.contains(&name))
    }

    /// The object-pattern property face ([`TypeOps::object_property_invalid`]):
    /// the member's own declared/returned type head resolves nowhere in
    /// the snapshot's universe — the precomputed
    /// `invalid_property_tys` pair set (own-declared members only; an
    /// absent or inherited spelling answers `false`, the under-report
    /// direction).
    fn object_property_invalid(&self, owner: u32, field: u32, names: &[Box<str>]) -> bool {
        self.observe_names(names);
        let (Some(owner), Some(field)) = (
            names.get(owner as usize),
            names.get(field as usize),
        ) else {
            return false;
        };
        let pair = (self.strings.intern(owner), self.strings.intern(field));
        self.snapshot.invalid_property_tys.contains(&pair)
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
    /// `Response<InvalidType>` keeps the test decidable. A fuel trip
    /// anywhere inside (#2636) keeps `from` whole — a tripped mapping's
    /// vacuous 0c arm must not bottom the remainder and dead a branch
    /// the oracle keeps live.
    fn factor_typed(
        &self,
        from: FlowTy,
        from_args: u32,
        what: FlowTy,
        what_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> FlowTy {
        // An undefined tested head contributes no remainder fact (#2450):
        // the oracle's is-check against a name that resolves nowhere bails
        // (`type_test_with_undefined_name` on the spelling, neither branch
        // dead), so the 0c vacuous arm never turns the remainder `Never`.
        if self.tested_head_undefined(what, names) {
            return from;
        }
        // A lossy-overload head factors name-level
        // ([`Self::head_overloaded`]) — the resolved remainder would
        // bottom over `Invalid`'s vacuous subtype arm and dead a branch
        // the oracle keeps live. An unresolved `from` against a
        // resolved `what` delegates the same way.
        if self.head_overloaded(from, names) || self.head_overloaded(what, names) {
            return NameTypeOps.factor_typed(from, from_args, what, what_args, arena, names);
        }
        self.observe_names(names);
        let mark = self.work_mark();
        let factored =
            self.factor_typed_walk(from, from_args, what, what_args, arena, names);
        if self.work_tripped_since(mark) {
            return from;
        }
        factored
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
        let mark = self.work_mark();
        // The undefined tested head keeps the guaranteed-succeed face
        // silent (#2450 — [`Self::tested_head_undefined`]): no subtype
        // verdict runs over a name the oracle reports
        // `type_test_with_undefined_name` on.
        if self.tested_head_undefined(right, names) {
            return false;
        }

        // A lossy-overload head answers name-level
        // ([`Self::head_overloaded`]) — the resolved verdict would ride
        // the wrong class half the time, and its `Invalid` face would
        // vacuously dead branches the oracle keeps live. An unresolved
        // left side against a resolved right delegates the same way.
        if self.head_overloaded(left, names) || self.head_overloaded(right, names) {
            return NameTypeOps.is_subtype_typed(left, left_args, right, right_args, arena, names);
        }
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
        // Fuel trip (#2636): a tripped mapping answers `Invalid`, whose
        // vacuous 0c subtype arm is `true` — a false constant-true that
        // deads a live else. Not provable instead.
        self.system.is_subtype(l, r) && !self.work_tripped_since(mark)
    }

    fn try_promote_to_typed(
        &self,
        to: FlowTy,
        to_args: u32,
        from: FlowTy,
        from_args: u32,
        arena: &[Vec<ArgTy>],
        names: &[Box<str>],
    ) -> Option<FlowTy> {
        // No promotion rides an undefined tested head (#2450 —
        // [`Self::tested_head_undefined`]): the oracle's promotion gate
        // never fires for a spelling it reports
        // `type_test_with_undefined_name` on.
        if self.tested_head_undefined(to, names) {
            return None;
        }

        // A lossy-overload head promotes name-level
        // ([`Self::head_overloaded`]) — the resolved strict gate would
        // consult the wrong class half the time, forcing or refusing
        // steps the oracle decides otherwise; the name-level default's
        // posture stands over the spelling.
        if self.head_overloaded(to, names) || self.head_overloaded(from, names) {
            return NameTypeOps.try_promote_to_typed(to, to_args, from, from_args, arena, names);
        }
        self.observe_names(names);
        let mark = self.work_mark();
        let t = self.map_tested(to, to_args, arena, names);
        let f = self.map_scrutinee(from, from_args, arena, names);
        // The strict-subtype gate (`isValidPromotionStep`,
        // flow_analysis.dart:7229-7247): `to <: from` is required, and a
        // `to` the current type already satisfies (`from <: to` —
        // identical types included, `x is int` on `int x`) is no step.
        // A fuel trip (#2636) refuses the step — a tripped mapping's
        // vacuous 0c arm could force `to <: from` and promote wrongly.
        (self.system.is_subtype(t, f)
            && !self.system.is_subtype(f, t)
            && !self.work_tripped_since(mark))
        .then_some(to)
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
        // The superclass-hop CHAIN spelling the matched declaration. The
        // declared-ness gate keeps `supertypes`'s panic contract.
        if self.snapshot.hierarchy.decl(tested_decl).is_none() {
            return None;
        }
        // The official `downwardInferObjectPatternRequiredType` walks the
        // superclass chain step by step — `final class AD<T> extends
        // AR<T>` under `sealed class AR<T> extends AV<T>` reaches `AV` in
        // two hops, each hop's clause arguments instantiated against the
        // substitution the walk carries. A direct-edges-only scan solved
        // just the one-hop children and refused every grandchild (the raw
        // spelling then failed the promotion gate and the scrutinee kept
        // its declared type through the case body). The first path that
        // reaches the matched declaration decides, in edge order — the
        // order the direct scan's `find` used.
        let mut queue: std::collections::VecDeque<(TypeDeclId, TypeId)> =
            std::collections::VecDeque::new();
        for &edge in self.snapshot.hierarchy.supertypes(tested_decl) {
            queue.push_back((tested_decl, edge));
        }
        let mut visited: FxHashSet<(TypeDeclId, TypeId)> = FxHashSet::default();
        let hop_args: Vec<TypeId> = loop {
            let Some((owner, edge)) = queue.pop_front() else {
                return None;
            };
            if !visited.insert((owner, edge)) {
                continue;
            }
            if visited.len() > 512 {
                return None;
            }
            let TypeKind::Interface { decl, args: range } = self.snapshot.table.get(edge).kind()
            else {
                continue;
            };
            if decl == from_decl {
                break self.snapshot.table.args(range).to_vec();
            }
            // Extend the path: this hop's own supertype edges, their
            // clause arguments instantiated against the arguments the
            // walk carries for the hop — the hop IS `decl` instantiated
            // as `args(range)`, so the substitution keys are the TARGET
            // declaration's own formals (its edges are spelled over
            // them), not the source's.
            let Some(target_params) = self.snapshot.decl_params.get(&decl) else {
                continue;
            };
            let map: Vec<(TypeParamId, TypeId)> = target_params
                .iter()
                .copied()
                .zip(self.snapshot.table.args(range).iter().copied())
                .collect();
            if self.snapshot.hierarchy.decl(decl).is_none() {
                continue;
            }
            for &next in self.snapshot.hierarchy.supertypes(decl) {
                let substituted = self.snapshot.table.substitute(next, &map);
                queue.push_back((decl, substituted));
            }
        };
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
    /// `diagnose/tests.rs` runs, with the ops adapter mounted. The
    /// #2636 probe variant: a mutation hook into the snapshot (the
    /// synthetic cycles no real build produces), a per-call budget
    /// override, a probe over the mounted ops (run before the fold), and
    /// the fuel counters read after it.
    #[expect(
        clippy::type_complexity,
        reason = "the pin's harness shape; folding the closures into a config struct would grow the test surface, not shrink it"
    )]
    fn diagnose_codes_probe(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
        mutate: impl FnOnce(&StringTable, &mut LibraryTypes),
        budget: Option<u32>,
        probe: impl FnOnce(&ResolvedTypeOps),
    ) -> (Vec<&'static str>, u32, u32) {
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
        let mut library_types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
        mutate(&strings, &mut library_types);
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);
        if let Some(budget) = budget {
            ops.override_work_budget_for_test(budget);
        }
        probe(&ops);
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
        let codes = fold
            .report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
        (codes, ops.ops_bailed(), ops.ops_peak_work())
    }

    /// The unit's flow-driven diagnostic code names — the same harness
    /// `diagnose/tests.rs` runs, with the ops adapter mounted.
    fn diagnose_codes(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> Option<ImportEdge>,
    ) -> Vec<&'static str> {
        diagnose_codes_probe(source, make_edge, |_, _| {}, None, |_| {}).0
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, main_file);
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, main_file);
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, main_file);
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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
        let ops = ResolvedTypeOps::new(&library_types, &strings, main_file);
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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
    /// the arity error reports (`recv_self_arity.dart` probe reports only
    /// `wrong_number_of_type_arguments`; the `is!` polarity likewise,
    /// probed 3.47.1). The oracle's liveness rides the tested head
    /// RESOLVING — dart:core's `String` — so the rig declares it through
    /// the unmodeled import, the exact face a `dart:` SDK class outside
    /// the builtin table presents in the real pipeline. An UNRESOLVED
    /// head over the same receiver keeps the oracle's dead else
    /// (`Missing` probe: `type_test_with_undefined_name` + `dead_code`)
    /// — the 0c arm the recovery must not paper over.
    #[test]
    fn ticket_2380_wrong_arity_receiver_is_string_stays_silent() {
        let source = "class Response<T> { T? data; } \
             void f(Response<int, int> x) { \
               if (x is String) { 1; } else { 2; } \
             }";
        let codes = diagnose_codes_unmodeled(source, &["String"]);
        assert!(
            !codes.contains(&"unnecessary_type_check") && !codes.contains(&"dead_code"),
            "a wrong-arity receiver's is String must stay silent: {codes:?}"
        );
        let source = "class Response<T> { T? data; } \
             void f(Response<int, int> x) { \
               if (x is! String) { 1; } else { 2; } \
             }";
        let codes = diagnose_codes_unmodeled(source, &["String"]);
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
                param_bounds: Vec::new().into_boxed_slice(),
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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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
        let ops = ResolvedTypeOps::new(&library_types, &strings, file);

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

    // —— #2636: per-call work fuel ———————————————————————————————————

    /// The divergence pin: a written-argument arena whose row references
    /// itself (the recorder-bug class the fuel defends against — without
    /// it `map_name` → `map_node` → `map_name` never bottoms out). The
    /// budget converts the spin into the `Invalid` face with the bail
    /// counted.
    #[test]
    fn fuel_trips_on_a_cyclic_written_arena() {
        diagnose_codes_probe(
            "void main() {}\n",
            |_| None,
            |strings, snapshot| {
                snapshot.non_containers.insert(strings.intern("Cyc"));
            },
            None,
            |ops| {
                let names: Vec<Box<str>> = vec!["Cyc".into()];
                // The entry trip: a zero budget refuses the first tick and
                // answers the `Invalid` face.
                ops.override_work_budget_for_test(0);
                let mapped = ops.map_name("Cyc", EMPTY_ARGS, &[], &names, false);
                assert!(
                    matches!(ops.snapshot.table.get(mapped).kind(), TypeKind::Invalid),
                    "an entry trip answers the invalid face"
                );
                assert!(ops.ops_bailed() >= 1);
                // The interior trip: arena row 1 references itself —
                // `Cyc<Cyc<…>>` ad infinitum (row 0 is the reserved
                // argument-less list) — bottoms out in the opaque arm's
                // existing unknown face (`opaque_args` reads an `Invalid`
                // argument as `dynamic`) instead of recursing forever.
                ops.override_work_budget_for_test(16);
                let arena: Vec<Vec<ArgTy>> = vec![
                    Vec::new(),
                    vec![ArgTy {
                        base: Base::Name(0),
                        nullable: false,
                        args: 1,
                    }],
                ];
                let mapped = ops.map_name("Cyc", 1, &arena, &names, false);
                assert!(
                    matches!(
                        ops.snapshot.table.get(mapped).kind(),
                        TypeKind::Interface { .. }
                    ),
                    "the interior trip bottoms in the opaque unknown face"
                );
                assert!(ops.ops_bailed() >= 2);
            },
        );
    }

    /// The `typedef A = B; typedef B = A;` snapshot: the chase is
    /// length-bounded (the terminal keeps the opaque face — the bound
    /// the fuel mirrors), converging without a trip under the real
    /// budget, and a tripped budget answers the `Invalid` face with the
    /// bail counted.
    #[test]
    fn fuel_typedef_cycle_maps_conservatively() {
        diagnose_codes_probe(
            "typedef A = B;\ntypedef B = A;\nvoid main() {}\n",
            |_| None,
            |strings, snapshot| {
                let a = strings.intern("A");
                let b = strings.intern("B");
                snapshot.typedef_targets.insert(a, (b, false));
                snapshot.typedef_targets.insert(b, (a, false));
                snapshot.non_containers.insert(a);
                snapshot.non_containers.insert(b);
            },
            None,
            |ops| {
                let arena: Vec<Vec<ArgTy>> = vec![Vec::new()];
                let names: Vec<Box<str>> = vec!["A".into(), "B".into()];
                let mapped = ops.map_name("A", EMPTY_ARGS, &arena, &names, false);
                assert!(
                    matches!(
                        ops.snapshot.table.get(mapped).kind(),
                        TypeKind::Interface { .. }
                    ),
                    "the cycle's terminal keeps the opaque face"
                );
                assert_eq!(ops.ops_bailed(), 0);
                ops.override_work_budget_for_test(0);
                let mapped = ops.map_name("A", EMPTY_ARGS, &arena, &names, false);
                assert!(
                    matches!(ops.snapshot.table.get(mapped).kind(), TypeKind::Invalid),
                    "a tripped chase answers the invalid face"
                );
                assert!(ops.ops_bailed() >= 1);
            },
        );
    }

    /// The F-bounded self-reference (`class C<T extends C<T>>`
    /// instantiated to bounds): converges under the real budget, and a
    /// tripped budget keeps the conservative posture — no dead row rides
    /// a vacuous `Invalid` arm (the name-level fallback the trips take
    /// is the pre-mount behavior, never an over-report) — with the bail
    /// counted.
    #[test]
    fn fuel_f_bounded_generic_converges_and_trips_conservative() {
        let source = "class C<T extends C<T>> {}\n\nvoid main(C c) {\n  if (c is C) {\n    c;\n  }\n}\n";
        let (_, bailed, _) = diagnose_codes_probe(source, |_| None, |_, _| {}, None, |_| {});
        assert_eq!(bailed, 0);
        let (codes, bailed, _) =
            diagnose_codes_probe(source, |_| None, |_, _| {}, Some(2), |_| {});
        assert!(bailed >= 1);
        assert!(
            !codes.contains(&"dead_code"),
            "a tripped fold deads no branch: {codes:?}"
        );
    }

    /// The self-nested generic (`typedef F = List<F>`): the RHS cycle
    /// guard registers no shape (the alias keeps the opaque face — the
    /// under-report direction), converging under the real budget, and a
    /// tripped budget keeps the same no-dead-row posture with the bail
    /// counted.
    #[test]
    fn fuel_self_nested_typedef_converges_and_trips_conservative() {
        let source = "typedef F = List<F>;\n\nvoid main(F f) {\n  if (f is F) {\n    f;\n  }\n}\n";
        let (_, bailed, _) = diagnose_codes_probe(source, |_| None, |_, _| {}, None, |_| {});
        assert_eq!(bailed, 0);
        let (codes, bailed, _) =
            diagnose_codes_probe(source, |_| None, |_, _| {}, Some(2), |_| {});
        assert!(bailed >= 1);
        assert!(
            !codes.contains(&"dead_code"),
            "a tripped fold deads no branch: {codes:?}"
        );
    }

    /// The budget-sizing pin (mirroring `ticket_2626_tests`'s
    /// step-budget face): the most work one armed call spends over a
    /// generic-heavy fixture stays an order of magnitude under
    /// [`WORK_BUDGET_PER_CALL`]; the corpus-measured worst rides the
    /// same faces (see the const's rustdoc).
    #[test]
    fn legit_mapping_work_stays_far_under_budget() {
        let source = "typedef JsonMap = Map<String, List<Map<int, Object?>>>;\n\n\
             abstract class Repo<T extends Object> {\n  \
             FutureOr<List<T?>> page(FutureOr<JsonMap> source);\n\
             }\n\n\
             void main(Repo<String> repo, JsonMap m) {\n  \
             if (repo is Repo<String>) {\n    print(m);\n  }\n  \
             switch (m) {\n    \
             case Map<String, List<int>>():\n      break;\n    \
             default:\n      break;\n  }\n\
             }\n";
        let (_, bailed, peak) = diagnose_codes_probe(source, |_| None, |_, _| {}, None, |_| {});
        assert_eq!(bailed, 0);
        assert!(
            peak * 10 < WORK_BUDGET_PER_CALL,
            "legit peak {peak} within 10x of the budget"
        );
    }
}
/// The `?`-stripped projection of `ty` (same kind, non-nullable face).
/// Function/record shapes have no nullable re-intern here and keep their
/// suffix — the binding sites that reach them are cold edge faces.
fn non_nullable_of(table: &TypeTable, ty: TypeId) -> TypeId {
    let face = table.get(ty);
    if !face.is_nullable() {
        return ty;
    }
    let uninterned = match face.kind() {
        TypeKind::Never => UninternedType::Never,
        TypeKind::Dynamic => UninternedType::Dynamic,
        TypeKind::Void => UninternedType::Void,
        TypeKind::Invalid => UninternedType::Invalid,
        TypeKind::Null => UninternedType::Null,
        TypeKind::Interface { decl, args } => UninternedType::Interface {
            decl,
            args: table.args(args),
        },
        TypeKind::TypeParameter { parameter } => UninternedType::TypeParameter { parameter },
        TypeKind::Function { .. } | TypeKind::FutureOr { .. } | TypeKind::Record { .. } => {
            return ty
        }
    };
    table.intern(uninterned, false)
}

impl ResolvedTypeOps<'_> {
    /// `recv` viewed as an instance of the container `target` (the official
    /// `asInstanceOf`, element.dart `InterfaceTypeImpl.asInstanceOf`):
    /// `target`'s type parameters paired with the instantiated arguments
    /// (`List<int>` as `Iterable` reads `E := int`), mapped back into the
    /// diagnosing pool. `None` when the receiver does not map into the
    /// snapshot, `target` is unregistered, the receiver is not an instance
    /// of it, or an argument carries a shape the pool cannot spell.
    pub(crate) fn instance_env(
        &self,
        recv: Ty,
        target: StringId,
        pool: &RefCell<ArgPool>,
    ) -> Option<Vec<(StringId, Ty)>> {
        let from = self.map_ty_strict(recv, &pool.borrow())?;
        let &decl = self
            .snapshot
            .registry
            .get(&target)
            .filter(|_| !self.snapshot.non_containers.contains(&target))?;
        let instance = self.system.as_instance_of(from, decl)?;
        let TypeKind::Interface { args, .. } = self.snapshot.table.get(instance).kind() else {
            return None;
        };
        let params = self.snapshot.decl_params.get(&decl)?;
        let args = self.snapshot.table.args(args);
        if args.len() != params.len() {
            return None;
        }
        let mut env = Vec::with_capacity(args.len());
        for (&param, &arg) in params.iter().zip(args.iter()) {
            env.push((self.param_name(param)?, self.unmap_ty(arg, pool)?));
        }
        Some(env)
    }

    /// A snapshot type spelled back into the diagnosing pool: interfaces
    /// and type parameters through the inverted registries, the atoms as
    /// themselves. Function and record shapes answer `None` (their
    /// re-interning is not a question any consumer asks yet).
    fn unmap_ty(&self, id: TypeId, pool: &RefCell<ArgPool>) -> Option<Ty> {
        let ty = self.snapshot.table.get(id);
        let nullable = ty.is_nullable();
        match ty.kind() {
            TypeKind::Dynamic => Some(Ty::Dynamic),
            TypeKind::Void => Some(Ty::Void),
            TypeKind::Null => Some(Ty::Null),
            TypeKind::Never => Some(if nullable { Ty::Null } else { Ty::Never }),
            TypeKind::Invalid | TypeKind::Function { .. } | TypeKind::Record { .. } => None,
            TypeKind::TypeParameter { parameter } => Some(Ty::Named {
                name: self.param_name(parameter)?,
                nullable,
                args: ArgListId::EMPTY,
            }),
            TypeKind::FutureOr { base } => {
                let base = self.unmap_ty(base, pool)?;
                let head = self.strings.intern("FutureOr");
                Some(pool.borrow_mut().named(head, nullable, &[base]))
            }
            TypeKind::Interface { decl, args } => {
                let name = self.decl_name(decl)?;
                let ids = self.snapshot.table.args(args);
                let mut mapped = Vec::with_capacity(ids.len());
                for id in ids {
                    mapped.push(self.unmap_ty(id, pool)?);
                }
                Some(pool.borrow_mut().named(name, nullable, &mapped))
            }
        }
    }

    fn decl_name(&self, decl: TypeDeclId) -> Option<StringId> {
        let mut cache = self.decl_names.borrow_mut();
        let names = cache.get_or_insert_with(|| {
            self.snapshot
                .registry
                .iter()
                .map(|(&name, &decl)| (decl, name))
                .collect()
        });
        names.get(&decl).copied()
    }

    /// A container's own parameter spelling, or the name-keyed synthetic
    /// parameter's (the element-model `TypeParam` declarations).
    fn param_name(&self, param: TypeParamId) -> Option<StringId> {
        if let Some(&name) = self.snapshot.param_names.get(&param) {
            return Some(name);
        }
        let mut cache = self.param_names.borrow_mut();
        let names = cache.get_or_insert_with(|| {
            self.snapshot
                .type_params
                .iter()
                .map(|(&name, &param)| (param, name))
                .collect()
        });
        names.get(&param).copied()
    }
}
