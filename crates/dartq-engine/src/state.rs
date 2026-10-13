//! Resident state (incremental-l1-spec §3.1 table): content residency, the
//! pending overlay, the library arena with its reverse-import index, the
//! per-library result cache, and the bounded per-file facts cache.

pub mod persist;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use dartq_diagnostics::Diagnostic;
use dartq_index::{FileCapture, LibraryIndex};
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
const FACTS_SHAPE_VERSION: u32 = 9;

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

// The fold above is injective only while the signature fits its 16-bit lane:
// a SIGNATURE_VERSION of 2^16 or more would wrap and alias an older
// snapshot's key as current. Widening the key rides the next
// FACTS_SHAPE_VERSION bump (a container-format change anyway).
const _: () = assert!(dartq_semantic::SIGNATURE_VERSION <= u16::MAX as u32);

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
/// - `sdk_library_dirs` is the reverse of `SdkSnapshot::uri_to_path`: each
///   target library's directory (`<root>/lib/<dirname(uri)>`) maps to its
///   name, so a file attributed by its disk path — the engine keeps no
///   `dart:` URIs — recovers the owning library exactly when the path was
///   produced by a `dart:` resolution.
/// - `memo` is the `(owner, version) -> FeatureSet` cache the per-file
///   lanes share: one derivation per distinct pair per engine, O(1) lookups
///   after first touch. `Mutex`-guarded because the lanes read `&state` on
///   rayon workers (the [`StringTable`] interning-lock precedent); the
///   critical section is a hash lookup.
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
    memo: Mutex<FxHashMap<(GrantOwner, LanguageVersion), FeatureSet>>,
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
            for (name, library) in &segment.libraries {
                if !sdk.platform_metadata.for_sdk_library(name).is_empty() {
                    any = true;
                }
                // The library's directory: `<lib>/<dirname(uri)>` — the
                // join always has a parent (`uri` is non-empty and relative).
                let dir = lib_root
                    .join(&library.uri)
                    .parent()
                    .expect("library path under lib/ always has a directory")
                    .to_path_buf();
                sdk_library_dirs.insert(Box::from(dir), Box::from(name.as_str()));
            }
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
    /// a version skew that must surface (`UnknownExperiments`), not
    /// silently disable the grant — it fails loudly here, once per
    /// unresolvable `(owner, version)`. The options merge runs after the
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
        let set = if !self.any {
            FeatureSet::for_language_version(version)
        } else {
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
                    let set = match &owner {
                        GrantOwner::Package(name) => {
                            experiments.feature_set_for_package(name, version)
                        }
                        GrantOwner::SdkLibrary(name) => {
                            experiments.feature_set_for_sdk_library(name, version)
                        }
                    }
                    .unwrap_or_else(|unknown| {
                        panic!(
                            "allowed-experiments grant for `{owner:?}` cannot be applied: {unknown}"
                        )
                    });
                    memo.insert((owner, version), set);
                    set
                }
                None => FeatureSet::for_language_version(version),
            }
        };
        self.with_options_experiments(experiments, packages, files, file, version, set)
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
        let mut best: Option<(usize, Option<&[dartq_language::Feature]>)> = None;
        for (dir, flags) in &self.options_experiments {
            if path.starts_with(dir.as_ref()) {
                let depth = dir.components().count();
                if best.is_none_or(|(depth_so_far, _)| depth > depth_so_far) {
                    best = Some((depth, flags.as_deref()));
                }
            }
        }
        // A declaring options chain replaces the CLI context base; no
        // section anywhere in the subtree's chain keeps the base.
        let flags = match best {
            Some((_, Some(flags))) => flags,
            _ => self.cli_experiments.as_ref(),
        };
        for &feature in flags {
            set = set.with_experiment_for_version(version, feature);
        }
        set
    }

    /// The grant owner of `file`: an SDK library when the path lies under
    /// the SDK's `lib/` (walked up to the nearest library directory — the
    /// exact inverse of `SdkSnapshot::uri_to_path`, so parts attribute to
    /// their `dart:` library), else the containing package. The SDK branch
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
/// `part`-directive order (res-spec §2.6 — the library content identity
/// folds the member contents in exactly this order).
#[derive(Debug)]
pub(crate) struct LibraryInfo {
    /// Canonical URI the library identity keys on. Shares the
    /// canonical-URI memo's allocation ([`EngineState::canonical_uris`]):
    /// formation clones an `Arc` refcount per formed library, never
    /// re-copies the bytes.
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
    pub diagnostics: Vec<Diagnostic>,
    /// The condensation fingerprint of the library's cycle at store time —
    /// the reuse key the post-closure gate compares (PRD §11.2, g485 W2).
    /// The digest folds the member signatures and the dependency-cycle
    /// fingerprint set, so `recorded == current` means every member face and
    /// every transitive dependency face is byte-identical to this run's
    /// inputs. The apply drops the entry on every identity move, so a
    /// present result is identity-fresh by construction: the pair of halves
    /// is exactly the reuse condition, and any doubt (a `None` mapping, a
    /// moved digest) recomputes — monotone versus the L2 behavior.
    pub cycle_fingerprint: ApiSignature,
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

/// Everything the engine keeps between API calls (§3.1). `&mut self`
/// exclusivity on the `Engine` makes the enqueue→apply→analyze order total,
/// so neither the overlay nor the store need interior mutability: the store
/// is owned outright (reads take `&self`, `put` runs inside apply), and the
/// only locks in the crate are the [`StringTable`]'s interning lock and the
/// [`FeatureGrants`] memo (worker threads share both during the parallel
/// recompute).
pub(crate) struct EngineState {
    /// Package identity table over one resolution (PRD §8).
    pub packages: IdentityTable,
    /// URI-resolution face built from the same pub config (engine-api-spec
    /// §0, dartq-resolution `PackageTable`).
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
    /// Reverse import edges (§3.1): resident target file → importing
    /// library → edge multiplicity (two members of one library importing the
    /// same target count twice, so one of them re-targeting away keeps the
    /// edge exact). Maintained by the apply phase's graph delta: every
    /// add is paired with a remove from the file's previous directive face.
    /// The adjacency is a sparse per-target map; the 10⁵-scale upgrade path
    /// is a CSR projection built lazily from this map, not a second
    /// maintained structure.
    pub importers: FxHashMap<FileId, FxHashMap<LibraryId, u32>>,
    /// Raw part-directive offers (residency-independent): target file →
    /// defining files whose current facts contain a `part` directive
    /// resolving to it. The claim-resolution candidate set: resolution picks
    /// the path-smallest candidate whose `may_claim` holds against current
    /// facts, so precedence stays deterministic-total while the delta keeps
    /// lookups O(offers) per affected part file. Maintained by paired
    /// subtract/add on every facts change; bounded by the live directive
    /// set.
    pub part_offers: FxHashMap<FileId, FxHashSet<FileId>>,
    /// Raw import/export sources (residency-independent): target file →
    /// files whose current facts contain an import/export directive
    /// resolving to it. This is the residency-flip fan-out: when a file
    /// enters residency, its importers are exactly the owners of these
    /// sources; the reverse query (`importers`) cannot answer it because it
    /// only holds resident targets. Maintained like [`Self::part_offers`].
    pub import_sources: FxHashMap<FileId, FxHashSet<FileId>>,
    /// Per-library result cache.
    pub results: FxHashMap<LibraryId, LibraryResult>,
    /// Post-closure dirty set of the last apply; consumed by analyze.
    /// Invariant: every entry is a live library id — apply assigns `close`'s
    /// output outright; `close` seeds the incoming leftovers (filtered to the
    /// post-delta live set) into its dirty set, so a cancelled analyze's
    /// still-invalid libraries carry forward. Unioning the incoming set back
    /// directly could instead resurrect ids retired by an intervening remove
    /// (#230), and analyze unwraps every queued id's info.
    pub invalidated: Vec<LibraryId>,
    /// Whether residency/structure changed since the last resolution-graph
    /// build (rebuild gate for the analyze phase).
    pub structure_dirty: bool,
    /// The library-dependency condensation (PRD §11.2, g485 W1): Tarjan SCC
    /// over the maintained deps edges with a per-cycle blake3 fingerprint.
    /// Derived data, rebuilt wholesale at the same [`Self::structure_dirty`]
    /// gate that rebuilds the resolution graph (`pipeline::analyze`) —
    /// never patched incrementally, never persisted, and retired ids answer
    /// no cycle between rebuilds.
    pub condensation: crate::cycle::Condensation,

    /// Pending overlay, insertion-ordered.
    pub pending: FxHashMap<FileId, PendingChange>,
    pub pending_order: Vec<FileId>,

    /// Resolved analysis options per governing directory (PRD §12): the
    /// include-merged file of every package root (or the synthesized
    /// default) plus every nested options directory inside the roots,
    /// merged once at context assembly; discovery applies the exclude
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
    /// The memory-budget runtime (PRD §17.1), `None` when no limit is set —
    /// the zero-cost shape (the poll short-circuits at one `Option` test,
    /// the sampler never runs). Armed through [`crate::Engine::set_memory_limit`].
    pub(crate) budget: Option<crate::budget::MemoryBudget>,
    /// The `--trace-events` JSONL sink (PRD §17.4), `None` when unset —
    /// the zero-cost shape (no sink object, no per-event work). Armed
    /// through [`crate::Engine::set_trace_sink`].
    pub(crate) trace: Option<crate::event::TraceSink>,
    /// The L3 content-addressed store (PRD §17.2), `None` when not armed —
    /// the zero-cost shape (the write-batch flush short-circuits at one
    /// `Option` test). Armed through
    /// [`crate::Engine::open_content_store`]; every apply batch persists
    /// its changed contents and flushes (pipeline's `flush_l3`).
    pub(crate) l3: Option<crate::store::ContentStore>,
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
        experiments: Vec<String>,
    ) -> Result<Self, crate::EngineError> {
        let uri_packages = UriPackageTable::new(pub_config, config_dir)?;
        // Options are a context field: the include chains merge once here,
        // keyed by governing directory — package roots plus the nested
        // options directories inside them (cold path, workspace-assembly
        // time). Built before the grant faces, which read the options
        // experiments (#936).
        let mut roots: Vec<PathBuf> = packages
            .iter()
            .map(|package| package.root.clone())
            .collect();
        roots.sort();
        roots.dedup();
        let options =
            crate::analysis_options::AnalysisOptions::for_roots(&roots, pub_config, config_dir);
        // The grant faces read the tables before they move into the state.
        let feature_grants = FeatureGrants::new(&sdk, &target, &packages, &options, &experiments);
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
            canonical_uris: FxHashMap::default(),
            libraries: LibraryTable::default(),
            library_of_file: FxHashMap::default(),
            importers: FxHashMap::default(),
            part_offers: FxHashMap::default(),
            import_sources: FxHashMap::default(),
            results: FxHashMap::default(),
            invalidated: Vec::new(),
            structure_dirty: false,
            condensation: crate::cycle::Condensation::default(),
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
            reported: FxHashMap::default(),
            capture_dirty: Vec::new(),
            gc_debt: 0,
            budget: None,
            trace: None,
            l3: None,
        })
    }

    /// The borrowed per-file grant faces (`l1::Grants`): everything the
    /// feature-set merge consults, in one disjoint-field borrow. Every
    /// per-file lane builds this once per item.
    pub(crate) fn grants(&self) -> crate::l1::Grants<'_> {
        crate::l1::Grants {
            grants: &self.feature_grants,
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
    pub(crate) fn strict_modes_for(&self, file: FileId) -> dartq_semantic::StrictModes {
        let Some(path) = self.files.path(file) else {
            return dartq_semantic::StrictModes::default();
        };
        let mut best: Option<(usize, &crate::analysis_options::AnalysisOptions)> = None;
        for (key, options) in &self.options {
            if path.starts_with(key) {
                let depth = key.components().count();
                if best.is_none_or(|(depth_so_far, _)| depth >= depth_so_far) {
                    best = Some((depth, options));
                }
            }
        }
        let strict_flags = best.map(|(_, options)| options);
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
            self.residency.len() + self.facts_cache.len(),
            FxBuildHasher,
        );
        live.extend(self.residency.values().map(|facts| facts.content));
        live.extend(self.facts_cache.keys().map(|key| key.content));
        self.store
            .collect_garbage(&|id: ContentId| live.contains(&id));
    }

    /// The phase-boundary budget poll (PRD §17.1): descends at most one
    /// ladder tier and applies its shed — facts cache (tier 1), scanned
    /// faces (tier 2), warm-start staging + canonical-URI memo + an eager
    /// store sweep (tier 3). Every shed target is documented-evictable
    /// recomputable state; residency, results, captures, and surfaces are
    /// never touched, so diagnostics stay complete at the bottom tier.
    /// `None` when unarmed, under the limit, or without an RSS reading —
    /// the caller pays one `Option` test per boundary.
    pub(crate) fn poll_budget(&mut self) -> Option<(crate::budget::Tier, u64)> {
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
        Some((tier, limit_mib))
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
    use dartq_source::content_hash;

    /// A live info for `uri`, the shape `retire` unregisters through.
    fn stub_info(uri: &str) -> LibraryInfo {
        LibraryInfo {
            uri: Arc::from(uri),
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
        assert!(table.ids_by_uri.get("package:a/a.dart").is_none());
        assert!(table.ids_by_uri.get("package:c/c.dart").is_none());
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
