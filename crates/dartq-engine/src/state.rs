//! Resident state (incremental-l1-spec §3.1 table): content residency, the
//! pending overlay, the library arena with its reverse-import index, the
//! per-library result cache, and the bounded per-file facts cache.

pub mod persist;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_index::{FileCapture, LibraryIndex};
use dartq_language::FeatureSet;
use dartq_package::PackageTable as IdentityTable;
use dartq_pub::PackageConfig;
use dartq_resolution::{
    Combinators, PackageTable as UriPackageTable, ResolutionCode, ResolvedTarget, UriResolver,
};
use dartq_scanner::TokenTable;
use dartq_sdk::SdkSnapshot;
use dartq_semantic::FileElements;
use dartq_source::{
    ContentId, FileId, FileTable, LineIndex, SourceRange, SourceStore, StringId, StringTable,
};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

/// The engine-side facts-shape version: bumped with any engine-side change
/// to what [`UnitFacts`] records (directive extraction, diagnostic baking).
/// Shape history — v2: name-form `part of` directives skip URI
/// resolution, and the capture-time duplicate log bakes as
/// `duplicate_definition`. v3: an above-latest `// @dart` override bakes
/// the official `invalid_language_version_override` STATIC_WARNING and
/// analyzes at the package/default version instead of adopting the
/// override (#245) — v2-cached entries for such files carry neither.
/// v4: the directive face folded into [`UnitFacts::api_signature`]
/// switched to length-prefixed lexeme framing (#398 F12) — the baked
/// signature bytes changed for every file with a directive region, so
/// v3-cached entries and snapshots must not survive the upgrade.
/// v5: conditional-URI evaluation flipped from the CFE target-availability
/// table to the oracle's declared-variables semantics (#455) —
/// v4-cached entries for files with configurable directives carry the
/// wrong selected URIs and must not survive the upgrade.
const FACTS_SHAPE_VERSION: u32 = 5;

/// The facts-cache-key version (the `AstKey.parser_version` component, PRD
/// §5.6 `EngineSemanticVersion`): the engine facts shape folded with
/// `dartq_semantic::SIGNATURE_VERSION` (the element-face composition the
/// baked [`UnitFacts::api_signature`] carries). Cached facts carry that
/// baked signature, so a signature bump must invalidate them — the fold
/// makes it automatic, in memory ([`FactsKey`]'s hash) and across restarts
/// (the snapshot `EntryKey`s and the generation header key on this same
/// value), with no manual cross-crate bump discipline and no separate
/// version comparison anywhere.
///
/// Public since #341: the snapshot wiring keys every
/// `dartq_snapshot::EntryKey` and the generation header's
/// `semantic_version` on it.
pub const FACTS_SEMANTIC_VERSION: u32 =
    (FACTS_SHAPE_VERSION << 16) | (dartq_semantic::SIGNATURE_VERSION & 0xffff);

/// Facts-cache capacity in entries. Steady-state residency is bounded by the
/// live file set; this cache only memoizes *previous* contents so a
/// change-back (git branch flip, undo) reparses for free. The entry cap
/// keeps the overhead a bounded fraction of input memory (memory law, PRD
/// §14): insertion beyond the cap evicts oldest-first.
const FACTS_CACHE_CAP: usize = 1024;

/// Scanned-face-cache capacity in entries, same discipline as
/// [`FACTS_CACHE_CAP`] (the scan face is the token table the semdiag fold
/// and the index captures consume — the engine perf charter: one keystroke
/// on a part-heavy library must not re-scan every member). Token tables
/// cost a small multiple of the source bytes, so the cap — not residency —
/// bounds the footprint; workspaces above the cap fall back to per-consumer
/// scans for the coldest entries.
const SCANNED_CACHE_CAP: usize = 1024;

/// Dead-content tally that triggers one content-store sweep (mark-sweep,
/// the rust-analyzer vfs GC posture). Every apply orphans content ids —
/// replaced or removed residency, evicted facts-cache entries — and sweeping
/// per keystroke would cost O(residency + cache + entries) each time, so
/// collection amortizes: one sweep runs once the tally crosses this bound.
/// Cache-cap scale (matches [`FACTS_CACHE_CAP`], #374): at 10⁵-file
/// residency a 64-entry cadence re-marked the whole workspace every few
/// keystrokes, while the unswept-dead tail stays the same order the facts
/// cache already retains. The timing never affects correctness (the sweep
/// keeps everything referenced, a later trigger only delays reclamation),
/// only the steady-state entry bound: residency + [`FACTS_CACHE_CAP`] +
/// fewer than this many unswept dead versions.
const GC_DEAD_THRESHOLD: usize = 1024;

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
/// residency, the facts cache, and the semantic-diagnosis seam. The
/// batch-internal dedupe clones a member's facts from its same-key
/// template's pre-resolution state (every field copies cheap: interned
/// ids, spans, diagnostics — the element model moves by `Arc` refcount).
#[derive(Clone, Debug)]
pub struct UnitFacts {
    /// The content the facts were computed from.
    pub(crate) content: ContentId,
    /// The file's API signature (PRD §11.3, #282): the canonical hash of the
    /// element face (`dartq_semantic::FileElements::api_signature`) folded
    /// with the directive-region lexemes. The L2 gate compares it against the
    /// previous residency to decide whether the library cascade propagates.
    pub(crate) api_signature: dartq_semantic::ApiSignature,
    /// Directives in source order.
    pub directives: Vec<DirectiveFacts>,
    /// The semantic element model (dartq-semantic `collect`), shared so
    /// every consumer — the batch dedupe, the facts cache, the semdiag
    /// fold — pays a refcount bump, never a deep clone (#308).
    pub elements: Arc<FileElements>,
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

    /// The file's baked API signature — the value the L2 strong-incremental
    /// gate compares (PRD §11.3) and the facts snapshot persists (#341, the
    /// #344 load seam's comparison input).
    #[must_use]
    pub const fn api_signature(&self) -> dartq_semantic::ApiSignature {
        self.api_signature
    }

    /// The unit's `part of` directive, if the unit is a part file. When both
    /// part-of forms are present the name form wins regardless of source
    /// order — the oracle's `_updateKind` priority (file_state.dart:933-949):
    /// partOfName over partOfUri.
    #[must_use]
    pub fn part_of(&self) -> Option<&DirectiveFacts> {
        self.directives
            .iter()
            .find(|directive| directive.kind == DirectiveKind::PartOf && directive.name.is_some())
            .or_else(|| {
                self.directives
                    .iter()
                    .find(|directive| directive.kind == DirectiveKind::PartOf)
            })
    }

    /// Whether the unit is a part file: a `part of` directive and no
    /// `library` directive — the oracle classifies a file carrying a
    /// `library` directive as a library regardless of any `part of`
    /// (`_updateKind`, file_state.dart:937-939).
    #[must_use]
    pub fn is_part(&self) -> bool {
        !self
            .directives
            .iter()
            .any(|directive| directive.kind == DirectiveKind::Library)
            && self.part_of().is_some()
    }
}

/// The persisted scan face of one `(content, feature set)` identity: the
/// token table the parser-level consumers (the semdiag fold, the index
/// captures) parse through, plus the *settled* feature set (the `@dart`
/// override applied — `l1::scan_unit`'s output). A pure function of the
/// [`FactsKey`] inputs, so the cache entry serves every consumer that would
/// otherwise re-derive it per dirty library or per query.
pub struct ScannedFace {
    /// The settled token table (override rescans included).
    pub tokens: TokenTable,
    /// The feature set in force after override settlement.
    pub features: FeatureSet,
}

/// One library of the analysis set: a defining file plus its parts in
/// `part`-directive order (res-spec §2.6 — the library content identity
/// folds the member contents in exactly this order).
#[derive(Debug)]
pub(crate) struct LibraryInfo {
    /// Canonical URI the library identity keys on. Shares the
    /// canonical-URI memo's allocation ([`EngineState::canonical_uris`]):
    /// reformation clones an `Arc` refcount per defining library per apply,
    /// never re-copies the bytes.
    pub uri: Arc<str>,
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
    /// Resolved targets of the members' `part` directives that point at a
    /// resident file this library does not own — neither a member (the claim
    /// pass only attaches part files) nor a dependency edge (only
    /// import/export count). Removing such a target changes no other
    /// identity component, so the residency flip rides here explicitly.
    /// Sorted by file index, deduplicated.
    pub part_targets: Box<[FileId]>,
}

/// The per-library cached result (incremental-l1-spec §3.1 last row): the
/// merged diagnostic set keyed by the library's content + dependency
/// identity. Invalidation is explicit (dirty marking), never key comparison.
#[derive(Debug)]
pub(crate) struct LibraryResult {
    pub content: Box<[ContentId]>,
    pub deps: Vec<LibraryId>,
    /// The [`LibraryInfo::part_targets`] snapshot this result was computed
    /// from; `close` compares it against the rebuilt identity.
    pub part_targets: Box<[FileId]>,
    /// The library's diagnostics in canonical order (path bytes, offset,
    /// code name, message, length, correction — `pipeline::
    /// compare_canonical`): sorted once per recompute at store time, so the
    /// analyze-time merge interleaves the runs without re-sorting the cache.
    pub diagnostics: Vec<Diagnostic>,
}

/// Cache key of the per-file facts cache: the `AstKey{content, feature_set,
/// parser_version}` triple of PRD §5.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FactsKey {
    pub content: ContentId,
    pub feature_fingerprint: u64,
    /// The [`FACTS_SEMANTIC_VERSION`] component — rides the hash, so any
    /// engine facts-shape or semantic signature bump invalidates the entry.
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
/// removed library returns its id to the arena's reuse freelist.
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
/// Retired ids are recycled through a freelist, so the slot count tracks the
/// historical live peak rather than the total interns.
#[derive(Debug, Default)]
pub(crate) struct LibraryTable {
    ids_by_uri: FxHashMap<Box<str>, LibraryId>,
    infos: Vec<Option<LibraryInfo>>,
    /// Retired ids available for reuse. Entries are distinct (an id can
    /// re-enter only through a later [`Self::retire_except`]), so the list
    /// is bounded by the historical live peak. A recycled id is handed to
    /// whatever interns next and is indistinguishable from its previous
    /// incarnation — reuse safety is the caller's ordering contract (see
    /// [`Self::retire_except`]).
    free: Vec<LibraryId>,
}

impl LibraryTable {
    /// The library registered under `uri`, interning the id on first sight.
    /// A previously retired id is reused before the arena grows. The uri is
    /// borrowed — the table clones only on the insert path, so the hot
    /// hit-per-apply path pays no heap allocation.
    pub(crate) fn intern(&mut self, uri: &str) -> LibraryId {
        if let Some(&id) = self.ids_by_uri.get(uri) {
            return id;
        }
        let id = if let Some(id) = self.free.pop() {
            id
        } else {
            let id = LibraryId::next(self.infos.len());
            self.infos.push(None);
            id
        };
        self.ids_by_uri.insert(Box::from(uri), id);
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

    /// Drops every registration whose id is not in `keep`, returning their
    /// ids to the reuse freelist. Returns the retired ids (ascending).
    ///
    /// Ordering contract (the freelist-recycling hazard, #374): a recycled
    /// id aliases its previous incarnation — stale snapshots holding it
    /// (`LibraryResult::deps`, library identity arrays) compare equal
    /// against the new tenant. Recycling is safe only when nothing stale is
    /// left to alias: the caller must complete every intern of the current
    /// batch *before* retiring (an id retired early could be re-minted to a
    /// different URI within the same batch) and must drop cached results of
    /// retired ids alongside. l1's apply honors this: `retire_except` runs
    /// at write-back, after the reform passes' interns, and removes each
    /// retired id's result in the same sweep.
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
            self.free.push(*id);
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
    /// §0, dartq-resolution `PackageTable`).
    pub uri_packages: UriPackageTable,
    /// SDK snapshot; `dart:` targets resolve against [`Self::target`].
    pub sdk: SdkSnapshot,
    /// The SDK target segment (the `libraries.json` panel — `vm`, `js`, …)
    /// `dart:` URI resolution dispatches against. A context dimension
    /// (#483): `EngineConfig::target` carries it, `"vm"` by default (the
    /// `dart analyze` posture).
    pub target: String,
    /// Declared variables (`DeclaredVariables`) of the conditional-import
    /// evaluation: the `-D name=value` pairs configurable directives test
    /// against. Empty by default — the no-`-D` oracle context
    /// (`driver.dart:583-593`), under which every configurable directive
    /// resolves its primary URI. A context dimension (#483):
    /// `EngineConfig::declared_variables` carries it.
    pub declared_variables: FxHashMap<String, String>,
    /// Content-addressed byte storage, engine-owned (`Send + Sync` so the
    /// fan-out can hold `&EngineState`; jobs never touch the store —
    /// they receive owned source strings).
    pub store: Box<dyn SourceStore + Send + Sync>,
    /// Workspace paths → `FileId`s.
    pub files: FileTable,
    /// Declaration-name interning, shared with dartq-semantic and the
    /// resolution graph.
    pub strings: StringTable,

    /// Live files: content id + current facts. Removed files leave the map
    /// entirely (their `FileId`s stay interned in [`Self::files`]).
    pub residency: FxHashMap<FileId, Arc<UnitFacts>>,
    /// Canonical URI memo per interned file (a file's path never changes
    /// once interned, so neither does its canonical URI). The `Arc`
    /// allocation is shared, not copied: [`LibraryInfo::uri`] reuses it, so
    /// reformation pays a refcount per defining library per apply, never a
    /// heap clone.
    pub canonical_uris: FxHashMap<FileId, Arc<str>>,
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
    /// Invariant: every entry is a live library id — apply assigns `close`'s
    /// output outright; `close` seeds the incoming leftovers (filtered to the
    /// post-reform live set) into its dirty set, so a cancelled analyze's
    /// still-invalid libraries carry forward. Unioning the incoming set back
    /// directly could instead resurrect ids retired by an intervening remove
    /// (#230), and analyze unwraps every queued id's info.
    pub invalidated: Vec<LibraryId>,
    /// Whether residency/structure changed since the last resolution-graph
    /// build (rebuild gate for the analyze phase).
    pub structure_dirty: bool,

    /// Pending overlay, insertion-ordered.
    pub pending: FxHashMap<FileId, PendingChange>,
    pub pending_order: Vec<FileId>,

    /// Resolved analysis options per package root (PRD §12): the include
    /// chain merged once at context assembly; discovery applies the exclude
    /// globs, the lint face reads the rule names.
    pub options: FxHashMap<PathBuf, Arc<crate::analysis_options::AnalysisOptions>>,

    /// Warm-start staging (#482): a loaded facts snapshot's payloads, keyed
    /// by the entry triple. `Engine::ingest_facts_snapshot` fills it; the
    /// next apply consumes exact identity matches — the lookup IS the
    /// content-hash verification, a hit serving only a file whose live
    /// content id equals the key's hash — and drops everything left, so
    /// stale entries never survive an apply. Ingestion is grow-only: it
    /// seeds recomputes, it never removes residency (#377).
    pub warm_start: FxHashMap<dartq_snapshot::EntryKey, persist::PersistedFacts>,

    /// Memoized pure per-file facts of previous contents — pre-resolution,
    /// so a cross-batch hit re-runs the per-file attribution and URI
    /// resolution at the hit site (l1 `derive_facts`; the [`FactsKey`] triple
    /// is path-free by contract, PRD §5.3). Bounded, `FACTS_CACHE_CAP`.
    facts_cache: FxHashMap<FactsKey, Arc<UnitFacts>>,
    facts_order: VecDeque<FactsKey>,
    /// Memoized scan faces keyed by the same [`FactsKey`] triple as the
    /// facts cache: the token table + settled feature set the semdiag fold
    /// and the index captures parse through. Filled at the apply recompute
    /// (the one place a scan runs) and drained consumers fall back to a
    /// local scan on a miss. Bounded, `SCANNED_CACHE_CAP`.
    scanned: FxHashMap<FactsKey, Arc<ScannedFace>>,
    scanned_order: VecDeque<FactsKey>,
    /// The maintained symbol-index inputs (index-design-spec §6/§8 — the
    /// Engine owns dirty scheduling, the index never invalidates itself):
    /// one capture per resident file (declaration surface + raw sites, the
    /// latter feeding `LibraryBuilder` on every dirty-library rebuild), and
    /// one reference surface per live library. The captures replace whole
    /// when a file's content moves and drop when it leaves residency; the
    /// surfaces rebuild at the `LibraryAnalyzed` point of each analyze
    /// (pipeline) and drop with their library at retirement. Bounded by
    /// residency and the live library set respectively.
    pub(crate) captures: BTreeMap<FileId, Arc<FileCapture>>,
    pub(crate) library_surfaces: BTreeMap<LibraryId, LibraryIndex>,
    /// Resident files whose content moved since the last analyze — the
    /// recapture queue the analyze phase drains. A body edit with a stable
    /// API signature keeps its library clean (the L2 win) but still moves
    /// declaration spans, so recapture tracks changed files directly, not
    /// dirty libraries.
    capture_dirty: Vec<FileId>,
    /// Dead-content tally since the last store sweep — the amortization
    /// signal of [`GC_DEAD_THRESHOLD`] (never a liveness decision; the sweep
    /// itself checks real references).
    gc_debt: usize,
}

impl EngineState {
    /// Assembles the state from the immutable analysis context.
    ///
    /// # Errors
    ///
    /// [`crate::EngineError::PackageConfig`] when the pub package config
    /// cannot back the URI-resolution table (the shared `resolve` face
    /// rejected it).
    pub(crate) fn new(
        packages: IdentityTable,
        pub_config: &PackageConfig,
        config_dir: &Path,
        sdk: SdkSnapshot,
        store: Box<dyn SourceStore + Send + Sync>,
        target: String,
        declared_variables: FxHashMap<String, String>,
    ) -> Result<Self, crate::EngineError> {
        let uri_packages = UriPackageTable::new(pub_config, config_dir)?;
        // Options are a context field: the include chains merge once here,
        // keyed by package root (cold path, workspace-assembly time).
        let mut roots: Vec<PathBuf> = packages
            .iter()
            .map(|package| package.root.clone())
            .collect();
        roots.sort();
        roots.dedup();
        let options =
            crate::analysis_options::AnalysisOptions::for_roots(&roots, pub_config, config_dir);
        Ok(Self {
            packages,
            uri_packages,
            sdk,
            target,
            declared_variables,
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
            options,
            warm_start: FxHashMap::default(),
            facts_cache: FxHashMap::default(),
            facts_order: VecDeque::new(),
            scanned: FxHashMap::default(),
            scanned_order: VecDeque::new(),
            captures: BTreeMap::new(),
            library_surfaces: BTreeMap::new(),
            capture_dirty: Vec::new(),
            gc_debt: 0,
        })
    }

    /// Fact-cache lookup.
    pub(crate) fn cached_facts(&self, key: FactsKey) -> Option<Arc<UnitFacts>> {
        self.facts_cache.get(&key).cloned()
    }

    /// Fact-cache insert with oldest-first cap eviction. Live residency
    /// holds its own `Arc`s, so evicting a live entry costs nothing. Each
    /// eviction tallies one dead-content candidate: the evicted key's
    /// content may still be referenced elsewhere (residency, another cache
    /// entry sharing the id), and the sweep — not this tally — resolves
    /// that.
    pub(crate) fn cache_facts(&mut self, key: FactsKey, facts: Arc<UnitFacts>) {
        if self.facts_cache.len() >= FACTS_CACHE_CAP && !self.facts_cache.contains_key(&key) {
            while self.facts_cache.len() >= FACTS_CACHE_CAP {
                let Some(oldest) = self.facts_order.pop_front() else {
                    break;
                };
                self.facts_cache.remove(&oldest);
                self.gc_debt += 1;
            }
        }
        if self.facts_cache.insert(key, facts).is_none() {
            self.facts_order.push_back(key);
        }
    }

    /// Scanned-face lookup.
    pub(crate) fn cached_scanned(&self, key: FactsKey) -> Option<Arc<ScannedFace>> {
        self.scanned.get(&key).cloned()
    }

    /// Scanned-face insert with the facts cache's oldest-first cap
    /// eviction. No GC tally: a scanned face holds interned ids and token
    /// tables, never a store `ContentId`, so its eviction cannot orphan
    /// content.
    pub(crate) fn cache_scanned(&mut self, key: FactsKey, face: ScannedFace) {
        if self.scanned.len() >= SCANNED_CACHE_CAP && !self.scanned.contains_key(&key) {
            while self.scanned.len() >= SCANNED_CACHE_CAP {
                let Some(oldest) = self.scanned_order.pop_front() else {
                    break;
                };
                self.scanned.remove(&oldest);
            }
        }
        if self.scanned.insert(key, Arc::new(face)).is_none() {
            self.scanned_order.push_back(key);
        }
    }

    /// Files whose facts moved since the last analyze (apply queues them;
    /// the analyze phase drains and recaptures).
    pub(crate) fn mark_captures_dirty(&mut self, file: FileId) {
        self.capture_dirty.push(file);
    }

    /// Takes the recapture queue. The caller filters out files that left
    /// residency (a change followed by a removal between analyzes).
    pub(crate) fn take_capture_dirty(&mut self) -> Vec<FileId> {
        std::mem::take(&mut self.capture_dirty)
    }

    /// Amortized content-store collection (mark-sweep): tallies the content
    /// ids just orphaned and, once [`GC_DEAD_THRESHOLD`] is crossed, sweeps
    /// every store entry that neither residency nor the facts cache still
    /// references. This is the single place that knows both reference
    /// holders, so the liveness rule lives here and nowhere else; the store
    /// only sweeps what it is told is dead.
    ///
    /// The library identity arrays (`LibraryInfo::content`,
    /// `LibraryResult::content`) hold `ContentId`s too, but only compare
    /// them — no byte reads — so eviction under them is safe: content
    /// addressing reproduces the same id if the bytes ever return.
    ///
    /// Called at the end of every non-empty apply (l1 `apply`), after
    /// residency has been rewritten, so replaced contents are already
    /// unreferenced when the sweep runs.
    pub(crate) fn collect_garbage(&mut self, newly_dead: usize) {
        self.gc_debt += newly_dead;
        if self.gc_debt < GC_DEAD_THRESHOLD {
            return;
        }
        self.gc_debt = 0;
        let mut live: FxHashSet<ContentId> = FxHashSet::with_capacity_and_hasher(
            self.residency.len() + self.facts_cache.len(),
            FxBuildHasher,
        );
        live.extend(self.residency.values().map(|facts| facts.content));
        live.extend(self.facts_cache.keys().map(|key| key.content));
        self.store
            .collect_garbage(&|id: ContentId| live.contains(&id));
    }

    /// A URI resolver over the engine's package table, SDK snapshot, and file
    /// table; `dart:` URIs dispatch against [`Self::target`], conditional
    /// directives evaluate against [`Self::declared_variables`]. Resolution
    /// interns directive targets into [`Self::files`], hence the exclusive
    /// borrow. `resident` is the file-state existence face — which interned
    /// ids hold content in the analyzed world (disk files and declared
    /// overlays); resolution codes missing-target diagnostics only against
    /// it. Faces that never probe existence (e.g. `canonical_uri`) pass
    /// `&|_| false`.
    pub(crate) fn uri_resolver<'a>(
        &'a mut self,
        resident: &'a dyn Fn(FileId) -> bool,
    ) -> UriResolver<'a> {
        UriResolver::new(
            &self.uri_packages,
            &self.sdk,
            &self.target,
            &self.declared_variables,
            &mut self.files,
            resident,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_the_live_id_and_reuses_retired_ones() {
        let mut table = LibraryTable::default();
        let a = table.intern("package:a/a.dart");
        let b = table.intern("package:b/b.dart");
        assert_ne!(a, b);
        assert_eq!(table.intern("package:a/a.dart"), a, "live uri keeps its id");
        assert_eq!(table.arena_len(), 2);

        // Retiring everything returns both ids ascending and unregisters
        // them; the next interns re-mint those ids for different URIs — the
        // aliasing the retire_except ordering contract guards against. The
        // freelist pops in hash-iteration order (only the returned vec is
        // sorted), so which tenant gets which id is unspecified.
        let retired = table.retire_except(&FxHashSet::default());
        assert_eq!(retired, vec![a, b]);
        let c = table.intern("package:c/c.dart");
        let d = table.intern("package:d/d.dart");
        assert!(retired.contains(&c) && retired.contains(&d) && c != d);
        assert_eq!(
            table.arena_len(),
            2,
            "reuse keeps the arena at the live peak"
        );
    }

    #[test]
    fn retire_except_keeps_live_ids_and_recycles_retired_ones() {
        let mut table = LibraryTable::default();
        let a = table.intern("package:a/a.dart");
        let b = table.intern("package:b/b.dart");
        let c = table.intern("package:c/c.dart");
        let mut keep = FxHashSet::default();
        keep.insert(b);
        let retired = table.retire_except(&keep);
        assert_eq!(retired, vec![a, c]);

        // The kept id stays registered; the retired uris are unregistered.
        assert_eq!(table.ids_by_uri.get("package:b/b.dart"), Some(&b));
        assert!(table.ids_by_uri.get("package:a/a.dart").is_none());
        assert!(table.ids_by_uri.get("package:c/c.dart").is_none());

        // A retired uri interns as a fresh registration, not a resurrection
        // of stale info: it draws a recycled id from the freelist instead of
        // growing the arena.
        let a2 = table.intern("package:a/a.dart");
        assert!(a2 == a || a2 == c, "interns draw from the freelist");
        assert_eq!(table.ids_by_uri.get("package:a/a.dart"), Some(&a2));
        assert_eq!(table.arena_len(), 3);
    }
}
