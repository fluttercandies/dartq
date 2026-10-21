//! Resident state: content residency, the
//! pending overlay, the library arena with its reverse-import index, the
//! per-library result cache, and the bounded per-file facts cache.

pub mod persist;

use std::borrow::Borrow;
use std::collections::VecDeque;
use std::hash::Hash;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use dartq_diagnostics::Diagnostic;
use dartq_index::{FileCapture, LibraryIndex, Postings, distinct_decl_names};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_package::PackageTable as IdentityTable;
use dartq_pub::PackageConfig;
use dartq_resolution::{
    Combinators, Configuration, PackageTable as UriPackageTable, ResolutionCode, ResolvedTarget,
    UriResolver,
};
use dartq_scanner::TokenTable;
use dartq_sdk::{AllowedExperiments, SdkSnapshot};
use dartq_semantic::{ApiSignature, FileElements};
use dartq_source::{
    ContentId, FileId, FileTable, LineIndex, SourceRange, SourceStore, SourceText, StringId,
    StringTable, saturate_slot_u32,
};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

/// The engine-side facts-shape version: bumped with any engine-side change
/// to what [`UnitFacts`] records (directive extraction, diagnostic baking).
/// Shape history — v2: name-form `part of` directives skip URI
/// resolution, and the capture-time duplicate log bakes as
/// `duplicate_definition`. v3: an above-latest `// @dart` override bakes
/// the official `invalid_language_version_override` `STATIC_WARNING` and
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
/// v6: directives carry their `if`-clause configurations with per-clause
/// URI spans (#512) — resolution forks per context and a selected branch's
/// missing-target diagnostic attributes to the branch literal; v5 payloads
/// carry no clause rows, so a restored configurable directive would lose
/// its fork decision input.
/// v7: combinators carry their written token sequence with per-token UTF-16
/// spans (#539) — the undefined-combinator-name report anchors (one report
/// per written token, at its captured span); v6 payloads restore span-less
/// through the span-less constructor, so a warm start would regress the
/// reports to offset-0 anchors and lose written order and duplicates.
/// v8: combinators persist one payload element per source clause (#719) —
/// the written tokens carry their clause boundary
/// ([`Combinators::written_with_clauses`]) because W3 made boundaries
/// observable (`written_shapes_equal`'s duplicate-directive identity and
/// the duplicate-combinator check's per-clause seen-set); v7 payloads
/// merged adjacent same-kind clauses, so a warm start would over-report
/// duplicate combinators and equate `show a show b` with `show a, b`.
/// v9: the capture interns written type arguments into the unit's
/// `ArgPool` (#728) — no payload-layout delta (the args columns already
/// round-trip), but a v8 payload decodes every declared type
/// argument-less, so a warm start would diverge from a fresh capture
/// without any error; the bump forks the generation header and forces
/// re-capture.
/// v10: the unit facts fold the scan face's lexical diagnostics in
/// (#1325) — no payload-layout delta (the diagnostics column already
/// round-trips), but a v9 payload decodes scanner-row-less, so a warm
/// start would keep missing every unterminated-string/illegal-character
/// row until the file changes; the bump forks the generation header and
/// forces recompute.
/// v11: diagnostics carry their `contextMessages` rows (#1821) — a
/// v10 payload restores context-less, so a warm start would silently
/// drop every related location a producer attached (#1798's emitters);
/// the field is `skip_serializing_if = "Option::is_none"`, so the
/// wire bytes of a no-context row are byte-identical to v10 and the
/// shape pin (`FACTS_SHAPE_JSON_PIN`) stays put. The same train also
/// grew the semantic mirror columns under this fold rather than their
/// own bumps (every field `serde(default)`, so a v10 payload decodes
/// exactly): formals and extension-type rep-formals carry the covariant
/// bit and the default-value source text, and members carry their
/// annotation stars (#1637); annotations carry their argument-text slot
/// (#1083); members carry their name-token anchor span (#1867). The
/// shape probe rode empty then, so these rows first pin through the
/// populated-elements probe — this entry is the record of when the
/// columns entered the tree.
/// v12: declared types carry the function-shape variant
/// (`PersistedTy::Function` — the return type, the type-parameter count,
/// and the formal columns as nested content, the record-variant recipe) —
/// a v11 payload decodes every function-typed formal and return
/// `Unknown`, so a warm start would silently lose the function-shape
/// faces the diagnostics and lint rules read. The same train raised
/// `dartq_semantic::SIGNATURE_VERSION` to 7 (the capture output changed
/// with the layout: formals no longer capture `Unknown` for written
/// function shapes), so the folded `FACTS_SEMANTIC_VERSION` moves on
/// both lanes. The function-typed formal row first pins through the
/// populated-elements probe (the v11 precedent).
/// v13: containers carry the const-constructor bit
/// (`PersistedContainer::has_const_constructor`) — a v12 payload decodes
/// every container const-less, so a warm start would re-fire the
/// cross-file `const_with_non_const` /
/// `const_initialized_with_non_constant_value` over-report the bit
/// closes. The same train raised `dartq_semantic::SIGNATURE_VERSION` to
/// 8.
/// v14: unit facts carry the extracted `@docImport` rows
/// (`UnitFacts::doc_imports`, `PersistedFacts::doc_imports`) — a v13
/// payload restores doc-import-less, so a warm start would drop every
/// `uri_does_not_exist_in_doc_import` row and mis-slice the URI
/// diagnostic tail (the tail counts coded doc-import rows too).
/// v15: the extension-face mirror carries the type-parameter bounds
/// (`PersistedExtensionFace::type_param_bounds`, #2431's additive
/// column). The column landed WITHOUT its bump — the shape probes both
/// rode `extension_faces` empty, so the guard saw zero bytes move — and
/// the `serde(default)` decode is not the safe direction its field doc
/// claimed: a pre-column payload restores bounds-empty and the bound
/// gate reads past-the-end as `Dynamic`, so a warm start over-admits
/// every bound-excluded face (#2470's forensic bug class). This entry is
/// the record of when the column entered the tree and the retroactive
/// bump that forks the pre-column generations; the populated-elements
/// probe now carries a bounds-carrying face, so the next layout edit in
/// that subtree moves bytes again. The same ticket restored the guard
/// itself: the typedef-RHS train left the probe's `PersistedTopDecl`
/// initializer without that train's `type_param_names` field, so the
/// lib-test module — both shape pins with it — had been uncompilable and
/// the structural guard unrunnable ever since.
///
/// # Bump contract (structural guard)
///
/// Any change to the persisted payload layout — a field added to, removed
/// from, renamed in, retyped in, or reordered within the
/// [`persist::PersistedFacts`] tree, a mirror-enum variant added, or a serde
/// representation change — must bump this version in the same change: a
/// payload written by the old shape must never be served to code expecting
/// the new one. The discipline is structural, not memory-resident:
/// `persist::tests::persisted_payload_shape_is_pinned` pins the byte-exact
/// JSON encoding of a representative payload, so a layout edit without a
/// bump fails the guard, and its message names the fix — bump this version,
/// extend the history above, re-pin. (Mirror-enum *variant* additions are
/// already forced through a compile review by the exhaustive `of`/`restore`
/// matches; they change the on-disk schema all the same, so they bump this
/// version too.) The bump lane also forks on capture-behavior changes with
/// no layout delta: a payload the old producer wrote must not be served to
/// a producer whose capture output diverges from it.
const FACTS_SHAPE_VERSION: u32 = 15;

/// The facts-cache-key version (the `AstKey.parser_version` component of
/// `EngineSemanticVersion`): the engine facts shape folded with
/// `dartq_semantic::SIGNATURE_VERSION` (the element-face composition the
/// baked [`UnitFacts::api_signature`] carries). Cached facts carry that
/// baked signature, so a signature bump must invalidate them — the fold
/// makes it automatic, in memory (`FactsKey`'s hash) and across restarts
/// (the snapshot `EntryKey`s and the generation header key on this same
/// value), with no manual cross-crate bump discipline and no separate
/// version comparison anywhere.
///
/// Public since #341: the snapshot wiring keys every
/// `dartq_snapshot::EntryKey` and the generation header's
/// `semantic_version` on it.
pub const FACTS_SEMANTIC_VERSION: u32 =
    (FACTS_SHAPE_VERSION << 16) | (dartq_semantic::SIGNATURE_VERSION & 0xffff);

// The fold above is injective only while the signature fits its 16-bit lane:
// a SIGNATURE_VERSION of 2^16 or more would wrap and alias an older
// snapshot's key as current. Widening the key rides the next
// FACTS_SHAPE_VERSION bump (a container-format change anyway).
const _: () = assert!(dartq_semantic::SIGNATURE_VERSION <= u16::MAX as u32);

/// Facts-cache capacity in entries. Steady-state residency is bounded by the
/// live file set; this cache only memoizes *previous* contents so a
/// change-back (git branch flip, undo) reparses for free. The entry cap
/// keeps the overhead a bounded fraction of input memory (the memory
/// law): insertion beyond the cap evicts oldest-first.
const FACTS_CACHE_CAP: usize = 1024;

/// Scanned-face-cache capacity in entries, same discipline as
/// [`FACTS_CACHE_CAP`] (the scan face is the token table the semdiag fold
/// and the index captures consume — the engine perf charter: one keystroke
/// on a part-heavy library must not re-scan every member). Token tables
/// cost a small multiple of the source bytes, so the cap — not residency —
/// bounds the footprint; workspaces above the cap fall back to per-consumer
/// scans for the coldest entries.
const SCANNED_CACHE_CAP: usize = 1024;

/// Raw-site-cache capacity in files, same oldest-first discipline as
/// [`FACTS_CACHE_CAP`] (`#1859`): a capture's raw reference sites are a
/// pure function of (content, feature set) and only the library rebuild
/// reads them, so all but the freshest `SITES_CACHE_CAP` recaptures shed
/// their site columns (~0.8–1.6 KB/file) and a rebuild miss re-derives
/// them locally — the scanned-face fallback posture. The declaration
/// surface stays resident for every capture (the query face reads it);
/// only the site columns are sheddable.
const SITES_CACHE_CAP: usize = 1024;

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

/// The estimated resident bytes per interned path of the [`FileTable`]:
/// one hash entry (key `Arc` + id, with load-factor slack), one id-arena
/// slot, one memo entry with its owned spelling key, and the `Arc`'s
/// path-bytes allocation. Est. — the components are not exactly derivable
/// (hash slack, path length), so the figure sits at the census's 200 B–1 KB
/// per-path band's low midpoint; its role is truthful degrade-face
/// reporting of the unsheddlable structural state, not measurement (#1887).
const FILE_TABLE_EST_BYTES_PER_PATH: usize = 256;

/// The measured resident bytes per interned entry of the [`StringTable`]:
/// append-only leaked text plus one hash slot and one id slot each, the
/// crate's documented ~93 B/entry measurement (#1887).
const STRING_TABLE_BYTES_PER_ENTRY: usize = 93;

/// The session live-path ceiling's degrade face (#1887): the
/// [`ResidencyDegradeLog`] row a tripped admission records, deduped per
/// (file, face) like every pairing row.
const LIVE_PATH_CEILING_FACE: &str = "session live-path ceiling";

/// One `@docImport` directive extracted from the doc comment attached to the
/// unit's first `library` (library file) or first `part of` directive (part
/// file) — the oracle's `_resolveDocImports` attach rule
/// (library_analyzer.dart:808-846): doc comments on any other declaration
/// are never validated. Extraction is a pure function of the unit's content
/// (the resolution crate's `doc_imports` over the captured comment lexemes);
/// `code` is the URI-resolution outcome's diagnostic face, filled by
/// `resolve_unit` like [`DirectiveFacts::code`].
#[derive(Clone, Debug)]
pub struct DocImportFacts {
    /// The URI string the oracle reports (quotes stripped, escapes decoded,
    /// adjacent strings concatenated), or `None` for an interpolated URI —
    /// the `uri_with_interpolation` face, same as a real import.
    pub uri: Option<Box<str>>,
    /// UTF-16 span of the URI string literal (raw text, quotes included) —
    /// the diagnostic anchor.
    pub uri_span: SourceRange,
    /// The frozen URI-layer diagnostic code for this outcome, if any.
    pub code: Option<ResolutionCode>,
}

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
    /// URI-layer resolution outcome: file targets are
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
    /// The `if (dottedName (== 'value')?) 'uri'` clauses of an import/export,
    /// in source order — the conditional-import fork's decision input
    /// (#512). Empty for every other directive family and for clause-free
    /// imports/exports. Resolution picks the first clause the context's
    /// declared variables match; the primary URI is the fallback.
    pub configurations: Box<[Configuration]>,
    /// UTF-16 span of each clause's URI literal, index-locked to
    /// [`DirectiveFacts::configurations`] (built together in the diet walk).
    /// A selected branch's missing-target diagnostic attributes to the
    /// branch's own literal — the oracle reports `state.selectedUri`
    /// (library_analyzer.dart:698-717), never the primary's span.
    pub configuration_spans: Box<[SourceRange]>,
}

/// Everything one compilation unit contributes, keyed by its content
/// (the per-file cache rows "per-file AST" and "per-file unlinked
/// facts"): extracted directives with their URI resolution, the semantic
/// element model, and the file's syntax + URI-layer diagnostics.
///
/// Produced by the apply-phase recompute (fan-out for scan/parse/
/// collect, then sequential URI resolution) and shared as an `Arc` between
/// residency, the facts cache, and the semantic-diagnosis seam. The
/// batch-internal dedupe clones a member's facts from its same-key
/// template's pre-resolution state (every field copies cheap: interned
/// ids, spans — the element model and the diagnostics rows move by
/// `Arc` refcount, re-attributed rows excepted).
#[derive(Clone, Debug)]
pub struct UnitFacts {
    /// The content the facts were computed from.
    pub(crate) content: ContentId,
    /// The file's API signature: the canonical hash of the
    /// element face (`dartq_semantic::FileElements::api_signature`) folded
    /// with the directive-region lexemes. The L2 gate compares it against the
    /// previous residency to decide whether the library cascade propagates.
    pub(crate) api_signature: dartq_semantic::ApiSignature,
    /// Directives in source order.
    pub directives: Vec<DirectiveFacts>,
    /// The `@docImport`s of the file's one validating doc comment (empty for
    /// almost every file): the `library`-/`part of`-attached comment's
    /// extracted directives, resolved under [`DocImportFacts::code`]. The
    /// URI diagnostic tail counts one pushed row per coded entry here too,
    /// the same construction invariant the directive column keeps.
    pub doc_imports: Vec<DocImportFacts>,
    /// The semantic element model (dartq-semantic `collect`), shared so
    /// every consumer — the batch dedupe, the facts cache, the semdiag
    /// fold — pays a refcount bump, never a deep clone (#308).
    pub elements: Arc<FileElements>,
    /// Syntax (scan + parse recovery) and URI-layer diagnostics, all
    /// attributed to this file — a pure function of content, feature set,
    /// and the immutable analysis context. Rows are refcounted: the
    /// per-library results hold handles to these very rows (the S4
    /// dedup, #1501), so the batch-internal dedupe and the facts-cache
    /// clones share bytes instead of copying them.
    pub diagnostics: Vec<Arc<Diagnostic>>,
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
    /// gate compares and the facts snapshot persists (#341, the
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
/// `FactsKey` inputs, so the cache entry serves every consumer that would
/// otherwise re-derive it per dirty library or per query.
pub struct ScannedFace {
    /// The settled token table (override rescans included).
    pub tokens: TokenTable,
    /// The feature set in force after override settlement.
    pub features: FeatureSet,
    /// The settled version the feature set was derived at (the `@dart`
    /// override when one applies, else the package-derived version) — the
    /// face the lint fan-out's registration gates consult. Note the scanned
    /// cache identity keys the face by feature fingerprint, so two versions
    /// with byte-identical granted sets share one face; a version gate
    /// between two such versions would need the version folded into
    /// `FactsKey` (no current gate does — the 3.7 gate aligns with the
    /// `wildcard_variables` fingerprint edge).
    pub version: LanguageVersion,
}

/// One library surface's reference rows regrouped by use file: per file,
/// the row indices into [`LibraryIndex::refs`], offset-ascending — the
/// position face's per-file probe domain.
pub(crate) type UseView = Box<[(FileId, Box<[u32]>)]>;

/// Builds `surface`'s [`UseView`]: a pure function of the surface's rows,
/// so the analyze fan-out derives it in the job that built the surface and
/// the serial store loop only installs it.
pub(crate) fn use_view_of(surface: &LibraryIndex) -> UseView {
    let mut grouped: Vec<(FileId, u32, u32)> = surface
        .refs()
        .iter()
        .enumerate()
        .map(|(i, row)| (row.file, row.range.offset, saturate_slot_u32(i)))
        .collect();
    grouped.sort_unstable_by_key(|&(file, offset, _)| (file, offset));
    let mut view: Vec<(FileId, Vec<u32>)> = Vec::new();
    for (file, _, i) in grouped {
        match view.last_mut() {
            Some((current, rows)) if *current == file => rows.push(i),
            _ => view.push((file, vec![i])),
        }
    }
    view.into_iter()
        .map(|(file, rows)| (file, rows.into_boxed_slice()))
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

/// One member entry's owned heap bytes: the boxed slices behind its
/// type-parameter names, annotations, and formals (the `Formals` shell
/// itself is a `Box` in the entry, already inside the entry's
/// `size_of::<MemberEntry>()` — only the payloads count).
fn member_heap_bytes(member: &dartq_semantic::MemberEntry) -> usize {
    std::mem::size_of_val(&*member.type_param_names)
        + std::mem::size_of_val(&*member.annotations)
        + member.formals.as_deref().map_or(0, |formals| {
            std::mem::size_of_val(formals)
                + std::mem::size_of_val(&*formals.positional)
                + std::mem::size_of_val(&*formals.named)
        })
}

/// One [`FileElements`] model's enumerable heap bytes — the element
/// models ride `Arc<FileElements>` behind residency and the facts cache
/// and are the parse side's standing product. Exact over the enumerable
/// columns (the `NameMap` rows, the clause slices, the boxed slices);
/// the `ArgPool` interning tables and the `Ty` enum's nested payloads are
/// not enumerable from this crate, so the total under-reports by that
/// residue — bounded by the interned-type count, not file size (the
/// family is tagged `est` for it).
fn element_heap_bytes(elements: &FileElements) -> usize {
    let mut bytes = std::mem::size_of_val(elements)
        + elements.declarations().count()
            * (size_of::<StringId>() + size_of::<dartq_semantic::TopDecl>())
        + elements.containers().count()
            * (size_of::<StringId>() + size_of::<dartq_semantic::Container>())
        + std::mem::size_of_val(elements.container_order())
        + std::mem::size_of_val(elements.extension_faces())
        + std::mem::size_of_val(elements.duplicates())
        + std::mem::size_of_val(elements.arity_violations())
        + std::mem::size_of_val(elements.type_args());
    for (_, container) in elements.containers() {
        bytes += container.members.len()
            * (size_of::<StringId>() + size_of::<dartq_semantic::MemberEntry>())
            + std::mem::size_of_val(&*container.with_clause)
            + std::mem::size_of_val(&*container.implements_clause)
            + std::mem::size_of_val(&*container.on_clause)
            + std::mem::size_of_val(&*container.header_residue)
            + std::mem::size_of_val(&*container.primary_formals)
            + std::mem::size_of_val(&*container.enum_constants)
            + std::mem::size_of_val(&*container.constructors)
            + std::mem::size_of_val(&*container.annotations)
            + container.ctor_formals.len()
                * (size_of::<StringId>() + size_of::<usize>())
            + container
                .ctor_formals
                .values()
                .map(|formals| {
                    std::mem::size_of_val(&**formals)
                        + std::mem::size_of_val(&*formals.positional)
                        + std::mem::size_of_val(&*formals.named)
                })
                .sum::<usize>()
            + container
                .members
                .values()
                .map(member_heap_bytes)
                .sum::<usize>();
    }
    for face in elements.extension_faces() {
        bytes += face.members.len()
            * (size_of::<StringId>() + size_of::<dartq_semantic::MemberEntry>())
            + std::mem::size_of_val(&*face.type_param_names)
            + face.members.values().map(member_heap_bytes).sum::<usize>();
    }
    bytes
}

/// The deepest entry of `map` whose key contains `path`, probing the
/// path's own ancestor chain deepest-first ([`Path::ancestors`] — the
/// `Engine::apply_severity_options` lookup shape). The containing keys of
/// a path are exactly its ancestors, ancestors are pairwise nested, and
/// the map holds distinct keys — so exactly one entry has the greatest
/// depth and a tie cannot occur; the first hit is the deepest match by
/// construction, with no tie-break and no dependence on iteration order.
fn deepest_containing<'a, K: Borrow<Path> + Eq + Hash, V>(
    map: &'a FxHashMap<K, V>,
    path: &Path,
) -> Option<&'a V> {
    path.ancestors().find_map(|dir| map.get(dir))
}

/// The file-side owner of allowed-experiment grants — a package name from
/// the pub config, or a `dart:` library name from the SDK's target table.
/// The two namespaces are distinct in `allowed_experiments.json`
/// (`packages` vs `sdk.libraries`), so one enum keeps a package named
/// `core` from reading the library's grant.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GrantOwner {
    Package(Box<str>),
    SdkLibrary(Box<str>),
}

/// The per-file allowed-experiment authorization, merged into every file's
/// derived [`FeatureSet`] (audit #708 P2-1).
///
/// The oracle derives each file's feature set over its version *and* the
/// SDK's experiment whitelist: `dart:` libraries get
/// `AllowedExperiments.forSdkLibrary` grants and package files get
/// `forPackage` grants on top of the version baseline
/// (`feature_set_provider.dart:58-77`); a `// @dart` override re-gates the
/// same grants at the override version (`scanner.dart:127`,
/// `featureSetForOverriding.restrictToVersion`). The grants ride the SDK
/// snapshot fingerprint, so a snapshot change already forks identities —
/// this state only memoizes the per-owner derivation.
///
/// Structure:
///
/// - `any` gates the whole query: when no owner the engine can attribute
///   carries a non-empty grant set (every current corpus), the merge is a
///   literal no-op — one branch, then the plain version baseline. This is
///   what keeps the empty-grant differential flat by construction.
/// - `sdk_library_dirs` is the reverse of `SdkSnapshot::flat_library_to_path`:
///   each `dart:` library's directory (`<root>/lib/<dirname(uri)>`) from the
///   flat existence table maps to its name, so a file attributed by its disk
///   path — the engine keeps no `dart:` URIs — recovers the owning library
///   exactly when the path was produced by a `dart:` resolution.
/// - `memo` is the `(owner, version) -> FeatureSet` cache the per-file
///   lanes share: one derivation per distinct pair per engine, O(1) lookups
///   after first touch. `Mutex`-guarded because the lanes read `&state` on
///   rayon workers (the [`StringTable`] interning-lock precedent); the
///   critical section is a hash lookup.
/// - `skew` is the version-skew degrade log (#1304): the first derivation
///   of an unknown whitelist name queues one record; the apply drain emits
///   them as `ExperimentDegrade` events. The report set dedupes across
///   owners and versions, so one skewed name is one event per engine —
///   the same all-lanes shape the memo's `Mutex` guards.
pub struct FeatureGrants {
    any: bool,
    sdk_lib_root: Box<Path>,
    sdk_library_dirs: FxHashMap<Box<Path>, Box<str>>,
    /// The options-file `enable-experiment` flags per governing directory
    /// (#936): every directory the workspace assembly registered an options
    /// entry for, mapped to its decoded flag list (empty = the entry
    /// governs but enables nothing — a nested options file without the
    /// section suppresses the farther list for its subtree, the official
    /// per-context resolution). Flags decode once here — unknown names
    /// (`unsupported_option` rows) and expired ones (dropped by the
    /// official decode) never reach the set; the per-file release gate
    /// applies at merge time, against the file's own version.
    options_experiments: FxHashMap<Box<Path>, Option<Box<[dartq_language::Feature]>>>,
    /// The CLI `--enable-experiment` flags, decoded once — the context-level
    /// base every package file's feature set starts from
    /// (`FeatureSet.fromEnableFlags2` over the context builder's flags);
    /// an options chain declaring its own section replaces it for that
    /// subtree, the official `_applyEnableExperiments` assignment.
    cli_experiments: Box<[dartq_language::Feature]>,
    /// Whether any options entry or the CLI base carries a non-empty flag
    /// list — the merge fast path for the dominant no-experiments workspace.
    experiments_any: bool,
    /// The SDK version whose whitelist the grants read (the `version` file's
    /// trimmed content) — the [`crate::event::EngineEvent::ExperimentDegrade`]
    /// payload's skew face (#1304).
    sdk_version: String,
    /// The version-skew degrade log (#1304): unknown whitelist names already
    /// reported plus the records the next apply drain turns into events.
    skew: Mutex<SkewLog>,
    memo: Mutex<FxHashMap<(GrantOwner, LanguageVersion), FeatureSet>>,
}

/// One version-skew degrade waiting for the apply drain (#1304): the SDK
/// version whose whitelist named the experiments, and the names no earlier
/// record of this engine has carried. The apply drain reads both fields.
pub(crate) struct PendingSkew {
    pub(crate) sdk_version: String,
    pub(crate) unknown: Vec<String>,
}

/// The version-skew degrade log of a [`FeatureGrants`] (#1304):
/// `reported` dedupes by experiment name across every derivation, and
/// `pending` holds the records the next apply drain emits. `Mutex`-guarded
/// because the grant lanes derive on rayon workers — the grant `memo`
/// precedent.
#[derive(Default)]
struct SkewLog {
    reported: FxHashSet<String>,
    pending: Vec<PendingSkew>,
}

impl FeatureGrants {
    /// Derives the grant faces from the loaded SDK snapshot, the target
    /// segment, the package table, and the resolved options table. Only
    /// owners that can actually win a file's attribution are scanned: the
    /// table's packages and the target segment's libraries (a grant for an
    /// unknown package or another segment can never fire).
    pub(crate) fn new(
        sdk: &SdkSnapshot,
        target: &str,
        packages: &IdentityTable,
        options: &FxHashMap<PathBuf, Arc<crate::analysis_options::AnalysisOptions>>,
        experiments: &[String],
    ) -> Self {
        let lib_root: Box<Path> = Box::from(sdk.dart_sdk_root().join("lib"));
        let mut sdk_library_dirs = FxHashMap::default();
        let mut any = false;
        if let Some(segment) = sdk.target(target) {
            for name in segment.libraries.keys() {
                if !sdk.platform_metadata.for_sdk_library(name).is_empty() {
                    any = true;
                }
            }
        }
        // Library directories come from the flat existence face — the same
        // table `dart:` URI resolution answers from — so a path produced by
        // a resolution attributes to its library whatever segment (or
        // platform variant) its source lives in. The library's directory:
        // `<lib>/<dirname(uri)>` — the join always has a parent (`uri` is
        // non-empty and relative).
        for (name, uri) in dartq_sdk::FLAT_LIBRARIES.iter().copied() {
            let dir = lib_root
                .join(uri)
                .parent()
                .expect("library path under lib/ always has a directory")
                .to_path_buf();
            sdk_library_dirs.insert(Box::from(dir), Box::from(name));
        }
        any |= packages.iter().any(|package| {
            sdk.platform_metadata
                .for_package(package.name.as_str())
                .is_some_and(|set| !set.is_empty())
        });
        // The flags decode is shared by the options entries and the CLI
        // base (`no-` requests never reach a set: a disable only bites
        // version-shipped features, every one of which is expired — the
        // decode drops it — and an off-by-default feature is already off).
        let decode = |names: &[String]| {
            let mut flags = Vec::new();
            for name in names {
                let flag = name.strip_prefix("no-").unwrap_or(name);
                if let Ok(feature) = dartq_language::Feature::try_from(flag)
                    && !feature.is_expired()
                {
                    flags.push(feature);
                }
            }
            flags
        };
        // `None` = no `enable-experiment` section anywhere in that entry's
        // chain: the subtree keeps the CLI context base. `Some` (an empty
        // list included) replaces it — the official per-context
        // `_applyEnableExperiments` assignment.
        let mut options_experiments = FxHashMap::default();
        let mut experiments_any = false;
        for (dir, entry) in options {
            let flags = entry
                .experiments
                .as_ref()
                .map(|names| decode(names).into_boxed_slice());
            if let Some(flags) = &flags {
                experiments_any |= !flags.is_empty();
            }
            options_experiments.insert(Box::from(dir.as_path()), flags);
        }
        let cli_experiments = decode(experiments).into_boxed_slice();
        experiments_any |= !cli_experiments.is_empty();
        Self {
            any,
            sdk_lib_root: lib_root,
            sdk_library_dirs,
            options_experiments,
            cli_experiments,
            experiments_any,
            sdk_version: sdk.dart.version.clone(),
            skew: Mutex::new(SkewLog::default()),
            memo: Mutex::new(FxHashMap::default()),
        }
    }

    /// The feature set `file` analyzes under at `version`: the version
    /// baseline with the owning owner's whitelist grants merged in, plus —
    /// for package files whose owner carries no whitelist entry — the
    /// governing options file's `enable-experiment` flags (#936).
    ///
    /// The options flags ride the *replace* posture, probed on the 3.13.1
    /// oracle (probe corpus: a grants-bearing package keeps its gate row
    /// with the flag requested, a grant-free one lifts): an
    /// `allowed_experiments.json` entry for the package replaces the
    /// options experiments outright, and `dart:` libraries and ownerless
    /// files never read them. The vendored 3.15-main provider
    /// (`feature_set_provider.dart:59-79`) agrees — grants or context
    /// features, never both — so the pinned choice is uncontested. Flags
    /// re-restrict at the file's own version
    /// ([`FeatureSet::with_experiment_for_version`] — probed: the same
    /// flag request that lifts at the SDK version stays gated at 3.5), the
    /// same contract the whitelist grants ride.
    ///
    /// An unattributable file (no package, not under the SDK's `lib/`) and
    /// a no-grant engine both get the plain baseline. An SDK whitelist
    /// naming an experiment the vendored feature registry does not know is
    /// a version skew: the unknown grant cannot be honored, so it degrades
    /// to the known-only set instead of aborting the process — any input
    /// must analyze (ticket #1293; the pre-#1293 panic fired on the first
    /// newer-SDK file). The degrade queues an
    /// [`crate::event::EngineEvent::ExperimentDegrade`] (#1304) that the
    /// apply drain emits once per unknown name. The options merge runs after the
    /// memo on purpose: it is per-directory, so it can never ride the
    /// `(owner, version)` key.
    pub(crate) fn granted(
        &self,
        experiments: &AllowedExperiments,
        packages: &IdentityTable,
        files: &FileTable,
        file: FileId,
        version: LanguageVersion,
    ) -> FeatureSet {
        let set = if self.any {
            match self.owner_for(packages, files, file) {
                Some(owner) => {
                    let mut memo = self.memo.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(set) = memo.get(&(owner.clone(), version)) {
                        return self.with_options_experiments(
                            experiments,
                            packages,
                            files,
                            file,
                            version,
                            *set,
                        );
                    }
                    let set = self.owner_grant(experiments, &owner, version);
                    memo.insert((owner, version), set);
                    set
                }
                None => FeatureSet::for_language_version(version),
            }
        } else {
            FeatureSet::for_language_version(version)
        };
        self.with_options_experiments(experiments, packages, files, file, version, set)
    }

    /// The owner's grant at `version`: the strict whitelist lookup first —
    /// its `UnknownExperiments` payload feeds the skew log (#1304) — then
    /// the lenient re-derivation on skew (#1293's known-only posture; the
    /// two agree on every known name, so the fallback reproduces exactly
    /// the grants the pre-#1304 lenient call produced). The strict pass
    /// costs nothing extra on the clean path: the same merge either way.
    fn owner_grant(
        &self,
        experiments: &AllowedExperiments,
        owner: &GrantOwner,
        version: LanguageVersion,
    ) -> FeatureSet {
        match owner {
            GrantOwner::Package(name) => match experiments.feature_set_for_package(name, version) {
                Ok(set) => set,
                Err(skew) => {
                    self.record_skew(skew.0);
                    experiments.feature_set_for_package_lenient(name, version)
                }
            },
            GrantOwner::SdkLibrary(name) => {
                match experiments.feature_set_for_sdk_library(name, version) {
                    Ok(set) => set,
                    Err(skew) => {
                        self.record_skew(skew.0);
                        experiments.feature_set_for_sdk_library_lenient(name, version)
                    }
                }
            }
        }
    }

    /// Queues the unknown whitelist names a grant skipped (#1304): a name
    /// reports once per engine — the report set dedupes across owners and
    /// versions, so a skew in the SDK-default set every library owner
    /// re-derives still yields exactly one event — and the next non-empty
    /// apply's drain turns the pending records into `ExperimentDegrade`
    /// events. Reached at the grant memo-miss only, so the memo bounds the
    /// call sites per `(owner, version)`.
    fn record_skew(&self, unknown: Vec<String>) {
        let mut skew = self.skew.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh: Vec<String> = unknown
            .into_iter()
            .filter(|name| skew.reported.insert(name.clone()))
            .collect();
        if !fresh.is_empty() {
            skew.pending.push(PendingSkew {
                sdk_version: self.sdk_version.clone(),
                unknown: fresh,
            });
        }
    }

    /// Takes the pending degrade records — the apply drain's one-shot call.
    pub(crate) fn take_skew(&self) -> Vec<PendingSkew> {
        std::mem::take(
            &mut self
                .skew
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pending,
        )
    }

    /// The options-flag merge tail of [`Self::granted`]: the deepest
    /// governing options entry's decoded flags, applied only where the
    /// oracle reads them (a package file with no whitelist entry; the
    /// probe-pinned replace posture). No-op without options experiments —
    /// the dominant workspace shape pays one boolean test.
    fn with_options_experiments(
        &self,
        experiments: &AllowedExperiments,
        packages: &IdentityTable,
        files: &FileTable,
        file: FileId,
        version: LanguageVersion,
        mut set: FeatureSet,
    ) -> FeatureSet {
        if !self.experiments_any {
            return set;
        }
        let Some(path) = files.path(file) else {
            return set;
        };
        // `dart:`-owned files answer from their library's whitelist; a file
        // outside every package keeps the plain baseline — the oracle reads
        // context features only through the package branch.
        if path.starts_with(self.sdk_lib_root.as_ref()) {
            return set;
        }
        let Some(identity) = packages.containing(path) else {
            return set;
        };
        if experiments.for_package(identity.name.as_str()).is_some() {
            return set;
        }
        // A declaring options chain replaces the CLI context base; no
        // section anywhere in the subtree's chain keeps the base. The
        // ancestor walk picks the deepest governing entry — distinct
        // containing keys are nested, so the first hit is the deepest
        // (the same uniqueness [`deepest_containing`] documents).
        let flags = deepest_containing(&self.options_experiments, path)
            .and_then(|entry| entry.as_deref())
            .unwrap_or(self.cli_experiments.as_ref());
        for &feature in flags {
            set = set.with_experiment_for_version(version, feature);
        }
        set
    }

    /// The grant owner of `file`: an SDK library when the path lies under
    /// the SDK's `lib/` (walked up to the nearest library directory — the
    /// exact inverse of `SdkSnapshot::flat_library_to_path`, so parts
    /// attribute to their `dart:` library), else the containing package. The SDK branch
    /// runs first because the oracle attributes `dart:`-URI files before
    /// any package lookup (`feature_set_provider.dart:58-69`); an SDK tree
    /// nested under a package root is still dart:-owned.
    fn owner_for(
        &self,
        packages: &IdentityTable,
        files: &FileTable,
        file: FileId,
    ) -> Option<GrantOwner> {
        let path = files.path(file)?;
        if path.starts_with(self.sdk_lib_root.as_ref()) {
            let mut dir = path.parent();
            while let Some(candidate) = dir {
                if let Some(name) = self.sdk_library_dirs.get(candidate) {
                    return Some(GrantOwner::SdkLibrary(name.clone()));
                }
                dir = candidate.parent();
            }
        }
        packages
            .containing(path)
            .map(|identity| GrantOwner::Package(Box::from(identity.name.as_str())))
    }
}

/// One library of the analysis set: a defining file plus its parts in
/// `part`-directive order (the library content identity folds the member
/// contents in exactly this order).
#[derive(Debug)]
pub(crate) struct LibraryInfo {
    /// Canonical URI the library identity keys on. Shares the
    /// canonical-URI memo's allocation ([`EngineState::canonical_uris`]):
    /// formation clones an `Arc` refcount per formed library, never
    /// re-copies the bytes.
    pub uri: Arc<str>,
    /// The declared `library <name>;` name, `""` when the defining file has
    /// none (the oracle's empty default, `library_builder.dart:750-763`):
    /// fed to the resolution graph at build time, where the semantic layer
    /// reads it as the package:meta recognition key (upstream
    /// `element.dart:2100-2117`). One boxed copy per library.
    pub name: Box<str>,
    /// The defining (non-`part of`) file.
    pub defining: FileId,
    /// Included part files, part-directive order.
    pub parts: Vec<FileId>,
    /// Content identity: the defining file's `ContentId` followed by the
    /// parts' ids in part-directive order (the ordered member set — stored
    /// directly, so identity comparison is exact, not hashed).
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

/// The per-library cached result: the
/// merged diagnostic set. Invalidation is explicit (the apply phase's dirty
/// marking, derived from the graph delta); the L3 fingerprint gate (g485 W2)
/// then refines the recompute set by comparing this entry's key, so the
/// entry also carries the cycle fingerprint its run was computed under.
#[derive(Debug)]
pub(crate) struct LibraryResult {
    /// The library's diagnostics in canonical order (path bytes, offset,
    /// code name, message, length, correction — `pipeline::
    /// compare_canonical`): sorted once per recompute at store time, so the
    /// analyze-time merge interleaves the runs without re-sorting the cache.
    /// Rows are refcount handles into the stores that own the bytes (the
    /// residency facts' syntax rows, the semdiag memo's fold rows — the S4
    /// dedup, #1501): the store costs a bump per row. The text bytes inside
    /// each row are refcounted too (`SharedStr` message, `Arc<str>`
    /// correction — #1510), so the merged output's flat copy bumps handles
    /// as well: no message or correction bytes exist twice anywhere.
    pub diagnostics: Vec<Arc<Diagnostic>>,
    /// The condensation fingerprint of the library's cycle at store time —
    /// the reuse key the post-closure gate compares.
    /// The digest folds the member signatures and the dependency-cycle
    /// fingerprint set, so `recorded == current` means every member face and
    /// every transitive dependency face is byte-identical to this run's
    /// inputs. The apply drops the entry on every identity move, so a
    /// present result is identity-fresh by construction: the pair of halves
    /// is exactly the reuse condition, and any doubt (a `None` mapping, a
    /// moved digest) recomputes — monotone versus the L2 behavior.
    pub cycle_fingerprint: ApiSignature,
}

/// The standing merged diagnostic output of the last successful analyze
/// (audit g1034 F1): every live library's run already interleaved into one
/// canonically-sorted vector, kept so a cycle in which no contributor
/// re-analyzed can serve it wholesale instead of re-walking the k-way merge.
#[derive(Debug)]
pub(crate) struct MergedOutput {
    /// The merged rows, canonically sorted — byte-identical to a full
    /// [`crate::pipeline`] merge of the contributor runs at store time. The
    /// rows are owned struct copies, but their text is shared: a rebuild's
    /// flat copy bumps the `SharedStr`/`Arc<str>` refcounts instead of
    /// re-owning message or correction bytes (#1510).
    pub output: Arc<Vec<Diagnostic>>,
    /// `(library, generation)` of every contributing library at store time,
    /// ascending by id — the cache key. The reuse condition is exact list
    /// equality against the current contributors (`pipeline::
    /// merge_contributors`): an id appearing, vanishing (retirement, dropped
    /// record), or advancing its generation all mismatch, so any stale shape
    /// recomputes. Generations are monotone per id and never swept, so a
    /// recycled library id inherits its predecessor's count and a cache from
    /// the previous incarnation can never match.
    pub contributors: Vec<(LibraryId, u32)>,
}

/// Cache key of the per-file facts cache: the `AstKey{content, feature_set,
/// parser_version}` triple of
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FactsKey {
    pub content: ContentId,
    pub feature_fingerprint: u64,
    /// The [`FACTS_SEMANTIC_VERSION`] component — rides the hash, so any
    /// engine facts-shape or semantic signature bump invalidates the entry.
    pub semantic_version: u32,
}

/// A pending overlay entry: last write per path wins.
#[derive(Debug)]
pub(crate) enum PendingChange {
    /// Content override as a [`SourceText`] lease: canonical text with its
    /// [`ContentId`] baked in (the pipeline downstream is `&str`; invalid
    /// sequences became U+FFFD at canonicalization, documented at
    /// `Engine::set_content`). The lease is shared by refcount between the
    /// overlay, the apply batch, and the content store — no text copy on
    /// any seam.
    Set(SourceText),
    /// Removal request.
    Remove,
}

/// A small stable integer naming one library of the analysis set. Opaque,
/// engine-internal, process-lifetime only; the
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
        Self(saturate_slot_u32(existing))
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
    /// re-enter only through a later [`Self::retire`]), so the list
    /// is bounded by the historical live peak. A recycled id is handed to
    /// whatever interns next and is indistinguishable from its previous
    /// incarnation — reuse safety is the caller's ordering contract (see
    /// [`Self::retire`]).
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
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn arena_len(&self) -> u32 {
        self.infos.len() as u32
    }

    /// Live libraries with their infos, ascending by id.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
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
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn live(&self) -> impl Iterator<Item = LibraryId> + '_ {
        self.infos
            .iter()
            .enumerate()
            .filter_map(|(index, info)| info.is_some().then_some(LibraryId(index as u32)))
    }

    /// Retires one live id — the incremental twin of a whole-set sweep: the
    /// graph delta knows exactly which keys lost their tenant, so
    /// retirement is O(1) per key, never O(arena). The id's uri is
    /// unregistered and the id returns to the reuse freelist. A live id
    /// always carries info (the formation writes it), so the unregister
    /// reads the uri from there.
    ///
    /// Ordering contract (the freelist-recycling hazard, #374): a recycled
    /// id aliases its previous incarnation — stale snapshots holding it
    /// compare equal against the new tenant. Recycling is safe only when
    /// nothing stale is left to alias: the caller must complete every intern
    /// of the current batch *before* retiring (an id retired early could be
    /// re-minted to a different URI within the same batch) and must drop the
    /// retired id's cached result and reference surface alongside. The apply
    /// phase honors this: retirement runs after the delta's interns, and
    /// results/surfaces go in the same step.
    pub(crate) fn retire(&mut self, id: LibraryId) {
        let index = id.index() as usize;
        if let Some(info) = self.infos[index].take() {
            self.ids_by_uri.remove(info.uri.as_ref());
            self.free.push(id);
        }
    }
}

/// The reachability memo's state (#1383). The analyze's gate is the single
/// invalidation site — the same condition the eager build fired on (changed
/// or removed residency, or any recomputed library); no existing counter
/// folds exactly that face (the library generations are per-library, the
/// graph generation is structure-only), so the memo carries its own flags.
#[derive(Default)]
pub(crate) struct ReachabilityMemo {
    /// An analyze's gate ran at least once — the map may exist. Before the
    /// first gate the read answers `None`, exactly the pre-laziness shape.
    pub(crate) armed: bool,
    /// An apply landed since the last gate (marked in `l1::apply`):
    /// residency moved without the drain/recompute pass, so the state is
    /// not coherent enough to build over — reads serve the standing map
    /// (what the eager build left behind) until the next gate clears this.
    pub(crate) applied: bool,
    /// The gate saw the pass's inputs move: the standing map is stale and
    /// the next coherent read rebuilds it.
    pub(crate) stale: bool,
    /// The standing map, `None` until the first build.
    pub(crate) map: Option<Arc<crate::reachability::Reachability>>,
}

/// The semdiag member-fold memo's shared state (#1384): one entry per
/// member file, keyed by [`FileId`], served only when the entry's identity
/// inputs (see [`crate::semdiag::MemberFold`]) stand against the current
/// fold. `Mutex`-guarded because the analyze fan-out's workers look up and
/// fill concurrently — the grants-memo precedent; the guard covers hash
/// operations only, the fold itself runs outside it. Not laziness-flagged
/// like [`ReachabilityMemo`]: the memo is consumed exactly where the fold
/// runs (inside the analyze, past the apply), so a stale entry simply
/// misses its key — there is no read that can land over in-flight state.
#[derive(Default)]
pub(crate) struct SemdiagMemo {
    pub(crate) folds: FxHashMap<FileId, Arc<crate::semdiag::MemberFold>>,
    /// Memo serves this session (test instrumentation only): the pin
    /// asserts the salvage actually fires on the standing-member steps.
    #[cfg(test)]
    pub(crate) serves: std::sync::atomic::AtomicUsize,
}

/// The residency-pairing degrade log (stability lane #1459): the pairing
/// misses the engine's derived passes salvage over — the reachability
/// build's live members whose capture, residency face, store content, or
/// UTF-8 decode did not hold, the semdiag fold's members and posture
/// derivations over the same pairing. A miss skips the unit (never a
/// crash) and records one row per (file, face) pair. `Mutex`-guarded
/// because the passes run beside rayon workers on `&EngineState` — the
/// grants-skew precedent ([`FeatureGrants::skew`]); the parallel closures
/// themselves never take the guard, their misses ride the job results back
/// to a sequential tail. Bounded: the report set dedupes by pair, so the
/// pending rows cap at the distinct pairs the engine ever saw and each
/// miss reports once per engine.
#[derive(Default)]
pub(crate) struct ResidencyDegrades {
    log: Mutex<ResidencyDegradeLog>,
}

#[derive(Default)]
struct ResidencyDegradeLog {
    reported: FxHashSet<(FileId, &'static str)>,
    pending: Vec<(FileId, &'static str)>,
}

impl ResidencyDegrades {
    /// Records the passes' pairing misses: a fresh (file, face) pair queues
    /// one row, an already-reported pair no-ops (the dedup bounds the log).
    pub(crate) fn record(&self, misses: &[(FileId, &'static str)]) {
        if misses.is_empty() {
            return;
        }
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        for &(file, face) in misses {
            if log.reported.insert((file, face)) {
                log.pending.push((file, face));
            }
        }
    }

    /// Takes the pending rows — the apply drain's one-shot call.
    pub(crate) fn take(&self) -> Vec<(FileId, &'static str)> {
        std::mem::take(
            &mut self
                .log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pending,
        )
    }

    /// Records the session live-path ceiling trip (#1887): one row naming
    /// the ceiling, anchored at `anchor` — the refused batch's
    /// representative path's id. At-most-once emission is the caller's
    /// discipline (the daemon trips the row once per session); the log's
    /// (file, face) dedup holds regardless.
    pub(crate) fn record_ceiling(&self, anchor: FileId) {
        self.record(&[(anchor, LIVE_PATH_CEILING_FACE)]);
    }
}

/// One reverse-import edge of a target file: the importing library and the
/// edge's multiplicity (two members of one library importing the same target
/// count twice — one member re-targeting away keeps the edge exact). Row
/// elements sort by `library`, so reads are contiguous and deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImportEdge {
    pub library: LibraryId,
    pub count: u32,
}

/// Dense `FileId`-indexed compact adjacency rows — the row-level CSR
/// projection of the per-target indexes (#1779): `slots[file.index()]`
/// holds one sorted, exactly-sized boxed row, so a lookup is one index plus
/// a contiguous slice and a live target costs exactly one allocation.
/// Replaces per-target nested hash containers whose shells and load-factor
/// slack outweighed their 1–5 entries several-fold at 10⁵-file residency.
///
/// Slots grow lazily by `resize_with` on first write (the
/// `captures`/`decl_order`/`use_views` dense-arena idiom), so absent
/// targets cost 16 B of slot space per interned file and interned-but-
/// never-targeted files never allocate. Rows are maintained sorted by
/// their key — `FileId` for the source sets, `LibraryId` for edge
/// multiplicities — which pins a deterministic iteration order strictly
/// stronger than the hash order the maps served.
///
/// Mutation rewrites only the touched row: the row vectors are 1–5 entries,
/// so a boxed-slice rebuild is a bounded small copy, and no shared `values`
/// tail ever shifts under an incremental apply — the write pattern a flat
/// global CSR would force O(total edges) per keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetRows<T> {
    slots: Vec<Option<Box<[T]>>>,
}

impl<T> Default for TargetRows<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<T> TargetRows<T> {
    /// The row `file` owns, empty when absent — the read face every
    /// consumer iterates.
    pub(crate) fn row(&self, file: FileId) -> &[T] {
        self.slots
            .get(file.index() as usize)
            .and_then(Option::as_deref)
            .unwrap_or(&[])
    }

    /// Whether `file` owns a non-empty row — the presence face tests probe.
    #[cfg(test)]
    pub(crate) fn has(&self, file: FileId) -> bool {
        self.slots
            .get(file.index() as usize)
            .is_some_and(Option::is_some)
    }

    /// Live row count (targets with a non-empty row).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Rewrites `file`'s row through `f`: the current row materializes into
    /// a scratch `Vec`, `f` mutates it in place, and the result is boxed
    /// back — an emptied row drops the slot to `None`, so the index stays
    /// bounded by the live directive set. Bounded by the row's own size;
    /// no other row moves.
    pub(crate) fn edit_row(&mut self, file: FileId, f: impl FnOnce(&mut Vec<T>)) {
        let index = file.index() as usize;
        let mut row: Vec<T> = match self.slots.get_mut(index).and_then(Option::take) {
            Some(row) => {
                // One slot of headroom: an insert lands without a regrow, and
                // the boxed rebuild below is the row's only reallocation.
                let mut row = row.into_vec();
                row.reserve_exact(1);
                row
            }
            None => Vec::with_capacity(1),
        };
        f(&mut row);
        if index >= self.slots.len() {
            // Grow the arena geometrically (a fresh walk interns ids in
            // ascending order: an exact resize per edit is quadratic copying).
            let wanted = (index + 1).max(self.slots.len() * 2);
            self.slots.resize_with(wanted, || None);
        }
        self.slots[index] = if row.is_empty() {
            None
        } else {
            Some(row.into_boxed_slice())
        };
    }

    /// Drops `file`'s row (the target left residency). Bounds-tolerant: a
    /// target removed before any edge landed never grew the arena.
    pub(crate) fn remove_row(&mut self, file: FileId) {
        if let Some(slot) = self.slots.get_mut(file.index() as usize) {
            *slot = None;
        }
    }

    /// The exact byte footprint: the slot arena at reserved capacity plus
    /// each live row's boxed payload (boxed slices are exact-capacity) —
    /// the counted metric the S6 decomposition reports.
    pub(crate) fn memory_bytes(&self) -> usize {
        self.slots.capacity() * size_of::<Option<Box<[T]>>>()
            + self
                .slots
                .iter()
                .flatten()
                .map(|row| row.len() * size_of::<T>())
                .sum::<usize>()
    }
}

/// Everything the engine keeps between API calls. `&mut self`
/// exclusivity on the `Engine` makes the enqueue→apply→analyze order total,
/// so neither the overlay nor the store need interior mutability: the store
/// is owned outright (reads take `&self`, `put` runs inside apply), and the
/// only locks in the crate are the [`StringTable`]'s interning lock, the
/// [`FeatureGrants`] memo and skew log, the reachability memo, the semdiag
/// memo, and the residency-degrade log (worker threads share them during
/// the parallel recompute).
pub(crate) struct EngineState {
    /// Package identity table over one resolution.
    pub packages: IdentityTable,
    /// URI-resolution face built from the same pub config (the
    /// dartq-resolution `PackageTable`).
    pub uri_packages: UriPackageTable,
    /// SDK snapshot; `dart:` targets resolve against [`Self::target`].
    pub sdk: SdkSnapshot,
    /// Memoized per-file experiment grants (audit #708 P2-1): the
    /// allowed-experiments authorization merged into every file's derived
    /// feature set. See [`FeatureGrants`].
    pub feature_grants: FeatureGrants,
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
    /// resolution graph. Ids are immortal; the table is bounded by its
    /// documented session ceiling, never swept (see [`StringTable`] and
    /// [`Self::sweep_store`]).
    pub strings: StringTable,

    /// Live files: content id + current facts. Removed files leave the map
    /// entirely (their `FileId`s stay interned in [`Self::files`]).
    pub residency: FxHashMap<FileId, Arc<UnitFacts>>,
    /// The lazy dependency-interface cache (the `deps` module): pub-cache
    /// files a live library's `package:` import resolved to but discovery
    /// never made resident, loaded on demand at the graph build so the
    /// workspace's own rows resolve their faces. Bounded by
    /// [`crate::deps::DEP_FILE_CAP`]; never an engine library, so its
    /// units report nothing. Session-stable by construction (the pub cache
    /// does not rewrite under a running analysis), never persisted.
    pub deps: crate::deps::DepIndex,
    /// Monotone residency-growth counter, the generation tag the
    /// nested-pubspec scan's turn gate compares against (the
    /// rust-analyzer incremental-invalidation posture: revision-tagged
    /// inputs, O(1) up-to-date checks). Moved only by
    /// [`Self::admit_residency`] when a key is newly added — content
    /// edits over already-resident files and removals leave it alone,
    /// because neither can surface a directory the scan has not seen.
    pub residency_epoch: u64,
    /// Canonical URI memo per interned file (a file's path never changes
    /// once interned, so neither does its canonical URI). Filled lazily —
    /// only for files the apply phase actually touches (changed files plus
    /// files whose URI a formation key needs) — so maintenance is
    /// O(changed), never O(residency). The `Arc`
    /// allocation is shared, not copied: [`LibraryInfo::uri`] reuses it, so
    /// reformation pays a refcount per formed library, never a heap clone.
    pub canonical_uris: FxHashMap<FileId, Arc<str>>,
    /// Library arena with per-library membership and dependency data.
    pub libraries: LibraryTable,
    /// File → owning library (defining or part), maintained by the apply
    /// phase's graph delta (only entries whose ownership moved are written).
    pub library_of_file: FxHashMap<FileId, LibraryId>,
    /// Reverse import edges: resident target file → importing
    /// library → edge multiplicity (two members of one library importing the
    /// same target count twice, so one of them re-targeting away keeps the
    /// edge exact). Maintained by the apply phase's graph delta: every
    /// add is paired with a remove from the file's previous directive face.
    /// Row-level CSR layout — see [`TargetRows`].
    pub importers: TargetRows<ImportEdge>,
    /// Raw part-directive offers (residency-independent): target file →
    /// defining files whose current facts contain a `part` directive
    /// resolving to it. The claim-resolution candidate set: resolution picks
    /// the path-smallest candidate whose `may_claim` holds against current
    /// facts, so precedence stays deterministic-total while the delta keeps
    /// lookups O(offers) per affected part file. Maintained by paired
    /// subtract/add on every facts change; bounded by the live directive
    /// set. Row-level CSR layout — see [`TargetRows`].
    pub part_offers: TargetRows<FileId>,
    /// Raw import/export sources (residency-independent): target file →
    /// files whose current facts contain an import/export directive
    /// resolving to it. This is the residency-flip fan-out: when a file
    /// enters residency, its importers are exactly the owners of these
    /// sources; the reverse query (`importers`) cannot answer it because it
    /// only holds resident targets. Maintained like [`Self::part_offers`].
    pub import_sources: TargetRows<FileId>,
    /// Raw `part of '<uri>'` sources (residency-independent): target file →
    /// part files whose current facts carry a URI-form `part of` directive
    /// resolving to it. The relink pass's holder index for the claim side
    /// the two indexes above do not cover: a created file can satisfy a
    /// previously missing `part of` URI, and a removed one must un-resolve
    /// it. Name-form part-ofs carry no URI and never land here. Maintained
    /// like [`Self::part_offers`]. Row-level CSR layout — see
    /// [`TargetRows`].
    pub part_of_sources: TargetRows<FileId>,
    /// Per-library result cache.
    pub results: FxHashMap<LibraryId, LibraryResult>,
    /// Change generation per library: bumped each time a fresh run is stored
    /// (the recompute loop in `pipeline::analyze`). The merged-output cache's
    /// key half (`Self::merged`). Monotone per id and never swept — the arena
    /// freelist recycles library ids, and a recycled id must not match a
    /// cache recorded under its previous incarnation.
    pub library_generations: FxHashMap<LibraryId, u32>,
    /// The standing merged output of the last successful analyze
    /// ([`MergedOutput`]). Rebuilt whenever any contributor's generation
    /// advances or the contributor set moves; served wholesale otherwise.
    /// Never persisted, never shed (the budget ladder never touches results,
    /// so the cache's inputs stay live at the bottom tier).
    pub merged: Option<MergedOutput>,
    /// Post-closure dirty set of the last apply; consumed by analyze.
    /// Invariant: every entry is a live library id — apply assigns `close`'s
    /// output outright; `close` seeds the incoming leftovers (filtered to the
    /// post-delta live set) into its dirty set, so a cancelled analyze's
    /// still-invalid libraries carry forward. Unioning the incoming set back
    /// directly could instead resurrect ids retired by an intervening remove
    /// (#230), and analyze unwraps every queued id's info.
    pub invalidated: Vec<LibraryId>,
    /// Whether any signature-bearing face moved since the last analyze (the
    /// analyze phase's rebuild gate): the flag drives the condensation
    /// re-derivation and the structure-digest refresh; the resolution graph
    /// itself rebuilds only when the refreshed digests report a structural
    /// move (#1040).
    pub structure_dirty: bool,
    /// The library-dependency condensation: Tarjan SCC
    /// over the maintained deps edges with a per-cycle blake3 fingerprint.
    /// Derived data, rebuilt wholesale at the [`Self::structure_dirty`]
    /// gate (`pipeline::analyze`) — never patched incrementally, never
    /// persisted, and retired ids answer no cycle between rebuilds.
    pub condensation: crate::cycle::Condensation,
    /// The lazily-built reachability memo (#1383, [`ReachabilityMemo`]):
    /// the analyze's gate moves the memo's flags and the first
    /// `Engine::reachability` read pays the build. `Mutex`-guarded because
    /// the lint lanes read `&Engine` from rayon jobs while the first of
    /// them builds; the guard is the memo's, never the state's, so the
    /// build reads the state lock-free beside it. Derived-only, never
    /// persisted.
    pub(crate) reachability: Mutex<ReachabilityMemo>,
    /// The residency-pairing degrade log (stability lane #1459): the
    /// derived passes' skipped units — live members whose capture,
    /// residency, store, or UTF-8 pairing missed — one deduped row per
    /// (file, face) pair per engine, drained into the trace face by the
    /// apply. See [`ResidencyDegrades`].
    pub(crate) residency_degrades: ResidencyDegrades,
    /// The semdiag member-fold memo (#1384, [`SemdiagMemo`]): the default
    /// fold serves a member's `dartq_semantic::diagnose_unit` product from
    /// here when its identity inputs stood, so a recompute of one member's
    /// library re-folds only the members whose faces moved. Entries are
    /// bounded by residency (one per member file, replaced on identity
    /// move, dropped with the file at [`Self::clear_semdiag_fold`]);
    /// derived-only, never persisted.
    pub(crate) semdiag: Mutex<SemdiagMemo>,
    /// The semdiag posture epoch (#1384): bumped by the apply whenever a
    /// changed file's settled feature face moved against its previous one
    /// (an `// @dart` header edit — the one fold input no per-library cycle
    /// fingerprint folds, since the header is a comment and moves no
    /// signature), or when a previous face is no longer derivable (evicted
    /// scanned face, warm-started facts). Rides every memo key, so a bump
    /// forks all entries conservatively; pure comment/body/signature churn
    /// leaves it standing and salvage intact.
    pub(crate) semdiag_posture_epoch: u32,
    /// The per-file structure fingerprints (#1040): the digests the analyze
    /// gate ([`crate::pipeline::refresh_structure_digests`]) recomputes from
    /// the resident facts and compares to decide whether the O(workspace)
    /// resolution-graph rebuild must run. One entry per resident file —
    /// the map mirrors [`Self::residency`], entries swept when their file is
    /// removed — so the footprint is bounded by residency at 8 bytes per
    /// file. Memory-resident only: a digest is a fold of the live facts,
    /// recomputed at every gate, never persisted.
    pub structure_digests: FxHashMap<FileId, u64>,
    /// Apply batches that landed outside an analyze ([`Engine::apply_pending`],
    /// the daemon's write face): the files the next analyze's structure gate
    /// must fold into [`Self::structure_digests`]. An analyze's own apply
    /// feeds the gate its applied sets directly and never touches these —
    /// the accumulator exists for the out-of-analyze apply alone, whose
    /// moves are otherwise invisible to the gate (its local applied sets
    /// are empty), leaving a formed library outside the standing graph's
    /// `graph_ids`. Bounded by the apply batches since the last analyze;
    /// every daemon write co-enqueues the read whose analyze drains them.
    pub(crate) structure_changed: Vec<FileId>,
    pub(crate) structure_removed: Vec<FileId>,
    /// The resolution-graph generation: bumped every time
    /// [`crate::pipeline::build_library_graph`] replaces the standing
    /// graph. The structure gate's probe — a non-structural signature edit
    /// must leave this standing while a structural edit bumps it.
    pub graph_generation: u32,

    /// Pending overlay, insertion-ordered.
    pub pending: FxHashMap<FileId, PendingChange>,
    pub pending_order: Vec<FileId>,

    /// Resolved analysis options per governing directory: the
    /// include-merged file of every package root (or the synthesized
    /// default) plus every nested options directory inside the roots,
    /// merged once at context assembly; discovery applies the exclude
    /// globs, the lint face reads the rule names.
    pub options: FxHashMap<PathBuf, Arc<crate::analysis_options::AnalysisOptions>>,

    /// Warm-start staging (#482): a loaded facts snapshot's payloads, keyed
    /// by the entry triple. `Engine::ingest_facts_snapshot` fills it; an
    /// apply consumes exact identity matches — the lookup IS the
    /// content-hash verification, a hit serving only a file whose live
    /// content id equals the key's hash — and claims (removes) the entries
    /// it served (#1387). Entries no batch claims stay staged: a stale entry
    /// can never serve a moved content, and a later batch whose content
    /// reverts to the staged identity is the hit the retention keeps. The
    /// eviction faces are the claim, the budget's tier-3 shed, and the next
    /// ingest. Ingestion is grow-only: it seeds recomputes, it never removes
    /// residency (#377).
    pub warm_start: FxHashMap<dartq_snapshot::EntryKey, persist::PersistedFacts>,

    /// Memoized pure per-file facts of previous contents — pre-resolution,
    /// so a cross-batch hit re-runs the per-file attribution and URI
    /// resolution at the hit site (l1 `derive_facts`; the [`FactsKey`] triple
    /// is path-free by contract). Bounded, `FACTS_CACHE_CAP`.
    facts_cache: FxHashMap<FactsKey, Arc<UnitFacts>>,
    facts_order: VecDeque<FactsKey>,
    /// Memoized scan faces keyed by the same [`FactsKey`] triple as the
    /// facts cache: the token table + settled feature set the semdiag fold
    /// and the index captures parse through. Filled at the apply recompute
    /// (the one place a scan runs) and drained consumers fall back to a
    /// local scan on a miss. Bounded, `SCANNED_CACHE_CAP`.
    scanned: FxHashMap<FactsKey, Arc<ScannedFace>>,
    scanned_order: VecDeque<FactsKey>,
    /// The maintained symbol-index inputs (the Engine owns dirty
    /// scheduling, the index never invalidates itself):
    /// one capture per resident file (declaration surface + raw sites —
    /// the sites capped by [`SITES_CACHE_CAP`] and shed oldest-first, the
    /// declaration surface always resident, the sites re-derived on a
    /// rebuild miss), and
    /// one reference surface per live library. The captures replace whole
    /// when a file's content moves and drop when it leaves residency; the
    /// surfaces rebuild at the `LibraryAnalyzed` point of each analyze
    /// (pipeline) and drop with their library at retirement. Both are
    /// dense-id arenas ([`FileTable::paths`] / [`LibraryTable`] idiom):
    /// `Vec` slots indexed by the id, `None` = absent, removals never
    /// shrink — so ascending slot order is the deterministic iteration
    /// order the query face serves. Bounded by the interned-file and
    /// historical-live-library peaks respectively.
    captures: Vec<Option<Arc<FileCapture>>>,
    /// The mounted raw-site faces over those captures, capped oldest-first
    /// (`SITES_CACHE_CAP`, `#1859`): `sites_order` is the feed order (the
    /// eviction queue), `sites_cached` the membership set the refresh and
    /// removal paths keep in lockstep with it. Every `set_capture` feeds —
    /// a recapture's sites are the freshest — and the feed sheds the
    /// oldest file's site columns via its capture's `without_sites` twin;
    /// a rebuild miss re-derives the sites locally instead of re-feeding
    /// (the scanned-face fallback: re-derivation never refills the cache).
    /// The set and the queue hold exactly the files whose standing capture
    /// carries sites, so the queue length is the occupancy the memory
    /// face reports.
    sites_cached: FxHashSet<FileId>,
    sites_order: VecDeque<FileId>,
    library_surfaces: Vec<Option<LibraryIndex>>,
    /// Offset-keyed views over those arenas (the position-resolution face's
    /// substrate, `query::resolve_at`): `decl_order` re-indexes a capture's
    /// declaration rows by span start (the rows themselves sort by
    /// `(name, offset)` — no span-ordered probe exists without it), and
    /// `use_views[library]` regroups the surface's reference rows by use
    /// file, each group's row indices offset-ascending (the surface itself
    /// sorts by target). Both are pure re-indexings — 4 B per row, the rows
    /// stay in the arenas — and both swap at the mutators below in
    /// lockstep with the structure they index, so they cannot drift.
    decl_order: Vec<Option<Box<[u32]>>>,
    use_views: Vec<Option<UseView>>,
    /// The postings directories over those arenas (the `postings` module of
    /// dartq-index): name → declaring files and name → libraries with
    /// resolved uses, so `query symbol` is a hash probe plus the answer's
    /// own rows instead of a workspace scan. Maintained inside the capture
    /// and surface mutators below under the same whole-unit law the crate's
    /// `SymbolIndex` applies — every swap unlists the old surface's names
    /// and lists the new one's — so the directories cannot drift from the
    /// arenas they index. 4 B per (name, unit) pair plus one hash slot per
    /// distinct posted name; the rows stay in the arenas.
    pub(crate) decl_postings: Postings<FileId>,
    pub(crate) ref_postings: Postings<LibraryId>,
    /// Reported non-Dart files (the pubspec lint face): the workspace's
    /// pubspecs, interned with their captured text and line index so their
    /// rows resolve positions through [`Engine::content`]/[`Engine::line_index`]
    /// fallbacks without Dart residency — the options-file shape
    /// ([`crate::analysis_options::AnalysisOptions`] layout). Refreshed
    /// every analyze pass, so the captured text is the analyzed one.
    pub(crate) reported: FxHashMap<FileId, (Arc<str>, LineIndex)>,
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
    /// The memory-budget runtime, `None` when no limit is set —
    /// the zero-cost shape (the poll short-circuits at one `Option` test,
    /// the sampler never runs). Armed through [`crate::Engine::set_memory_limit`].
    pub(crate) budget: Option<crate::budget::MemoryBudget>,
    /// The `--trace-events` JSONL sink, `None` when unset —
    /// the zero-cost shape (no sink object, no per-event work). Armed
    /// through [`crate::Engine::set_trace_sink`].
    pub(crate) trace: Option<crate::event::TraceSink>,
    /// The L3 content-addressed store, `None` when not armed —
    /// the zero-cost shape (the write-batch flush short-circuits at one
    /// `Option` test). Armed through
    /// [`crate::Engine::open_content_store`]; every apply batch persists
    /// its changed contents and flushes (pipeline's `flush_l3`).
    pub(crate) l3: Option<crate::store::ContentStore>,
}

/// The context roots the analysis surfaces derive from: every package
/// root — or, when the identity table carries no package at all (the bare
/// no-pubspec floor, `PackageTable::basic` over `None`), the table's
/// workspace root, the analyzed directory itself. The oracle's
/// `BasicWorkspace` over `Packages.empty` roots its context at the
/// analyzed directory and walks that whole folder (context_locator.dart
/// :517-522), so the bare floor's discovery and options faces key off the
/// same directory a package root names in the configured face.
pub(crate) fn context_roots(packages: &IdentityTable) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = packages
        .iter()
        .map(|package| package.root.clone())
        .collect();
    if roots.is_empty() {
        roots.push(packages.workspace_root().to_path_buf());
    }
    roots.sort();
    roots.dedup();
    roots
}

impl EngineState {
    /// Assembles the state from the immutable analysis context.
    ///
    /// # Errors
    ///
    /// [`crate::EngineError::PackageConfig`] when the pub package config
    /// cannot back the URI-resolution table (the shared `resolve` face
    /// rejected it).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        packages: IdentityTable,
        pub_config: &PackageConfig,
        config_dir: &Path,
        sdk: SdkSnapshot,
        store: Box<dyn SourceStore + Send + Sync>,
        target: String,
        declared_variables: FxHashMap<String, String>,
        experiments: &[String],
    ) -> Result<Self, crate::EngineError> {
        let uri_packages = UriPackageTable::new(pub_config, config_dir)?;
        // Options are a context field: the include chains merge once here,
        // keyed by governing directory — package roots plus the nested
        // options directories inside them (cold path, workspace-assembly
        // time). Built before the grant faces, which read the options
        // experiments (#936).
        let options = crate::analysis_options::AnalysisOptions::for_roots(
            &context_roots(&packages),
            pub_config,
            config_dir,
        );
        // The grant faces read the tables before they move into the state.
        let feature_grants = FeatureGrants::new(&sdk, &target, &packages, &options, experiments);
        Ok(Self {
            packages,
            uri_packages,
            sdk,
            feature_grants,
            target,
            declared_variables,
            store,
            files: FileTable::new(),
            strings: StringTable::new(),
            residency: FxHashMap::default(),
            deps: crate::deps::DepIndex::default(),
            residency_epoch: 0,
            canonical_uris: FxHashMap::default(),
            libraries: LibraryTable::default(),
            library_of_file: FxHashMap::default(),
            importers: TargetRows::default(),
            part_offers: TargetRows::default(),
            import_sources: TargetRows::default(),
            part_of_sources: TargetRows::default(),
            results: FxHashMap::default(),
            library_generations: FxHashMap::default(),
            merged: None,
            invalidated: Vec::new(),
            structure_dirty: false,
            condensation: crate::cycle::Condensation::default(),
            reachability: Mutex::new(ReachabilityMemo::default()),
            residency_degrades: ResidencyDegrades::default(),
            semdiag: Mutex::new(SemdiagMemo::default()),
            semdiag_posture_epoch: 0,
            structure_digests: FxHashMap::default(),
            structure_changed: Vec::new(),
            structure_removed: Vec::new(),
            graph_generation: 0,
            pending: FxHashMap::default(),
            pending_order: Vec::new(),
            options,
            warm_start: FxHashMap::default(),
            facts_cache: FxHashMap::default(),
            facts_order: VecDeque::new(),
            scanned: FxHashMap::default(),
            scanned_order: VecDeque::new(),
            captures: Vec::new(),
            sites_cached: FxHashSet::default(),
            sites_order: VecDeque::new(),
            library_surfaces: Vec::new(),
            decl_order: Vec::new(),
            use_views: Vec::new(),
            decl_postings: Postings::default(),
            ref_postings: Postings::default(),
            reported: FxHashMap::default(),
            capture_dirty: Vec::new(),
            gc_debt: 0,
            budget: None,
            trace: None,
            l3: None,
        })
    }

    /// Admits a file into residency — the single mutation entry point for
    /// [`Self::residency`], so [`Self::residency_epoch`] moves exactly
    /// when a key is newly added. An overwrite (the `Some` previous of a
    /// content edit over an already-resident file) leaves the tag alone:
    /// that file's directory is already in the nested scan's memo.
    pub(crate) fn admit_residency(&mut self, file: FileId, facts: Arc<UnitFacts>) {
        if self.residency.insert(file, facts).is_none() {
            self.residency_epoch += 1;
        }
    }

    /// The borrowed per-file grant faces (`l1::Grants`): everything the
    /// feature-set merge consults, in one disjoint-field borrow. Every
    /// per-file lane builds this once per item.
    pub(crate) fn grants(&self) -> crate::l1::Grants<'_> {
        crate::l1::Grants {
            tables: &self.feature_grants,
            experiments: &self.sdk.platform_metadata,
            packages: &self.packages,
            files: &self.files,
        }
    }

    /// Fact-cache lookup.
    pub(crate) fn cached_facts(&self, key: FactsKey) -> Option<Arc<UnitFacts>> {
        self.facts_cache.get(&key).cloned()
    }

    /// The language-strictness modes governing `file`: the deepest options
    /// entry whose directory contains the file's path, its strict flags
    /// each `unwrap_or(false)` (the official off posture — the
    /// `AnalysisOptionsImpl` defaults). Resolved once per library at the
    /// diagnoser seam (the defining file's modes govern the fold).
    ///
    /// The governing entry is unique, not a tie-break winner: the options
    /// keys containing a path are that path's ancestors, ancestors are
    /// pairwise nested, and the map holds distinct keys — so exactly one
    /// entry has the greatest depth and an equal-specificity match cannot
    /// exist. (The former linear scan's `>=` on equal depths was therefore
    /// unreachable dead ordering — and encoded an impossible tie as if it
    /// were expected.) [`deepest_containing`] makes the uniqueness
    /// structural, one exact probe per ancestor level.
    pub(crate) fn strict_modes_for(&self, file: FileId) -> dartq_semantic::StrictModes {
        let strict_flags = self
            .files
            .path(file)
            .and_then(|path| deepest_containing(&self.options, path));
        dartq_semantic::StrictModes {
            casts: strict_flags.is_some_and(|o| o.strict_casts.unwrap_or(false)),
            inference: strict_flags.is_some_and(|o| o.strict_inference.unwrap_or(false)),
            raw_types: strict_flags.is_some_and(|o| o.strict_raw_types.unwrap_or(false)),
        }
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

    /// The persisted scan face of a resident file, addressed under the exact
    /// key the apply recompute stored it under ([`crate::l1::facts_key`] —
    /// the content + granted-feature-fingerprint triple): the one shared
    /// read by file, so no consumer can hit an entry under a key it was
    /// never stored under. `None` when the file is not resident or its face
    /// was capped out of the cache; a file whose facts were served entirely
    /// from warm start also has none, because `scanned_to_cache` only pushes
    /// faces computed by a cold recompute — every caller falls back to
    /// re-deriving the pure face locally.
    pub(crate) fn settled_face(&self, file: FileId) -> Option<Arc<ScannedFace>> {
        let facts = self.residency.get(&file)?;
        let key = crate::l1::facts_key(&self.grants(), file, facts.content);
        self.cached_scanned(key)
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

    /// The maintained capture of `file`, `None` before its first analyze
    /// covers it or after the file left residency.
    pub(crate) fn capture(&self, file: FileId) -> Option<&Arc<FileCapture>> {
        self.captures
            .get(file.index() as usize)
            .and_then(Option::as_ref)
    }

    /// Stores (or replaces, on recapture) `file`'s capture, re-listing the
    /// declaration postings for the whole-unit swap (the crate's
    /// `SymbolIndex::insert_file` law: the replaced surface unlists its
    /// names, the fresh one lists its).
    pub(crate) fn set_capture(&mut self, file: FileId, capture: Arc<FileCapture>) {
        let index = file.index() as usize;
        if index >= self.captures.len() {
            self.captures.resize_with(index + 1, || None);
        }
        if let Some(old) = self.captures[index].replace(capture) {
            for name in distinct_decl_names(&old.index) {
                self.decl_postings.unlist(name, file);
            }
        }
        let fresh = self.captures[index].as_ref().expect("just stored");
        for name in distinct_decl_names(&fresh.index) {
            self.decl_postings.list(name, file);
        }
        // The decl view re-indexes the fresh rows by span start, in
        // lockstep with the swap.
        if index >= self.decl_order.len() {
            self.decl_order.resize_with(index + 1, || None);
        }
        let decls = fresh.index.decls();
        let mut order: Vec<u32> = (0..saturate_slot_u32(decls.len())).collect();
        order.sort_unstable_by_key(|&i| decls[i as usize].range.offset);
        self.decl_order[index] = Some(order.into_boxed_slice());
        self.feed_sites(file);
    }

    /// Feeds the raw-site cache: a stored capture's sites are the
    /// freshest, so the file joins (or refreshes to) the back of the
    /// feed-order queue and the over-cap head sheds. Shedding swaps the
    /// standing capture for its sites-less twin — the declaration rows,
    /// postings, and decl view are untouched (identical index — the twin
    /// is `without_sites`, not a recapture) — and a later rebuild
    /// re-derives the sites from content on the miss.
    fn feed_sites(&mut self, file: FileId) {
        if !self.sites_cached.insert(file) {
            if let Some(pos) = self.sites_order.iter().position(|&f| f == file) {
                self.sites_order.remove(pos);
            }
        }
        self.sites_order.push_back(file);
        while self.sites_order.len() > SITES_CACHE_CAP {
            let Some(oldest) = self.sites_order.pop_front() else {
                break;
            };
            self.sites_cached.remove(&oldest);
            if let Some(slot) = self.captures.get_mut(oldest.index() as usize) {
                if slot.is_some() {
                    let old = slot.take().expect("checked");
                    *slot = Some(Arc::new(old.without_sites()));
                }
            }
        }
    }

    /// How many resident files' raw sites are mounted — the site cache's
    /// occupancy, the count the memory-accounting face reports.
    pub(crate) fn sites_occupancy(&self) -> usize {
        self.sites_order.len()
    }

    /// The capture a library build folds `file`'s sites from: the standing
    /// capture when its sites are mounted, else a local re-derivation over
    /// the file's content and persisted scan face (the scanned-face
    /// fallback posture — re-derivation is a read-side rebuild and never
    /// refills the site cache). `None` when the file has no standing
    /// capture or its content cannot back one — the caller's bounded
    /// degrade.
    pub(crate) fn rebuild_capture(&self, file: FileId) -> Option<Arc<FileCapture>> {
        let capture = self.capture(file)?;
        if capture.sites_present() {
            return Some(Arc::clone(capture));
        }
        let facts = self.residency.get(&file)?;
        let bytes = self.store.get(facts.content)?;
        let text = std::str::from_utf8(bytes).ok()?;
        let face = match self.settled_face(file) {
            Some(face) => face,
            None => {
                let version = crate::l1::language_version_for(&self.packages, &self.files, file);
                let (feature_set, settled, scanned, _) =
                    crate::l1::scan_unit(text, version, file, &self.grants());
                Arc::new(ScannedFace {
                    tokens: scanned.tokens,
                    features: feature_set,
                    version: settled,
                })
            }
        };
        Some(Arc::new(dartq_index::capture(
            file,
            text,
            &face.tokens,
            dartq_parser::ParserFeatures::from_feature_set(face.features),
            &self.strings,
        )))
    }

    /// Drops `file`'s capture (the file left residency). Bounds-tolerant:
    /// a file removed before its first analyze never grew the arena. The
    /// declaration postings follow the removal.
    pub(crate) fn clear_capture(&mut self, file: FileId) {
        if let Some(old) = self
            .captures
            .get_mut(file.index() as usize)
            .and_then(Option::take)
        {
            for name in distinct_decl_names(&old.index) {
                self.decl_postings.unlist(name, file);
            }
        }
        if let Some(slot) = self.decl_order.get_mut(file.index() as usize) {
            *slot = None;
        }
        // The site cache follows the removal: a stale entry would inflate
        // the occupancy count and shed nothing.
        if self.sites_cached.remove(&file) {
            if let Some(pos) = self.sites_order.iter().position(|&f| f == file) {
                self.sites_order.remove(pos);
            }
        }
    }

    /// Every live library's surface with its id, ascending by id — the
    /// deterministic order the query face serves.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = (LibraryId, &LibraryIndex)> + '_ {
        self.library_surfaces
            .iter()
            .enumerate()
            .filter_map(|(index, surface)| {
                surface
                    .as_ref()
                    .map(|surface| (LibraryId(index as u32), surface))
            })
    }

    /// The reference surface of `library`, if analyzed — the per-unit row
    /// read the query face's directory probes serve through.
    pub(crate) fn surface(&self, library: LibraryId) -> Option<&LibraryIndex> {
        self.library_surfaces
            .get(library.index() as usize)
            .and_then(Option::as_ref)
    }

    /// Stores (or replaces, at the `LibraryAnalyzed` point) `library`'s
    /// reference surface with its precomputed use view ([`use_view_of`],
    /// built in the analyze fan-out beside the surface), unlisting the
    /// replaced surface's reference postings at once and deferring the
    /// fresh listings into `listings` — the caller lands every stored
    /// library's listings in one [`Postings::list_batch`] after its store
    /// loop. The deferral is the cost fix: the store loop walks libraries
    /// in topological order, so per-library listing paid a sorted-insert
    /// memmove per (name, library) pair, quadratic in a shared name's
    /// library count; nothing reads the reference postings between the
    /// loop's stores (the loop holds the engine exclusively), and the
    /// batch leaves exactly the directory the per-library calls built.
    pub(crate) fn set_surface(
        &mut self,
        library: LibraryId,
        surface: LibraryIndex,
        view: UseView,
        listings: &mut Vec<(StringId, LibraryId)>,
    ) {
        let index = library.index() as usize;
        if index >= self.library_surfaces.len() {
            self.library_surfaces.resize_with(index + 1, || None);
        }
        if let Some(old) = self.library_surfaces[index].replace(surface) {
            for name in old.distinct_reference_names() {
                self.ref_postings.unlist(name, library);
            }
        }
        let fresh = self.library_surfaces[index].as_ref().expect("just stored");
        listings.extend(fresh.distinct_reference_names().map(|name| (name, library)));
        if index >= self.use_views.len() {
            self.use_views.resize_with(index + 1, || None);
        }
        self.use_views[index] = Some(view);
    }

    /// Lands the reference listings [`Self::set_surface`] deferred.
    pub(crate) fn list_references(&mut self, listings: &mut Vec<(StringId, LibraryId)>) {
        self.ref_postings.list_batch(listings);
    }

    /// The owned heap bytes of every maintained capture and library
    /// surface plus their views — the index segment of the memory
    /// decomposition. Exact over the owned columns (allocator rounding
    /// aside); a capture shared by refcount with an in-flight holder counts
    /// once, here.
    pub(crate) fn index_bytes(&self) -> (usize, usize) {
        let captures = self.captures.capacity() * size_of::<Option<Arc<FileCapture>>>()
            + self
                .captures
                .iter()
                .flatten()
                .map(|capture| size_of::<FileCapture>() + capture.heap_bytes())
                .sum::<usize>()
            + self.decl_order.capacity() * size_of::<Option<Box<[u32]>>>()
            + self
                .decl_order
                .iter()
                .flatten()
                .map(|order| order.len() * size_of::<u32>())
                .sum::<usize>();
        let surfaces = self.library_surfaces.capacity() * size_of::<Option<LibraryIndex>>()
            + self
                .library_surfaces
                .iter()
                .flatten()
                .map(LibraryIndex::heap_bytes)
                .sum::<usize>()
            + self.use_views.capacity() * size_of::<Option<UseView>>()
            + self
                .use_views
                .iter()
                .flatten()
                .map(|view| {
                    view.len() * size_of::<(FileId, Box<[u32]>)>()
                        + view.iter().map(|(_, rows)| rows.len() * size_of::<u32>()).sum::<usize>()
                })
                .sum::<usize>();
        (captures, surfaces)
    }

    /// The memory decomposition (#2210): one line per resident family with
    /// its byte total, each tagged `exact` (owned columns at capacity,
    /// allocator rounding aside) or `est` (per-entry constant × count).
    /// Read-only and O(residency); called only when the
    /// `DARTQ_RSS_DECOMPOSITION` probe is armed, so a disarmed run pays
    /// nothing. The process RSS readings ride beside it in the caller's
    /// dump — the gap between the two is allocator slack plus the
    /// families no counter covers (the transient fan-out products).
    pub(crate) fn memory_report(&self) -> Vec<(&'static str, u64, &'static str)> {
        let mut facts = 0usize;
        let mut line_index = 0usize;
        let mut directives = 0usize;
        let mut fact_rows = 0usize;
        for unit in self.residency.values() {
            facts += size_of::<UnitFacts>() + size_of::<(FileId, Arc<UnitFacts>)>();
            line_index += unit.line_index.line_count() as usize * size_of::<u32>();
            directives += unit.directives.capacity() * size_of::<DirectiveFacts>()
                + unit
                    .directives
                    .iter()
                    .map(|d| d.uri.as_ref().map_or(0, |uri| uri.len()))
                    .sum::<usize>();
            fact_rows += unit.diagnostics.capacity() * size_of::<Arc<Diagnostic>>()
                + unit.diagnostics.len() * size_of::<Diagnostic>();
        }
        let (captures, surfaces) = self.index_bytes();
        let (semdiag, semdiag_slots) = {
            let memo = self.semdiag.lock().unwrap_or_else(PoisonError::into_inner);
            (
                memo.folds
                    .values()
                    .map(|fold| fold.heap_bytes())
                    .sum::<usize>(),
                memo.folds.capacity()
                    * (size_of::<(FileId, Arc<crate::semdiag::MemberFold>)>() + 1),
            )
        };
        let results = self
            .results
            .values()
            .map(|result| result.diagnostics.capacity() * size_of::<Arc<Diagnostic>>())
            .sum::<usize>();
        let results_map = self.results.capacity()
            * (size_of::<(LibraryId, LibraryResult)>() + 1);
        let (merged, merged_contributors) = self.merged.as_ref().map_or((0, 0), |merged| {
            (
                merged.output.capacity() * size_of::<Diagnostic>(),
                merged.contributors.capacity() * size_of::<(LibraryId, u32)>(),
            )
        });
        let scanned = self
            .scanned
            .values()
            .map(|face| face.tokens.len() * (size_of::<dartq_scanner::Token>() + 4))
            .sum::<usize>();
        let scanned_slots = self.scanned.capacity()
            * (size_of::<(FactsKey, Arc<ScannedFace>)>() + 1);
        let element_models = self
            .residency
            .values()
            .map(|unit| element_heap_bytes(&unit.elements))
            .sum::<usize>();
        // The facts cache's payloads share the residency shape; entries
        // share `Arc`s with live facts (a live file's cache entry adds no
        // payload), so summing the full composition double-counts what
        // residency already reports. Counted anyway — the family answers
        // "what the cache pins", the same conservatism `warm_start` gets.
        let cached_facts = self
            .facts_cache
            .values()
            .map(|unit| {
                size_of::<UnitFacts>()
                    + unit.line_index.line_count() as usize * size_of::<u32>()
                    + unit.directives.capacity() * size_of::<DirectiveFacts>()
                    + unit.diagnostics.capacity() * size_of::<Arc<Diagnostic>>()
                    + element_heap_bytes(&unit.elements)
            })
            .sum::<usize>();
        let facts_cache_slots = self.facts_cache.capacity()
            * (size_of::<(FactsKey, Arc<UnitFacts>)>() + 1);
        let (warm_start, warm_start_slots) = self.warm_start_bytes();
        let session_scratch = self.session_scratch_bytes();
        let residency_slots = self.residency.capacity()
            * (size_of::<(FileId, Arc<UnitFacts>)>() + 1);
        let as_u64 = |bytes: usize| bytes as u64;
        vec![
            ("store_bytes", self.store.len_bytes(), "exact"),
            ("store_mapped_bytes", self.store.mapped_bytes(), "exact"),
            ("residency_facts_shell", as_u64(facts), "exact"),
            ("residency_map_slots", as_u64(residency_slots), "exact"),
            ("residency_line_index", as_u64(line_index), "exact"),
            ("residency_directives", as_u64(directives), "exact"),
            ("residency_diagnostic_rows", as_u64(fact_rows), "est"),
            ("residency_element_models", as_u64(element_models), "est"),
            ("index_captures", as_u64(captures), "exact"),
            ("index_library_surfaces", as_u64(surfaces), "exact"),
            (
                "index_postings",
                as_u64(self.decl_postings.bytes() + self.ref_postings.bytes()),
                "exact",
            ),
            ("graph_maps", self.graph_maps_bytes(), "exact"),
            ("semdiag_memo", as_u64(semdiag), "exact"),
            ("semdiag_map_slots", as_u64(semdiag_slots), "exact"),
            ("library_results_handles", as_u64(results), "exact"),
            ("library_results_map", as_u64(results_map), "exact"),
            ("merged_output_rows", as_u64(merged), "exact"),
            ("merged_contributor_rows", as_u64(merged_contributors), "exact"),
            ("scanned_cache_tokens", as_u64(scanned), "exact"),
            ("scanned_map_slots", as_u64(scanned_slots), "exact"),
            ("facts_cache_facts", as_u64(cached_facts), "est"),
            ("facts_cache_map_slots", as_u64(facts_cache_slots), "exact"),
            ("warm_start_payloads", as_u64(warm_start), "est"),
            ("warm_start_map_slots", as_u64(warm_start_slots), "exact"),
            ("session_scratch", as_u64(session_scratch), "exact"),
            ("scanned_cache_entries", self.scanned.len() as u64, "count"),
            ("facts_cache_entries", self.facts_cache.len() as u64, "count"),
            ("warm_start_entries", self.warm_start.len() as u64, "count"),
            ("residency_files", self.residency.len() as u64, "count"),
            ("string_table_entries", self.strings.len() as u64, "count"),
            ("structural_tables", self.structural_bytes(), "est"),
        ]
    }

    /// Warm-start staging's byte split: the payload heap at capacity (the
    /// payload's own `Vec`/`String` columns — shallow over the
    /// serde-mirrored tree, deep walks cost more than the family recovers,
    /// so the column-level capacities plus the string bytes stand in as
    /// `est`) plus the map slots (exact).
    fn warm_start_bytes(&self) -> (usize, usize) {
        let payloads = self
            .warm_start
            .values()
            .map(|facts| {
                size_of::<persist::PersistedFacts>()
                    + facts.directives.capacity()
                        * size_of::<persist::PersistedDirective>()
                    + facts
                        .directives
                        .iter()
                        .map(|d| {
                            d.uri.as_ref().map_or(0, String::len)
                                + d.prefix.as_ref().map_or(0, String::len)
                                + d.name.as_ref().map_or(0, String::len)
                        })
                        .sum::<usize>()
                    + facts.doc_imports.capacity() * size_of::<persist::PersistedDocImport>()
                    + facts
                        .doc_imports
                        .iter()
                        .map(|d| d.uri.as_ref().map_or(0, String::len))
                        .sum::<usize>()
                    + facts.diagnostics.capacity() * size_of::<persist::PersistedDiagnostic>()
                    + facts
                        .diagnostics
                        .iter()
                        .map(|d| {
                            d.message.len()
                                + d.code_name.len()
                                + d.code_machine.len()
                                + d.correction.as_ref().map_or(0, String::len)
                        })
                        .sum::<usize>()
                    + facts
                        .files
                        .iter()
                        .map(|path| path.as_os_str().len() + size_of::<PathBuf>())
                        .sum::<usize>()
            })
            .sum::<usize>();
        let slots = self.warm_start.capacity()
            * (size_of::<(dartq_snapshot::EntryKey, persist::PersistedFacts)>() + 1);
        (payloads, slots)
    }

    /// Session scratch with session lifetime: the apply/analyze queues
    /// and membership sets that hold their reserved capacity between
    /// passes — the recapture queue, the site-cache feed, the dirty and
    /// digest bookkeeping, the pending overlay, the canonical-URI memo,
    /// the per-library generation table, the pubspec lint face. Exact
    /// over owned columns at capacity.
    fn session_scratch_bytes(&self) -> usize {
        self.capture_dirty.capacity() * size_of::<FileId>()
            + self.sites_order.capacity() * size_of::<FileId>()
            + self.sites_cached.capacity() * (size_of::<FileId>() + 1)
            + self.invalidated.capacity() * size_of::<LibraryId>()
            + self.structure_changed.capacity() * size_of::<FileId>()
            + self.structure_removed.capacity() * size_of::<FileId>()
            + self.pending_order.capacity() * size_of::<FileId>()
            + self.pending.capacity() * (size_of::<(FileId, PendingChange)>() + 1)
            + self.canonical_uris.capacity() * (size_of::<(FileId, Arc<str>)>() + 1)
            + self
                .canonical_uris
                .values()
                .map(|uri| uri.len())
                .sum::<usize>()
            + self.library_generations.capacity() * (size_of::<(LibraryId, u32)>() + 1)
            + self.structure_digests.capacity() * (size_of::<(FileId, u64)>() + 1)
            + self
                .reported
                .capacity()
                * (size_of::<(FileId, (Arc<str>, LineIndex))>() + 1)
            + self
                .reported
                .values()
                .map(|(text, index)| {
                    text.len()
                        + index.line_count() as usize * size_of::<u32>()
                        + size_of::<LineIndex>()
                })
                .sum::<usize>()
    }

    /// Drops `library`'s surface (the library retired); the reference
    /// postings follow the removal.
    pub(crate) fn clear_surface(&mut self, library: LibraryId) {
        if let Some(old) = self
            .library_surfaces
            .get_mut(library.index() as usize)
            .and_then(Option::take)
        {
            for name in old.distinct_reference_names() {
                self.ref_postings.unlist(name, library);
            }
        }
        if let Some(slot) = self.use_views.get_mut(library.index() as usize) {
            *slot = None;
        }
    }

    /// The capture's declaration-row indices, offset-ascending — the
    /// span-ordered probe order the position face binary-searches. `None`
    /// with the capture's own absence (the same arena law).
    pub(crate) fn decl_order(&self, file: FileId) -> Option<&[u32]> {
        self.decl_order
            .get(file.index() as usize)
            .and_then(Option::as_deref)
    }

    /// The library surface's reference rows regrouped by use file, each
    /// group's row indices offset-ascending — the position face's per-file
    /// probe domain. `None` with the surface's own absence.
    pub(crate) fn use_view(&self, library: LibraryId) -> Option<&[(FileId, Box<[u32]>)]> {
        self.use_views
            .get(library.index() as usize)
            .and_then(Option::as_deref)
    }

    /// The analyze's reachability gate (#1383): records that a completed
    /// analyze exists (first gate arms the map's existence), clears the
    /// in-flight-apply window (the gate runs after the drain and the
    /// recompute — the build's coherence precondition), and arms the
    /// rebuild when the pass's inputs moved (`moved`). A zero-dirty cycle
    /// leaves the standing map standing.
    pub(crate) fn reachability_gate(&self, moved: bool) {
        let mut memo = self
            .reachability
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        memo.armed = true;
        memo.applied = false;
        memo.stale |= moved;
    }

    /// The apply landed (`l1::apply` — the single choke point both
    /// [`crate::Engine::apply_pending`] and the analyze driver run
    /// through): residency moved without the drain/recompute pass, so
    /// reachability reads serve the standing map until the next gate.
    pub(crate) fn mark_reachability_applied(&self) {
        self.reachability
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .applied = true;
    }

    /// Drops a removed file's memoized semdiag fold (the apply's removal
    /// arm): the map tracks residency, so an entry outliving its file would
    /// only leak — `FileId`s are never recycled, so it could never be
    /// served by mistake.
    pub(crate) fn clear_semdiag_fold(&self, file: FileId) {
        self.semdiag
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .folds
            .remove(&file);
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

    /// Exact byte footprint of the four per-target adjacency indexes —
    /// [`Self::importers`] + [`Self::part_offers`] + [`Self::import_sources`]
    /// + [`Self::part_of_sources`] slot arenas and live rows (the S6
    /// graph-maps segment of the RSS decomposition probe). Exact under the
    /// new row-level CSR layout: no hash slack or per-entry shell remains
    /// uncounted.
    pub(crate) fn graph_maps_bytes(&self) -> u64 {
        (self.importers.memory_bytes()
            + self.part_offers.memory_bytes()
            + self.import_sources.memory_bytes()
            + self.part_of_sources.memory_bytes()) as u64
    }

    /// The structural resident state's byte estimate: the two immortal-id
    /// tables the budget ladder can never shed ([`FileTable`] +
    /// [`StringTable`], the per-entry constants above × their interned
    /// counts). Tier-3's degrade row names it, so the floor report is
    /// truthful about what remains once every sheddable cache is gone
    /// (#1887) — the admission ceiling bounds this state, the ladder only
    /// reports it. O(1): two counters × two constants.
    pub(crate) fn structural_bytes(&self) -> u64 {
        (self.files.len() * FILE_TABLE_EST_BYTES_PER_PATH
            + self.strings.len() * STRING_TABLE_BYTES_PER_ENTRY) as u64
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
    /// The interned-string table is deliberately excluded: its ids are held
    /// by live facts and results whose names no single content accounts for
    /// (one text arrives from many files and from synthesized spellings, and
    /// a dedup hit keeps only the first interner's origin), so no
    /// origin-based liveness over strings is sound. The table rides its
    /// documented session ceiling ([`StringTable`] in dartq-source) instead.
    ///
    /// Called at the end of every non-empty apply (l1 `apply`), after
    /// residency has been rewritten, so replaced contents are already
    /// unreferenced when the sweep runs.
    pub(crate) fn collect_garbage(&mut self, newly_dead: usize) {
        self.gc_debt += newly_dead;
        if self.gc_debt < GC_DEAD_THRESHOLD {
            return;
        }
        self.sweep_store();
    }

    /// One unconditional mark-sweep over the content store (the amortized
    /// path's body and the budget ladder's tier-3 shed): every store entry
    /// neither residency nor the facts cache references goes. See
    /// [`Self::collect_garbage`] for the liveness rule's authority note.
    fn sweep_store(&mut self) {
        self.gc_debt = 0;
        let mut live: FxHashSet<ContentId> = FxHashSet::with_capacity_and_hasher(
            self.residency.len() + self.facts_cache.len() + self.deps.units.len(),
            FxBuildHasher,
        );
        live.extend(self.residency.values().map(|facts| facts.content));
        live.extend(self.facts_cache.keys().map(|key| key.content));
        // The dependency-interface units' sources stay live too: a swept dep
        // content would break the clause-tree capture's store read (the
        // under-report direction, but an avoidable one).
        live.extend(
            self.deps
                .units
                .values()
                .map(|unit| unit.facts.content()),
        );
        self.store
            .collect_garbage(&|id: ContentId| live.contains(&id));
    }

    /// The phase-boundary budget poll: descends at most one
    /// ladder tier and applies its shed — facts cache (tier 1), scanned
    /// faces (tier 2), warm-start staging + canonical-URI memo + an eager
    /// store sweep (tier 3). Every shed target is documented-evictable
    /// recomputable state; residency, results, captures, and surfaces are
    /// never touched, so diagnostics stay complete at the bottom tier.
    /// The third field is [`Self::structural_bytes`] at the fired boundary —
    /// the tier-3 report includes it, since the bottom tier sheds nothing
    /// structural and must say so (#1887). `None` when unarmed, under the
    /// limit, or without an RSS reading — the caller pays one `Option`
    /// test per boundary.
    pub(crate) fn poll_budget(&mut self) -> Option<(crate::budget::Tier, u64, u64)> {
        let (tier, limit_mib) = crate::budget::poll(&mut self.budget)?;
        match tier {
            crate::budget::Tier::LazyParse => {
                self.facts_cache.clear();
                self.facts_order.clear();
            }
            crate::budget::Tier::SemanticSlices => {
                self.scanned.clear();
                self.scanned_order.clear();
            }
            crate::budget::Tier::AstCold => {
                self.warm_start.clear();
                self.canonical_uris.clear();
                self.sweep_store();
            }
        }
        Some((tier, limit_mib, self.structural_bytes()))
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
            &self.declared_variables,
            &mut self.files,
            resident,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::content_hash;

    /// The live-path ceiling's degrade row (#1887): one row naming the
    /// ceiling, anchored at the caller's id, and the (file, face) dedup
    /// keeps repeats and later anchors from growing the log.
    #[test]
    fn ceiling_degrade_row_records_once_with_the_ceiling_face() {
        let degrades = ResidencyDegrades::default();
        let mut files = FileTable::new();
        let anchor = files.intern(std::path::Path::new("/w/lib/a.dart"));

        degrades.record_ceiling(anchor);
        assert_eq!(
            degrades.take(),
            vec![(anchor, LIVE_PATH_CEILING_FACE)],
            "one row naming the ceiling"
        );
        degrades.record_ceiling(anchor);
        assert!(degrades.take().is_empty(), "the same anchor dedups");
    }

    /// The structural accounting's per-entry constants (#1887): the file
    /// table's est. per-path figure and the string table's measured
    /// per-entry figure — the values the tier-3 degrade report scales
    /// with. Pinned so an accidental drift is a reviewed change, not a
    /// silent re-basing of the observability face.
    #[test]
    fn structural_accounting_constants_carry_the_documented_figures() {
        assert_eq!(FILE_TABLE_EST_BYTES_PER_PATH, 256);
        assert_eq!(STRING_TABLE_BYTES_PER_ENTRY, 93);
    }

    /// A live info for `uri`, the shape `retire` unregisters through.
    fn stub_info(uri: &str) -> LibraryInfo {
        LibraryInfo {
            uri: Arc::from(uri),
            name: "".into(),
            defining: FileTable::new().intern(std::path::Path::new("/stub.dart")),
            parts: Vec::new(),
            content: Box::new([content_hash(uri.as_bytes())]),
            deps: Vec::new(),
            part_targets: Box::new([]),
        }
    }

    #[test]
    fn intern_returns_the_live_id_and_reuses_retired_ones() {
        let mut table = LibraryTable::default();
        let a = table.intern("package:a/a.dart");
        let b = table.intern("package:b/b.dart");
        assert_ne!(a, b);
        assert_eq!(table.intern("package:a/a.dart"), a, "live uri keeps its id");
        assert_eq!(table.arena_len(), 2);

        // Retiring live registrations returns their ids to the freelist and
        // unregisters them; the next interns re-mint those ids for different
        // URIs — the aliasing the retire ordering contract guards against.
        // The freelist pops in retirement order, so which tenant gets which
        // id follows the retire sequence.
        table.set_info(a, stub_info("package:a/a.dart"));
        table.set_info(b, stub_info("package:b/b.dart"));
        table.retire(a);
        table.retire(b);
        let c = table.intern("package:c/c.dart");
        let d = table.intern("package:d/d.dart");
        assert_eq!(c, b, "the freelist pops in retirement order");
        assert_eq!(d, a);
        assert_eq!(
            table.arena_len(),
            2,
            "reuse keeps the arena at the live peak"
        );
    }

    #[test]
    fn retire_unregisters_retired_uris_and_keeps_live_ones() {
        let mut table = LibraryTable::default();
        let a = table.intern("package:a/a.dart");
        let b = table.intern("package:b/b.dart");
        let c = table.intern("package:c/c.dart");
        table.set_info(a, stub_info("package:a/a.dart"));
        table.set_info(b, stub_info("package:b/b.dart"));
        table.set_info(c, stub_info("package:c/c.dart"));
        table.retire(a);
        table.retire(c);

        // The kept id stays registered; the retired uris are unregistered.
        assert_eq!(table.ids_by_uri.get("package:b/b.dart"), Some(&b));
        assert!(!table.ids_by_uri.contains_key("package:a/a.dart"));
        assert!(!table.ids_by_uri.contains_key("package:c/c.dart"));
        assert!(table.info(a).is_none());
        assert!(table.info(c).is_none());

        // A retired uri interns as a fresh registration, not a resurrection
        // of stale info: it draws a recycled id from the freelist instead of
        // growing the arena.
        let a2 = table.intern("package:a/a.dart");
        assert!(a2 == a || a2 == c, "interns draw from the freelist");
        assert_eq!(table.ids_by_uri.get("package:a/a.dart"), Some(&a2));
        assert_eq!(table.arena_len(), 3);
    }
}
