//! The lint face's mount (PRD §13): the analyze pipeline's bridge to
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
//! to a [`Diagnostic`] at emission ([`row`]): severity is the registry default
//! ([`DiagnosticType::Lint::severity`] — oracle-INFO, the
//! `dtype_matches_oracle_severity` invariant; the layer's `EnableSet`
//! models no severity overrides), the code identity derives from the rule
//! registry (name and UPPERCASE machine form, type `LINT`, never
//! published docs — mirroring the hand-written `PREFER_SINGLE_QUOTES`
//! row), and the UTF-16 row span becomes the engine's
//! [`SourceRange`]. `EnableSet` carries no severity overrides, so there is
//! nothing else to map.
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
//! enable set, containing-package facts, path — so a daemon turn's lint
//! stage answers unchanged files from memory and does work only for the
//! turn's dirty set. Rows store pre-file-id-stamping (file-relative
//! spans); the id stamps at emission exactly as a fresh run's does, so a
//! hit re-attributes byte-identically. The CLI passes a throwaway memo —
//! a one-shot run shares nothing across processes.

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::collections::VecDeque;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use dartq_diagnostics::{CodeInfo, Diagnostic, DiagnosticType, SourceRange};
use dartq_engine::Engine;
use dartq_engine::analysis_options::AnalysisOptions;
use dartq_language::{DEFAULT_LANGUAGE_VERSION, Feature, FeatureSet, LanguageVersion};
use dartq_lints::{
    EnableSet, Features, Lint, RULES, RuleId, WorkspaceFacts, check_name, check_url, parse_unit,
    rule_by_name, run,
};
use dartq_package::PackageIdentity;
use dartq_pub::pubspec_facts::scan as scan_pubspec;
use dartq_pub::{Pubspec, pubspec_facts::Scalar};
use dartq_source::{ContentId, FileId, LineIndex};

/// The enable set one governing entry selects: `None` (no options file in
/// the governed file's ancestry) selects nothing — the official
/// options-less posture — and a present options file's effective rule list
/// replaces it. See the module docs for the selection semantics.
#[must_use]
pub fn enable_set(rules: Option<&[String]>) -> EnableSet {
    match rules {
        None => EnableSet::none(),
        Some(names) => EnableSet::resolve(names.iter().map(String::as_str)).0,
    }
}

/// Per-rule code identity, registry-parallel (`RuleId` indexes it): the
/// rule name with its UPPERCASE machine form, type `LINT`, never published
/// docs. The machine forms leak once per rule (68 bounded, process-lifetime
/// strings) because `CodeInfo` pins `&'static str` and the names are
/// runtime-independent statics.
static CODES: LazyLock<Vec<CodeInfo>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|spec| {
            let machine: &'static str = Box::leak(spec.name.to_uppercase().into_boxed_str());
            CodeInfo::new(spec.name, machine, DiagnosticType::Lint, false)
        })
        .collect()
});

/// Maps one lint row to an engine diagnostic — the oracle's lint shape:
/// INFO severity, the rule name as the code, the row's UTF-16 span, the
/// official problem/correction texts verbatim.
fn row(lint: Lint, file: FileId) -> Diagnostic {
    Diagnostic::new(
        DiagnosticType::Lint.severity(),
        CODES[lint.rule.0 as usize],
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
    let version = identity
        .language_version
        .map(dartq_pub::LanguageVersion::into_inner)
        .unwrap_or(DEFAULT_LANGUAGE_VERSION);
    PackageFace {
        workspace,
        features: features_for(version),
    }
}

/// The per-file lint-row memo (#772): a daemon turn's lint stage answers
/// every unchanged file from this table instead of re-parsing and
/// re-running the rules — one [`analyze_lints`] call over a 1000-file
/// workspace with one dirty file does one file's work, not a thousand's.
///
/// The key carries everything the rows read (the engine's `FactsKey`
/// discipline, PRD §5.3): the content hash plus one folded fingerprint
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
/// spans); the id stamps at emission ([`row`]), exactly as a fresh run's
/// does. The memo lives in-process only (a `Session` field, or a CLI
/// run's throwaway), so a binary update can never serve stale rows.
///
/// Bounded at [`LINT_MEMO_CAP`] entries with oldest-first eviction (the
/// engine's facts-cache idiom), and the RSS cost is bounded with it: a
/// row is a rule id, a UTF-16 range, and two short owned strings, and a
/// clean workspace's rows are empty, so the table holds well under a
/// megabyte on clean workspaces and at most a few megabytes on
/// lint-dense ones.
#[derive(Default)]
pub struct LintMemo {
    entries: FxHashMap<LintMemoKey, Arc<Vec<Lint>>>,
    /// Insertion order, for oldest-first eviction.
    order: VecDeque<LintMemoKey>,
}

/// The memo key: the content identity plus one fingerprint over every
/// other row input ([`LintMemo`]).
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
/// over every other input the rows read — the feature bits, the enable
/// set (enabled rule ids in registry order), the containing package's
/// facts, and the file's path. The set folds are commutative (wrapping
/// add of per-entry hashes) because `FxHashSet` iteration order is not
/// stable, and every variable-length field ends in a separator byte so
/// concatenations cannot collide across field boundaries.
fn memo_key(
    content: ContentId,
    features: Features,
    set: &EnableSet,
    workspace: Option<&WorkspaceFacts>,
    path: &Path,
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
    LintMemoKey {
        content,
        fingerprint: h.finish(),
    }
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
/// ancestry governs ([`governing_options`]) — the same deepest-governing
/// resolution the engine's severity and exclude faces apply, so a nested
/// options file's `linter.rules` section refines its subtree here too.
/// Sets are resolved once per distinct file directory. An empty resolved
/// set skips the file without parsing (opt-in cost ruling). Files that are
/// no longer resident or are not UTF-8 are skipped — the engine's own pass
/// already reported nothing for them.
///
/// The fan-out is parallel (#765): the per-directory enable sets and the
/// per-package-root faces ([`package_face`], one pubspec parse per
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
/// The package-aware rules read the containing package's facts through
/// `Unit.workspace` and the unit's features from the engine's settled
/// per-file face (#722) — the `@dart` override and grants already folded
/// in — falling back to the containing package's language version where no
/// face is settled. The pubspec pass runs after the file fan-out: the root
/// package's and every workspace member's `pubspec.yaml`, through the same
/// enable-set resolution, the scanned facts, and the YAML `# ignore:`
/// variant ([`YamlIgnores`]).
///
/// The memo (`memo`, #772) turns a daemon turn's cost from O(workspace)
/// into O(dirty): before any fan-out, the serial partition derives each
/// file's [`memo_key`] (hash work only — no parse, no rule run) and
/// answers hits by cloning their rows' `Arc`. Only misses fan out — a
/// rayon job cannot borrow the memo `&mut`, so jobs read `&Engine` and
/// statics only, and the keyed misses insert back serially after the
/// collect. A hit's rows re-stamp byte-identically (the [`LintMemo`]
/// determinism contract).
#[must_use]
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
    let mut faces: FxHashMap<PathBuf, Arc<PackageFace>> = FxHashMap::default();
    for file in &files {
        let Some(path) = engine.file_table().path(*file) else {
            continue;
        };
        let dir = path.parent().unwrap_or(Path::new(""));
        enabled.entry(dir.to_path_buf()).or_insert_with(|| {
            enable_set(
                governing_options(engine, dir, roots).map(|options| options.lint_rules.as_slice()),
            )
        });
        if let Some(identity) = engine.packages().containing(path) {
            faces
                .entry(identity.root.clone())
                .or_insert_with(|| Arc::new(package_face(identity)));
        }
    }

    // The serial memo partition: keyed hits lift their rows outright,
    // misses queue for the fan-out, and rowless shapes (no path, or an
    // empty enable set — the opt-in cost ruling) contribute nothing
    // exactly as their jobs always did. A file whose facts are absent is
    // linted all the same — it just carries no content id to key with.
    let mut slotted: Vec<Option<Arc<Vec<Lint>>>> = vec![None; files.len()];
    let mut misses: Vec<(usize, FileId, Option<LintMemoKey>)> = Vec::new();
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
        let (workspace, fallback) = match engine
            .packages()
            .containing(path)
            .and_then(|identity| faces.get(&identity.root))
        {
            Some(face) => (face.workspace.as_ref(), face.features),
            None => (None, features_for(DEFAULT_LANGUAGE_VERSION)),
        };
        let features = engine.settled_features(*file).map_or(fallback, features_of);
        let Some(facts) = engine.file_facts(*file) else {
            misses.push((slot, *file, None));
            continue;
        };
        let key = memo_key(facts.content(), features, set, workspace, path);
        match memo.get(&key) {
            Some(rows) => slotted[slot] = Some(rows),
            None => misses.push((slot, *file, Some(key))),
        }
    }

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
            let (workspace, fallback) = match engine
                .packages()
                .containing(path)
                .and_then(|identity| faces.get(&identity.root))
            {
                Some(face) => (face.workspace.as_ref(), face.features),
                None => (None, features_for(DEFAULT_LANGUAGE_VERSION)),
            };
            let features = engine.settled_features(file).map_or(fallback, features_of);
            let parsed = parse_unit(text, features);
            let mut unit = parsed.unit(Some(path));
            unit.workspace = workspace;
            (slot, run(&unit, set))
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
    out.extend(pubspec_rows(engine, roots));
    out
}

/// The pubspec pass (#434): the root package's and every workspace
/// member's `pubspec.yaml` — the same set the engine captured at analyze
/// time — under the same enable-set resolution the Dart files get. A
/// capture miss (no engine pass yet, or no pubspec file) leaves the file
/// unreported. Rows carry the official value-node span (UTF-16 over the
/// pubspec text) and the `IgnoreInfo.forYaml` suppression face.
fn pubspec_rows(engine: &Engine, roots: &[PathBuf]) -> Vec<Diagnostic> {
    let table = engine.packages();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(root) = table.root().and_then(|name| table.get(name.as_str())) {
        dirs.push(root.root.clone());
    }
    if let Some(members) = table.workspace_members() {
        dirs.extend(
            members
                .iter()
                .filter_map(|name| table.get(name.as_str()))
                .map(|identity| identity.root.clone()),
        );
    }

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
        let urls_on = urls_rule.is_some_and(|id| set.is_enabled(id));
        let names_on = names_rule.is_some_and(|id| set.is_enabled(id));
        if !urls_on && !names_on {
            continue;
        }
        let facts = scan_pubspec(text);
        let ignores = YamlIgnores::parse(text);
        let lines = LineIndex::new(text);
        let mut lints = Vec::new();
        if urls_on {
            let rule = urls_rule.expect("checked above");
            let spec = &RULES[rule.0 as usize];
            for Scalar { range, value } in facts.urls.iter().chain(&facts.dep_urls) {
                let mut hits = Vec::new();
                check_url(range.clone(), value, &mut hits);
                lints.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(hit.range.start as u32);
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
                lints.extend(hits.into_iter().filter_map(|hit| {
                    let (line, _) = lines.byte_to_line_col(hit.range.start as u32);
                    (!ignores.ignored(line, "package_names")).then(|| Lint {
                        rule,
                        range: utf16_range(&lines, text, &hit.range),
                        message: spec.render_problem(&[&hit.name]),
                        correction: spec.render_correction(&[]),
                    })
                }));
            }
        }
        out.extend(lints.into_iter().map(|lint| row(lint, file)));
    }
    out
}

/// A pubspec row's byte span in UTF-16 (the engine's diagnostic
/// coordinate).
fn utf16_range(
    lines: &LineIndex,
    text: &str,
    range: &core::ops::Range<usize>,
) -> core::ops::Range<u32> {
    lines.byte_to_utf16(text, range.start as u32)..lines.byte_to_utf16(text, range.end as u32)
}

/// The YAML-face suppression table (`IgnoreInfo.forYaml`,
/// pkg/analyzer/lib/src/ignore_comments/ignore_info.dart:97-104, 145-163):
/// `# ignore:` entries — alone on their line suppress the NEXT line,
/// trailing suppress their own — and `# ignore_for_file:` entries
/// file-wide. The names are the comma-separated, whitespace-trimmed list;
/// matching is case-insensitive (the Dart-token comment path is a
/// different face and is not reused).
struct YamlIgnores {
    on_line: FxHashMap<u32, Vec<String>>,
    for_file: Vec<String>,
}

impl YamlIgnores {
    fn parse(text: &str) -> Self {
        let mut info = Self {
            on_line: FxHashMap::default(),
            for_file: Vec::new(),
        };
        for (index, line) in text.lines().enumerate() {
            let line_no = u32::try_from(index).expect("line count fits u32") + 1;
            if let Some((before, names)) = yaml_ignore_entry(line, "ignore:") {
                // Alone on the line -> the next line (ignore_info.dart:157).
                let target = if before.trim().is_empty() {
                    line_no + 1
                } else {
                    line_no
                };
                info.on_line.entry(target).or_default().extend(names);
            }
            if let Some((_, names)) = yaml_ignore_entry(line, "ignore_for_file:") {
                info.for_file.extend(names);
            }
        }
        info
    }

    /// Whether a lint named `name` reported on 1-based `line` is
    /// suppressed (`_ignoredAt`, ignore_info.dart:186-212).
    fn ignored(&self, line: u32, name: &str) -> bool {
        self.for_file
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(name))
            || self
                .on_line
                .get(&line)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eq_ignore_ascii_case(name)))
    }
}

/// The `#`-commented marker entry of one YAML line: the text before the
/// `#` run, and the name list after the marker. The official regexes'
/// greedy `.*` reads the LAST `#` run the rest still matches from; the
/// scan keeps the last candidate the same way.
fn yaml_ignore_entry<'a>(line: &'a str, marker: &str) -> Option<(&'a str, Vec<String>)> {
    let bytes = line.as_bytes();
    let mut found = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'#' {
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'#' {
            j += 1;
        }
        let mut k = j;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if line[k..].starts_with(marker) {
            found = Some((&line[..i], &line[k + marker.len()..]));
        }
    }
    found.map(|(before, rest)| {
        (
            before,
            rest.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    })
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
fn governing_options<'a>(
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
            .map(PathBuf::as_path)
            .unwrap_or_else(|| Path::new("")),
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

    /// The #355 activation end to end: a fixture workspace whose only
    /// library carries an `unnecessary_new` shape produces an INFO lint row
    /// through the full pipeline — engine pass, transient re-parse, row
    /// mapping, oracle order — with the semantic row of the same file.
    #[test]
    fn analyze_lints_rows_flow_through_the_resident_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_activation",
            &[("lib/bad.dart", "var x = new Object();\nvar ok = unknown;\n")],
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

    /// The version→features mapping is the vendored
    /// `experimental_features.yaml`'s `enabledIn` table: each bit's
    /// shipping milestone decides it (`unnamed_libraries` 2.19, `patterns`
    /// 3.0, `wildcard_variables` 3.7) and `enhanced_parts` never (no
    /// `enabledIn`). The latest version yields `Features::default()`
    /// exactly; the wave9 pin (≤2.19) drops `patterns`.
    #[test]
    fn features_derive_from_the_language_version() {
        let v = |major: u32, minor: u32| LanguageVersion::new(major, minor);
        assert_eq!(features_for(v(3, 13)), Features::default());
        assert_eq!(features_for(v(3, 7)), Features::default());
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
    /// package's duplicate `case` values report through the full pipeline
    /// (the legacy `SwitchCase` face), while the same shape under a 3.x
    /// package stays silent (the `patterns` posture the AstBuilder keys).
    #[test]
    fn legacy_language_version_reports_legacy_duplicates() {
        const DUPLICATE_CASES: &str =
            "void f(int x) {\n  switch (x) {\n    case 1:\n    case 1:\n      break;\n  }\n}\n";
        let run_codes = |version: &str| {
            let root = crate::workspace::test_fixture::workspace(
                "lints_version_face",
                &[("lib/a.dart", DUPLICATE_CASES)],
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
    /// 3.x package runs the lint face at the *pinned* posture — the
    /// duplicate `case` values report through the settled per-file face
    /// (#722) even though the package version alone would keep them silent.
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
                &[("lib/a.dart", &source)],
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

    /// The pubspec pass (#434) end to end: the root `pubspec.yaml`'s
    /// insecure URLs report at their value spans (the official value-node
    /// anchor), the secure ones stay silent.
    #[test]
    fn pubspec_url_rows_report_at_the_value_span() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_urls", &[]);
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

    /// `package_names` over the same pass: a camel-case `name:` reports at
    /// its value span with the official argument fill. `fooBar` is a legal
    /// pub name, so the assembly needs the config's root entry renamed to
    /// match (a real `pub get` writes it).
    #[test]
    fn package_names_row_reports_the_camel_case_name() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_names", &[]);
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

    /// The YAML-face suppression (`IgnoreInfo.forYaml`): a `# ignore:`
    /// line above the `homepage:` entry drops its row.
    #[test]
    fn pubspec_ignore_line_suppresses_the_url_row() {
        let root = crate::workspace::test_fixture::workspace("lints_pubspec_ignore", &[]);
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

    /// The package-aware directive rules ride the pipeline with the
    /// mounted facts: a foreign import and a foreign `src/` import report
    /// in `lib/`, the own package and the dev dependency stay silent
    /// where the official gates hold.
    #[test]
    fn package_aware_directive_rules_flow_through_the_pipeline() {
        let root = crate::workspace::test_fixture::workspace(
            "lints_pub_facts",
            &[
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
                    d.code.name.as_ref(),
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
        let text = "var x = new Object();\n";
        let set = enable_set(Some(&["unnecessary_new".to_owned()]));
        let features = Features::default();
        let content = dartq_source::content_hash(text.as_bytes());
        let rows = || {
            let parsed = parse_unit(text, features);
            let mut unit = parsed.unit(Some(path));
            unit.workspace = None;
            run(&unit, &set)
        };

        let fresh: Vec<Diagnostic> = rows().into_iter().map(|lint| row(lint, file)).collect();
        let mut memo = LintMemo::default();
        let key = memo_key(content, features, &set, None, path);
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

    /// The key folds everything the rows read: the features, the enable
    /// set, the containing-package facts, and the path each separate it
    /// from the same content's key (`file_names` reads the basename; the
    /// pub-aware rules gate on the path's directory class and read the
    /// package name sets). The name-set folds are order-independent.
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
        let key = |features, set: &EnableSet, workspace: Option<&WorkspaceFacts>, path| {
            memo_key(content, features, set, workspace, path)
        };

        assert_eq!(
            key(features, &set, None, path),
            key(features, &set, None, path),
            "same inputs, same key"
        );
        assert_ne!(
            key(features, &set, None, path),
            key(
                Features {
                    patterns: false,
                    ..features
                },
                &set,
                None,
                path
            ),
            "the feature bits fold in"
        );
        assert_ne!(
            key(features, &set, None, path),
            key(
                features,
                &enable_set(Some(&["unnecessary_new".to_owned()])),
                None,
                path
            ),
            "the enable set folds in"
        );
        assert_ne!(
            key(features, &set, None, path),
            key(features, &set, Some(&facts), path),
            "the workspace facts fold in"
        );
        assert_ne!(
            key(features, &set, None, path),
            key(features, &set, None, Path::new("/w/test/a.dart")),
            "the path folds in"
        );
        let with_facts = key(features, &set, Some(&facts), path);
        let twin = WorkspaceFacts {
            dependencies: FxHashSet::from_iter(["dep".to_owned(), "w".to_owned()]),
            ..facts
        };
        assert_eq!(
            with_facts,
            key(features, &set, Some(&twin), path),
            "the name-set fold is order-independent"
        );
    }

    /// The cap evicts oldest-first, and re-inserting a live key evicts
    /// nothing (the facts-cache idiom).
    #[test]
    fn memo_evicts_oldest_at_the_cap() {
        let mut memo = LintMemo::default();
        let set = EnableSet::none();
        let features = Features::default();
        let path = Path::new("/w/a.dart");
        let key = |n: u16| {
            memo_key(
                dartq_source::content_hash(&n.to_le_bytes()),
                features,
                &set,
                None,
                path,
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
}
