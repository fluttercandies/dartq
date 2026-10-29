//! The lint face's mount: the analyze pipeline's bridge to
//! `dartq-lints` — enable-set resolution from the merged analysis options,
//! and the layer's rows mapped into engine diagnostics. This module lives
//! here, not in the engine: dartq-lints already depends on engine-side
//! types, so an engine→lints edge would be a cycle.
//!
//! # Selection
//!
//! [`enable_set`] resolves one enable set from one governing entry's
//! merged options ([`AnalysisOptions::lint_rules`], the include-chain
//! merge):
//!
//! * No options file in the root's ancestry (`None`) — the empty set: no
//!   lint runs. The official posture (`context_builder.dart:225-232` — an
//!   options-less package resolves to the default `AnalysisOptionsImpl`,
//!   whose lints are empty; probed zero lint rows on the 3.13.1 oracle).
//! * Options present — their effective rule list is the run's set,
//!   verbatim (the official replace semantics: the merged `linter.rules`
//!   section *is* the set; `name: false` drops the name before it reaches
//!   us, so "disable one rule" means the include chain supplies the rest).
//!
//! Unknown rule names in the list resolve to nothing (the layer's
//! `EnableSet::resolve` reports them back; the oracle surfaces them as
//! options-file warnings on a separate face dartq has not built — they are
//! dropped here, not errors).
//!
//! # Row shape
//!
//! [`run`] yields the layer's rows over one unit; the stage maps each row
//! to a [`Diagnostic`] at emission (`row`): severity is the code's
//! registry default (`DiagnosticType::Lint::severity` — oracle-INFO — for
//! the lint family, WARNING for the analyzer-diagnostic mirrors, the
//! `dtype_matches_oracle_severity` invariant; the layer's `EnableSet`
//! models no severity overrides), the code identity derives from the rule
//! registry (name and UPPERCASE machine form, typed per rule — lints are
//! `LINT` with never-published docs, mirroring the hand-written
//! `PREFER_SINGLE_QUOTES` row; the analyzer-diagnostic mirrors are
//! `STATIC_WARNING` with their published docs), and the UTF-16 row span
//! becomes the engine's [`SourceRange`]. `EnableSet` carries no severity
//! overrides, so there is nothing else to map.
//!
//! # Activation
//!
//! [`analyze_lints`] is the production activation: it parses each resident
//! file's source text transiently ([`dartq_lints::parse_unit`] — the #355
//! bridge; engine residency retains neither tokens nor trees) and runs the
//! enabled rules, extending the engine's diagnostics — except where the
//! caller's [`LintMemo`] already holds a file's rows (see below). The CLI
//! face calls it from `analyze::run_overlaid`, the daemon from
//! `session::run_group`, both before their display sorts. The check face
//! stays lint-free by design (scan+parse only).
//!
//! # Memo
//!
//! [`LintMemo`] keys rows by everything they read — content, features,
//! enable set, containing-package facts, path, the breaking-version gate —
//! so a daemon turn's lint stage answers unchanged files from memory and
//! does work only for the turn's dirty set. Rows store
//! pre-file-id-stamping (file-relative spans); the id stamps at emission
//! exactly as a fresh run's does, so a hit re-attributes byte-identically.
//! The CLI passes a throwaway memo —
//! a one-shot run shares nothing across processes.

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::collections::VecDeque;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, SourceRange};
use dartq_engine::analysis_options::{
    AnalysisOptions, canonical_diagnostic_name, is_known_diagnostic_code,
};
use dartq_engine::{Engine, LibraryId, Reachability, YamlIgnores};
use dartq_flow::FlowFact;
use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion};
use dartq_lints::{
    AsyncContextUse, Comments, EnableSet, Features, IgnoreInfo, Lint, RULES, ReachabilityFace,
    RuleId, UnignorableIgnore, WorkspaceFacts, check_name, check_section, check_url,
    is_breaking_version, mounts_semantics, parse_unit, parse_unit_with_tokens, rule_by_name, run,
    run_breaking_version_annotations, type_use_counts, unignorable_ignores,
    unnecessary_ignore_analyzer_rows, unnecessary_ignore_late_rows,
};
use dartq_package::PackageIdentity;
use dartq_parser::ParserFeatures;
use dartq_pub::pubspec_facts::scan as scan_pubspec;
use dartq_pub::{Pubspec, pubspec_facts::Scalar};
use dartq_resolution::{LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces};
use dartq_semantic::{
    ElementIndex, MicroKind, StrictModes, analyze_unit, container_modifiers, fold_unit,
};
use dartq_source::{ContentId, FileId, LineIndex, saturate_slot_u32};

/// The enable set one governing entry selects: `None` (no options file in
/// the governed file's ancestry) selects nothing — the official
/// options-less posture — and a present options file's effective rule list
/// replaces it. See the module docs for the selection semantics.
#[must_use]
pub fn enable_set(rules: Option<&[String]>) -> EnableSet {
    match rules {
        None => EnableSet::none(),
        // A name the pinned registry does not know reports `undefined_lint`
        // and never runs, even when dartq implements a later SDK's rule.
        Some(names) => {
            EnableSet::resolve(
                names
                    .iter()
                    .map(String::as_str)
                    .filter(|name| crate::lint_names::is_known(name)),
            )
            .0
        }
    }
}

/// The analyzer-diagnostic mirrors of the registry: true analyzer
/// diagnostics — `DiagnosticType.STATIC_WARNING` with published docs —
/// registered in the lint-name space so the options and ignore faces
/// resolve the names. The pubspec dependency-validator family (upstream
/// Dart SDK source,
/// `pkg/analyzer/lib/src/pubspec/validators/dependency_validator.dart`,
/// `diagnostic.g.dart` :13986-14030) emits from the engine's validator
/// face (`Engine::analyze`'s `validator_issue_rows`); this table keeps
/// the three names' `CodeInfo` identity for the registry-parallel
/// `CODES` mapping. The type-check warning (the
/// always-on `BestPracticesVerifier` pass, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/error/best_practices_verifier.dart:987-1041`,
/// `diagnostic.g.dart` :17642-17666 — the linter-side rule file left the
/// tree in the Dart 3.15 main re-pin, so the row emits from the semantic
/// tier's fold and the registry spec's AST arm is structurally off).
/// Every other registry entry is a lint: type `LINT`, never published docs.
const DIAGNOSTIC_MIRROR_CODES: [&str; 4] = [
    "path_does_not_exist",
    "path_not_posix",
    "path_pubspec_does_not_exist",
    "unnecessary_type_check",
];

/// Per-rule code identity, registry-parallel (`RuleId` indexes it): the
/// rule name with its UPPERCASE machine form, typed per rule — lints carry
/// type `LINT` (never published docs), the analyzer-diagnostic mirrors
/// carry `STATIC_WARNING` with their published docs
/// ([`DIAGNOSTIC_MIRROR_CODES`]). The machine forms leak once per rule
/// (process-lifetime strings) because `CodeInfo` pins `&'static str` and
/// the names are runtime-independent statics.
static CODES: LazyLock<Vec<CodeInfo>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|spec| {
            let machine: &'static str = Box::leak(spec.name.to_uppercase().into_boxed_str());
            if DIAGNOSTIC_MIRROR_CODES.contains(&spec.name) {
                CodeInfo::new(spec.name, machine, DiagnosticType::StaticWarning, true)
            } else {
                CodeInfo::new(spec.name, machine, DiagnosticType::Lint, false)
            }
        })
        .collect()
});

/// `The diagnostic '{0}' can't be ignored.` (registry :17369-17381,
/// `STATIC_WARNING` → WARNING; published docs). The `IgnoreValidator` face's
/// code: an ignore comment naming a `cannot-ignore` code reports once at the
/// name token and the name can never suppress.
const UNIGNORABLE_IGNORE: CodeInfo = CodeInfo::new(
    "unignorable_ignore",
    "UNIGNORABLE_IGNORE",
    DiagnosticType::StaticWarning,
    true,
);

/// Maps one lint row to an engine diagnostic — the oracle's shape: the
/// code's registry-default severity (INFO for lints, WARNING for the
/// dependency-validator family), the rule name as the code, the row's
/// UTF-16 span, the official problem/correction texts verbatim.
fn row(lint: Lint, file: FileId) -> Diagnostic {
    let code = CODES[lint.rule.0 as usize];
    Diagnostic::new(
        code.dtype.severity(),
        code,
        lint.message,
        file,
        SourceRange {
            offset: lint.range.start,
            length: lint.range.end - lint.range.start,
        },
        lint.correction,
    )
}

/// The unit's language-feature bits from its settled feature set: every bit
/// reads the engine-derived [`FeatureSet`] — generated from the vendored
/// `experimental_features.yaml`, so each bit's `enabledIn` milestone decides
/// it (`unnamed_libraries` 2.19, `patterns` 3.0, `wildcard_variables` 3.7).
/// `enhanced_parts` has no `enabledIn` (experimental, unshipped), so no
/// version enables it — the same posture the bridge's fixed parse surface
/// holds. The engine's settled set carries the `@dart` override and the
/// allowed-experiment grants (#722), so a `@dart=2.19` library inside a 3.x
/// package derives `patterns: false` here where the package version alone
/// would open them.
#[must_use]
pub(crate) fn features_of(set: FeatureSet) -> Features {
    Features {
        wildcard_variables: set.is_enabled(Feature::WildcardVariables),
        enhanced_parts: set.is_enabled(Feature::EnhancedParts),
        unnamed_libraries: set.is_enabled(Feature::UnnamedLibraries),
        patterns: set.is_enabled(Feature::Patterns),
        primary_constructors: set.is_enabled(Feature::PrimaryConstructors),
        null_aware_elements: set.is_enabled(Feature::NullAwareElements),
        super_parameters: set.is_enabled(Feature::SuperParameters),
    }
}

/// The package-version fallback of [`features_of`]: the same derivation off
/// a bare language version — the posture for files the engine has not
/// settled (outside the analyzed set, or a face evicted under the memory
/// budget). A default (3.x) library therefore yields [`Features::default()`]
/// exactly; a pre-3.0 library drops `patterns` (the legacy duplicate
/// reporting, the wave9 pin face).
#[must_use]
pub(crate) fn features_for(version: LanguageVersion) -> Features {
    features_of(FeatureSet::for_language_version(version))
}

/// The semantic fold's scan posture: the lints bridge's fixed production
/// surface (bridge `SCAN_CONFIG`, Dart 3.15 — `>>>` and `augment` gated on),
/// so the face's token stream is the same one the rules dispatch over.
pub(crate) const SEMANTIC_SCAN_CONFIG: dartq_scanner::ScanConfig = dartq_scanner::ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

/// The semantic fold's parse posture: the lints bridge's fixed production
/// surface (bridge `PARSE_FEATURES`, Dart 3.15), for the same
/// span-consistency reason as [`SEMANTIC_SCAN_CONFIG`].
pub(crate) const SEMANTIC_PARSE_FEATURES: ParserFeatures = ParserFeatures {
    patterns_enabled: true,
    variance_enabled: false,
    primary_constructors_enabled: true,
    augmentations_enabled: true,
    enhanced_parts_enabled: false,
    dot_shorthands_enabled: true,
};

const _: () = assert!(
    SEMANTIC_SCAN_CONFIG.enable_triple_shift && SEMANTIC_SCAN_CONFIG.enable_augmentations,
    "the semantic fold's scan posture is the lint bridge's production surface \
     (both scanner gates on); the lint parse adopts the fold's token table, so \
     relaxing a gate here would parse a stream a fresh scan would never build",
);

/// Whether `set` enables the library type-use face's consumer
/// (`avoid_private_typedef_functions`, #1640). The face's mounted counts
/// fold every member unit's content, so the rows are sibling-dependent:
/// this gates both the per-job reduce and the memo key's generation half
/// (a sibling's edit re-keys this file's rows exactly as the fold-facing
/// rules' generation does).
fn type_use_rule_enabled(set: &EnableSet) -> bool {
    rule_by_name("avoid_private_typedef_functions").is_some_and(|id| set.is_enabled(id))
}

/// Whether `set` arms the `unnecessary_ignore` analyzer-half join (#2132):
/// the rule's own enable bit, a mount predicate separate from the fold's
/// ([`mounts_semantics`]). The join's substrate builds per citing file
/// inside the fan-out — the graph and element tables only, never the full
/// `Analysis` fold, which stays keyed to [`crate::lints`]' fold-consuming
/// rules — so a corpus enabling the rule without any fold consumer reaches
/// the analyzer half instead of the pre-#2132 dark silence.
fn unnecessary_ignore_enabled(set: &EnableSet) -> bool {
    rule_by_name("unnecessary_ignore").is_some_and(|id| set.is_enabled(id))
}

/// The semantic rows' library-context generation: a fold over the owning
/// library's member files' (content id, API signature) — the exact
/// substrate the per-file `Analysis` context declares into its graph and
/// element index. A sibling file's edit re-keys this file's memo entry even
/// though its own content id stood still (the widened half of the #1049
/// memo key). `None` — no live owning library, or a member without facts —
/// folds nothing, exactly when the face would be absent (`None`).
fn semantic_generation(
    engine: &Engine,
    file: FileId,
    members: &FxHashMap<LibraryId, Vec<FileId>>,
) -> Option<u64> {
    let members = members.get(&engine.library_of(file)?)?;
    let mut h = FxHasher::default();
    for &member in members {
        let facts = engine.file_facts(member)?;
        h.write(facts.content().as_bytes());
        h.write(facts.api_signature().as_bytes());
    }
    Some(h.finish())
}

/// The containing-package face one package root maps to, precomputed once
/// per run: the pub-aware rules' facts and the unit's *fallback* language
/// features — the settled face the engine holds supersedes them per file
/// (#722), so this derivation only answers for files outside the analyzed
/// set. The pubspec parse runs once per package root and only its names
/// are kept. `workspace: None` — an unreadable pubspec or a parse failure
/// — is the official no-pubspec posture: the package-aware rules stay
/// silent for those files. Files outside any package resolve to no entry:
/// the fan-out answers their fallback directly from the default version.
struct PackageFace {
    workspace: Option<WorkspaceFacts>,
    features: Features,
    /// The identity's language version the fallback features derive from —
    /// the version face the fan-out mounts where the engine holds no
    /// settled per-file version (#1053).
    version: LanguageVersion,
}

/// Builds the face for one resolved package identity: one pubspec read +
/// parse (cold-path I/O), the fallback features from the identity's
/// language version. Run once per package root by [`analyze_lints`]' serial
/// precompute.
fn package_face(identity: &PackageIdentity) -> PackageFace {
    let workspace = std::fs::read_to_string(identity.root.join("pubspec.yaml"))
        .ok()
        .and_then(|text| Pubspec::parse(&text).ok())
        .map(|spec| {
            let mut dependencies: FxHashSet<String> = spec.dependencies.keys().cloned().collect();
            dependencies.insert(spec.name.clone());
            WorkspaceFacts {
                own_package: spec.name,
                dependencies,
                dev_dependencies: spec.dev_dependencies.keys().cloned().collect(),
                root: identity.root.clone(),
                lib_dir: identity.lib_dir.clone(),
            }
        });
    let version = identity.language_version.map_or(
        DEFAULT_LANGUAGE_VERSION,
        dartq_pub::LanguageVersion::into_inner,
    );
    PackageFace {
        workspace,
        features: features_for(version),
        version,
    }
}

/// The per-file lint-row memo (#772): a daemon turn's lint stage answers
/// every unchanged file from this table instead of re-parsing and
/// re-running the rules — one [`analyze_lints`] call over a 1000-file
/// workspace with one dirty file does one file's work, not a thousand's.
///
/// The key carries everything the rows read (the engine's `FactsKey`
/// discipline): the content hash plus one folded fingerprint
/// over the features, the enable set, the containing-package facts, and
/// the file's path. Structurally path-free — no `PathBuf` in the key —
/// but the path folds in because the rows are not path-independent:
/// `file_names` reads the basename and the pub-aware rules gate on the
/// path's directory class (`depend_on_referenced_packages`'
/// `isInPublicDir`, `implementation_imports`' `lib/` gate), so a
/// content-only key would alias `lib/a.dart` against `test/a.dart`. The
/// same fold covers the package names the pub rules read and separates
/// the session's contexts for free (one table per session is sound
/// because a context differs only through these folded inputs).
///
/// Determinism contract: the rule set is pure over the unit — the same
/// content, features, enable set, package facts, and path make [`run`]
/// yield the same rows — so a hit re-stamps byte-identically to a
/// recompute. Rows store pre-file-id-stamping (file-relative UTF-16
/// spans); the id stamps at emission (`row`), exactly as a fresh run's
/// does. The memo lives in-process only (a `Session` field, or a CLI
/// run's throwaway), so a binary update can never serve stale rows.
///
/// Bounded at `LINT_MEMO_CAP` entries with oldest-first eviction (the
/// engine's facts-cache idiom), and the RSS cost is bounded with it: a
/// row is a rule id, a UTF-16 range, and two short owned strings, and a
/// clean workspace's rows are empty, so the table holds well under a
/// megabyte on clean workspaces and at most a few megabytes on
/// lint-dense ones.
///
/// The semantic fold widens the key: a file whose enabled set includes a
/// fold-facing rule runs the per-file `Analysis` over its owning library's
/// context, so its fingerprint folds a library-context generation
/// (`semantic_generation` — the member files' content ids and API
/// signatures). A sibling edit re-keys the file even though its own content
/// id stood still. Files without fold-facing rules fold no generation: their
/// rows cannot read the context, so a sibling edit must not churn them.
#[derive(Default)]
pub struct LintMemo {
    entries: FxHashMap<LintMemoKey, Arc<Vec<Lint>>>,
    /// Insertion order, for oldest-first eviction.
    order: VecDeque<LintMemoKey>,
}

/// The memo key: the content identity plus one fingerprint over every
/// other row input ([`LintMemo`]), including the semantic library-context
/// generation where the enabled set runs fold-facing rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LintMemoKey {
    content: ContentId,
    fingerprint: u64,
}

/// The memo's entry cap. Generous against real workspaces (a daemon
/// session rarely exceeds a few thousand resident files) while the
/// eviction keeps a pathological one bounded.
const LINT_MEMO_CAP: usize = 1024;

impl LintMemo {
    /// The rows stored under `key`, or `None` on a miss.
    fn get(&self, key: &LintMemoKey) -> Option<Arc<Vec<Lint>>> {
        self.entries.get(key).cloned()
    }

    /// Inserts one file's rows with oldest-first cap eviction (the
    /// engine's `cache_facts` idiom): at the cap the oldest entry
    /// yields, re-inserting a live key evicts nothing, and live `Arc`
    /// clones elsewhere keep serving their rows.
    fn insert(&mut self, key: LintMemoKey, rows: Arc<Vec<Lint>>) {
        if self.entries.len() >= LINT_MEMO_CAP && !self.entries.contains_key(&key) {
            while self.entries.len() >= LINT_MEMO_CAP {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.entries.remove(&oldest);
            }
        }
        if self.entries.insert(key, rows).is_none() {
            self.order.push_back(key);
        }
    }
}

/// Derives one file's memo key: the content id plus one `FxHash` fold
/// over every other input the rows read — the feature bits, the settled
/// language version (the version-gated registrations consult it), the
/// enable set (enabled rule ids in registry order), the containing
/// package's facts, and the file's path — plus the semantic library-context
/// generation (`Some` only where the set runs fold-facing rules; `None` folds
/// a single discriminant byte and no generation, so faceless files never
/// churn on it) and the breaking-version gate byte. The set folds are
/// commutative (wrapping add of per-entry hashes) because `FxHashSet`
/// iteration order is not stable, and every variable-length field ends in
/// a separator byte so concatenations cannot collide across field
/// boundaries.
#[expect(
    clippy::too_many_arguments,
    reason = "the key threads every rows input the caller precomputed; a \
              struct would bundle what the two call sites read piecemeal"
)]
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn memo_key(
    content: ContentId,
    features: Features,
    version: LanguageVersion,
    set: &EnableSet,
    workspace: Option<&WorkspaceFacts>,
    path: &Path,
    unignorable: &FxHashSet<String>,
    generation: Option<u64>,
    breaking: bool,
) -> LintMemoKey {
    let mut h = FxHasher::default();
    let mut bits = 0u8;
    if features.wildcard_variables {
        bits |= 1;
    }
    if features.enhanced_parts {
        bits |= 1 << 1;
    }
    if features.unnamed_libraries {
        bits |= 1 << 2;
    }
    if features.patterns {
        bits |= 1 << 3;
    }
    h.write_u8(bits);
    h.write_u32(version.major);
    h.write_u32(version.minor);
    for (index, _) in RULES.iter().enumerate() {
        if set.is_enabled(RuleId(index as u16)) {
            h.write_u16(index as u16);
        }
    }
    match workspace {
        None => h.write_u8(0),
        Some(facts) => {
            h.write_u8(1);
            fold_str(&mut h, &facts.own_package);
            h.write_u64(fold_names(&facts.dependencies));
            h.write_u64(fold_names(&facts.dev_dependencies));
            fold_path(&mut h, &facts.root);
            fold_path(&mut h, &facts.lib_dir);
        }
    }
    // The path folds in every context, not only the package-attributed one:
    // `file_names` reads the basename and the directory class rules gate on
    // it with no facts attached just the same.
    fold_path(&mut h, path);
    // The `cannot-ignore` set: it gates the suppression face, so its
    // membership is part of the rows' identity (order-independent fold —
    // the set is a hash set).
    h.write_u64(fold_strs(unignorable));
    // The semantic library-context generation, behind a discriminant byte:
    // `None` (no fold-facing rules, or no context) must key identically
    // regardless of any generation value, while each `Some` generation is
    // its own identity.
    match generation {
        None => h.write_u8(0),
        Some(generation) => {
            h.write_u8(1);
            h.write_u64(generation);
        }
    }
    // The breaking-version gate byte (`remove_deprecations_in_breaking_
    // versions`): the face arms on the containing package's pubspec
    // `version:`, so a version crossing the `x.0.0`/`0.x.0` boundary
    // re-keys the file.
    h.write_u8(u8::from(breaking));
    LintMemoKey {
        content,
        fingerprint: h.finish(),
    }
}

/// The breaking-version gate: the rule enabled and the containing
/// package's pubspec `version:` a breaking release — the exact condition
/// under which the per-file job extends its rows with the annotation face
/// (and the key folds the gate byte). The rule check runs first: a
/// disabled rule never makes a version fact row-relevant.
fn breaking_gate(set: &EnableSet, pubspec_version: Option<&str>) -> bool {
    rule_by_name("remove_deprecations_in_breaking_versions").is_some_and(|id| set.is_enabled(id))
        && pubspec_version.is_some_and(is_breaking_version)
}

/// The containing package's pubspec `version:` value (the cooked scalar),
/// the breaking-version gate's fact. A capture miss — no pubspec file in
/// the engine's table, no content, or a non-UTF-8 body — is `None`, the
/// official no-pubspec posture.
fn pubspec_version(engine: &Engine, root: &Path) -> Option<String> {
    let file = engine
        .file_table()
        .try_file_id(&root.join("pubspec.yaml"))?;
    let bytes = engine.content(file)?;
    let text = std::str::from_utf8(bytes).ok()?;
    scan_pubspec(text).version.map(|scalar| scalar.value)
}

/// Folds one string set order-independently ([`fold_names`] over strings).
fn fold_strs(values: &FxHashSet<String>) -> u64 {
    let mut sum = 0u64;
    for value in values {
        let mut h = FxHasher::default();
        h.write(value.as_bytes());
        sum = sum.wrapping_add(h.finish());
    }
    sum
}

/// Folds one string with a trailing separator byte.
fn fold_str(h: &mut FxHasher, value: &str) {
    h.write(value.as_bytes());
    h.write_u8(0xff);
}

/// Folds one path's raw bytes with the same separator discipline.
fn fold_path(h: &mut FxHasher, path: &Path) {
    h.write(path.as_os_str().as_encoded_bytes());
    h.write_u8(0xff);
}

/// Folds a name set order-independently.
fn fold_names(names: &FxHashSet<String>) -> u64 {
    let mut sum = 0u64;
    for name in names {
        let mut h = FxHasher::default();
        h.write(name.as_bytes());
        sum = sum.wrapping_add(h.finish());
    }
    sum
}

/// The analyze pipeline's lint stage (#355 activation): every resident
/// file's rows — one transient parse + rule run per file the caller's
/// [`LintMemo`] cannot answer, an `Arc` clone per hit (#772; the bridge's
/// zero-persistent-engine-memory ruling stands — nothing enters engine
/// residency) — plus the pubspec pass over the workspace's `pubspec.yaml`
/// files (#434).
///
/// Per-subtree enable sets: each file runs under the options entry its own
/// ancestry governs (`governing_options`) — the same deepest-governing
/// resolution the engine's severity and exclude faces apply, so a nested
/// options file's `linter.rules` section refines its subtree here too.
/// Sets are resolved once per distinct file directory. An empty resolved
/// set skips the file without parsing (opt-in cost ruling). Files that are
/// no longer resident or are not UTF-8 are skipped — the engine's own pass
/// already reported nothing for them.
///
/// The fan-out is parallel (#765): the per-directory enable sets and the
/// per-package-root faces (`package_face`, one pubspec parse per
/// package) are precomputed serially into read-only maps — the cold-path
/// options-ancestry walks and pubspec reads happen once, deterministically,
/// before any job starts — then `rayon` runs one independent job per file
/// over `&Engine` reads only (the lock-free content store, the settled
/// per-file face, the package table) and the layer's `LazyLock` statics;
/// no lock is added. Each job's rows land in a slot indexed by its
/// position in the sorted file list, so the reassembly replays the serial
/// emission order exactly: the file order, the per-file rule traversal
/// order, and the Dart-rows-before-pubspec-rows order hold by
/// construction, and the faces' single stable display sort downstream
/// stays tie-identical to a serial run.
///
/// The semantic-fold substrate is a per-library shared artifact, built
/// once per multi-member needing library in its own fan-out before the
/// per-file one: every linted member of such a library mounts the
/// identical graph/namespaces/index tables (each job's own scan and
/// analysis stay job-local), so the library pays its substrate build once
/// instead of once per member; a lone-member library builds inline as
/// before — sharing one member's tables through a map would only add a
/// pass.
///
/// The package-aware rules read the containing package's facts through
/// `Unit.workspace` and the unit's features from the engine's settled
/// per-file face (#722) — the `@dart` override and grants already folded
/// in — falling back to the containing package's language version where no
/// face is settled. The fold-consuming rules
/// ([`dartq_lints::SEMANTIC_RULES`]) read the per-file
/// [`dartq_semantic::Analysis`] the job builds before the unit — the
/// owning library's shared context (declarations and element models, no
/// import edges; one build per library), mounted through
/// [`dartq_lints::Unit::run_with_semantics`] — and the same fold's
/// `AsyncContextUse` facts mount as `Unit::async_context_uses` for
/// `use_build_context_synchronously` (#1198). The engine's reachability
/// map (built at the end of every analyze, #1199) joins the capture rows'
/// name-token offsets into `Unit::reachability` for
/// `unreachable_from_main`. The library type-use seam (#1640) folds
/// [`dartq_lints::type_use_counts`] across the owning library's
/// registration-order units into `Unit::library_type_uses` for
/// `avoid_private_typedef_functions`. The pubspec pass runs after
/// the file fan-out: every `pubspec.yaml` the engine captured — the root
/// package's, the workspace members', and the nested package boundaries'
/// — through the same enable-set resolution, the scanned
/// facts, and the YAML `# ignore:` variant (`YamlIgnores`).
///
/// The memo (`memo`, #772) turns a daemon turn's cost from O(workspace)
/// into O(dirty): before any fan-out, the serial partition derives each
/// file's `memo_key` (hash work only — no parse, no rule run) and
/// answers hits by cloning their rows' `Arc`. Only misses fan out — a
/// rayon job cannot borrow the memo `&mut`, so jobs read `&Engine` and
/// statics only, and the keyed misses insert back serially after the
/// collect. A hit's rows re-stamp byte-identically (the [`LintMemo`]
/// determinism contract).
///
/// # Panics
///
/// Panics when a resident file's directory lacks its precomputed enable /
/// cannot-ignore set — an internal invariant breach (the serial precompute
/// covers every library file's directory).
#[must_use]
#[expect(clippy::too_many_lines)] // serial precompute then per-file row emit; the phases share locally scoped state
pub fn analyze_lints(engine: &Engine, roots: &[PathBuf], memo: &mut LintMemo) -> Vec<Diagnostic> {
    let mut files: Vec<FileId> = engine
        .libraries()
        .into_iter()
        .flat_map(|(_, files)| files)
        .collect();
    files.sort_unstable();
    files.dedup();

    // Serial precompute of the fan-out's shared read-only state: one enable
    // set per distinct file directory (governance is by ancestry, so one
    // directory always maps to one entry) and one package face per package
    // root. Both resolve through cold-path I/O — the options-ancestry walk
    // and the pubspec read — deterministic per run, so hoisting them here
    // keeps the parallel phase free of shared mutation and byte-identical
    // to a serial pass.
    let mut enabled: FxHashMap<PathBuf, EnableSet> = FxHashMap::default();
    let mut unignorable: FxHashMap<PathBuf, Arc<FxHashSet<String>>> = FxHashMap::default();
    let mut faces: FxHashMap<PathBuf, Arc<PackageFace>> = FxHashMap::default();
    // The breaking-version gate's fact, one pubspec scan per package root:
    // the cooked `version:` value, `None` when the package carries no
    // readable one (the official registration bail — no pubspec, or no
    // version field).
    let mut pubspec_versions: FxHashMap<PathBuf, Option<Arc<str>>> = FxHashMap::default();
    // The semantic face's per-directory strictness modes (the same
    // governing entry the enable set reads, all-off without one — the
    // official defaults) and the library membership the semantic run
    // declares into its per-file context.
    let mut stricts: FxHashMap<PathBuf, StrictModes> = FxHashMap::default();
    let library_members: FxHashMap<LibraryId, Vec<FileId>> =
        engine.libraries().into_iter().collect();
    for file in &files {
        let Some(path) = engine.file_table().path(*file) else {
            continue;
        };
        let dir = path.parent().unwrap_or(Path::new(""));
        // One governing-options walk per distinct directory feeds all three
        // faces (the enable set, the cannot-ignore set, and the strict modes
        // read the same entry); a directory already seen skips the
        // ancestry stat walk outright.
        if !enabled.contains_key(dir) {
            let options = governing_options(engine, dir, roots);
            enabled.insert(
                dir.to_path_buf(),
                enable_set(options.map(|options| options.lint_rules.as_slice())),
            );
            unignorable.insert(
                dir.to_path_buf(),
                Arc::new(
                    options
                        .map(|options| options.unignorable().clone())
                        .unwrap_or_default(),
                ),
            );
            stricts.insert(
                dir.to_path_buf(),
                StrictModes {
                    casts: options
                        .and_then(|options| options.strict_casts)
                        .unwrap_or(false),
                    inference: options
                        .and_then(|options| options.strict_inference)
                        .unwrap_or(false),
                    raw_types: options
                        .and_then(|options| options.strict_raw_types)
                        .unwrap_or(false),
                },
            );
        }
        if let Some(identity) = engine.packages().containing(path) {
            faces
                .entry(identity.root.clone())
                .or_insert_with(|| Arc::new(package_face(identity)));
            pubspec_versions
                .entry(identity.root.clone())
                .or_insert_with(|| pubspec_version(engine, &identity.root).map(Arc::from));
        }
    }

    // The serial memo partition: keyed hits lift their rows outright,
    // misses queue for the fan-out, and rowless shapes (no path, or an
    // empty enable set — the opt-in cost ruling) contribute nothing
    // exactly as their jobs always did. A file whose facts are absent is
    // linted all the same — it just carries no content id to key with.
    let mut slotted: Vec<Option<Arc<Vec<Lint>>>> = vec![None; files.len()];
    let mut misses: Vec<(usize, FileId, Option<LintMemoKey>)> = Vec::new();
    // The libraries whose misses mount the semantic fold and carry two or
    // more members: one shared substrate is built per such library below,
    // so every linted member answers from one graph/namespaces/index
    // build instead of each file rebuilding the identical tables.
    let mut needing: FxHashSet<LibraryId> = FxHashSet::default();
    for (slot, file) in files.iter().enumerate() {
        let Some(path) = engine.file_table().path(*file) else {
            continue;
        };
        let dir = path.parent().unwrap_or(Path::new(""));
        let set = enabled
            .get(dir)
            .expect("enable set precomputed for every resident file's directory");
        if *set == EnableSet::none() {
            continue;
        }
        let (workspace, fallback, fallback_version) = match engine
            .packages()
            .containing(path)
            .and_then(|identity| faces.get(&identity.root))
        {
            Some(face) => (face.workspace.as_ref(), face.features, face.version),
            None => (
                None,
                features_for(DEFAULT_LANGUAGE_VERSION),
                DEFAULT_LANGUAGE_VERSION,
            ),
        };
        let features = engine.settled_features(*file).map_or(fallback, features_of);
        let version = engine.settled_version(*file).unwrap_or(fallback_version);
        let Some(unit_facts) = engine.file_facts(*file) else {
            if mounts_semantics(set)
                && let Some(library) = engine.library_of(*file)
                && library_members
                    .get(&library)
                    .is_some_and(|members| members.len() > 1)
            {
                needing.insert(library);
            }
            misses.push((slot, *file, None));
            continue;
        };
        let pubspec_version = engine
            .packages()
            .containing(path)
            .and_then(|identity| pubspec_versions.get(&identity.root))
            .cloned()
            .flatten();
        let breaking = breaking_gate(set, pubspec_version.as_deref());
        let key = memo_key(
            unit_facts.content(),
            features,
            version,
            set,
            workspace,
            path,
            unignorable
                .get(dir)
                .expect("cannot-ignore set precomputed for every resident file's directory"),
            // The semantic fold's generation half: folded only where the set
            // makes rows sibling-dependent — the fold-facing rules read the
            // library context (the query face and the async-gap facts
            // alike), the library type-use face (#1640) folds the member
            // units' contents, and the `unnecessary_ignore` analyzer half
            // (#2132) re-folds over the same member-derived substrate. The
            // citation probe that would narrow that last case needs a scan
            // this parse-free partition doesn't pay, so the rule's enable
            // bit keys the generation half — over-keying, the safe
            // direction. A sibling's edit re-keys this file's rows in every
            // mounted case.
            if mounts_semantics(set)
                || type_use_rule_enabled(set)
                || unnecessary_ignore_enabled(set)
            {
                semantic_generation(engine, *file, &library_members)
            } else {
                None
            },
            breaking,
        );
        match memo.get(&key) {
            Some(rows) => slotted[slot] = Some(rows),
            None => {
                // Sharing pays from two members up: a lone-member library
                // builds its substrate inline in the job, exactly as
                // before, and never enters the shared map.
                if mounts_semantics(set)
                    && let Some(library) = engine.library_of(*file)
                    && library_members
                        .get(&library)
                        .is_some_and(|members| members.len() > 1)
                {
                    needing.insert(library);
                }
                misses.push((slot, *file, Some(key)));
            }
        }
    }

    // The shared substrates, one per multi-member needing library, built
    // in a per-library fan-out ahead of the per-file one — the pipeline's
    // shared-artifact shape (the same pure member-set fold the per-file
    // jobs used to repeat once per member). Members carry a deterministic
    // order, so each library's build is byte-identical to the standalone
    // per-file build it replaces.
    let artifacts: FxHashMap<LibraryId, LibrarySemantics> = needing
        .into_par_iter()
        .filter_map(|library| {
            let members = library_members.get(&library)?;
            Some((
                library,
                build_library_semantics(engine, members[0], members),
            ))
        })
        .collect();

    // One independent job per miss: `&Engine` reads and `LazyLock` statics
    // only, the rows pre-stamping (file-relative). The slot index rides
    // along so the reassembly replays the serial loop's emission order
    // exactly.
    let computed: Vec<(usize, Vec<Lint>)> = misses
        .par_iter()
        .map(|&(slot, file, _)| {
            let path = engine
                .file_table()
                .path(file)
                .expect("the partition held this file's path");
            let Some(bytes) = engine.content(file) else {
                return (slot, Vec::new());
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                return (slot, Vec::new());
            };
            let dir = path.parent().unwrap_or(Path::new(""));
            let set = enabled
                .get(dir)
                .expect("enable set precomputed for every resident file's directory");
            let (workspace, fallback, fallback_version) = match engine
                .packages()
                .containing(path)
                .and_then(|identity| faces.get(&identity.root))
            {
                Some(face) => (face.workspace.as_ref(), face.features, face.version),
                None => (
                    None,
                    features_for(DEFAULT_LANGUAGE_VERSION),
                    DEFAULT_LANGUAGE_VERSION,
                ),
            };
            let features = engine.settled_features(file).map_or(fallback, features_of);
            // The owning library's member files — the semantic fold's
            // substrate and the reachability face's entry-scan reach.
            let members: &[FileId] = engine
                .library_of(file)
                .and_then(|library| library_members.get(&library))
                .map_or(&[], |members| members.as_slice());
            // The semantic fold runs before the unit is built: its context
            // tables are the owning library's shared substrate (built once
            // above, from two members up; a lone-member library builds the
            // identical tables inline), and the analysis borrowing them is
            // job-local. The flow-fact rows the async-gap seam mounts come
            // off the analysis (#1198). Only where the enabled set consumes
            // the fold — a fold-less file parses and runs as before.
            let context = if mounts_semantics(set) && !members.is_empty() {
                let strict = stricts
                    .get(dir)
                    .copied()
                    .expect("strict modes precomputed for every resident file's directory");
                let mounted = match engine
                    .library_of(file)
                    .and_then(|library| artifacts.get(&library))
                {
                    Some(shared) => semantic_context(text, file, shared),
                    None => build_semantic_context(engine, text, file, members),
                };
                Some((mounted, strict))
            } else {
                None
            };
            let analysis = context.as_ref().map(|(context, strict)| {
                // The modern `class-modifiers` posture throughout: the lint
                // driver folds units standalone, outside the analyze
                // fan-out's per-analyze posture build. The oracle's own
                // posture for a modern package is the same.
                let posture = LibraryClassModifiers::default();
                analyze_unit(
                    text,
                    &context.scanned.tokens,
                    SEMANTIC_PARSE_FEATURES,
                    engine.strings(),
                    file,
                    context.lib,
                    &context.graph,
                    &context.namespaces,
                    &context.index,
                    *strict,
                    &posture,
                )
            });
            // The async-gap seam (#1198): the fold's `AsyncContextUse` facts
            // mapped to the lint face's row shape — the usage span is the
            // report anchor; the origin and gap fields are substrate-
            // internal. Collected only where the rule runs; unmounted or
            // empty, the shell is silent either way.
            let async_rows: Vec<AsyncContextUse> =
                if rule_by_name("use_build_context_synchronously")
                    .is_some_and(|id| set.is_enabled(id))
                {
                    analysis
                        .iter()
                        .flat_map(|analysis| analysis.facts())
                        .filter_map(|fact| match fact {
                            FlowFact::AsyncContextUse {
                                span,
                                guarded,
                                unrelated_mounted,
                                ..
                            } => Some(AsyncContextUse {
                                range: span.range.offset..span.range.offset + span.range.length,
                                guarded: *guarded,
                                unrelated_mounted: *unrelated_mounted,
                            }),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            // The reachability seam (#1199): the engine's per-declaration
            // flags joined with the capture rows' name-token offsets, built
            // only where the rule runs. `None` — the rule off — keeps the
            // shell silent exactly as an absent map does.
            let reachability =
                if rule_by_name("unreachable_from_main").is_some_and(|id| set.is_enabled(id)) {
                    engine
                        .reachability()
                        .map(|map| reachability_inputs(engine, file, members, &map))
                } else {
                    None
                };
            // The fold's scan is the parse's scan: the context scanned these
            // exact job-local bytes with the bridge's production posture
            // ([`SEMANTIC_SCAN_CONFIG`], const-pinned to it above), so the
            // parse adopts a copy of that table instead of rescanning. The
            // context keeps its own table — the fold faces read the raw
            // pre-parse stream (`analyze_unit` above, the `unnecessary_ignore`
            // neutralized re-fold below), never the parse's unified one.
            let parsed = match &context {
                Some((context, _)) => {
                    parse_unit_with_tokens(text, context.scanned.tokens.clone(), features)
                }
                None => parse_unit(text, features),
            };
            // The library type-use seam (#1640): the owning library's
            // named-type-row counts by name lexeme, folded across the
            // registration-order units — defining first, then parts in
            // directive order, the set `Analysis::library_units` publishes
            // and `members` carries. Built only where the rule runs;
            // unmounted keeps the rule silent. This unit's rows come off
            // the parse above (no second scan); each sibling parses once
            // and drops after its count — the keys borrow the engine's
            // bytes, never the transient parses.
            let library_type_uses = if type_use_rule_enabled(set) {
                let mut uses = type_use_counts(text, &parsed.ast, &parsed.tokens);
                for &member in members.iter().filter(|&&m| m != file) {
                    let Some(bytes) = engine.content(member) else {
                        continue;
                    };
                    let Ok(sibling) = std::str::from_utf8(bytes) else {
                        continue;
                    };
                    let sibling_unit = parse_unit(sibling, features);
                    for (lexeme, count) in
                        type_use_counts(sibling, &sibling_unit.ast, &sibling_unit.tokens)
                    {
                        *uses.entry(lexeme).or_insert(0) += count;
                    }
                }
                Some(uses)
            } else {
                None
            };
            let mut unit = parsed.unit(Some(path));
            unit.workspace = workspace;
            // The settled language version mounts by assignment like the
            // workspace face (#1053): the version-gated rule registrations
            // consult it, falling back to the package/default derivation
            // where the engine holds no settled face.
            unit.version = Some(engine.settled_version(file).unwrap_or(fallback_version));
            // The two consumption seams mount by assignment after the call
            // (the [`ParsedUnit::unit`] shape): a flag join the shell reads
            // per candidate, and the fold's async-gap verdict rows.
            if let Some((has_entry, flags)) = &reachability {
                unit.reachability = Some(ReachabilityFace {
                    has_entry: *has_entry,
                    flags,
                });
            }
            if !async_rows.is_empty() {
                unit.async_context_uses = Some(&async_rows);
            }
            // The type-use mount (#1640): the fold's map is answer-sized
            // and mounts by borrow, exactly the reachability/async seams'
            // shape.
            if let Some(uses) = &library_type_uses {
                unit.library_type_uses = Some(uses);
            }
            let cannot_ignore = unignorable
                .get(dir)
                .expect("cannot-ignore set precomputed for every resident file's directory");
            let mut rows = match &analysis {
                Some(analysis) => unit.run_with_semantics(set, cannot_ignore, Some(analysis)),
                None => run(&unit, set, cannot_ignore),
            };
            // The breaking-version face joins after the dispatch walk: the
            // rule registers no kinds, so its rows are the per-unit epilogue
            // over the metadata rows (the gate rides the key above).
            let pubspec_version = engine
                .packages()
                .containing(path)
                .and_then(|identity| pubspec_versions.get(&identity.root))
                .cloned()
                .flatten();
            // The late-join tail (#1904): the run's epilogue cannot answer
            // the late-join names — their rows join here or never. The
            // breaking walk returns the rows its ignore gate dropped; the
            // join's late half re-runs over those drops (empty when the
            // gate kept the walk off — the rule registers only on a
            // breaking version upstream, so the citation is genuinely
            // unused and reports). A citation that dropped a late row
            // stays silent; one that dropped nothing reports — the
            // official single pass over the unfiltered final list
            // (library_analyzer.dart:327-341).
            let (late_rows, late_drops) = if breaking_gate(set, pubspec_version.as_deref()) {
                let fact = pubspec_version.expect("a breaking gate implies a version fact");
                run_breaking_version_annotations(&unit, set, cannot_ignore, &fact)
            } else {
                (Vec::new(), Vec::new())
            };
            rows.extend(late_rows);
            rows.extend(unnecessary_ignore_late_rows(
                &unit,
                set,
                cannot_ignore,
                &late_drops,
            ));
            // The analyzer-layer half (#2116): citations naming analyzer
            // diagnostic codes answer against the rows the semantic layer
            // produces for this file — the official match pass consumes a
            // citation for every reported diagnostic, lint and analyzer row
            // alike (ignore_validator.dart:137-144). The fold's own ignore
            // gate strips suppressed rows before its report returns, so the
            // consumption facts come from one neutralized re-fold: the
            // `ignore` keywords rewrite to same-length filler inside comment
            // bytes only, the fold sees no suppression entries, and its rows
            // are the pre-filter production set. Gated on the rule (#2132 —
            // the mount predicate is the rule's enable bit, not the fold's:
            // a corpus enabling it with no fold-consuming rule reaches this
            // half instead of the pre-#2132 dark silence, the under-report
            // direction — never a false row) and on the probe: only a file
            // citing a known diagnostic code pays the substrate and the
            // fold.
            if unnecessary_ignore_enabled(set) {
                let comments = Comments::extract(text, &parsed.tokens);
                let ignore = IgnoreInfo::parse(text, &comments);
                let entries = ignore.name_entries(text);
                // The citation match (ignore_info.dart:39-52) admits a name
                // OR a uniqueName spelling — a uniqueName citation can
                // suppress a diagnostic, so the fold is paid either way
                // (#2131/#2136). The unnecessary report filter itself stays
                // name-only (ignore_validator.dart:208-213).
                if entries.iter().any(|entry| {
                    is_known_diagnostic_code(&entry.name)
                        || canonical_diagnostic_name(&entry.name).is_some()
                }) {
                    // The substrate (#2132): the shell's mount when a
                    // fold-consuming rule needs the Analysis anyway, a
                    // per-hit build otherwise — the probe gate above keeps
                    // the tables to the files whose citations can answer,
                    // and the full `Analysis` fold never runs for this face
                    // alone.
                    let join_context;
                    let (context, strict) = match &context {
                        Some(mounted) => (&mounted.0, mounted.1),
                        None => {
                            // The library's shared substrate when one was
                            // built, the standalone build otherwise (a
                            // library no semantic-mount miss needed) — the
                            // same tables either way.
                            join_context = match engine
                                .library_of(file)
                                .and_then(|library| artifacts.get(&library))
                            {
                                Some(shared) => semantic_context(text, file, shared),
                                None => build_semantic_context(engine, text, file, members),
                            };
                            let join_strict = stricts.get(dir).copied().expect(
                                "strict modes precomputed for every resident file's directory",
                            );
                            (&join_context, join_strict)
                        }
                    };
                    let neutral = neutralize_ignore_keywords(text, &comments);
                    let posture = LibraryClassModifiers::default();
                    let fold = fold_unit(
                        &neutral,
                        &context.scanned.tokens,
                        SEMANTIC_PARSE_FEATURES,
                        engine.strings(),
                        file,
                        context.lib,
                        &context.graph,
                        &context.namespaces,
                        &context.index,
                        strict,
                        &posture,
                        true,
                    );
                    let mut suppressed: Vec<(u32, &'static str)> = Vec::new();
                    for diagnostic in fold
                        .report
                        .diagnostics
                        .iter()
                        .chain(fold.parse_diagnostics.iter())
                    {
                        let name = diagnostic.code.name;
                        // Suppression match = name OR uniqueName
                        // (ignore_info.dart:39-52): a citation of a
                        // uniqueName suppresses the diagnostic reported
                        // under its shared name, and the record keeps the
                        // canonical name (#2136).
                        if entries.iter().any(|entry| {
                            entry.name.eq_ignore_ascii_case(name)
                                || canonical_diagnostic_name(&entry.name)
                                    .is_some_and(|c| c.eq_ignore_ascii_case(name))
                        }) {
                            let (line, _) =
                                parsed.lines.line_col_utf16(text, diagnostic.range.offset);
                            suppressed.push((line, name));
                        }
                    }
                    rows.extend(unnecessary_ignore_analyzer_rows(
                        &unit,
                        set,
                        cannot_ignore,
                        &suppressed,
                        is_known_diagnostic_code,
                        canonical_diagnostic_name,
                    ));
                }
            }
            (slot, rows)
        })
        .collect();

    // Serial insert + reassembly: keyed misses join the memo, then the
    // slots replay the file order, the per-file rule traversal order, and
    // the Dart-rows-before-pubspec-rows order — each row's file id stamps
    // at this emission, hits and fresh rows alike.
    for ((_, rows), &(slot, _, key)) in computed.into_iter().zip(misses.iter()) {
        let rows = Arc::new(rows);
        if let Some(key) = key {
            memo.insert(key, Arc::clone(&rows));
        }
        slotted[slot] = Some(rows);
    }
    let mut out = Vec::new();
    for (slot, rows) in slotted.into_iter().enumerate() {
        let Some(rows) = rows else {
            continue;
        };
        let file = files[slot];
        out.extend(rows.iter().map(|lint| row(lint.clone(), file)));
    }
    out.extend(unignorable_rows(engine, &files, &unignorable));
    out.extend(pubspec_rows(engine, roots));
    out
}

/// One copy of `text` with every `ignore` keyword inside a comment lexeme
/// rewritten to same-length filler — the neutralized source the #2116
/// re-fold reads. The semantic fold's ignore gate parses suppression
/// entries straight from source lines, so blanking the keyword (wherever it
/// occurs inside a comment — a superset of the gate's standalone-line
/// grammar) leaves the fold with an empty suppression table and its rows
/// come back pre-filter. Comment byte ranges only (each comment carries a
/// byte span): string literals and code bytes never move, offsets and line
/// structure are preserved, and the swap writes ASCII over ASCII.
fn neutralize_ignore_keywords(text: &str, comments: &Comments) -> String {
    const KEYWORD: &[u8] = b"ignore";
    let mut bytes = text.as_bytes().to_vec();
    for comment in comments.all() {
        let start = comment.range.start as usize;
        let end = comment.range.end as usize;
        let mut cursor = start;
        while cursor + KEYWORD.len() <= end {
            if let Some(found) = bytes[cursor..end]
                .windows(KEYWORD.len())
                .position(|window| window == KEYWORD)
            {
                let at = cursor + found;
                bytes[at..at + KEYWORD.len()].fill(b'x');
                cursor = at + KEYWORD.len();
            } else {
                break;
            }
        }
    }
    // ASCII-for-ASCII swaps over valid UTF-8: the bytes stay valid UTF-8.
    String::from_utf8(bytes).expect("same-length ASCII substitution keeps UTF-8")
}

/// The library's prefixed imports in the per-file lint graph: every
/// `import … as p` member directive registers its prefix (the official
/// `PrefixScope`) with a target node seeded from the imported surface —
/// a resident file's declarations, or the generated SDK face of a `dart:`
/// library. An unresolvable target still registers the prefix with an
/// empty table, so `p.x` answers "prefix" and a miss reports. Without the
/// edges `[p.X]` doc references never see `p` and all report.
fn seed_prefixed_imports(
    engine: &Engine,
    builder: &mut LibraryGraphBuilder<'_>,
    lib: dartq_resolution::LibraryId,
    members: &[FileId],
    core_file: FileId,
) {
    let strings = engine.strings();
    for &member in members {
        let Some(facts) = engine.file_facts(member) else {
            continue;
        };
        for directive in &facts.directives {
            if directive.kind != dartq_engine::DirectiveKind::Import || directive.prefix.is_none() {
                continue;
            }
            let target = match (&directive.target, directive.uri.as_deref()) {
                (dartq_resolution::ResolvedTarget::Resolved(file), _) => {
                    engine.file_facts(*file).map(|target_facts| {
                        let node = builder.intern(&format!("lint_import:{}", file.index()));
                        for (name, declaration) in target_facts.elements.declarations() {
                            if declaration.kind != MicroKind::TypeParam {
                                builder.declare(node, name, *file);
                            }
                        }
                        node
                    })
                }
                (dartq_resolution::ResolvedTarget::External, Some(uri)) => uri
                    .strip_prefix("dart:")
                    .and_then(|short| {
                        dartq_engine::sdk_faces_gen::SDK_LIBRARY_FACES
                            .iter()
                            .find(|(face, _)| *face == short)
                    })
                    .map(|(_, names)| {
                        let node = builder.intern(uri);
                        for name in *names {
                            builder.declare(node, strings.intern(name), core_file);
                        }
                        node
                    }),
                _ => None,
            };
            builder.import(
                lib,
                dartq_resolution::ImportEdge {
                    target,
                    prefix: directive.prefix,
                    deferred: directive.deferred,
                    combinators: directive.combinators.clone(),
                },
            );
        }
    }
}

/// One library's semantic-fold substrate, built once per library and
/// shared by every linted member: the library's isolated context (its
/// member files' declarations declared into a [`LibraryGraphBuilder`]
/// graph — the class family with its modifier facts, exactly the
/// analyze-built graph's declaration posture — plus the members' element
/// models in the [`ElementIndex`]) and the library node the declarations
/// declared under. Every field is a pure function of the member set and
/// the engine's facts, so one build answers all members identically —
/// the pipeline's per-library shared-artifact shape (the retired
/// per-file rebuild paid the identical build once per linted member).
pub(crate) struct LibrarySemantics {
    lib: dartq_resolution::LibraryId,
    graph: Arc<LibraryGraph>,
    namespaces: Arc<Namespaces>,
    index: Arc<ElementIndex>,
}

/// One file's semantic-fold mount: its owning library's shared
/// [`LibrarySemantics`] plus the file's own scan. The tables are shared
/// read-only across the library's jobs ([`analyze_unit`] and the fold
/// borrow them for the analysis' lifetime), so the bundle outlives the
/// analysis and the rule run — the job holds the binding straight
/// through the run.
pub(crate) struct SemanticContext {
    pub(crate) lib: dartq_resolution::LibraryId,
    pub(crate) graph: Arc<LibraryGraph>,
    pub(crate) namespaces: Arc<Namespaces>,
    pub(crate) index: Arc<ElementIndex>,
    pub(crate) scanned: dartq_scanner::ScanResult,
}

/// Builds one library's shared substrate: the owning library alone,
/// never import edges — cross-library faces under-report `None` — the M1
/// posture (a query never fabricates a binding the context did not
/// decide), not a divergence from the semantic tier's answers about *this*
/// library's own declarations.
fn build_library_semantics(engine: &Engine, file: FileId, members: &[FileId]) -> LibrarySemantics {
    let strings = engine.strings();
    let mut builder = LibraryGraphBuilder::new(strings);
    // The library node's identity is internal to this per-library graph; the
    // defining file's path spells it deterministically.
    let node_uri = engine
        .file_table()
        .path(members.first().copied().unwrap_or(file))
        .map_or_else(
            || "lint_semantic.dart".to_owned(),
            |path| path.to_string_lossy().into_owned(),
        );
    let lib = builder.intern(&node_uri);
    for &member in members {
        let Some(facts) = engine.file_facts(member) else {
            continue;
        };
        for (name, declaration) in facts.elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            match facts.elements.container(name).map(container_modifiers) {
                Some(modifiers) => builder.declare_class(lib, name, member, modifiers),
                None => builder.declare(lib, name, member),
            }
        }
    }
    // The implicit dart:core import the oracle adds to every library: the
    // generated core face (own surface plus the async/collection
    // re-exports) declared at the engine's synthetic `<dart:core>` file
    // when a graph build already interned it, the analyzed file otherwise
    // (both stay outside the per-library index, so SDK names resolve
    // "defined" with no element model).
    let core_file = engine
        .file_table()
        .try_file_id(Path::new("<dart:core>"))
        .unwrap_or(file);
    let (_, core_face) = dartq_engine::sdk_faces_gen::SDK_LIBRARY_FACES
        .iter()
        .find(|(uri, _)| *uri == "core")
        .expect("generated SDK table carries dart:core");
    builder.seed_implicit_core(lib, core_file, core_face);
    seed_prefixed_imports(engine, &mut builder, lib, members, core_file);
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    for &member in members {
        if let Some(facts) = engine.file_facts(member) {
            index.insert(member, Arc::clone(&facts.elements));
        }
    }
    LibrarySemantics {
        lib,
        graph: Arc::new(graph),
        namespaces: Arc::new(namespaces),
        index: Arc::new(index),
    }
}

/// Mounts one file's fold substrate: the library's shared tables by
/// reference plus the file's own scan of the exact bytes the job
/// analyzes.
fn semantic_context(text: &str, file: FileId, shared: &LibrarySemantics) -> SemanticContext {
    SemanticContext {
        lib: shared.lib,
        graph: Arc::clone(&shared.graph),
        namespaces: Arc::clone(&shared.namespaces),
        index: Arc::clone(&shared.index),
        scanned: dartq_scanner::scan(text, SEMANTIC_SCAN_CONFIG, file),
    }
}

/// Builds one file's semantic-fold substrate standalone: the owning
/// library's substrate built inline plus the file's scan. The one-off
/// face for callers outside the lint fan-out (the fix evidence and
/// assist mounts); the fan-out itself shares one
/// [`LibrarySemantics`] per library.
pub(crate) fn build_semantic_context(
    engine: &Engine,
    text: &str,
    file: FileId,
    members: &[FileId],
) -> SemanticContext {
    let shared = build_library_semantics(engine, file, members);
    semantic_context(text, file, &shared)
}

/// Joins the engine's reachability flags (#1199) with the file's capture
/// rows — the [`ReachabilityFace`] inputs: whether the owning library
/// declares an entry point, and this file's `(name-token UTF-16 offset,
/// reachable)` flags in the face's ascending offset order. One pass over
/// the capture (each row carries both its name-token offset and its bit
/// index, so no offset→row search exists or is needed) plus one sort.
///
/// `has_entry` reads the map itself: the pass seeds exactly the entry
/// verdicts (`main`, the `vm:entry-point` pragma, `@Preview` — the
/// resolution and annotation checks the engine owns) and marks nothing
/// reachable before propagation, so a reachable row in any member file is
/// precisely "an entry point exists". Probing captures for the three
/// shapes here would re-derive those checks in the wrong layer. A member
/// without a capture or a map entry contributes nothing — the
/// conservative no-entry direction.
#[expect(clippy::cast_possible_truncation)] // capture row counts are u32-scale (the map's bit-index width)
fn reachability_inputs(
    engine: &Engine,
    file: FileId,
    members: &[FileId],
    map: &Reachability,
) -> (bool, Vec<(u32, bool)>) {
    let has_entry = members.iter().any(|&member| {
        engine.file_capture(member).is_some_and(|capture| {
            (0..capture.index.decls().len())
                .any(|row| map.is_reachable(member, row as u32) == Some(true))
        })
    });
    let Some(capture) = engine.file_capture(file) else {
        return (has_entry, Vec::new());
    };
    // A row the map cannot answer (a stale row index against a recaptured
    // file) stays out of the face: the shell reads an absent offset as
    // "outside the universe" and never reports it.
    let mut flags: Vec<(u32, bool)> = capture
        .index
        .decls()
        .iter()
        .enumerate()
        .filter_map(|(row, decl)| {
            map.is_reachable(file, row as u32)
                .map(|reachable| (decl.range.offset, reachable))
        })
        .collect();
    // `decls` sorts by name, not offset; the face binary-searches offsets.
    flags.sort_unstable_by_key(|&(offset, _)| offset);
    (has_entry, flags)
}

/// The `IgnoreValidator` rows (#938): one `unignorable_ignore` per
/// ignore-comment name a governing `cannot-ignore` set claims. The face
/// runs outside the memo over (content, set) — the same inputs the memo key
/// folds, so a memoized file's rows stay identical to a fresh run's — and
/// the scan-only derivation (comments, never an AST) rides the same
/// fan-out. Any-empty-set short-circuits before any scan: the dominant
/// workspace pays one map scan per pass.
fn unignorable_rows(
    engine: &Engine,
    files: &[FileId],
    unignorable: &FxHashMap<PathBuf, Arc<FxHashSet<String>>>,
) -> Vec<Diagnostic> {
    if !unignorable.values().any(|set| !set.is_empty()) {
        return Vec::new();
    }
    let hits: Vec<(FileId, Vec<UnignorableIgnore>)> = files
        .par_iter()
        .filter_map(|file| {
            let path = engine.file_table().path(*file)?;
            let set = unignorable.get(path.parent().unwrap_or(Path::new("")))?;
            let bytes = engine.content(*file)?;
            let text = std::str::from_utf8(bytes).ok()?;
            let hits = unignorable_ignores(text, set);
            (!hits.is_empty()).then_some((*file, hits))
        })
        .collect();
    hits.into_iter()
        .flat_map(|(file, hits)| {
            hits.into_iter().map(move |hit| {
                Diagnostic::new(
                    UNIGNORABLE_IGNORE.dtype.severity(),
                    UNIGNORABLE_IGNORE,
                    format!("The diagnostic '{}' can't be ignored.", hit.name),
                    file,
                    SourceRange {
                        offset: hit.offset,
                        length: hit.length,
                    },
                    Some(
                        "Try removing the name from the list, or removing the whole comment \
                         if this is the only name in the list."
                            .to_owned(),
                    ),
                )
            })
        })
        .collect()
}

/// The pubspec pass (#434): every `pubspec.yaml` the engine captured at
/// analyze time — the root package's, every workspace member's, and every
/// nested package boundary's ([`Engine::captured_pubspecs`]; a
/// folder-local `pubspec.yaml` under a package root is a package
/// boundary, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/workspace/pub.dart` `findPackageFor` :341-351,
/// and the oracle's driver lints every pubspec under the analysis roots)
/// — under the same enable-set resolution the Dart files get. The
/// dependency-validator family (`path_not_posix`, `path_does_not_exist`,
/// `path_pubspec_does_not_exist`) and the screenshots `path` checks emit
/// from the engine's validator face (`Engine::analyze`'s
/// `validator_issue_rows`), not here: this pass owns only the three
/// enable-set-gated lints. A listed family name in `linter.rules` cannot
/// double-report — the specs register `kinds: &[]`, so the AST run stays
/// silent and the validator face is the family's only emission path. Rows
/// carry the official value-node span (UTF-16 over the pubspec text) and
/// the `IgnoreInfo.forYaml` suppression face.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
fn pubspec_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let dirs: Vec<PathBuf> = engine
        .captured_pubspecs()
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();

    let mut out = Vec::new();
    for dir in dirs {
        let path = dir.join("pubspec.yaml");
        let Some(file) = engine.file_table().try_file_id(&path) else {
            continue;
        };
        let Some(bytes) = engine.content(file) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let set = enable_set(
            governing_options(engine, &dir, roots).map(|options| options.lint_rules.as_slice()),
        );
        let urls_rule = rule_by_name("secure_pubspec_urls");
        let names_rule = rule_by_name("package_names");
        let sort_rule = rule_by_name("sort_pub_dependencies");
        let urls_on = urls_rule.is_some_and(|id| set.is_enabled(id));
        let names_on = names_rule.is_some_and(|id| set.is_enabled(id));
        let sort_on = sort_rule.is_some_and(|id| set.is_enabled(id));
        if !urls_on && !names_on && !sort_on {
            continue;
        }
        let facts = scan_pubspec(text);
        let ignores = YamlIgnores::parse(text);
        let lines = LineIndex::new(text);
        let mut rows = Vec::new();
        if urls_on {
            let rule = urls_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            for Scalar { range, value } in facts.urls.iter().chain(&facts.dep_urls) {
                let mut hits = Vec::new();
                check_url(range.clone(), value, &mut hits);
                rows.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(saturate_slot_u32(hit.range.start));
                    (!ignores.ignored(line, "secure_pubspec_urls")).then(|| Lint {
                        rule,
                        range: utf16_range(&lines, text, &hit.range),
                        message: spec.render_problem(&[hit.scheme]),
                        correction: spec.render_correction(&[]),
                    })
                }));
            }
        }
        if names_on {
            let rule = names_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            if let Some(name) = &facts.name {
                let mut hits = Vec::new();
                check_name(name.range.clone(), &name.value, &mut hits);
                rows.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(saturate_slot_u32(hit.range.start));
                    (!ignores.ignored(line, "package_names")).then(|| Lint {
                        rule,
                        range: utf16_range(&lines, text, &hit.range),
                        message: spec.render_problem(&[&hit.name]),
                        correction: spec.render_correction(&[]),
                    })
                }));
            }
        }
        if sort_on {
            let rule = sort_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            let sections = dep_sections(text);
            for entries in [
                &sections.dependencies,
                &sections.dev_dependencies,
                &sections.dependency_overrides,
            ] {
                let Some(range) = check_section(entries) else {
                    continue;
                };
                let (line, _) = lines.byte_to_line_col(saturate_slot_u32(range.start));
                if ignores.ignored(line, "sort_pub_dependencies") {
                    continue;
                }
                rows.push(Lint {
                    rule,
                    range: utf16_range(&lines, text, &range),
                    message: spec.render_problem(&[]),
                    correction: spec.render_correction(&[]),
                });
            }
        }
        out.extend(rows.into_iter().map(|lint| row(lint, file)));
    }
    out
}

/// The mounting face's feed for `sort_pub_dependencies`: each dependency
/// section's named entries as `(name span, name text)` pairs in document
/// order. The judgment mirrors `pubspec_facts::scan`'s section face — a
/// section opens only in block form at the top level, the first indented
/// line fixes the entry indent, a later line at that indent is the next
/// entry — and a name span is the raw key extent (quotes included, the
/// `YamlNode.span` face). List items carry no name and are dropped: the
/// analyzer model leaves them unnamed, so they neither report nor advance
/// the sort walk.
struct DepSections<'text> {
    dependencies: Vec<(core::ops::Range<usize>, &'text str)>,
    dev_dependencies: Vec<(core::ops::Range<usize>, &'text str)>,
    dependency_overrides: Vec<(core::ops::Range<usize>, &'text str)>,
}

#[must_use]
fn dep_sections(text: &str) -> DepSections<'_> {
    let mut sections = DepSections {
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
        dependency_overrides: Vec::new(),
    };
    let mut current: Option<u8> = None;
    let mut entry_indent: Option<usize> = None;
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent..];
        let line_start = offset;
        offset += raw.len();
        // Blank and comment-only lines carry no keys.
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let Some(colon) = key_colon(body) else {
            // A colonless line (a bare list item, a stray scalar) has no
            // key and no name.
            continue;
        };
        let key = body[..colon].trim();
        if indent == 0 {
            // Block form only: an inline value is a flow section the scan
            // face also leaves alone; any other top-level key closes the
            // current section.
            current = match key {
                "dependencies" => Some(0),
                "dev_dependencies" => Some(1),
                "dependency_overrides" => Some(2),
                _ => None,
            }
            .filter(|_| body[colon + 1..].trim().is_empty());
            entry_indent = None;
            continue;
        }
        let Some(index) = current else {
            continue;
        };
        let at = *entry_indent.get_or_insert(indent);
        if indent != at || key.starts_with("- ") || key == "-" {
            // Deeper lines are the entry's nested spec, shallower ones the
            // section's end, and a list entry has no name.
            continue;
        }
        let lead = body[..colon].len() - body[..colon].trim_start().len();
        let start = line_start + indent + lead;
        let entry = (start..start + key.len(), unquoted(key));
        match index {
            0 => sections.dependencies.push(entry),
            1 => sections.dev_dependencies.push(entry),
            _ => sections.dependency_overrides.push(entry),
        }
    }
    sections
}

/// The first `:` of `body` that terminates a key (followed by a space or
/// end of line — the `http://` colons inside values never are), the same
/// judgment `pubspec_facts::scan` applies.
fn key_colon(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    (0..bytes.len()).find(|&i| bytes[i] == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' '))
}

/// One key's comparison text: one layer of matching quotes stripped — the
/// cooked text the YAML model compares (the span keeps the raw extent).
fn unquoted(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    let quoted = matches!(bytes.first(), Some(b'\'' | b'"'))
        && bytes.len() >= 2
        && bytes[bytes.len() - 1] == bytes[0];
    if quoted { &raw[1..raw.len() - 1] } else { raw }
}

/// A pubspec row's byte span in UTF-16 (the engine's diagnostic
/// coordinate).
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn utf16_range(
    lines: &LineIndex,
    text: &str,
    range: &core::ops::Range<usize>,
) -> core::ops::Range<u32> {
    lines.byte_to_utf16(text, range.start as u32)..lines.byte_to_utf16(text, range.end as u32)
}

/// The options entry governing a file in `dir`: the nearest
/// `analysis_options.yaml` at or above `dir`
/// ([`AnalysisOptions::find_for_dir`]) names the governing directory, and
/// the engine's options table is keyed by exactly those directories — the
/// package roots plus every nested options directory the workspace
/// assembly registered ([`AnalysisOptions::for_roots`]). An options file
/// above the package root (the repository-wide shape) carries no key of
/// its own; its entry is the containing root's, which loaded it. `None` —
/// no options file in the ancestry — is the empty (no-lints) posture.
/// `pub(crate)`: the `removed_lint_use` face (#1005) resolves the same
/// per-file governing entry for its `unnecessary_ignore` gate.
pub(crate) fn governing_options<'a>(
    engine: &'a Engine,
    dir: &Path,
    roots: &[PathBuf],
) -> Option<&'a AnalysisOptions> {
    let options_file = AnalysisOptions::find_for_dir(dir)?;
    engine
        .options_for_root(options_file.parent()?)
        .or_else(|| engine.options_for_root(containing_root(dir, roots)))
}

/// The longest `roots` entry that is a byte-prefix directory of `dir`, or
/// the first root (single-root workspaces are the dominant shape).
fn containing_root<'a>(dir: &Path, roots: &'a [PathBuf]) -> &'a Path {
    let mut best: Option<&Path> = None;
    for root in roots {
        if dir.starts_with(root)
            && best.is_none_or(|longest| root.components().count() > longest.components().count())
        {
            best = Some(root);
        }
    }
    best.unwrap_or(
        roots
            .first()
            .map_or_else(|| Path::new(""), PathBuf::as_path),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_diagnostics::{DiagnosticType, FileLayoutProvider as _, Severity};
    use dartq_engine::Engine;
    use dartq_engine::analysis_options::AnalysisOptions;
    use dartq_lints::rule_by_name;
    use dartq_pub::PackageConfig;
    use dartq_source::FileTable;

    use super::*;

    /// The no-options default is the official empty posture: nothing is
    /// enabled, whatever the registry carries (`context_builder.dart`
    /// :225-232 — the default `AnalysisOptionsImpl` lints are empty).
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn absent_options_default_to_no_lints() {
        let set = enable_set(None);
        assert_eq!(set, EnableSet::none(), "no options file, no lints");
        for index in 0..RULES.len() {
            assert!(
                !set.is_enabled(RuleId(index as u16)),
                "the default posture enables nothing"
            );
        }
    }

    /// A present options file replaces the default: exactly the listed
    /// names resolve, nothing else.
    #[test]
    fn explicit_options_replace_the_default() {
        let set = enable_set(Some(&["avoid_empty_else".to_owned()]));
        assert!(set.is_enabled(rule_by_name("avoid_empty_else").expect("wired")));
        assert!(!set.is_enabled(rule_by_name("unnecessary_new").expect("wired")));
    }

    /// The options-file path end to end: the real `linter.rules` merge
    /// feeds [`enable_set`] — the list spelling enables exactly its names,
    /// the `name: false` spelling silences the rule.
    #[test]
    fn options_file_rules_drive_the_enable_set() {
        let root = crate::workspace::test_fixture::workspace("lints_options", &[]);
        let packages = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("fixture config");
        let config_dir = root.join(".dart_tool");
        let load = |yaml: &str| {
            let path = root.join("analysis_options.yaml");
            std::fs::write(&path, yaml).expect("options file");
            AnalysisOptions::load(&path, &packages, &config_dir)
        };

        let enabled = enable_set(Some(
            &load("linter:\n  rules:\n    - avoid_empty_else\n").lint_rules,
        ));
        assert!(enabled.is_enabled(rule_by_name("avoid_empty_else").expect("wired")));
        assert!(!enabled.is_enabled(rule_by_name("unnecessary_new").expect("wired")));

        let disabled = enable_set(Some(
            &load("linter:\n  rules:\n    avoid_empty_else: false\n").lint_rules,
        ));
        assert!(
            !disabled.is_enabled(rule_by_name("avoid_empty_else").expect("wired")),
            "the disable spelling silences the rule"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Rows map to the oracle's lint shape: INFO severity (the registry's
    /// single `DiagnosticType::Lint` derivation), the rule name and its
    /// UPPERCASE machine form as an unpublished `LINT` code, the UTF-16
    /// span, and the official texts verbatim — with and without a
    /// correction.
    #[test]
    fn lint_rows_map_to_oracle_diagnostics() {
        let mut files = FileTable::default();
        let file = files.intern(Path::new("t.dart"));
        let rule = rule_by_name("unnecessary_new").expect("wired");

        let corrected = row(
            Lint {
                rule,
                range: 19..22,
                message: "Unnecessary 'new' keyword.".to_owned(),
                correction: Some("Try removing the 'new' keyword.".to_owned()),
            },
            file,
        );
        assert_eq!(corrected.severity, Severity::Info);
        assert_eq!(corrected.severity, DiagnosticType::Lint.severity());
        assert_eq!(corrected.code.name, "unnecessary_new");
        assert_eq!(corrected.code.machine, "UNNECESSARY_NEW");
        assert_eq!(corrected.code.dtype, DiagnosticType::Lint);
        assert!(!corrected.code.has_published_docs);
        assert_eq!(corrected.message, "Unnecessary 'new' keyword.");
        assert_eq!(corrected.file, file);
        assert_eq!(corrected.range.offset, 19);
        assert_eq!(corrected.range.length, 3);
        assert_eq!(
            corrected.correction.as_deref(),
            Some("Try removing the 'new' keyword.")
        );

        let bare = row(
            Lint {
                rule,
                range: 0..0,
                message: "Unnecessary 'new' keyword.".to_owned(),
                correction: None,
            },
            file,
        );
        assert_eq!(bare.correction, None);
        assert_eq!(bare.range.length, 0);
    }

    /// The #355 activation end to end: a fixture workspace whose governing
    /// options enable `unnecessary_new` and whose only library carries the
    /// shape produces an INFO lint row through the full pipeline — engine
    /// pass, transient re-parse, row mapping, oracle order — with the
    /// semantic row of the same file. (The options file is the fixture's
    /// governing posture: options-less enables nothing, #936.)
    #[test]
    fn analyze_lints_rows_flow_through_the_resident_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_activation",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_new\n",
                ),
                ("lib/bad.dart", "var x = new Object();\nvar ok = unknown;\n"),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, Severity, u32)> = run
            .diagnostics
            .iter()
            .map(|d| (d.code.name.to_owned(), d.severity, d.range.offset))
            .collect();
        assert!(
            rows.contains(&("unnecessary_new".to_owned(), Severity::Info, 8)),
            "the lint row rides the pipeline: {rows:?}"
        );
        assert!(
            rows.contains(&("undefined_identifier".to_owned(), Severity::Error, 31)),
            "the semantic row still flows: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The #2169 corpus-shape pin: the engine run's fold resolves a bare
    /// `print` through the implicit dart:core face and arms the synthetic
    /// SDK binding row at the callee — the shape the unit `run_semantic`
    /// substrate never arms (its fold answers `None` at the same site, so
    /// the rule pins stayed green while the driver went silent). The rule
    /// must read that row as the core symbol (`is_sdk_binding`, never a
    /// user shadow) and report; the options gate the run; a local `print`
    /// declaration still shadows.
    #[test]
    fn avoid_print_faces_over_the_engine_sdk_binding() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_avoid_print_sdk_face",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    avoid_print: true\n",
                ),
                ("lib/fires.dart", "void f() {\n  print('x');\n}\n"),
                (
                    "lib/shadows.dart",
                    "void g() {\n  void print(Object? o) {}\n  print('local');\n}\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "avoid_print")
            .map(|d| {
                let path = engine
                    .file_table()
                    .path(d.file)
                    .map_or_else(String::new, |path| path.display().to_string());
                (path, d.range.offset)
            })
            .collect();
        assert_eq!(rows.len(), 1, "avoid_print rows: {rows:?}");
        assert!(rows[0].0.ends_with("lib/fires.dart"), "{rows:?}");
        assert_eq!(rows[0].1, 13, "the `print` callee token");
        std::fs::remove_dir_all(&root).ok();

        // The same shape without the rule name: the corpus stays silent.
        let root = crate::workspace::test_fixture::workspace(
            "lints_avoid_print_unenabled",
            &[("lib/t.dart", "void f() {\n  print('x');\n}\n")],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics.iter().all(|d| d.code.name != "avoid_print"),
            "the unenabled corpus stays silent: {:?}",
            run.diagnostics
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Drives the full pipeline over a fixture workspace and returns the
    /// `unnecessary_ignore` rows as (file path, offset) pairs — the shared
    /// harness of the #2132 gating pins. The `files` slice carries the
    /// options entry, exactly as [`crate::workspace::test_fixture::workspace`]
    /// takes it.
    fn unnecessary_ignore_rows_for(tag: &str, files: &[(&str, &str)]) -> Vec<(String, u32)> {
        let root = crate::workspace::test_fixture::workspace(tag, files);
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unnecessary_ignore")
            .map(|d| {
                let path = engine
                    .file_table()
                    .path(d.file)
                    .map_or_else(String::new, |path| path.display().to_string());
                (path, d.range.offset)
            })
            .collect();
        std::fs::remove_dir_all(&root).ok();
        rows
    }

    /// The #2132 gate: a corpus enabling `unnecessary_ignore` with no
    /// fold-consuming rule reaches the analyzer half. An unused citation of
    /// a known diagnostic code reports at the name token (the pre-#2132
    /// posture left it dark); the same code, produced on the cited line,
    /// keeps the citation silent — the join's substrate builds per citing
    /// file and the full `Analysis` fold never runs.
    #[test]
    fn analyzer_half_reaches_fold_less_corpora() {
        let rows = unnecessary_ignore_rows_for(
            "lints_ignore_gate_foldless",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_ignore\n",
                ),
                (
                    "lib/cites.dart",
                    "// ignore: undefined_identifier\nvoid f() {}\n",
                ),
                (
                    "lib/uses.dart",
                    "void f() {\n  // ignore: undefined_identifier\n  var x = unknown;\n}\n",
                ),
            ],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].0.ends_with("lib/cites.dart"), "{:?}", rows[0].0);
        assert_eq!(rows[0].1, 11, "the name token after `// ignore: `");
    }

    /// The fold-mounted posture is unchanged (#2132's invariant): with a
    /// fold-consuming rule also enabled the same citation faces answer
    /// identically — the join takes the shell's mounted substrate and the
    /// verdicts hold.
    #[test]
    fn fold_mounted_corpus_keeps_the_analyzer_half_verdicts() {
        let rows = unnecessary_ignore_rows_for(
            "lints_ignore_gate_mounted",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_ignore\n    - avoid_print\n",
                ),
                (
                    "lib/cites.dart",
                    "// ignore: undefined_identifier\nvoid f() {}\n",
                ),
                (
                    "lib/uses.dart",
                    "void f() {\n  // ignore: undefined_identifier\n  var x = unknown;\n}\n",
                ),
            ],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].0.ends_with("lib/cites.dart"), "{:?}", rows[0].0);
        assert_eq!(rows[0].1, 11);
    }

    /// The #2136 join (#2131's deferred consumer): a `uniqueName` citation
    /// is now a recognized analyzer-code citation — the joined probe pays
    /// the fold for it — while the name-only report filter
    /// (ignore_validator.dart:208-213) keeps the entry silent: no
    /// `unnecessary_ignore` row either way, exactly as upstream
    /// (necessaryIgnore_uniqueName fixtures). Fold-less corpus.
    #[test]
    fn unique_name_citation_stays_silent_fold_less() {
        let rows = unnecessary_ignore_rows_for(
            "lints_ignore_gate_uniquename",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_ignore\n",
                ),
                (
                    "lib/unique.dart",
                    "// ignore: invalid_null_aware_operator_after_short_circuit\nvoid f() {}\n",
                ),
                (
                    "lib/named.dart",
                    "// ignore: undefined_identifier\nvoid f() {}\n",
                ),
            ],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].0.ends_with("lib/named.dart"), "{:?}", rows[0].0);
    }

    /// Same #2136 verdict under the fold-mounted posture: the uniqueName
    /// citation stays silent and the unused name citation still reports —
    /// the join holds identically on the mounted substrate.
    #[test]
    fn unique_name_citation_stays_silent_fold_mounted() {
        let rows = unnecessary_ignore_rows_for(
            "lints_ignore_gate_uniquename_mounted",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_ignore\n    - avoid_print\n",
                ),
                (
                    "lib/unique.dart",
                    "// ignore: invalid_null_aware_operator_after_short_circuit\nvoid f() {}\n",
                ),
                (
                    "lib/named.dart",
                    "// ignore: undefined_identifier\nvoid f() {}\n",
                ),
            ],
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].0.ends_with("lib/named.dart"), "{:?}", rows[0].0);
    }

    /// The type-check mirror's differential face: the row emits always-on
    /// from the semantic tier's fold (the oracle's `BestPracticesVerifier`
    /// pass is not a lint the enable set gates — WARNING tier, the
    /// `STATIC_WARNING` derivation) — with no options file (zero lints
    /// enabled) and with the name explicitly enabled alike, exactly one
    /// row either way: the registry spec's AST arm is structurally off
    /// (`kinds: &[]`), so naming the rule adds no second, lint-tier row.
    #[test]
    fn unnecessary_type_check_reports_always_on_without_a_double() {
        const SOURCE: &str = "class int {}\nvoid f() {\n  1 is int;\n}\n";
        for (case, files) in [
            ("options-less", vec![("lib/main.dart", SOURCE)]),
            (
                "explicitly enabled",
                vec![
                    (
                        "analysis_options.yaml",
                        "linter:\n  rules:\n    - unnecessary_type_check\n",
                    ),
                    ("lib/main.dart", SOURCE),
                ],
            ),
        ] {
            let root = crate::workspace::test_fixture::workspace("lints_type_check_mirror", &files);
            let root = root.canonicalize().expect("canonical root");
            let (config, _) =
                crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
                .expect("run");
            let rows: Vec<(Severity, &str)> = run
                .diagnostics
                .iter()
                .filter(|d| d.code.name == "unnecessary_type_check")
                .map(|d| (d.severity, d.message.as_ref()))
                .collect();
            assert_eq!(
                rows,
                vec![(
                    Severity::Warning,
                    "Unnecessary type check; the result is always 'true'."
                )],
                "{case}: {rows:?}"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// The two W6 consumption seams fill in production (#1198/#1199): one
    /// fixture unit carrying `main` + a referenced + an unreferenced
    /// declaration and an async-gap `BuildContext` use, under options
    /// enabling both rules, reports through the full pipeline. The
    /// `unreachable_from_main` row lands exactly on the unreferenced
    /// declaration — the mounted face's flags (`stray` not reachable,
    /// `used`/`main`/`later` reachable) and its `has_entry` verdict in
    /// observable form, both seams being structurally silent when
    /// unmounted — and the `use_build_context_synchronously` row lands on
    /// the use behind the await gap: the fold's `AsyncContextUse` facts
    /// mapped onto the unit. An entry-less second library in the same
    /// workspace stays silent — the `has_entry` bail, not a flag miss.
    #[test]
    fn consumption_seams_fill_in_production() {
        const SEAM_SOURCE: &str = "\
Future<void> later(BuildContext context) async {
  await step;
  print(context);
}

void main() {
  later(ok);
  used;
}

void used() {}
void stray() {}
";
        let root = crate::workspace::test_fixture::workspace(
            "lints_seam_fill",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unreachable_from_main\n    - \
                     use_build_context_synchronously\n",
                ),
                ("lib/reach.dart", SEAM_SOURCE),
                ("lib/lonely.dart", "void lonely() {}\n"),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, Severity, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| {
                d.code.name == "unreachable_from_main"
                    || d.code.name == "use_build_context_synchronously"
            })
            .map(|d| (d.code.name.to_owned(), d.severity, d.range.offset))
            .collect();
        // The use behind the gap (`context`, line 3) and the unreachable
        // declaration (`stray`, line 12) — one verdict row each, nothing
        // more.
        assert!(
            rows.contains(&(
                "use_build_context_synchronously".to_owned(),
                Severity::Info,
                71
            )),
            "the async-gap use reports: {rows:?}"
        );
        assert!(
            rows.contains(&("unreachable_from_main".to_owned(), Severity::Info, 142)),
            "the unreachable declaration reports: {rows:?}"
        );
        assert_eq!(rows.len(), 2, "exactly the two verdict rows: {rows:?}");
        // The entry-less library's has_entry bail: its file carries no
        // unreachable row at all.
        let dead_files: Vec<String> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unreachable_from_main")
            .filter_map(|d| engine.file_table().path(d.file))
            .map(|path| {
                path.file_name()
                    .expect("named")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            dead_files,
            ["reach.dart"],
            "the entry-less library stays silent"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The version→features mapping is the vendored
    /// `experimental_features.yaml`'s `enabledIn` table: each bit's
    /// shipping milestone decides it (`unnamed_libraries` 2.19, `patterns`
    /// 3.0, `wildcard_variables` 3.7, `null_aware_elements` 3.8,
    /// `primary_constructors` 3.13) and `enhanced_parts` never (no
    /// `enabledIn`). The latest version yields `Features::default()`
    /// exactly; 3.7 predates the 3.8/3.13 bits; the wave9 pin (≤2.19)
    /// drops `patterns`.
    #[test]
    fn features_derive_from_the_language_version() {
        let v = |major: u32, minor: u32| LanguageVersion::new(major, minor);
        assert_eq!(features_for(v(3, 13)), Features::default());
        assert_eq!(
            features_for(v(3, 7)),
            Features {
                null_aware_elements: false,
                primary_constructors: false,
                ..Features::default()
            },
            "3.7 predates null-aware-elements' 3.8.0 and primary-constructors' 3.13.0 enabledIn",
        );
        let three_five = features_for(v(3, 5));
        assert!(three_five.patterns);
        assert!(!three_five.wildcard_variables);
        let two_nineteen = features_for(v(2, 19));
        assert!(!two_nineteen.patterns, "legacy duplicates report");
        assert!(two_nineteen.unnamed_libraries);
        assert!(!two_nineteen.wildcard_variables);
        let two_eighteen = features_for(v(2, 18));
        assert!(!two_eighteen.unnamed_libraries);
        assert!(!two_eighteen.patterns);
        assert!(
            !features_for(v(3, 13)).enhanced_parts,
            "unshipped: no version enables it"
        );
    }

    /// The per-unit version wiring end to end: a `languageVersion: "2.19"`
    /// package whose governing options enable `no_duplicate_case_values`
    /// reports the duplicate `case` values through the full pipeline (the
    /// legacy `SwitchCase` face), while the same shape under a 3.x package
    /// stays silent (the `patterns` posture the `AstBuilder` keys).
    #[test]
    fn legacy_language_version_reports_legacy_duplicates() {
        const DUPLICATE_CASES: &str =
            "void f(int x) {\n  switch (x) {\n    case 1:\n    case 1:\n      break;\n  }\n}\n";
        let run_codes = |version: &str| {
            let root = crate::workspace::test_fixture::workspace(
                "lints_version_face",
                &[
                    (
                        "analysis_options.yaml",
                        "linter:\n  rules:\n    - no_duplicate_case_values\n",
                    ),
                    ("lib/a.dart", DUPLICATE_CASES),
                ],
            );
            std::fs::write(
                root.join(".dart_tool/package_config.json"),
                format!(
                    r#"{{"configVersion": 2, "packages": [{{"name": "fixture", "rootUri": "../", "languageVersion": "{version}"}}], "generator": "pub"}}"#
                ),
            )
            .expect("config");
            let root = root.canonicalize().expect("canonical root");
            let (config, _) =
                crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
                .expect("run");
            let codes: Vec<String> = run
                .diagnostics
                .iter()
                .map(|d| d.code.name.to_string())
                .collect();
            std::fs::remove_dir_all(&root).ok();
            codes
        };
        assert!(
            run_codes("3.5")
                .iter()
                .all(|name| name != "no_duplicate_case_values"),
            "the 3.x posture is silent on legacy duplicates",
        );
        assert!(
            run_codes("2.19")
                .iter()
                .any(|name| name == "no_duplicate_case_values"),
            "the legacy posture reports them",
        );
    }

    /// The #720 seam regression pin: a `// @dart = 2.19` library inside a
    /// 3.x package whose governing options enable `no_duplicate_case_values`
    /// runs the lint face at the *pinned* posture — the duplicate `case`
    /// values report through the settled per-file face (#722) even though
    /// the package version alone would keep them silent.
    #[test]
    fn at_dart_override_closes_patterns_for_lint_face() {
        const DUPLICATE_CASES: &str =
            "void f(int x) {\n  switch (x) {\n    case 1:\n    case 1:\n      break;\n  }\n}\n";
        let codes = |pinned: bool| {
            let source = if pinned {
                format!("// @dart = 2.19\n{DUPLICATE_CASES}")
            } else {
                DUPLICATE_CASES.to_owned()
            };
            let root = crate::workspace::test_fixture::workspace(
                "lints_at_dart_face",
                &[
                    (
                        "analysis_options.yaml",
                        "linter:\n  rules:\n    - no_duplicate_case_values\n",
                    ),
                    ("lib/a.dart", &source),
                ],
            );
            std::fs::write(
                root.join(".dart_tool/package_config.json"),
                r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
            )
            .expect("config");
            let root = root.canonicalize().expect("canonical root");
            let (config, _) =
                crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
            let mut engine = Engine::new(config).expect("engine");
            let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
                .expect("run");
            let codes: Vec<String> = run
                .diagnostics
                .iter()
                .map(|d| d.code.name.to_string())
                .collect();
            std::fs::remove_dir_all(&root).ok();
            codes
        };
        assert!(
            codes(true)
                .iter()
                .any(|name| name == "no_duplicate_case_values"),
            "the pinned file runs the legacy posture inside the 3.x package",
        );
        assert!(
            codes(false)
                .iter()
                .all(|name| name != "no_duplicate_case_values"),
            "without the pin the package baseline stays silent",
        );
    }

    /// Nested options refine the enable set per subtree (#595): the root's
    /// `linter.rules` run outside the nested directory, the nested file's
    /// replace them inside it — the deepest-governing resolution the
    /// engine's severity and exclude faces already apply, now at the lint
    /// face too.
    #[test]
    fn nested_options_refine_the_subtree_enable_sets() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_nested",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - unnecessary_new\n",
                ),
                (
                    "lib/sub/analysis_options.yaml",
                    "linter:\n  rules:\n    - avoid_empty_else\n",
                ),
                ("lib/root_side.dart", "var x = new Object();\n"),
                (
                    "lib/sub/nested_side.dart",
                    "var y = new Object();\nvoid f(bool c) {\n  if (c) ; else ;\n}\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, String)> = run
            .diagnostics
            .iter()
            .map(|d| {
                (
                    run.layout.file_path(d.file).to_string_lossy().into_owned(),
                    d.code.name.to_owned(),
                )
            })
            .collect();
        let fires = |file: &str, code: &str| {
            rows.iter()
                .any(|(path, name)| path.ends_with(file) && name == code)
        };
        // Outside the subtree the root's set runs: `unnecessary_new` reports.
        assert!(fires("lib/root_side.dart", "unnecessary_new"), "{rows:?}");
        // Inside it the nested set replaces the root's: its own rule
        // reports and the root's does not, despite the same `new` shape.
        assert!(
            fires("lib/sub/nested_side.dart", "avoid_empty_else"),
            "{rows:?}"
        );
        assert!(
            !fires("lib/sub/nested_side.dart", "unnecessary_new"),
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The pubspec pass (#434) end to end: the governing options enable
    /// `secure_pubspec_urls`, the root `pubspec.yaml`'s insecure URLs
    /// report at their value spans (the official value-node anchor), the
    /// secure ones stay silent.
    #[test]
    fn pubspec_url_rows_report_at_the_value_span() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_urls",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - secure_pubspec_urls\n",
            )],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nhomepage: http://github.com/fixture\nrepository: https://github.com/fixture\nenvironment:\n  sdk: ^3.5.0\ndependency_overrides:\n  dep:\n    git:\n      url: git://github.com/x/y.git\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let mut spans: Vec<(u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "secure_pubspec_urls")
            .map(|d| (d.range.offset, d.range.offset + d.range.length))
            .collect();
        spans.sort_unstable();
        assert_eq!(spans, vec![(24, 49), (165, 189)], "{spans:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The dependency-validator family mounts always-on (from the
    /// engine's validator face, not this pass): no options file
    /// (the options-less posture enables zero lints) and the path rows
    /// still report — the oracle's `validatePathEntries` runs
    /// unconditionally — with the family's `STATIC_WARNING` severity, the
    /// missing folder and the pubspec-less folder each at their code, the
    /// populated folder silent, `dev_dependencies` walked, and the quoted
    /// value anchored at its raw span (quotes included) with the cooked
    /// scalar in the message. The POSIX gate fires first and returns
    /// (:51-58): the backslash path aimed at an existing, pubspec'd
    /// folder reports `path_not_posix` alone — no existence verdict for
    /// the same value.
    #[test]
    fn pubspec_path_family_reports_always_on() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_path", &[]);
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nenvironment:\n  sdk: ^3.5.0\ndependencies:\n  missing:\n    path: vendor/missing\n  bare:\n    path: vendor/bare\n  real:\n    path: vendor/real\ndev_dependencies:\n  quoted:\n    path: 'vendor/quoted'\n  windows:\n    path: 'vendor\\real'\n",
        )
        .expect("pubspec");
        std::fs::create_dir_all(root.join("vendor/bare")).expect("bare folder");
        std::fs::create_dir_all(root.join("vendor/quoted")).expect("quoted folder");
        std::fs::create_dir_all(root.join("vendor/real")).expect("real folder");
        std::fs::write(root.join("vendor/real/pubspec.yaml"), "name: real\n")
            .expect("nested pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let mut rows: Vec<(u32, u32, String, Severity, String)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name.starts_with("path_"))
            .map(|d| {
                (
                    d.range.offset,
                    d.range.offset + d.range.length,
                    d.code.name.to_owned(),
                    d.severity,
                    d.message.clone().into_owned(),
                )
            })
            .collect();
        rows.sort_unstable();
        assert_eq!(
            rows,
            vec![
                (
                    76,
                    90,
                    "path_does_not_exist".to_owned(),
                    Severity::Warning,
                    "The path 'vendor/missing' doesn't exist.".to_owned(),
                ),
                (
                    109,
                    120,
                    "path_pubspec_does_not_exist".to_owned(),
                    Severity::Warning,
                    "The directory 'vendor/bare' doesn't contain a pubspec.".to_owned(),
                ),
                (
                    189,
                    204,
                    "path_pubspec_does_not_exist".to_owned(),
                    Severity::Warning,
                    "The directory 'vendor/quoted' doesn't contain a pubspec.".to_owned(),
                ),
                (
                    226,
                    239,
                    "path_not_posix".to_owned(),
                    Severity::Warning,
                    "The path 'vendor\\real' isn't a POSIX-style path.".to_owned(),
                ),
            ],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// `package_names` over the same pass: the governing options enable
    /// the rule, a camel-case `name:` reports at its value span with the
    /// official argument fill. `fooBar` is a legal pub name, so the
    /// assembly needs the config's root entry renamed to match (a real
    /// `pub get` writes it).
    #[test]
    fn package_names_row_reports_the_camel_case_name() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_names",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - package_names\n",
            )],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fooBar\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fooBar", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
        .expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(u32, u32, String)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "package_names")
            .map(|d| {
                (
                    d.range.offset,
                    d.range.offset + d.range.length,
                    d.message.clone().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![(
                6,
                12,
                "The package name 'fooBar' isn't a lower_case_with_underscores identifier."
                    .to_owned()
            )],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The nested package boundary joins the pubspec pass: a folder-local
    /// `pubspec.yaml` under a package root is a package boundary (upstream
    /// Dart SDK source, `pkg/analyzer/lib/src/workspace/pub.dart`
    /// `findPackageFor` :341-351), so a config-listed subpackage the root
    /// pubspec does not declare as a workspace member still reports its
    /// rows — a mixed-case `name:` fires `package_names` keyed at
    /// `nested/pubspec.yaml` while the root's valid name stays silent.
    #[test]
    fn nested_package_pubspec_joins_the_pubspec_pass() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_nested_pubspec",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - package_names\n",
                ),
                ("lib/app.dart", "void main() {}\n"),
                (
                    "nested/pubspec.yaml",
                    "name: Nested_pack\nenvironment:\n  sdk: ^3.5.0\n",
                ),
                ("nested/lib/child.dart", "void main() {}\n"),
            ],
        );
        // The subpackage rides the resolution config (the corpus's shape:
        // a config package the root pubspec never declares as a workspace
        // member).
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "nested_pack", "rootUri": "../nested", "languageVersion": "3.5"}], "generator": "pub", "generatorVersion": "3.13.1"}"#,
        )
        .expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(bool, u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "package_names")
            .map(|d| {
                (
                    engine
                        .file_table()
                        .path(d.file)
                        .is_some_and(|path| path.ends_with("nested/pubspec.yaml")),
                    d.range.offset,
                    d.range.offset + d.range.length,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![(true, 6, 17)],
            "exactly the nested pubspec's name row: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The YAML-face suppression (`IgnoreInfo.forYaml`): with
    /// `secure_pubspec_urls` enabled by the governing options, a
    /// `# ignore:` line above the `homepage:` entry drops its row.
    #[test]
    fn pubspec_ignore_line_suppresses_the_url_row() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_ignore",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - secure_pubspec_urls\n",
            )],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\n# ignore: secure_pubspec_urls\nhomepage: http://a.dev\nrepository: https://b.dev\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "secure_pubspec_urls"),
            "the suppressed homepage row is gone: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The breaking-version mount (#1156) end to end: the governing options
    /// enable `remove_deprecations_in_breaking_versions`, the root
    /// pubspec's `version: 1.0.0` sits on the `x.0.0` breaking boundary,
    /// and `@deprecated` on a Dart declaration reports at its name
    /// identifier (the corpus `lint(1, 10)` face).
    #[test]
    fn breaking_version_annotation_reports_at_the_name() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_breaking_version",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - remove_deprecations_in_breaking_versions\n",
                ),
                ("lib/a.dart", "@deprecated foo {}\n"),
            ],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 1.0.0\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "remove_deprecations_in_breaking_versions")
            .map(|d| (d.range.offset, d.range.offset + d.range.length))
            .collect();
        assert_eq!(rows, vec![(1, 11)], "{rows:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The non-breaking posture stays silent: the same armed shape under
    /// `version: 2.1.0` — a minor increment off the boundary — reports
    /// nothing (the registration bail, upstream :36-48).
    #[test]
    fn non_breaking_version_keeps_the_annotations_silent() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_plain_version",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - remove_deprecations_in_breaking_versions\n",
                ),
                ("lib/a.dart", "@deprecated foo {}\n"),
            ],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 2.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "remove_deprecations_in_breaking_versions"),
            "the non-breaking package stays silent: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The package-aware directive rules ride the pipeline with the
    /// mounted facts: the governing options enable them, a foreign import
    /// and a foreign `src/` import report in `lib/`, the own package and
    /// the dev dependency stay silent where the official gates hold.
    #[test]
    fn package_aware_directive_rules_flow_through_the_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pub_facts",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - depend_on_referenced_packages\n    - implementation_imports\n",
                ),
                (
                    "lib/a.dart",
                    "import 'package:foreign/foreign.dart';\nimport 'package:foreign/src/impl.dart';\nimport 'package:fixture/src/own.dart';\n",
                ),
                (
                    "test/t.dart",
                    "import 'package:fixture_dev/dev.dart';\nimport 'package:foreign/src/impl.dart';\n",
                ),
            ],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\ndev_dependencies:\n  fixture_dev: ^1.0.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(String, String, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d.code.name,
                    "depend_on_referenced_packages" | "implementation_imports"
                )
            })
            .map(|d| {
                (
                    run.layout
                        .file_path(d.file)
                        .file_name()
                        .expect("name")
                        .to_string_lossy()
                        .into_owned(),
                    d.code.name.to_string(),
                    d.range.offset,
                )
            })
            .collect();
        // a.dart: the plain foreign import and the foreign `src/` import
        // both lack a dependency entry (the latter also reports as an
        // implementation import); the own package stays silent. t.dart:
        // the dev dependency is available outside the public dirs (no
        // row), `foreign` still is not, and the `src/` import is outside
        // the rule's `package:`-URI gate.
        //
        // The two a.dart rows at offset 46 tie through severity, path and
        // offset, so the oracle display order's final key decides
        // (`DiagnosticWithPath.compareTo`, dartdev analyze.dart:703-707 —
        // message text): "Import of a library…" < "The imported
        // package…".
        assert_eq!(
            rows,
            vec![
                (
                    "a.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    7
                ),
                ("a.dart".to_owned(), "implementation_imports".to_owned(), 46),
                (
                    "a.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    46
                ),
                (
                    "t.dart".to_owned(),
                    "depend_on_referenced_packages".to_owned(),
                    46
                ),
            ],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The memo's determinism pin (#772): a hit re-stamps byte-identical
    /// rows. The rule set is pure over the unit, so two independent runs
    /// over the same content, features, enable set, workspace, and path
    /// produce the same rows — and the memo round-trip (insert
    /// pre-stamping, get, stamp at emission) must reproduce the fresh
    /// run's diagnostics exactly.
    #[test]
    fn memo_hit_restamps_rows_identically() {
        let mut files = FileTable::default();
        let file = files.intern(Path::new("/w/lib/a.dart"));
        let path = Path::new("/w/lib/a.dart");
        let text = "var x = new Object;\n";
        let set = enable_set(Some(&["unnecessary_new".to_owned()]));
        let features = Features::default();
        let version = DEFAULT_LANGUAGE_VERSION;
        let content = dartq_source::content_hash(text.as_bytes());
        let rows = || {
            let parsed = parse_unit(text, features);
            let mut unit = parsed.unit(Some(path));
            unit.workspace = None;
            run(&unit, &set, &FxHashSet::default())
        };

        let fresh: Vec<Diagnostic> = rows().into_iter().map(|lint| row(lint, file)).collect();
        let mut memo = LintMemo::default();
        let key = memo_key(
            content,
            features,
            version,
            &set,
            None,
            path,
            &FxHashSet::default(),
            None,
            false,
        );
        memo.insert(key, Arc::new(rows()));
        let stamped: Vec<Diagnostic> = memo
            .get(&key)
            .expect("the inserted key hits")
            .iter()
            .map(|lint| row(lint.clone(), file))
            .collect();

        let shape = |rows: &[Diagnostic]| {
            rows.iter()
                .map(|d| {
                    (
                        d.severity,
                        d.code.name,
                        d.range.offset,
                        d.range.length,
                        d.message.clone(),
                        d.correction.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&stamped).len(), 1, "the unnecessary_new row flows");
        assert_eq!(shape(&stamped), shape(&fresh), "hit == recompute");
    }

    /// The key folds everything the rows read: the features, the settled
    /// language version, the enable set, the containing-package facts, the
    /// path, and the breaking-version gate each separate it from the same
    /// content's key (`file_names` reads the basename; the pub-aware rules
    /// gate on the path's directory class and read the package name sets).
    /// The name-set folds are order-independent.
    #[test]
    fn memo_keys_separate_on_every_row_input() {
        let content = dartq_source::content_hash(b"var x = 1;\n");
        let set = enable_set(None);
        let features = Features::default();
        let path = Path::new("/w/lib/a.dart");
        let facts = WorkspaceFacts {
            own_package: "w".to_owned(),
            dependencies: FxHashSet::from_iter(["w".to_owned(), "dep".to_owned()]),
            dev_dependencies: FxHashSet::default(),
            root: PathBuf::from("/w"),
            lib_dir: PathBuf::from("/w/lib"),
        };
        let key =
            |features, set: &EnableSet, workspace: Option<&WorkspaceFacts>, path, breaking| {
                memo_key(
                    content,
                    features,
                    DEFAULT_LANGUAGE_VERSION,
                    set,
                    workspace,
                    path,
                    &FxHashSet::default(),
                    None,
                    breaking,
                )
            };

        assert_eq!(
            key(features, &set, None, path, false),
            key(features, &set, None, path, false),
            "same inputs, same key"
        );
        assert_ne!(
            key(features, &set, None, path, false),
            key(
                Features {
                    patterns: false,
                    ..features
                },
                &set,
                None,
                path,
                false
            ),
            "the feature bits fold in"
        );
        assert_ne!(
            key(features, &set, None, path, false),
            key(
                features,
                &enable_set(Some(&["unnecessary_new".to_owned()])),
                None,
                path,
                false
            ),
            "the enable set folds in"
        );
        assert_ne!(
            key(features, &set, None, path, false),
            key(features, &set, Some(&facts), path, false),
            "the workspace facts fold in"
        );
        assert_ne!(
            key(features, &set, None, path, false),
            key(features, &set, None, Path::new("/w/test/a.dart"), false),
            "the path folds in"
        );
        assert_ne!(
            key(features, &set, None, path, false),
            key(features, &set, None, path, true),
            "the breaking-version gate folds in"
        );
        let with_facts = key(features, &set, Some(&facts), path, false);
        let twin = WorkspaceFacts {
            dependencies: FxHashSet::from_iter(["dep".to_owned(), "w".to_owned()]),
            ..facts
        };
        assert_eq!(
            with_facts,
            key(features, &set, Some(&twin), path, false),
            "the name-set fold is order-independent"
        );
    }

    /// The semantic generation half of the key (#1049): a faceless file's
    /// key folds no generation (`None` keys identically whatever generation
    /// exists), while a semantic file's key separates per generation — a
    /// sibling edit re-keys the file without touching its own content id.
    #[test]
    fn memo_key_widens_with_the_semantic_generation() {
        let content = dartq_source::content_hash(b"var x = 1;\n");
        let set = EnableSet::none();
        let path = Path::new("/w/lib/a.dart");
        let key = |generation| {
            memo_key(
                content,
                Features::default(),
                DEFAULT_LANGUAGE_VERSION,
                &set,
                None,
                path,
                &FxHashSet::default(),
                generation,
                false,
            )
        };
        assert_eq!(key(None), key(None), "faceless keys are generation-free");
        assert_ne!(
            key(None),
            key(Some(1)),
            "the generation separates a semantic file's key"
        );
        assert_ne!(key(Some(1)), key(Some(2)), "each generation is its own key");
    }

    /// The cap evicts oldest-first, and re-inserting a live key evicts
    /// nothing (the facts-cache idiom).
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn memo_evicts_oldest_at_the_cap() {
        let mut memo = LintMemo::default();
        let set = EnableSet::none();
        let features = Features::default();
        let path = Path::new("/w/a.dart");
        let key = |n: u16| {
            memo_key(
                dartq_source::content_hash(&n.to_le_bytes()),
                features,
                DEFAULT_LANGUAGE_VERSION,
                &set,
                None,
                path,
                &FxHashSet::default(),
                None,
                false,
            )
        };
        for n in 0..=LINT_MEMO_CAP as u16 {
            memo.insert(key(n), Arc::new(Vec::new()));
        }
        assert_eq!(memo.entries.len(), LINT_MEMO_CAP, "the cap holds");
        assert!(memo.get(&key(0)).is_none(), "the oldest entry evicted");
        assert!(memo.get(&key(LINT_MEMO_CAP as u16)).is_some());
        memo.insert(key(LINT_MEMO_CAP as u16), Arc::new(Vec::new()));
        assert_eq!(
            memo.entries.len(),
            LINT_MEMO_CAP,
            "re-inserting a live key does not evict"
        );
    }

    /// The pubspec pass gates on the enable set: options enabling only a
    /// Dart-file rule leave the pubspec unreported even when it carries a
    /// triggerable face (an insecure URL), while the Dart rule still fires —
    /// the run itself is live.
    #[test]
    fn pubspec_pass_skips_when_no_pubspec_rule_is_enabled() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_gate",
            &[("lib/main.dart", "var x = new Object;\n")],
        );
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - unnecessary_new\n",
        )
        .expect("options file");
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nhomepage: http://github.com/fixture\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "secure_pubspec_urls" && d.code.name != "package_names"),
            "{:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        assert!(
            run.diagnostics
                .iter()
                .any(|d| d.code.name == "unnecessary_new"),
            "the Dart rule still fires: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The YAML-face suppression reaches `package_names` too: a
    /// `# ignore:` line above the `name:` entry drops the camel-case row the
    /// same fixture reports without the comment.
    #[test]
    fn package_names_ignore_line_suppresses_the_name_row() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_names_ignore",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - package_names\n",
            )],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "# ignore: package_names\nname: fooBar\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fooBar", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "package_names"),
            "the suppressed name row is gone: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The pass walks the whole workspace: the root package's and every
    /// member's `pubspec.yaml` ride the same governing enable set — the
    /// member's insecure URL reports at the member's file, and the root's
    /// clean pubspec keeps the row's file attribution exact.
    #[test]
    fn workspace_member_pubspecs_get_the_pass() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_member", &[]);
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - secure_pubspec_urls\n",
        )
        .expect("options file");
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n",
        )
.expect("root pubspec");
        std::fs::create_dir_all(root.join("packages/a")).expect("member dir");
        std::fs::write(
            root.join("packages/a/pubspec.yaml"),
            "name: a\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\nhomepage: http://a.dev\n",
        )
.expect("member pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "a", "rootUri": "../packages/a", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let files: Vec<String> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "secure_pubspec_urls")
            .map(|d| run.layout.file_path(d.file).to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].ends_with("packages/a/pubspec.yaml"), "{files:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// `# ignore_for_file:` is file-wide: one entry drops every
    /// `secure_pubspec_urls` row the file would otherwise carry — the
    /// top-level face and the dependency-section face alike.
    #[test]
    fn pubspec_ignore_for_file_suppresses_every_row() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pubspec_ignore_for_file",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - secure_pubspec_urls\n",
            )],
        );
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\n# ignore_for_file: secure_pubspec_urls\nhomepage: http://a.dev\ndependency_overrides:\n  dep:\n    git:\n      url: git://github.com/x/y.git\nenvironment:\n  sdk: ^3.5.0\n",
        )
.expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        assert!(
            run.diagnostics
                .iter()
                .all(|d| d.code.name != "secure_pubspec_urls"),
            "both insecure URLs are suppressed: {:?}",
            run.diagnostics
                .iter()
                .map(|d| d.code.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// `sort_pub_dependencies` end to end: each section is judged with a
    /// fresh `previousName` — two unsorted sections report one row each at
    /// their first out-of-order name while the sorted third stays silent.
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn sort_rows_report_per_section_independently() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_sort_sections",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - sort_pub_dependencies\n",
            )],
        );
        let pubspec = "name: fixture\ndependencies:\n  zzz: ^1.0.0\n  aaa: ^1.0.0\ndev_dependencies:\n  mmm: ^1.0.0\n  bbb: ^1.0.0\ndependency_overrides:\n  aaa: ^1.0.0\n  zzz: ^1.0.0\nenvironment:\n  sdk: ^3.5.0\n";
        std::fs::write(root.join("pubspec.yaml"), pubspec).expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let mut spans: Vec<(u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "sort_pub_dependencies")
            .map(|d| (d.range.offset, d.range.offset + d.range.length))
            .collect();
        spans.sort_unstable();
        let span = |needle: &str| {
            let start = pubspec.find(needle).expect("needle in pubspec") as u32;
            (start, start + needle.len() as u32)
        };
        assert_eq!(spans, vec![span("aaa"), span("bbb")], "{spans:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The document-order walk with the `sdk:` form: `flutter`'s nested
    /// spec lines are no names, the first out-of-order key reports exactly
    /// at its name span, and the row carries the official message.
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn sort_row_reports_at_the_first_out_of_order_name() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_sort_doc_order",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - sort_pub_dependencies\n",
            )],
        );
        let pubspec = "name: fixture\ndependencies:\n  aaa: ^0.1.1\n  flutter:\n    sdk: flutter\n  bbb: ^0.15.8\nenvironment:\n  sdk: ^3.5.0\n";
        std::fs::write(root.join("pubspec.yaml"), pubspec).expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(u32, u32, String)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "sort_pub_dependencies")
            .map(|d| {
                (
                    d.range.offset,
                    d.range.offset + d.range.length,
                    d.message.clone().into_owned(),
                )
            })
            .collect();
        let start = pubspec.find("bbb").expect("bbb in pubspec") as u32;
        assert_eq!(
            rows,
            vec![(
                start,
                start + 3,
                "Dependencies not sorted alphabetically.".to_owned()
            )],
            "{rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Entries the analyzer model leaves unnamed — list items, with or
    /// without a colon — neither report nor advance the walk: the
    /// section's judgment runs over its named entries alone.
    #[test]
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn sort_pass_drops_unnamed_entries() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_sort_unnamed",
            &[(
                "analysis_options.yaml",
                "linter:\n  rules:\n    - sort_pub_dependencies\n",
            )],
        );
        let pubspec = "name: fixture\ndependencies:\n  zzz: ^1.0.0\n  - strays\n  - stray: item\n  aaa: ^1.0.0\nenvironment:\n  sdk: ^3.5.0\n";
        std::fs::write(root.join("pubspec.yaml"), pubspec).expect("pubspec");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let spans: Vec<(u32, u32)> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "sort_pub_dependencies")
            .map(|d| (d.range.offset, d.range.offset + d.range.length))
            .collect();
        let start = pubspec.find("aaa").expect("aaa in pubspec") as u32;
        assert_eq!(spans, vec![(start, start + 3)], "{spans:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// The mounted pass reaches workspace members too: the governing
    /// options enable the rule once, the member's unsorted pubspec
    /// reports at the member's file, and the root's sorted one stays
    /// silent.
    #[test]
    fn workspace_member_pubspec_sort_rows_ride_the_pass() {
        let root = crate::workspace::test_fixture::workspace("lints_sort_member", &[]);
        std::fs::write(
            root.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - sort_pub_dependencies\n",
        )
        .expect("options file");
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - packages/a\n",
        )
.expect("root pubspec");
        std::fs::create_dir_all(root.join("packages/a")).expect("member dir");
        std::fs::write(
            root.join("packages/a/pubspec.yaml"),
            "name: a\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\ndependencies:\n  zzz: ^1.0.0\n  aaa: ^1.0.0\n",
        )
.expect("member pubspec");
        std::fs::write(
            root.join(".dart_tool/package_config.json"),
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.5"}, {"name": "a", "rootUri": "../packages/a", "languageVersion": "3.5"}], "generator": "pub"}"#,
        )
.expect("config");
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let files: Vec<String> = run
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "sort_pub_dependencies")
            .map(|d| run.layout.file_path(d.file).to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].ends_with("packages/a/pubspec.yaml"), "{files:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A main + part library for the shared-substrate pins: the part
    /// declares the class the lookups probe, the main declares its own.
    /// The pins read the engine's facts after the run returns (the run's
    /// borrow ends with its scope).
    fn part_library_engine(tag: &str) -> (PathBuf, Engine) {
        let root = crate::workspace::test_fixture::workspace(
            tag,
            &[
                ("lib/main.dart", "part 'part.dart';\n\nclass Main {}\n"),
                (
                    "lib/part.dart",
                    "part of 'main.dart';\n\nclass PartWidget {}\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root)).expect("run");
        (root, engine)
    }

    /// The one two-member library of [`part_library_engine`]'s fixture
    /// and its member files (defining first).
    fn part_library_members(engine: &Engine) -> Vec<FileId> {
        let mut libraries = engine.libraries();
        libraries.retain(|(_, files)| files.len() == 2);
        let [(_, members)] = libraries.as_slice() else {
            panic!("one two-file library expected: {libraries:?}");
        };
        members.clone()
    }

    /// The shared substrate is the standalone build: for a two-file
    /// library, [`build_library_semantics`] + [`semantic_context`] answer
    /// exactly what the per-file [`build_semantic_context`] answers — the
    /// library node, the cross-file namespace lookups (the part-declared
    /// class resolves to the part's file in both), and the member element
    /// models in the index.
    #[test]
    fn shared_substrate_equals_the_standalone_build() {
        let (root, engine) = part_library_engine("lints_shared_substrate");
        let members = part_library_members(&engine);
        let (main, part) = (members[0], members[1]);
        let main_text =
            std::str::from_utf8(engine.content(main).expect("main bytes")).expect("main utf8");
        let shared = build_library_semantics(&engine, main, &members);
        let mounted = semantic_context(main_text, main, &shared);
        let standalone = build_semantic_context(&engine, main_text, main, &members);
        assert_eq!(mounted.lib, standalone.lib, "the library node identity");
        let strings = engine.strings();
        for name in ["Main", "PartWidget", "Object"] {
            let id = strings.intern(name);
            assert_eq!(
                mounted.namespaces.lookup(&mounted.graph, mounted.lib, id),
                standalone
                    .namespaces
                    .lookup(&standalone.graph, standalone.lib, id),
                "the lookup of {name} agrees"
            );
        }
        assert_eq!(
            mounted
                .namespaces
                .lookup(&mounted.graph, mounted.lib, strings.intern("PartWidget")),
            dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(part)),
            "the part-declared class resolves to the part's file"
        );
        for member in [main, part] {
            let count = |context: &SemanticContext| {
                context
                    .index
                    .get(member)
                    .map(|elements| elements.declarations().count())
            };
            assert_eq!(
                count(&mounted),
                count(&standalone),
                "the element model of member {member:?} is indexed alike"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The sharing contract: both members' mounts point at the one
    /// library artifact's tables (the Arc identity the fan-out leans on),
    /// while each member's scan stays its own.
    #[test]
    fn semantic_context_shares_the_library_tables() {
        let (root, engine) = part_library_engine("lints_shared_identity");
        let members = part_library_members(&engine);
        let shared = build_library_semantics(&engine, members[0], &members);
        let text =
            |file: FileId| std::str::from_utf8(engine.content(file).expect("bytes")).expect("utf8");
        let main = semantic_context(text(members[0]), members[0], &shared);
        let part = semantic_context(text(members[1]), members[1], &shared);
        assert!(Arc::ptr_eq(&main.graph, &shared.graph), "the graph shares");
        assert!(
            Arc::ptr_eq(&main.namespaces, &shared.namespaces),
            "the namespaces share"
        );
        assert!(Arc::ptr_eq(&main.index, &shared.index), "the index shares");
        assert!(
            Arc::ptr_eq(&part.graph, &shared.graph),
            "the part shares too"
        );
        assert_ne!(
            main.scanned.tokens.len(),
            part.scanned.tokens.len(),
            "each member scans its own bytes"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The fan-out end to end over a two-file library: one shared
    /// substrate mounts for both members' jobs and the rows still come
    /// out — the part file's `print` reports through the shared graph's
    /// implicit-core seed, exactly as the per-file build reported.
    #[test]
    fn two_member_library_lints_through_the_shared_substrate() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_shared_fanout",
            &[
                (
                    "analysis_options.yaml",
                    "linter:\n  rules:\n    - avoid_print\n",
                ),
                ("lib/main.dart", "part 'part.dart';\n"),
                (
                    "lib/part.dart",
                    "part of 'main.dart';\n\nvoid p() {\n  print('x');\n}\n",
                ),
            ],
        );
        let root = root.canonicalize().expect("canonical root");
        let (config, _) =
            crate::workspace::engine_config(&root, Some(&root.join("sdk"))).expect("assembly");
        let mut engine = Engine::new(config).expect("engine");
        let run = crate::analyze::run_overlaid(&mut engine, &[], std::slice::from_ref(&root))
            .expect("run");
        let rows: Vec<(u32, String)> = run
            .diagnostics
            .iter()
            .filter(|d| {
                d.code.name == "avoid_print"
                    && run.layout.file_path(d.file).ends_with("lib/part.dart")
            })
            .map(|d| (d.range.offset, d.message.clone().into_owned()))
            .collect();
        assert_eq!(rows.len(), 1, "the part's print reports once: {rows:?}");
        assert!(
            rows[0].1.contains("print"),
            "the avoid_print message: {rows:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
