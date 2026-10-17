//! Per-library member enumeration over the resolution layer (#117): the
//! library scope face, the in-library scoped lookup, and the name-level
//! member walk — the query surface the `undefined_*`/`wrong_number_*`
//! diagnostic tickets consume.
//!
//! # The library face (adjudicated essential #1)
//!
//! [`LibraryScopes`] materializes, per library, the public name face with a
//! micro-kind tag: local declarations and re-exports from the graph's export
//! scope (a declaration is identified by `Decl(file)` + looked-up name —
//! #117's contract; the kind comes from the declaring file's
//! [`FileElements`]), the exported names of every unprefixed import
//! (combinator filtered, never displacing local names), plus one `prefix`
//! marker per import prefix. A name
//! declared outside the indexed files (`dart:` SDK, an unanalyzed
//! dependency) surfaces as [`ScopeKind::External`] — present, but
//! kind-sensitive diagnostics must be suppressed.
//!
//! # In-library scope (the private-name face)
//!
//! The export scope only carries public names, but library-internal code
//! sees private ones too. [`lookup_scoped`] is the in-library chain:
//! the current file's own declarations first (private included, locals beat
//! imports), then the [`Namespaces`] lookup (import face, build-time
//! combinator filtering, ambiguity with all candidates).
//!
//! # Member walk (adjudicated essentials #2–#3)
//!
//! [`member_lookup`] resolves a container by name (file-local first, then
//! the namespace) and walks superclass name edges, returning the first hit.
//! A miss is **not** proof of undefined: the caller must still check the
//! extension pool ([`FileElements::extension_faces`]) of the library's
//! files before reporting an instance-scope `undefined_*`.
//!
//! # Interface-membership walk
//!
//! [`implements_interface`] answers the transitive supertype question a
//! rule asks as `type.implementsInterface(name, library)` (upstream Dart
//! SDK source, pkg/linter/lib/src/extensions.dart:324-331) over the
//! declared supertype graph — the same name-level edge discipline the
//! member walk applies, plus a small `dart:core` face for the names no
//! index can carry.
//!
//! # The part/part-of seam
//!
//! The parser consumes directives diet-style and emits no `part` events
//! yet, so this layer cannot assign files to libraries; callers pass the
//! owning library and current file explicitly, and **sibling-part private
//! names are not merged** (a part file's in-library lookup sees only its
//! own declarations plus the public face). The `part`/`part-of` family
//! codes ([`dartq_resolution::ResolutionCode`] `PartOf*`) are frozen in
//! the resolution layer for the diagnostic tickets; when the directive
//! milestone lands, the file→library fold moves behind this same API.

use dartq_resolution::{LibraryGraph, LibraryId, LocalLookup, Lookup, NamespaceEntry, Namespaces};
use dartq_source::{FileId, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

use crate::model::{Container, ContainerKind, FileElements, MemberEntry, MicroKind, TopDecl, Ty};

/// Per-file element models, keyed by file. Sized by the analyzed file set.
///
/// Models are shared by [`Arc`]: inserting from a shared store is a refcount
/// bump, so indexing beyond the diagnosing library — the cross-library
/// member-walk face (#236's zero-clone design) — costs no deep clones.
#[derive(Debug, Default)]
pub struct ElementIndex {
    files: FxHashMap<FileId, Arc<FileElements>>,
}

impl ElementIndex {
    /// Stores (or replaces, on re-analysis) the model of `file`. An
    /// [`Arc`] clone shares the model; an owned value is taken.
    pub fn insert(&mut self, file: FileId, elements: impl Into<Arc<FileElements>>) {
        self.files.insert(file, elements.into());
    }

    /// The model of `file`, if analyzed.
    #[must_use]
    pub fn get(&self, file: FileId) -> Option<&FileElements> {
        self.files.get(&file).map(Arc::as_ref)
    }
}

/// The kind tag of a name in a library face: a known [`MicroKind`], or
/// [`ScopeKind::External`] when the declaring file is outside the indexed
/// set — present, but kind-sensitive diagnostics must be suppressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// Declared in an indexed file; the kind is known.
    Kind(MicroKind),
    /// Declared outside the indexed files (`dart:` SDK, unanalyzed
    /// dependency) — presence is certain, kind is unknown.
    External,
}

/// The public name face of one library: name → kind tag.
#[derive(Debug, Default)]
pub struct ScopeFace {
    names: FxHashMap<StringId, ScopeKind>,
}

impl ScopeFace {
    /// The kind tag of `name`, if the face carries it.
    #[must_use]
    pub fn kind(&self, name: StringId) -> Option<ScopeKind> {
        self.names.get(&name).copied()
    }

    /// The number of names in the face.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the face is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every name with its tag, in arbitrary order.
    pub fn iter(&self) -> impl Iterator<Item = (StringId, ScopeKind)> + '_ {
        self.names.iter().map(|(name, kind)| (*name, *kind))
    }
}

/// The public faces of the analyzed libraries, indexed by
/// [`LibraryId::index`]; libraries not included in [`LibraryScopes::build`]
/// have no face. Sized by the graph; nothing grows after build.
#[derive(Debug, Default)]
pub struct LibraryScopes {
    scopes: Vec<Option<ScopeFace>>,
}

impl LibraryScopes {
    /// Builds the faces of `libs` over `graph` and `index`. The graph
    /// already folded each library's part tree into its local declarations
    /// (the caller declared every file), so a face needs no part information
    /// here.
    ///
    /// Beyond the library's own export scope, each unprefixed import joins
    /// the face with the target's exported names (#117's merge contract:
    /// the face is the library's visible-name surface) — combinator
    /// filtered, and never displacing a local or earlier-seeded name.
    #[must_use]
    pub fn build(graph: &LibraryGraph, index: &ElementIndex, libs: &[LibraryId]) -> Self {
        let mut scopes: Vec<Option<ScopeFace>> = (0..graph.len()).map(|_| None).collect();
        for &lib in libs {
            let mut face = ScopeFace::default();
            for (name, entry) in graph.export_scope(lib) {
                let kind = Self::kind_of(index, name, entry.file);
                face.names.insert(name, kind);
            }
            for edge in graph.import_edges(lib) {
                if let Some(prefix) = edge.prefix {
                    face.names
                        .entry(prefix)
                        .or_insert(ScopeKind::Kind(MicroKind::Prefix));
                    continue;
                }
                let Some(target) = edge.target else {
                    continue;
                };
                for (name, entry) in graph.export_scope(target) {
                    if !edge.combinators.allows(name) {
                        continue;
                    }
                    let kind = Self::kind_of(index, name, entry.file);
                    face.names.entry(name).or_insert(kind);
                }
            }
            scopes[lib.index() as usize] = Some(face);
        }
        Self { scopes }
    }

    /// The kind tag of an exported name from its declaring file: a known
    /// [`MicroKind`] inside the indexed set, [`ScopeKind::External`]
    /// otherwise.
    fn kind_of(index: &ElementIndex, name: StringId, file: FileId) -> ScopeKind {
        index
            .get(file)
            .and_then(|file| file.decl(name))
            .map_or(ScopeKind::External, |decl| ScopeKind::Kind(decl.kind))
    }

    /// The face of `lib`, if built.
    #[must_use]
    pub fn face(&self, lib: LibraryId) -> Option<&ScopeFace> {
        self.scopes
            .get(lib.index() as usize)
            .and_then(|s| s.as_ref())
    }
}

/// The outcome of an in-library scoped lookup ([`lookup_scoped`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopedHit<'a> {
    /// Not defined in the current file nor the import face.
    NotFound,
    /// Declared in the current file itself (private names included; the
    /// local declaration shadows imports).
    Local(&'a TopDecl),
    /// Resolved through the library namespace to one target.
    Found(NamespaceEntry),
    /// Ambiguous between distinct targets — reportable as `ambiguous_import`
    /// at the use point, with every candidate kept. The candidates are owned
    /// (the namespace layer's ambiguous set is built per probe, #1068).
    Ambiguous(Box<[NamespaceEntry]>),
}

/// The in-library scope chain for `name`: the current file's declarations
/// (private included), then the library's namespace. Use this for
/// diagnostics **inside** the library; the public face
/// ([`LibraryScopes`]) answers import-side checks.
///
/// The part seam applies: sibling-part private names are not visible until
/// the directive milestone lands (module docs).
#[must_use]
pub fn lookup_scoped<'a>(
    graph: &LibraryGraph,
    namespaces: &'a Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> ScopedHit<'a> {
    if let Some(decl) = index.get(file).and_then(|f| f.decl(name)) {
        return ScopedHit::Local(decl);
    }
    match namespaces.lookup(graph, lib, name) {
        Lookup::Found(entry) => ScopedHit::Found(entry),
        Lookup::Ambiguous(candidates) => ScopedHit::Ambiguous(candidates),
        Lookup::NotFound => ScopedHit::NotFound,
    }
}

/// The library whose scope resolves a name written in `owner_file`: the
/// superclass/superface names of a container resolve in the declaring
/// container's library (the oracle climbs the declaring library's scope
/// chain), not in the diagnosing one. The graph maps `(library, name)` to
/// the declaring file and `LibraryId` values are only constructible through
/// the edges, so the answer is the first edge-reachable library (BFS over
/// import/export edges from `start`) whose locals layer returns
/// `owner_file` for the resolved spelling.
pub(crate) fn owner_library(
    graph: &LibraryGraph,
    start: LibraryId,
    owner_file: FileId,
    name: StringId,
) -> Option<LibraryId> {
    if graph.local(start, name) == LocalLookup::Found(owner_file) {
        return Some(start);
    }
    let mut seen = FxHashSet::default();
    seen.insert(start.index());
    let mut queue = Vec::new();
    queue.push(start);
    while let Some(lib) = queue.pop() {
        let targets = graph
            .import_edges(lib)
            .iter()
            .filter_map(|edge| edge.target)
            .chain(
                graph
                    .export_edges(lib)
                    .iter()
                    .filter_map(|edge| edge.target),
            );
        for target in targets {
            if !seen.insert(target.index()) {
                continue;
            }
            if graph.local(target, name) == LocalLookup::Found(owner_file) {
                return Some(target);
            }
            queue.push(target);
        }
    }
    None
}

/// Resolves a container by name: the file's own declarations first, then the
/// library namespace. Returns the container and the file that owns it (for
/// the superclass walk's next hop).
///
/// Crate-visible so the diagnostic engine's member walk can tell a cleanly
/// walked chain (member genuinely absent) from a broken one (a superclass
/// name that resolves to nothing — suppression territory, never a report).
pub(crate) fn resolve_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    if let Some(container) = index.get(file).and_then(|f| f.container(name)) {
        return Some((container, file));
    }
    match namespaces.lookup(graph, lib, name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => {
            let container = index.get(owner).and_then(|f| f.container(name))?;
            Some((container, owner))
        }
        _ => None,
    }
}

/// Resolves a container through an import prefix: `prefix` must be a
/// registered `as` prefix of `lib` and `name` a name in its merged table —
/// all same-prefix imports in directive order, combinator filtered,
/// first-come-wins ([`Namespaces::lookup_prefixed`], the oracle's
/// `PrefixScope` composition, scope.dart:352-357). The bare-scope walk
/// ([`resolve_container`]) cannot serve a prefixed spelling: it resolves in
/// the diagnosing library's own scope, so a prefixed creation head
/// (`new p.C.named()`) would read a same-named local container in the wrong
/// scope and a missing one as unresolved.
///
/// `None` under-reports: an unregistered prefix, a prefix whose target
/// contributes nothing to the graph, a name the prefix's table lacks, and
/// a resolved name that is not an indexed container.
pub(crate) fn prefixed_container<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    prefix: StringId,
    name: StringId,
) -> Option<(&'a Container, FileId)> {
    match namespaces.lookup_prefixed(graph, lib, prefix, name) {
        Lookup::Found(NamespaceEntry::Decl(owner)) => {
            let container = index.get(owner).and_then(|f| f.container(name))?;
            Some((container, owner))
        }
        _ => None,
    }
}

/// Looks up `member` on `container`'s instance/static face: the container's
/// member set, then the mixin-application order (its own `with` mixins,
/// last-applied first — `class X extends S with M1, M2` is the official
/// sequence `S&M1 extends S` / `S&M2 extends S&M1` / `X extends S&M2`,
/// `inheritance_manager3.dart:571-579`), then direct-superclass name edges.
/// Each hop resolves file-locally first, then through the library that owns
/// the declaring container (cycle-guarded).
///
/// # Cross-pool boundary (G4 phase 2, funnel F3)
///
/// The returned entry is owned by the resolving file's
/// [`crate::model::ArgPool`]: a cross-file hit carries that unit's
/// [`crate::model::ArgListId`] slots. This walk has no
/// diagnosing-engine state to project into, so a caller that drives the
/// diagnostic engine must route the hit through the engine's boundary
/// ([`mod@crate::diagnose`]'s `Engine` boundary — the F1/F2 funnels' projection)
/// before its declared types reach rows or message text; the diagnosing
/// engine itself reaches member faces through `Engine`-owned walks and
/// never calls this free function.
///
/// A `None` here is **not** proof of undefined — before reporting an
/// instance-scope `undefined_*`, check the extension faces
/// ([`FileElements::extension_faces`]) of the library's files (the caller
/// owns the file set; see the part seam in the module docs).
#[must_use]
pub fn member_lookup<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    member: StringId,
) -> Option<&'a MemberEntry> {
    member_lookup_defining(
        graph, namespaces, index, lib, file, container, member, false,
    )
    .map(|(_, entry)| entry)
}

/// [`member_lookup`] with the walk facts its by-name callers drop: the name
/// of the container that declares the hit (the anchor itself when the hit
/// is a self-declaration), and `concrete_only` — abstract entries are
/// skipped and the walk continues (the official `getMember(concrete: …)`
/// cut; concreteness is the entry's `is_abstract` bit, the only concreteness
/// dimension the model carries). Crate-visible: the declaration-side
/// L-family queries need the defining container to tell an inherited hit
/// from a self-declaration.
pub(crate) fn member_lookup_defining<'a>(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &'a ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    member: StringId,
    concrete_only: bool,
) -> Option<(StringId, &'a MemberEntry)> {
    let acceptable =
        |entry: &'a MemberEntry| (!concrete_only || !entry.is_abstract).then_some(entry);
    let mut name = container;
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    let mut owner = file;
    let mut scope_lib = lib;
    loop {
        if !seen.insert(name) {
            return None; // malformed cyclic chain: treat as unresolved
        }
        let (found, found_owner) =
            resolve_container(graph, namespaces, index, scope_lib, owner, name)?;
        if let Some(entry) = found.members.get(&member).and_then(acceptable) {
            return Some((name, entry));
        }
        // The stacked applications shadow the superclass chain in reverse
        // declaration order (the later application wins). The mixin and
        // superclass names are written in `found_owner` and resolve through
        // that file's library (probed by the container name it declares,
        // #491 S2a).
        let owner_lib = owner_library(graph, lib, found_owner, name).unwrap_or(scope_lib);
        for mixin in found.with_clause.iter().rev() {
            if !seen.insert(*mixin) {
                return None;
            }
            if let Some(entry) =
                resolve_container(graph, namespaces, index, owner_lib, found_owner, *mixin)
                    .and_then(|(mixin_container, _)| mixin_container.members.get(&member))
                    .and_then(acceptable)
            {
                return Some((*mixin, entry));
            }
        }
        name = found.superclass?;
        owner = found_owner;
        scope_lib = owner_lib;
    }
}

/// Whether any container on `container`'s supertype chain — the `with`
/// mixins (application order) and the direct superclass, transitively; the
/// same chain [`member_lookup`] walks — declares at least one instance
/// member. The anchor's own members are not supertype members and do not
/// count; a class written without `extends` sits at the written chain's end
/// (the implicit `Object` root is outside the name-level model — its
/// members never count, the way the official
/// `avoid_classes_with_only_static_members` gate excludes
/// `dart:core Object`).
///
/// Returns `None` when the chain is unknowable: the anchor does not resolve,
/// or a written superclass/mixin name resolves to nothing indexed (an SDK
/// type, an unanalyzed dependency) — that face could carry instance members,
/// so the caller stays silent. `Some(false)` means every supertype on the
/// written chain is indexed and none declares an instance member.
pub(crate) fn supertype_has_instance_members(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
) -> Option<bool> {
    let (mut found, mut found_owner) =
        resolve_container(graph, namespaces, index, lib, file, container)?;
    let mut name = container;
    let mut scope_lib = lib;
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    seen.insert(container);
    loop {
        let owner_lib = owner_library(graph, lib, found_owner, name).unwrap_or(scope_lib);
        // A `with` name that does not resolve leaves the chain's member face
        // unknown — `?` answers `None`, not "absent".
        for mixin in found.with_clause.iter().rev() {
            if !seen.insert(*mixin) {
                return None; // malformed cyclic chain
            }
            let (mixin_container, _) =
                resolve_container(graph, namespaces, index, owner_lib, found_owner, *mixin)?;
            if mixin_container
                .members
                .values()
                .any(|entry| !entry.is_static)
            {
                return Some(true);
            }
        }
        let Some(super_name) = found.superclass else {
            return Some(false); // the written chain ends with no instance member on it
        };
        if !seen.insert(super_name) {
            return None; // malformed cyclic chain
        }
        (found, found_owner) =
            resolve_container(graph, namespaces, index, owner_lib, found_owner, super_name)?;
        if found.members.values().any(|entry| !entry.is_static) {
            return Some(true);
        }
        name = super_name;
        scope_lib = owner_lib;
    }
}

/// The `dart:core` interface-membership face for names outside the indexed
/// set: `(type, interface)` pairs whose membership is definitional —
/// `Iterable` is itself, and the direct supertype of `List` and `Set`
/// (upstream Dart SDK source, sdk/lib/core/{iterable,list,set}.dart). An
/// unindexed name is answered only when it appears here as implementing
/// `interface`; every other unindexed name (an unanalyzed dependency, an
/// SDK type absent from the table) leaves the question open. The table
/// carries no negative facts: an unindexed name never answers `false`,
/// only the open direction.
const CORE_INTERFACE_IMPLS: [(&str, &str); 3] = [
    ("Iterable", "Iterable"),
    ("List", "Iterable"),
    ("Set", "Iterable"),
];

/// Whether the name implements the interface, per
/// [`CORE_INTERFACE_IMPLS`] — the core face's definite hit.
fn core_implements(core: &[(StringId, StringId)], ty: StringId, interface: StringId) -> bool {
    core.iter()
        .any(|(name, iface)| *name == ty && *iface == interface)
}

/// The written supertype edges of `container` — the face the
/// interface-membership walk expands: the direct superclass, the `with`
/// mixins, and the `implements` interfaces for the class family; a mixin's
/// declared `on` constraints joined with its own `implements` clause (a
/// constraint type is a supertype of the mixin, and of every application
/// that satisfies it); the `implements` clause alone for the remaining
/// kinds (an extension type's representation is not a supertype — the
/// erasure face handles it — and an extension is not a type at all). The
/// implicit `Object` root is outside the name-level model and never an
/// edge, so the walk ends where the written chain ends.
fn supertype_edges(container: &Container) -> impl Iterator<Item = StringId> + '_ {
    let mixin_constraints = (container.kind == ContainerKind::Mixin)
        .then(|| container.on_clause.iter())
        .into_iter()
        .flatten();
    container
        .superclass
        .iter()
        .chain(container.with_clause.iter())
        .chain(container.implements_clause.iter())
        .chain(mixin_constraints)
        .copied()
}

/// Whether `name` implements `interface` — the transitive supertype
/// question `type.implementsInterface(interface, 'dart.core')` answers
/// (upstream Dart SDK source, pkg/linter/lib/src/extensions.dart:324-331):
/// the interface spelling itself, then the declared supertype graph
/// ([`supertype_edges`]) walked transitively, cross-file, each hop
/// resolving through its declaring container's library — the same
/// discipline [`member_lookup`]'s walk applies to superclass names.
///
/// The three-valued contract: `Some(true)` on a definite hit (an edge
/// spelled `interface` outside the indexed set — the core spelling — or a
/// [`CORE_INTERFACE_IMPLS`] hit anywhere on the graph); `Some(false)` only
/// when every reached name is an indexed container and no edge hits; `None`
/// when any reached name is outside both the index and the core face (that
/// face could implement the interface — the open direction outranks a
/// tentative `false`, a definite `true` outranks the open one). A written
/// supertype cycle is impossible in code the oracle analyzes; the seen-set
/// makes it terminate as an ordinary shared node.
pub(crate) fn implements_interface(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strings: &StringTable,
    lib: LibraryId,
    file: FileId,
    name: StringId,
    interface: StringId,
) -> Option<bool> {
    // The core face, pre-interned once per query: the walk compares ids,
    // never re-resolves spellings per node.
    let core: Vec<(StringId, StringId)> = CORE_INTERFACE_IMPLS
        .iter()
        .map(|(ty, iface)| (strings.intern(ty), strings.intern(iface)))
        .collect();
    // The seed: the name as the diagnosing scope resolves it, erased
    // through the representation type for an extension type. An unindexed
    // seed is answered by the core face alone — the name could implement
    // anything else.
    let seed: (StringId, FileId, LibraryId);
    match resolve_container(graph, namespaces, index, lib, file, name) {
        Some((container, owner)) if container.kind == ContainerKind::ExtensionType => {
            // Extension-type erasure: the interface check runs against the
            // representation type, not the declared clause
            // (`typeForInterfaceCheck`'s `extensionTypeErasure`,
            // extensions.dart:295). An uncaptured or unnameable
            // representation formal leaves the question open.
            let Some(Ty::Named { name: rep_name, .. }) =
                container.primary_formals.first().map(|r| r.formal.ty)
            else {
                return None;
            };
            seed = (
                rep_name,
                owner,
                owner_library(graph, lib, owner, name).unwrap_or(lib),
            );
        }
        Some((_, owner)) => seed = (name, owner, lib),
        None => {
            return core_implements(&core, name, interface).then_some(true);
        }
    }
    let (start, start_file, start_lib) = seed;
    let Some((container, owner)) =
        resolve_container(graph, namespaces, index, start_lib, start_file, start)
    else {
        return core_implements(&core, start, interface).then_some(true);
    };
    let mut answer = Some(false);
    let mut seen: FxHashSet<StringId> = FxHashSet::default();
    seen.insert(start);
    let mut queue: Vec<(StringId, FileId, LibraryId, &Container)> = Vec::new();
    queue.push((start, owner, start_lib, container));
    while let Some((current, current_owner, scope_lib, container)) = queue.pop() {
        let owner_lib = owner_library(graph, lib, current_owner, current).unwrap_or(scope_lib);
        for edge in supertype_edges(container) {
            if !seen.insert(edge) {
                continue;
            }
            match resolve_container(graph, namespaces, index, owner_lib, current_owner, edge) {
                Some((next, next_owner)) => queue.push((edge, next_owner, owner_lib, next)),
                // An indexed name spelled `interface` is a user type, never
                // the core spelling (the oracle compares the defining
                // library, extensions.dart:335-340) — it walked above. Only
                // an unindexed edge can be the core face; any other
                // unindexed edge could implement the interface, and the
                // definite answer dies (a later definite `true` still
                // outranks it).
                None => {
                    if core_implements(&core, edge, interface) {
                        return Some(true);
                    }
                    answer = None;
                }
            }
        }
    }
    answer
}

/// Whether `name` can enter `lib`'s unit scope through an import: an
/// `as` prefix spelled `name` (import prefixes are library-scope names,
/// upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// scope_analyzer.dart), or an unprefixed import whose target's export
/// scope carries `name` under the directive's combinators — the same
/// merge [`LibraryScopes::build`] runs over the import face, scoped to
/// the one-name question a shadowing check asks (a prefixed import
/// contributes its prefix spelling alone; its target's exports stay
/// outside the unit scope).
///
/// The direction is conservative: an unprefixed import whose target
/// contributes nothing to the graph (an unresolvable URI, an unanalyzed
/// dependency) answers `true` — that library could export any name.
/// `false` therefore means "no indexed import carries the name", never a
/// proof of absence everywhere.
pub(crate) fn import_shadows(graph: &LibraryGraph, lib: LibraryId, name: StringId) -> bool {
    graph
        .import_edges(lib)
        .iter()
        .any(|edge| match edge.prefix {
            Some(prefix) => prefix == name,
            None => match edge.target {
                None => true, // the target's export set is unknowable
                Some(target) => {
                    edge.combinators.allows(name) && graph.exported(target, name).is_some()
                }
            },
        })
}

/// Whether the container `name` resolves to — file-local first, then the
/// library namespace, cross-file exactly like [`member_lookup`]'s anchor
/// resolution — carries an annotation whose spelled name is `annotation`.
/// The name-level cut of the JS-interop classification, which reads the
/// class element's own metadata (`metadata.hasJS`, upstream Dart SDK
/// source, pkg/linter/lib/src/rules/
/// invalid_runtime_check_with_js_interop_types.dart
/// `_jsTypeForStaticInterop`): only the resolved container's own
/// annotation rows answer, the supertype chain is not walked.
///
/// `None` = the name does not resolve to an indexed container (an SDK
/// type, an unanalyzed dependency) — that container could carry the
/// annotation, so the caller stays silent. A prefix-qualified annotation
/// keeps its dot-joined spelling (`@p.JS()` stores `p.JS`), so it does
/// not match a bare `JS` query — the under-report direction.
pub(crate) fn container_has_annotation(
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    lib: LibraryId,
    file: FileId,
    container: StringId,
    annotation: StringId,
) -> Option<bool> {
    let (found, _) = resolve_container(graph, namespaces, index, lib, file, container)?;
    Some(found.annotations.iter().any(|a| a.name == annotation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
    use dartq_scanner::{ScanConfig, TokenTable};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// Scans + collects one unit, interning its names into `strings`.
    fn unit(
        source: &str,
        strings: &StringTable,
        files: &mut FileTable,
        path: &str,
    ) -> FileElements {
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

    /// Two-file library merge through #117: a's public declarations are
    /// visible in b's face with their origin kinds; the prefix is tagged;
    /// b's locals shadow the import; a's private name stays invisible.
    #[test]
    fn library_merge_through_namespaces() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let elements_a = unit(
            "class A {} \
             void fa({int? n}) {} \
             int _private = 0;",
            &strings,
            &mut files,
            "a.dart",
        );
        let elements_b = unit(
            "import 'a.dart'; \
             import 'a.dart' as p; \
             class A {} \
             class B {}",
            &strings,
            &mut files,
            "b.dart",
        );
        let file_a = files.try_file_id(Path::new("a.dart")).unwrap();
        let file_b = files.try_file_id(Path::new("b.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_a = builder.intern("package:t/a.dart");
        let lib_b = builder.intern("package:t/b.dart");
        let mut index = ElementIndex::default();
        for (lib, file, elements) in [(lib_a, file_a, &elements_a), (lib_b, file_b, &elements_b)] {
            index.insert(file, elements.clone());
            for (name, decl) in elements.declarations() {
                if decl.kind == MicroKind::TypeParam {
                    continue;
                }
                builder.declare(lib, name, file);
            }
        }
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib_b,
            ImportEdge {
                target: Some(lib_a),
                prefix: Some(strings.intern("p")),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let scopes = LibraryScopes::build(&graph, &index, &[lib_a, lib_b]);

        let kind = |lib: LibraryId, name: &str| {
            scopes
                .face(lib)
                .and_then(|face| face.kind(strings.intern(name)))
        };
        assert_eq!(
            kind(lib_b, "A"),
            Some(ScopeKind::Kind(MicroKind::Class)),
            "b's local class A wins the face seeding"
        );
        assert_eq!(
            kind(lib_b, "fa"),
            Some(ScopeKind::Kind(MicroKind::TopLevelFn))
        );
        assert_eq!(kind(lib_b, "p"), Some(ScopeKind::Kind(MicroKind::Prefix)));
        assert_eq!(kind(lib_b, "nope"), None);
        // Private names never reach the public face.
        assert_eq!(kind(lib_a, "_private"), None);

        // In-library scoped lookup: locals (incl. private) then imports.
        let hit = |file: FileId, lib: LibraryId, name: &str| {
            lookup_scoped(&graph, &namespaces, &index, lib, file, strings.intern(name))
        };
        assert_eq!(
            hit(file_b, lib_b, "A"),
            ScopedHit::Local(
                index
                    .get(file_b)
                    .unwrap()
                    .decl(strings.intern("A"))
                    .unwrap()
            ),
            "b's local A shadows the import"
        );
        assert_eq!(
            hit(file_b, lib_b, "fa"),
            ScopedHit::Found(NamespaceEntry::Decl(file_a))
        );
        assert!(matches!(
            hit(file_b, lib_b, "_private"),
            ScopedHit::NotFound
        ));
        assert!(matches!(
            hit(file_a, lib_a, "_private"),
            ScopedHit::Local(_)
        ));
    }

    /// Member walk: superclass edges resolve across files through the
    /// namespace; a miss falls to the extension-pool recipe.
    #[test]
    fn member_walk_crosses_superclass_edges() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let base = unit(
            "class B { void m() {} int get g => 1; } \
             extension on B { int get extG => 1; }",
            &strings,
            &mut files,
            "base.dart",
        );
        let derived = unit("class C extends B {}", &strings, &mut files, "use.dart");
        let base_file = files.try_file_id(Path::new("base.dart")).unwrap();
        let use_file = files.try_file_id(Path::new("use.dart")).unwrap();

        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/lib.dart");
        for (file, elements) in [(base_file, &base), (use_file, &derived)] {
            for (name, decl) in elements.declarations() {
                if decl.kind != MicroKind::TypeParam {
                    builder.declare(lib, name, file);
                }
            }
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(base_file, base.clone());
        index.insert(use_file, derived);

        let m = member_lookup(
            &graph,
            &namespaces,
            &index,
            lib,
            use_file,
            strings.intern("C"),
            strings.intern("m"),
        )
        .expect("inherited through the superclass edge");
        assert_eq!(m.kind, crate::model::MemberKind::Method);
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("g")
            )
            .is_some()
        );
        // The miss that must consult the extension pool before reporting.
        assert!(
            member_lookup(
                &graph,
                &namespaces,
                &index,
                lib,
                use_file,
                strings.intern("C"),
                strings.intern("extG")
            )
            .is_none()
        );
        assert!(
            index
                .get(base_file)
                .unwrap()
                .has_extension_member(strings.intern("extG"))
        );
    }
}
