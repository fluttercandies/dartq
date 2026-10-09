//! Resident state (incremental-l1-spec §3.1 table): content residency, the
//! pending overlay, the library arena with its reverse-import index, the
//! per-library result cache, and the bounded per-file facts cache.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use dartiq_diagnostics::Diagnostic;
use dartiq_package::PackageTable as IdentityTable;
use dartiq_pub::PackageConfig;
use dartiq_resolution::{
    Combinators, PackageTable as UriPackageTable, ResolutionCode, ResolvedTarget, UriResolver,
};
use dartiq_sdk::SdkSnapshot;
use dartiq_semantic::FileElements;
use dartiq_source::{
    ContentId, FileId, FileTable, LineIndex, SourceRange, SourceStore, StringId, StringTable,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// Bumped with any engine-side change to what [`UnitFacts`] records
/// (directive extraction, diagnostic baking); part of the facts cache key
/// (the `AstKey.parser_version` component, PRD §5.6
/// `EngineSemanticVersion`).
pub(crate) const FACTS_SEMANTIC_VERSION: u32 = 1;

/// The SDK target segment M1 resolves `dart:` URIs against. Context-level
/// target selection is an M2 milestone (engine-api-spec §1.1); M1 pins the
/// VM the way `dart analyze` defaults do.
pub(crate) const SDK_TARGET: &str = "vm";

/// Facts-cache capacity in entries. Steady-state residency is bounded by the
/// live file set; this cache only memoizes *previous* contents so a
/// change-back (git branch flip, undo) reparses for free. The entry cap
/// keeps the overhead a bounded fraction of input memory (memory law, PRD
/// §14): insertion beyond the cap evicts oldest-first.
const FACTS_CACHE_CAP: usize = 1024;

/// The directive family of one [`DirectiveFacts`] entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `import 'u' [deferred] [as p] [show/hide …];`
    Import,
    /// `export 'u' [show/hide …];`
    Export,
    /// `part 'u';`
    Part,
    /// `part of 'u';` or `part of a.b.c;`
    PartOf,
    /// `library [a.b.c];`
    Library,
}

/// One extracted, resolved directive of a compilation unit.
#[derive(Clone, Debug)]
pub struct DirectiveFacts {
    /// The directive family.
    pub kind: DirectiveKind,
    /// The URI literal with quotes stripped and escapes processed, or `None`
    /// when the directive carries no resolvable string (interpolation, or a
    /// dotted name in [`DirectiveFacts::name`]).
    pub uri: Option<Box<str>>,
    /// UTF-16 span of the URI literal (diagnostic attribution); empty when
    /// [`DirectiveFacts::uri`] is `None`.
    pub uri_span: SourceRange,
    /// URI-layer resolution outcome (engine-api-spec §0): file targets are
    /// interned into the engine's [`FileTable`].
    pub target: ResolvedTarget,
    /// The frozen URI-layer diagnostic code for this outcome, if any.
    pub code: Option<ResolutionCode>,
    /// The `as` prefix of an import, interned.
    pub prefix: Option<StringId>,
    /// Whether the import is `deferred` (recorded, never evaluated at M1).
    pub deferred: bool,
    /// The `show`/`hide` filters, in source order.
    pub combinators: Combinators,
    /// The dotted name of `library a.b;` / `part of a.b;`, source spelling.
    pub name: Option<Box<str>>,
}

/// Everything one compilation unit contributes, keyed by its content
/// (incremental-l1-spec §3.1 rows "per-file AST" and "per-file unlinked
/// facts"): extracted directives with their URI resolution, the semantic
/// element model, and the file's syntax + URI-layer diagnostics.
///
/// Produced by the apply-phase recompute (fan-out for scan/parse/
/// collect, then sequential URI resolution) and shared as an `Arc` between
/// residency, the facts cache, and the semantic-diagnosis seam.
#[derive(Debug)]
pub struct UnitFacts {
    /// The content the facts were computed from.
    pub(crate) content: ContentId,
    /// Directives in source order.
    pub directives: Vec<DirectiveFacts>,
    /// The semantic element model (dartiq-semantic `collect`).
    pub elements: FileElements,
    /// Syntax (scan + parse recovery) and URI-layer diagnostics, all
    /// attributed to this file — a pure function of content, feature set,
    /// and the immutable analysis context.
    pub diagnostics: Vec<Diagnostic>,
    /// The file's UTF-16 line index.
    pub line_index: LineIndex,
}

impl UnitFacts {
    /// The content id the facts were computed from.
    #[must_use]
    pub const fn content(&self) -> ContentId {
        self.content
    }

    /// The first `part of` directive, if the unit is a part file.
    #[must_use]
    pub fn part_of(&self) -> Option<&DirectiveFacts> {
        self.directives
            .iter()
            .find(|directive| directive.kind == DirectiveKind::PartOf)
    }

    /// Whether the unit carries a `part of` directive (a part file).
    #[must_use]
    pub fn is_part(&self) -> bool {
        self.part_of().is_some()
    }
}

/// One library of the analysis set: a defining file plus its parts in
/// `part`-directive order (res-spec §2.6 — the library content identity
/// folds the member contents in exactly this order).
#[derive(Debug)]
pub(crate) struct LibraryInfo {
    /// Canonical URI the library identity keys on.
    pub uri: Box<str>,
    /// The defining (non-`part of`) file.
    pub defining: FileId,
    /// Included part files, part-directive order.
    pub parts: Vec<FileId>,
    /// Content identity: the defining file's `ContentId` followed by the
    /// parts' ids in part-directive order (the ordered member set of res-spec
    /// §2.6 — stored directly, so identity comparison is exact, not hashed).
    pub content: Box<[ContentId]>,
    /// Libraries this one imports/exports (sorted, deduplicated).
    pub deps: Vec<LibraryId>,
}

/// The per-library cached result (incremental-l1-spec §3.1 last row): the
/// merged diagnostic set keyed by the library's content + dependency
/// identity. Invalidation is explicit (dirty marking), never key comparison.
#[derive(Debug)]
pub(crate) struct LibraryResult {
    pub content: Box<[ContentId]>,
    pub deps: Vec<LibraryId>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Cache key of the per-file facts cache: the `AstKey{content, feature_set,
/// parser_version}` triple of PRD §5.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FactsKey {
    pub content: ContentId,
    pub feature_fingerprint: u64,
    pub semantic_version: u32,
}

/// A pending overlay entry (engine-api-spec §1.2): last write per path wins.
#[derive(Debug)]
pub(crate) enum PendingChange {
    /// Content override (UTF-8 decoded eagerly — the pipeline downstream is
    /// `&str`; invalid sequences become U+FFFD, documented at
    /// `Engine::set_content`).
    Set(String),
    /// Removal request.
    Remove,
}

/// A small stable integer naming one library of the analysis set. Opaque,
/// engine-internal, process-lifetime only (engine-api-spec §1.2); the
/// identity stable across applies is the library's canonical URI, and a
/// removed library retires its id into a permanent hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LibraryId(u32);

impl LibraryId {
    /// The underlying integer, stable for the lifetime of the engine.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// Mints the next id of the arena.
    pub(crate) fn next(existing: usize) -> Self {
        Self(u32::try_from(existing).expect("more than u32::MAX libraries interned"))
    }
}

/// The library arena: canonical URI → `LibraryId`, stable across applies.
#[derive(Debug, Default)]
pub(crate) struct LibraryTable {
    ids_by_uri: FxHashMap<Box<str>, LibraryId>,
    infos: Vec<Option<LibraryInfo>>,
}

impl LibraryTable {
    /// The library registered under `uri`, interning the id on first sight.
    pub(crate) fn intern(&mut self, uri: Box<str>) -> LibraryId {
        if let Some(&id) = self.ids_by_uri.get(&uri) {
            return id;
        }
        let id = LibraryId::next(self.infos.len());
        self.infos.push(None);
        self.ids_by_uri.insert(uri, id);
        id
    }

    /// Size of the id arena (live libraries plus retired holes).
    pub(crate) fn arena_len(&self) -> u32 {
        self.infos.len() as u32
    }

    /// Live libraries with their infos, ascending by id.
    pub(crate) fn iter_infos(&self) -> impl Iterator<Item = (LibraryId, &LibraryInfo)> + '_ {
        self.infos
            .iter()
            .enumerate()
            .filter_map(|(index, info)| info.as_ref().map(|info| (LibraryId(index as u32), info)))
    }

    /// The registered library's info, if it currently exists.
    pub(crate) fn info(&self, id: LibraryId) -> Option<&LibraryInfo> {
        self.infos.get(id.index() as usize).and_then(Option::as_ref)
    }

    /// Replaces the info of an existing library id.
    pub(crate) fn set_info(&mut self, id: LibraryId, info: LibraryInfo) {
        self.infos[id.index() as usize] = Some(info);
    }

    /// Live library ids, ascending.
    pub(crate) fn live(&self) -> impl Iterator<Item = LibraryId> + '_ {
        self.infos
            .iter()
            .enumerate()
            .filter_map(|(index, info)| info.is_some().then_some(LibraryId(index as u32)))
    }

    /// Drops every registration whose id is not in `keep`, retiring their
    /// ids into permanent holes. Returns the retired ids (ascending).
    pub(crate) fn retire_except(&mut self, keep: &FxHashSet<LibraryId>) -> Vec<LibraryId> {
        let mut retired = Vec::new();
        self.ids_by_uri.retain(|_, id| {
            if keep.contains(id) {
                true
            } else {
                retired.push(*id);
                false
            }
        });
        for id in &retired {
            self.infos[id.index() as usize] = None;
        }
        retired.sort_unstable();
        retired
    }
}

/// Everything the engine keeps between API calls (§3.1). `&mut self`
/// exclusivity on the `Engine` makes the enqueue→apply→analyze order total,
/// so neither the overlay nor the store need interior mutability: the store
/// is owned outright (reads take `&self`, `put` runs inside apply), and the
/// only lock in the crate is the [`StringTable`]'s own interning lock
/// (worker threads intern names during the parallel recompute).
pub(crate) struct EngineState {
    /// Package identity table over one resolution (PRD §8).
    pub packages: IdentityTable,
    /// URI-resolution face built from the same pub config (engine-api-spec
    /// §0, dartiq-resolution `PackageTable`).
    pub uri_packages: UriPackageTable,
    /// SDK snapshot; `dart:` targets resolve against [`SDK_TARGET`].
    pub sdk: SdkSnapshot,
    /// Content-addressed byte storage, engine-owned (`Send + Sync` so the
    /// fan-out can hold `&EngineState`; jobs never touch the store —
    /// they receive owned source strings).
    pub store: Box<dyn SourceStore + Send + Sync>,
    /// Workspace paths → `FileId`s.
    pub files: FileTable,
    /// Declaration-name interning, shared with dartiq-semantic and the
    /// resolution graph.
    pub strings: StringTable,

    /// Live files: content id + current facts. Removed files leave the map
    /// entirely (their `FileId`s stay interned in [`Self::files`]).
    pub residency: FxHashMap<FileId, Arc<UnitFacts>>,
    /// Canonical URI memo per interned file (a file's path never changes
    /// once interned, so neither does its canonical URI).
    pub canonical_uris: FxHashMap<FileId, Box<str>>,
    /// Library arena with per-library membership and dependency data.
    pub libraries: LibraryTable,
    /// File → owning library (defining or part), rebuilt at apply.
    pub library_of_file: FxHashMap<FileId, LibraryId>,
    /// Reverse import edges (§3.1): target file → libraries whose member
    /// files import/export it. Rebuilt with the library set at apply.
    pub importers: FxHashMap<FileId, FxHashSet<LibraryId>>,
    /// Per-library result cache.
    pub results: FxHashMap<LibraryId, LibraryResult>,
    /// Post-closure dirty set of the last apply; consumed by analyze.
    pub invalidated: Vec<LibraryId>,
    /// Whether residency/structure changed since the last resolution-graph
    /// build (rebuild gate for the analyze phase).
    pub structure_dirty: bool,

    /// Pending overlay, insertion-ordered.
    pub pending: FxHashMap<FileId, PendingChange>,
    pub pending_order: Vec<FileId>,

    /// Memoized facts of previous contents (bounded, `FACTS_CACHE_CAP`).
    facts_cache: FxHashMap<FactsKey, Arc<UnitFacts>>,
    facts_order: VecDeque<FactsKey>,
}

impl EngineState {
    /// Assembles the state from the immutable analysis context.
    ///
    /// # Errors
    ///
    /// [`crate::EngineError::PackageConfig`] when the pub package config
    /// cannot back the URI-resolution table (a broken `rootUri`).
    pub(crate) fn new(
        packages: IdentityTable,
        pub_config: &PackageConfig,
        config_dir: &Path,
        sdk: SdkSnapshot,
        store: Box<dyn SourceStore + Send + Sync>,
    ) -> Result<Self, crate::EngineError> {
        let uri_packages = UriPackageTable::new(pub_config, config_dir)
            .map_err(crate::EngineError::PackageConfig)?;
        Ok(Self {
            packages,
            uri_packages,
            sdk,
            store,
            files: FileTable::new(),
            strings: StringTable::new(),
            residency: FxHashMap::default(),
            canonical_uris: FxHashMap::default(),
            libraries: LibraryTable::default(),
            library_of_file: FxHashMap::default(),
            importers: FxHashMap::default(),
            results: FxHashMap::default(),
            invalidated: Vec::new(),
            structure_dirty: false,
            pending: FxHashMap::default(),
            pending_order: Vec::new(),
            facts_cache: FxHashMap::default(),
            facts_order: VecDeque::new(),
        })
    }

    /// Fact-cache lookup.
    pub(crate) fn cached_facts(&self, key: FactsKey) -> Option<Arc<UnitFacts>> {
        self.facts_cache.get(&key).cloned()
    }

    /// Fact-cache insert with oldest-first cap eviction. Live residency
    /// holds its own `Arc`s, so evicting a live entry costs nothing.
    pub(crate) fn cache_facts(&mut self, key: FactsKey, facts: Arc<UnitFacts>) {
        if self.facts_cache.len() >= FACTS_CACHE_CAP && !self.facts_cache.contains_key(&key) {
            while self.facts_cache.len() >= FACTS_CACHE_CAP {
                let Some(oldest) = self.facts_order.pop_front() else {
                    break;
                };
                self.facts_cache.remove(&oldest);
            }
        }
        if self.facts_cache.insert(key, facts).is_none() {
            self.facts_order.push_back(key);
        }
    }

    /// A URI resolver over the engine's package table, SDK snapshot, and file
    /// table; `dart:` URIs dispatch against [`SDK_TARGET`]. Resolution interns
    /// directive targets into [`Self::files`], hence the exclusive borrow.
    pub(crate) fn uri_resolver(&mut self) -> UriResolver<'_> {
        UriResolver::new(&self.uri_packages, &self.sdk, SDK_TARGET, &mut self.files)
    }
}
