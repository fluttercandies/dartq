//! `analysis_options.yaml` loading and include-chain merge (PRD §12.1/§12.2).
//!
//! Semantics are pinned to the official merge in
//! `.third/dart-sdk/pkg/analyzer/lib/src/analysis_options/`:
//!
//! - **Depth-first, far-before-near** — `_applyParsedFiles`
//!   (analysis_options_parser.dart:384-408) applies every include
//!   recursively *before* the including file's own data, so a nearer file's
//!   entry overrides the farther one (scalars are "set if present"
//!   (`_ApplyState._applyLanguage`), lint rules land in a per-name map via
//!   `addAll` (`_applyLinterSection`) where nearer entries replace same-name
//!   farther entries and sets union).
//! - **Include resolution** — `sourceFactory.resolveUri` against the
//!   *including* file (parser:604-611): relative URIs resolve against the
//!   including file's directory, `package:` URIs through the package config.
//!   The include chain guards recursion (`includeChain.contains`, parser
//!   :395).
//! - **Exclude globs** — each pattern is recorded with its *declaring file*
//!   (`AnalysisOptionsExcludePattern`) and compiled relative to that file's
//!   directory (`_getExcludedGlobs`, context_locator.dart:625-639, with the
//!   `foo/**` pattern also excluding the `foo` directory itself so the walk
//!   stops descending). Matching tests the path relative to the declaring
//!   directory (`LocatedGlob.matches`, context_root.dart:183-194). A pattern
//!   that is not a string is ignored; a non-list `exclude:` section is
//!   ignored (`_ParsedExcludesData.parse`, `analysis_options_parse_model.dart`
//!   :634-681).
//! - **Linter rule entries** — `parseLinterSection`/`_parseRuleConfig`
//!   (lint/config.dart:20-104): a list of names, or a map of `name: bool`,
//!   `name: severity-string` (`disable` disables, any other string enables),
//!   or a nested group map; unknown shapes ignored. A `linter:` key that is
//!   present but not a map marks the section invalid, which *clears* the
//!   rules merged so far (`_applyLinterSection`, parser:278-289).
//! - **Severity overrides** — `analyzer.errors` is a per-code action map
//!   (`_ParsedErrorProcessorsData.parse`, `analysis_options_parse_model.dart`
//!   :541-631): the action is stringified and lowercased
//!   (`ErrorConfig._processMap`, `source/error_processor.dart`:36-54), and
//!   `ignore`/`false` filters the code out while `error`/`warning`/`info`
//!   re-levels it (`severityMap`, `error_processor.dart`:14-18); any other
//!   scalar contributes nothing. The include chain merges per code, a nearer
//!   file's entry replacing the farther one (`_applyErrorProcessors`'
//!   `processorsByCode` rebuild, parser:219-230), and the application point
//!   defers to the override before the declared severity (`computeSeverity`,
//!   `analyzer_cli error_severity.dart`:14-26) — a filtered code is dropped
//!   from the output entirely.
//! - **Cannot-ignore** — `analyzer.cannot-ignore` lists the codes that
//!   `// ignore:` comments cannot suppress (`_ParsedCannotIgnoreData.parse`,
//!   parse_model:346-405; `_applyCannotIgnore`, parser:173-197): explicit
//!   names lowercase into the set, while a severity name (`error`/`warning`/
//!   `info`) marks every registry code whose effective severity — this
//!   file's `errors` overrides first, else the registry severity, since the
//!   errors section applies before cannot-ignore (parser:166-167) — equals
//!   it. Enforcement is the suppression gate shape
//!   (library_analyzer.dart:552-563): a code in the set survives every
//!   ignore-comment suppression.
//! - **Language strictness scalars** — `analyzer.language.strict-casts` /
//!   `strict-inference` / `strict-raw-types` parse as booleans (YAML
//!   booleans or `true`/`false` strings, the `toBool` conversion,
//!   util/yaml.dart:156-169), set-if-present across the chain
//!   (`_ParsedLanguageData.parse`, parse_model:904-998; `_applyLanguage`,
//!   parser:250-260). Parsed and exposed only — the semantic consumption is
//!   #480's.
//! - **Experiments** — `analyzer.enable-experiment` parses as a flag list
//!   (`_ParsedEnableExperimentsData.parse`, parse_model:470-536: non-list →
//!   `invalid_section_format`, non-string entries row + skip) and applies
//!   set-if-present across the chain (`_applyEnableExperiments`,
//!   parser:207-215 — a nearer list replaces a farther one whole). The
//!   official `validateFlags` (experiments_impl.dart:183-222) rows ride the
//!   self-diagnostics: an unknown flag (the pinned `experiments.g.dart`
//!   table, `no-` stripped) reports `unsupported_option`, an expired flag
//!   reports `invalid_option` ("no longer required" vs "unconditionally
//!   enabled"), and an opposite re-request reports the conflict. Flags are
//!   parsed and exposed only — the engine consumes no experiment semantics.
//! - **Self-diagnostics (#457)** — the options file's own problems surface
//!   instead of degrading silently. The official reports every options
//!   diagnostic on the *initial* file: include-chain rows directly at the
//!   initial include span (`_AnalysisOptionsIncludeWalker`, `include_walker.dart`
//!   :121-151 — a missing include reports `include_file_not_found` and stops
//!   that edge; a parsed include resolving back to the initial file reports
//!   `recursive_include_file`, :161-171; a chain cycle not through the initial
//!   file is `included_file_warning`, a wrapping face dartq has not built),
//!   while the initial file's local shape problems report their raw codes
//!   (`invalid_section_format`, `unsupported_option`, `undefined_lint`,
//!   `analysis_options_parse_model.dart`) and *included* files' local problems
//!   are wrapped the same way — so local rows are recorded for the initial
//!   file only. Codes and message templates are verbatim from the pinned
//!   registry (`diagnostic.g.dart`), all `STATIC_WARNING` → WARNING. `serde_yaml`
//!   keeps no spans, so rows anchor at the token text's first occurrence in
//!   the initial file (`anchor_span`); the `(code, file)` differential face is
//!   exact regardless. `undefined_lint` needs the lint registry, which lives
//!   in dartq-lints (an engine dependency would cycle) — the engine records
//!   the initial file's declared rule names ([`AnalysisOptions::declared_rules`])
//!   and the CLI face (dartq-commands) validates them against
//!   `rule_by_name`. Rows ride the same severity face as every other output
//!   row (`Engine::analyze` appends them before `apply_severity_options`), and
//!   the options file — never resident as Dart — resolves its rows' positions
//!   through the engine's layout fallback (`Engine::content`/`line_index` over
//!   the text + [`LineIndex`] captured at load).
//! - Unreadable or malformed files are tolerated — the official reports them
//!   (`included_file_parse_error` / YAML errors, faces dartq has not built)
//!   and still produces effective options (missing include → skipped).
//!
//! The whole face is pure functions over paths + YAML: testable without the
//! CLI, and cold-path (workspace assembly), so clarity beats micro-cost.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use serde_yaml::Value;

use dartq_diagnostics::{
    CodeInfo, CodeRegistry, Diagnostic, DiagnosticType, FileId, Severity, SourceRange,
};
use dartq_pub::PackageConfig;
use dartq_source::{LineIndex, byte_offset_to_utf16, utf16_len_of};

/// The options file name the analyzer locates walking up from a context
/// root (`ContextLocator.locate`).
const OPTIONS_FILE_NAME: &str = "analysis_options.yaml";

// The options-file self-diagnostic codes, verbatim from the pinned registry
// (`diagnostic.g.dart`, dart 3.15 main): include_file_not_found :7757-7776,
// recursive_include_file :14675-14697, invalid_section_format :9708-9722,
// undefined_lint :16988-17001, unsupported_option :17832-17905 (the three
// variants share one name/machine form; the variant is the correction text).
// All STATIC_WARNING rows, published docs — the severity is the single
// `DiagnosticType::severity` derivation at the emit sites.

/// `The URI '{0}' included in '{1}' can't be found when analyzing '{2}'.`
/// (include_walker.dart:124-134.)
const INCLUDE_FILE_NOT_FOUND: CodeInfo = CodeInfo::new(
    "include_file_not_found",
    "INCLUDE_FILE_NOT_FOUND",
    DiagnosticType::StaticWarning,
    true,
);

/// `The URI '{0}' included in '{1}' includes '{1}', creating a circular
/// reference.` (include_walker.dart:161-171.)
const RECURSIVE_INCLUDE_FILE: CodeInfo = CodeInfo::new(
    "recursive_include_file",
    "RECURSIVE_INCLUDE_FILE",
    DiagnosticType::StaticWarning,
    true,
);

/// `Invalid format for the '{0}' section.`
const INVALID_SECTION_FORMAT: CodeInfo = CodeInfo::new(
    "invalid_section_format",
    "INVALID_SECTION_FORMAT",
    DiagnosticType::StaticWarning,
    true,
);

/// `The option '{1}' isn't supported by '{0}'.`
const UNSUPPORTED_OPTION: CodeInfo = CodeInfo::new(
    "unsupported_option",
    "UNSUPPORTED_OPTION",
    DiagnosticType::StaticWarning,
    true,
);

/// `'{0}' isn't a recognized lint rule.` (parse_model:1245-1251; emitted by
/// the CLI face, which owns the lint-registry seam.)
pub const UNDEFINED_LINT: CodeInfo = CodeInfo::new(
    "undefined_lint",
    "UNDEFINED_LINT",
    DiagnosticType::StaticWarning,
    true,
);

/// One options-file self-diagnostic recorded during a load: the registry
/// code identity, the verbatim rendered texts, and the anchor span in the
/// initial options file (UTF-16; [`OptionsIssue::to_diagnostic`] completes
/// the row).
#[derive(Debug, Clone)]
pub struct OptionsIssue {
    pub code: CodeInfo,
    pub message: String,
    pub correction: Option<String>,
    pub offset: u32,
    pub length: u32,
}

impl OptionsIssue {
    /// The engine diagnostic row. Severity is the code's registry family
    /// severity — every options-file code is `STATIC_WARNING` → WARNING.
    #[must_use]
    pub fn to_diagnostic(&self, file: FileId) -> Diagnostic {
        Diagnostic::new(
            self.code.dtype.severity(),
            self.code,
            self.message.clone(),
            file,
            SourceRange {
                offset: self.offset,
                length: self.length,
            },
            self.correction.clone(),
        )
    }

    /// The `undefined_lint` row for one declared-but-unregistered rule name
    /// (parse_model:1245-1251 — the registry check fires before the entry's
    /// value is even looked at, so disabled names report too).
    #[must_use]
    pub fn undefined_lint(rule: &DeclaredRule) -> Self {
        OptionsIssue {
            code: UNDEFINED_LINT,
            message: format!("'{}' isn't a recognized lint rule.", rule.name),
            correction: Some("Try using the name of a recognized lint rule.".to_owned()),
            offset: rule.offset,
            length: rule.length,
        }
    }
}

/// A lint name declared by the initial file's own `linter.rules`, with its
/// anchor span — the `undefined_lint` candidate list. The engine holds no
/// lint registry (that lives in dartq-lints, and an engine dependency would
/// cycle), so validity is the caller's seam.
#[derive(Debug, Clone)]
pub struct DeclaredRule {
    pub name: String,
    pub offset: u32,
    pub length: u32,
}

/// One governing directory's fully merged analysis options (the
/// `ResolvedAnalysisOptions` slice PRD §12.1 names, limited to the sections
/// this engine consumes: the lint rule set, the exclude globs, the
/// `errors`/`cannot-ignore` severity face, the language strictness scalars,
/// and the experiment flag list — the latter parsed and exposed only, #480
/// consumes the strictness flags).
#[derive(Debug, Default)]
pub struct AnalysisOptions {
    /// Effective enabled lint rule names — the union across the include
    /// chain with nearer entries winning per name, sorted and deduplicated
    /// (the data the lint engine resolves its enable set from).
    pub lint_rules: Vec<String>,
    /// `analyzer.language.strict-casts` (`parse_model:904-998`), merged
    /// set-if-present across the include chain. Parsed and exposed only;
    /// #480 consumes it.
    pub strict_casts: Option<bool>,
    /// `analyzer.language.strict-inference`, same merge and caveat as
    /// [`AnalysisOptions::strict_casts`].
    pub strict_inference: Option<bool>,
    /// `analyzer.language.strict-raw-types`, same merge and caveat as
    /// [`AnalysisOptions::strict_casts`].
    pub strict_raw_types: Option<bool>,
    /// `analyzer.enable-experiment` flag strings, in declaration order —
    /// set-if-present across the include chain, a nearer file's list
    /// replacing a farther one whole (`_applyEnableExperiments` assigns
    /// `builder.contextFeatures`, parser:207-215). Parsed and exposed only;
    /// the engine consumes no experiment semantics.
    pub experiments: Option<Vec<String>>,
    /// Per-code severity overrides, keyed by lowercased code name; `None`
    /// filters the code, `Some` re-levels it (the official
    /// `ErrorProcessor` list's per-code map form).
    severity_overrides: FxHashMap<String, Option<Severity>>,
    /// Lowercased codes `// ignore:` comments cannot suppress (the official
    /// `unignorableDiagnosticCodeNames`).
    unignorable: FxHashSet<String>,
    /// Precompiled exclude globs, each anchored to its declaring file's
    /// directory (official `LocatedGlob`).
    excludes: Vec<ExcludeGlob>,
    /// The initial options file this merge loaded, normalized — the file
    /// every recorded row attributes to. Empty for the synthesized default
    /// (no options file in the root's ancestry).
    pub file_path: PathBuf,
    /// The initial file's text and line index, captured at load so the
    /// output faces resolve the rows' positions through the engine's layout
    /// fallback (the file is never resident as Dart).
    pub(crate) layout: Option<(Arc<str>, LineIndex)>,
    /// Self-diagnostics recorded during the load (include-chain rows at
    /// every depth; local shape rows for the initial file only), in walk
    /// order — the output face sorts them canonically.
    pub issues: Vec<OptionsIssue>,
    /// Lint names the initial file's own `linter.rules` declares, in
    /// declaration order (`undefined_lint` candidates; validity is the
    /// caller's registry seam).
    pub declared_rules: Vec<DeclaredRule>,
}

/// One compiled exclude pattern plus the directory it is relative to
/// (official `LocatedGlob`: parent + `package:glob`).
#[derive(Debug)]
pub struct ExcludeGlob {
    /// The declaring options file's directory; candidate paths outside it
    /// never match.
    parent: PathBuf,
    /// The pattern, plus the directory pattern for a `foo/**` spelling (the
    /// `**` alternative cannot match `foo` itself, and the walk must stop
    /// descending — context_locator.dart:633-638).
    alternatives: Vec<dartq_pub::glob::Glob>,
}

impl ExcludeGlob {
    /// Compiles one pattern string relative to `parent` (the
    /// `addGlob` + `posix.split` normalization of `context_locator.dart`:
    /// 625-639: empty and `.` components dropped, a trailing `**` adds the
    /// prefix pattern). Malformed glob syntax leaves the pattern out — the
    /// official surface reports it and keeps analyzing.
    fn compile(pattern: &str, parent: &Path) -> Option<ExcludeGlob> {
        // A leading `/` anchors the glob at the filesystem root
        // (`excludeInvalidGlob` in the official surface): it can never match
        // a path relative to the declaring directory, so it compiles to
        // nothing.
        if pattern.starts_with('/') {
            return None;
        }
        let components: Vec<&str> = pattern
            .split('/')
            .filter(|component| !component.is_empty() && *component != ".")
            .collect();
        let mut alternatives = vec![dartq_pub::glob::Glob::new(&components.join("/")).ok()?];
        if components.len() > 1
            && components.last() == Some(&"**")
            && let Ok(glob) =
                dartq_pub::glob::Glob::new(&components[..components.len() - 1].join("/"))
        {
            alternatives.push(glob);
        }
        Some(ExcludeGlob {
            parent: parent.to_path_buf(),
            alternatives,
        })
    }

    /// Whether `path` (absolute, same universe as the declaring file) is
    /// excluded: only paths inside the declaring directory can match, and
    /// they match relative to it (`LocatedGlob.matches`).
    #[must_use]
    pub fn matches(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.parent) else {
            return false;
        };
        let relative = relative.to_string_lossy();
        !relative.is_empty() && self.alternatives.iter().any(|g| g.matches_path(&relative))
    }
}

impl AnalysisOptions {
    /// Whether any exclude glob matches `path` (the context-root walk calls
    /// this per resource; globs are precompiled at load).
    #[must_use]
    pub fn is_excluded(&self, path: &Path) -> bool {
        self.excludes.iter().any(|glob| glob.matches(path))
    }

    /// The severity one diagnostic surfaces at: an `analyzer.errors`
    /// override for `code` (lowercase name) defers to it — `None` drops the
    /// diagnostic from the output, `Some` re-levels it — and without one the
    /// declared severity stands (`computeSeverity`,
    /// `analyzer_cli error_severity.dart`:14-26: "if there is a processor
    /// for this error, defer to it").
    #[must_use]
    pub fn processed_severity(&self, code: &str, declared: Severity) -> Option<Severity> {
        match self.severity_overrides.get(code) {
            Some(action) => *action,
            None => Some(declared),
        }
    }

    /// Whether `// ignore:`-comment suppression may drop `code` (lowercase
    /// name): codes in the `analyzer.cannot-ignore` set survive every
    /// suppression (library_analyzer.dart:552-563 — "don't allow
    /// un-ignorable codes to be ignored"). The suppression faces consult
    /// this at the same application point as [`AnalysisOptions::processed_severity`].
    #[must_use]
    pub fn is_ignorable(&self, code: &str) -> bool {
        !self.unignorable.contains(code)
    }

    /// Whether any `analyzer.errors` override merged in (the
    /// severity-application fast path decides with this: without overrides
    /// the application is the identity).
    pub(crate) fn has_severity_overrides(&self) -> bool {
        !self.severity_overrides.is_empty()
    }

    /// The compiled exclude globs, in declaration order (far to near). The
    /// lint face applies the same predicate as discovery — one rule, one
    /// place.
    #[must_use]
    pub fn exclude_globs(&self) -> &[ExcludeGlob] {
        &self.excludes
    }

    /// Locates the options file governing `start`: the nearest
    /// `analysis_options.yaml` at or above `start` (the official walk-up in
    /// `ContextLocator`), `None` when no ancestor declares one.
    #[must_use]
    pub fn find_for_dir(start: &Path) -> Option<PathBuf> {
        let mut dir = Some(start);
        while let Some(current) = dir {
            let candidate = current.join(OPTIONS_FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
            dir = current.parent();
        }
        None
    }

    /// Loads and merges the options file at `path`. Total: an unreadable or
    /// malformed file, a missing include, or a cycle all degrade to
    /// "that input contributes nothing" (official observable behavior); the
    /// returned options are always usable, and the degradation that the
    /// official surfaces as a diagnostic is recorded on
    /// [`AnalysisOptions::issues`].
    #[must_use]
    pub fn load(path: &Path, packages: &PackageConfig, config_dir: &Path) -> AnalysisOptions {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let normalized = dartq_pub::normalize(path);
        let mut merge = MergeState {
            initial: normalized.clone(),
            initial_text: Arc::from(text.as_str()),
            context_root: normalized.parent().unwrap_or(Path::new("")).to_path_buf(),
            ..MergeState::default()
        };
        let mut chain = FxHashSet::default();
        merge.apply_file(&normalized, &mut chain, packages, config_dir);
        merge.build()
    }

    /// One merged [`AnalysisOptions`] per governing directory, sharing
    /// loaded files through `loaded` (workspace members under one repo-root
    /// options file parse it once). Every package root maps to its entry:
    /// the nearest `analysis_options.yaml` in its ancestry, or the
    /// synthesized default (no file at all). Nested options files inside
    /// each root's discovery surfaces are located and mapped to their own
    /// directories — the official `optionsFileMap` (context_locator.dart
    /// :393-401 registers each nested file at its folder; the consumer
    /// resolves a file's options through the deepest mapped ancestor,
    /// context_builder.dart:220-224) — each parsed with its own include
    /// chain, not chained onto the root's. A nested file's exclude globs
    /// join the root's walk predicate, anchored to its own directory (the
    /// official `root.excludedGlobs.addAll`, context_locator.dart:402-409).
    #[must_use]
    pub fn for_roots(
        roots: &[PathBuf],
        packages: &PackageConfig,
        config_dir: &Path,
    ) -> FxHashMap<PathBuf, Arc<AnalysisOptions>> {
        let mut loaded: FxHashMap<PathBuf, Arc<AnalysisOptions>> = FxHashMap::default();
        let mut per_root = FxHashMap::default();
        for root in roots {
            let file = Self::find_for_dir(root);
            let options = match &file {
                Some(file) => loaded
                    .entry(file.clone())
                    .or_insert_with(|| Arc::new(Self::load(file, packages, config_dir)))
                    .clone(),
                None => Arc::new(Self::default()),
            };
            per_root.insert(root.clone(), options);
        }
        for root in roots {
            let base = per_root[root].clone();
            Self::locate_nested(root, base, &mut loaded, &mut per_root, packages, config_dir);
        }
        per_root
    }

    /// Walks `root`'s discovery surfaces (the same dot rule and exclude
    /// pruning discovery applies) registering every nested options file at
    /// its directory. Descent prunes against the globs accumulated so far —
    /// the root's own first, each nested file's as it registers — mirroring
    /// the official locator's walk under `containingRoot.excludedGlobs`
    /// (context_locator.dart:411-431). Termination: the queue holds real
    /// directories only (symlinked dirs are never entered, `entry.file_type`
    /// is link-host) over a finite tree.
    fn locate_nested(
        root: &Path,
        base: Arc<AnalysisOptions>,
        loaded: &mut FxHashMap<PathBuf, Arc<AnalysisOptions>>,
        out: &mut FxHashMap<PathBuf, Arc<AnalysisOptions>>,
        packages: &PackageConfig,
        config_dir: &Path,
    ) {
        const SURFACES: [&str; 4] = ["lib", "bin", "test", "tool"];
        // The accumulated governing entries: the root's own first, each
        // nested file's as it registers. Exclusion tests every entry's
        // globs, each anchored to its declaring directory.
        let mut governing: Vec<Arc<AnalysisOptions>> = vec![base];
        let excluded = |path: &Path, governing: &[Arc<AnalysisOptions>]| {
            governing.iter().any(|options| options.is_excluded(path))
        };
        let mut queue: Vec<PathBuf> = SURFACES.iter().map(|s| root.join(s)).collect();
        while let Some(dir) = queue.pop() {
            // Pop-time exclusion: a glob registered while this dir sat in
            // the queue (its parent batch found a nested options file) must
            // prune it — registration order inside one batch must not leak
            // through.
            if excluded(&dir, &governing) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if file_type.is_dir() {
                    if !name.starts_with('.') {
                        queue.push(path);
                    }
                } else if name == OPTIONS_FILE_NAME {
                    let options = loaded
                        .entry(path.clone())
                        .or_insert_with(|| Arc::new(Self::load(&path, packages, config_dir)))
                        .clone();
                    governing.push(Arc::clone(&options));
                    out.insert(dir.clone(), options);
                }
            }
        }
    }
}

/// The mutable accumulator one include-graph traversal folds into (the
/// official `_ApplyState`: a per-name rule map the nearer file's `addAll`
/// overwrites into, and the declaring-file-anchored exclude list).
#[derive(Default)]
struct MergeState {
    /// Rule name → enabled; insertion order is irrelevant (the built set is
    /// sorted).
    rules: FxHashMap<String, bool>,
    /// Per-code severity overrides; a nearer file's entry overwrites the
    /// code (the `processorsByCode` rebuild, parser:219-230).
    errors: FxHashMap<String, Option<Severity>>,
    /// The accumulated `unignorableDiagnosticCodeNames` (lowercased).
    unignorable: FxHashSet<String>,
    strict_casts: Option<bool>,
    strict_inference: Option<bool>,
    strict_raw_types: Option<bool>,
    experiments: Option<Vec<String>>,
    excludes: Vec<ExcludeGlob>,
    /// Deduplication key of the official `Set<AnalysisOptionsExcludePattern>`:
    /// (declaring file, pattern).
    exclude_keys: FxHashSet<(PathBuf, String)>,
    /// The normalized initial file: include-chain rows resolve back to it
    /// (`recursive_include_file`), local rows record only for it, and every
    /// row attributes to it (`include_walker.dart` reports all rows on the
    /// initial file).
    initial: PathBuf,
    /// The initial file's text — the span anchor every row searches.
    initial_text: Arc<str>,
    /// The initial file's directory — the `contextRootPath` message argument
    /// of `include_file_not_found` (walker :126-131). For the dominant
    /// single-package shape the options file sits at the context root, so
    /// this matches the official argument exactly.
    context_root: PathBuf,
    issues: Vec<OptionsIssue>,
    declared_rules: Vec<DeclaredRule>,
}

impl MergeState {
    /// Records one row, anchored at `token`'s first occurrence in the
    /// initial file (the span stand-in — `serde_yaml` keeps no node spans;
    /// see the module docs).
    fn record(&mut self, code: CodeInfo, token: &str, message: String, correction: Option<String>) {
        let (offset, length) = anchor_span(&self.initial_text, token);
        self.issues.push(OptionsIssue {
            code,
            message,
            correction,
            offset,
            length,
        });
    }

    /// The official `_applyParsedFiles`: apply every resolvable include
    /// first, then this file's own data — depth-first, near data last.
    fn apply_file(
        &mut self,
        file: &Path,
        chain: &mut FxHashSet<PathBuf>,
        packages: &PackageConfig,
        config_dir: &Path,
    ) {
        if !chain.insert(file.to_path_buf()) {
            return; // chain cycle not through the initial file: contributes
            // once (parser:395); the official row is
            // `included_file_warning`, a face dartq has not built.
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            chain.remove(file);
            return;
        };
        let Ok(yaml) = serde_yaml::from_str::<Value>(&text) else {
            chain.remove(file);
            return;
        };
        let is_initial = file == self.initial;

        for uri in include_uris(&yaml) {
            match resolve_include(&uri, file, packages, config_dir) {
                // The edge decision order mirrors `_walkInclude`
                // (include_walker.dart:121-151): a missing target reports
                // `include_file_not_found` and stops the edge; a parsed
                // target resolving back to the initial file reports
                // `recursive_include_file` and stops it too (:161-171 —
                // missing wins for unresolvable edges because recursion is
                // only reached for parsed ones); anything else recurses.
                Some(included) if included.is_file() => {
                    if included == self.initial {
                        self.record(
                            RECURSIVE_INCLUDE_FILE,
                            &uri,
                            format!(
                                "The URI '{uri}' included in '{}' includes '{}', \
                                 creating a circular reference.",
                                file.display(),
                                file.display()
                            ),
                            Some(
                                "Try changing the chain of 'include's to break the circularity."
                                    .to_owned(),
                            ),
                        );
                    } else {
                        self.apply_file(&included, chain, packages, config_dir);
                    }
                }
                _ => self.record(
                    INCLUDE_FILE_NOT_FOUND,
                    &uri,
                    format!(
                        "The URI '{uri}' included in '{}' can't be found when analyzing '{}'.",
                        file.display(),
                        self.context_root.display()
                    ),
                    None,
                ),
            }
        }

        let declaring_dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
        self.apply_local(&yaml, &declaring_dir, is_initial);
        chain.remove(file);
    }

    /// Applies one parsed file's own sections (the official
    /// `_ApplyState.apply`, the sections this engine consumes). The section
    /// order mirrors `_applyAnalyzer` (parser:164-171) where it matters:
    /// `errors` before `cannot-ignore`, so the severity arms see this
    /// file's re-levels. Local shape problems record for the initial file
    /// only (an included file's rows are officially wrapped — see the
    /// module docs).
    fn apply_local(&mut self, yaml: &Value, declaring_dir: &Path, is_initial: bool) {
        let Some(map) = yaml.as_mapping() else {
            return;
        };
        match map.get(Value::from("linter")) {
            // `linter:` absent or null: inherit what the includes merged.
            None | Some(Value::Null) => {}
            // Present but not a map: invalid section — clears everything
            // merged so far (parser:282-284).
            Some(Value::Mapping(linter)) => {
                if let Some(rules) = linter
                    .get(Value::from("rules"))
                    .filter(|rules| !rules.is_null())
                {
                    apply_rules(rules, &mut self.rules);
                    if is_initial {
                        self.declared_rules
                            .extend(declared_rule_names(rules, &self.initial_text));
                    }
                }
            }
            Some(_) => self.rules.clear(),
        }
        for (code, action) in severity_override_entries(map) {
            self.errors.insert(code, action);
        }
        apply_cannot_ignore(map, &mut self.unignorable, &self.errors);
        if let Some(patterns) = exclude_patterns(map) {
            for pattern in patterns {
                if self
                    .exclude_keys
                    .insert((declaring_dir.to_path_buf(), pattern.clone()))
                    && let Some(glob) = ExcludeGlob::compile(&pattern, declaring_dir)
                {
                    self.excludes.push(glob);
                }
            }
        }
        let (casts, inference, raw_types) = language_scalars(map);
        self.strict_casts = casts.or(self.strict_casts);
        self.strict_inference = inference.or(self.strict_inference);
        self.strict_raw_types = raw_types.or(self.strict_raw_types);
        if let Some(experiments) = experiment_flags(map) {
            self.experiments = Some(experiments);
        }
        if is_initial {
            self.issues.extend(local_issues(map, &self.initial_text));
        }
    }

    /// Freezes the merged state: enabled rule names, sorted (the canonical
    /// order the official `build()` derives through the registry).
    fn build(self) -> AnalysisOptions {
        let mut names: Vec<String> = self
            .rules
            .into_iter()
            .filter_map(|(name, enabled)| enabled.then_some(name))
            .collect();
        names.sort();
        names.dedup();
        let line_index = LineIndex::new(&self.initial_text);
        AnalysisOptions {
            lint_rules: names,
            strict_casts: self.strict_casts,
            strict_inference: self.strict_inference,
            strict_raw_types: self.strict_raw_types,
            experiments: self.experiments,
            severity_overrides: self.errors,
            unignorable: self.unignorable,
            excludes: self.excludes,
            file_path: self.initial,
            layout: Some((self.initial_text, line_index)),
            issues: self.issues,
            declared_rules: self.declared_rules,
        }
    }
}

/// Merges one file's `linter.rules` value into the rule map
/// (`parseDiagnosticsSection`, lint/config.dart:20-64): a list of names, or
/// a map of per-rule entries; unknown shapes contribute nothing.
fn apply_rules(rules: &Value, merged: &mut FxHashMap<String, bool>) {
    match rules {
        Value::Sequence(list) => {
            for item in list {
                if let Some(name) = item.as_str() {
                    merged.insert(name.to_owned(), true);
                }
            }
        }
        Value::Mapping(map) => {
            for (key, value) in map {
                let Some(name) = key.as_str() else {
                    continue;
                };
                merge_rule_entry(name, value, merged);
            }
        }
        _ => {}
    }
}

/// One `name: value` rule entry (`_parseRuleConfig`, lint/config.dart
/// :83-104): `true`/`false`, a severity string (`disable` disables, any
/// other string enables), or a nested group map recursed entry-wise; null
/// and number values are skipped.
fn merge_rule_entry(name: &str, value: &Value, merged: &mut FxHashMap<String, bool>) {
    match value {
        Value::Bool(enabled) => {
            merged.insert(name.to_owned(), *enabled);
        }
        Value::String(severity) => {
            merged.insert(name.to_owned(), severity != "disable");
        }
        Value::Mapping(group) => {
            for (key, value) in group {
                if let Some(rule) = key.as_str() {
                    merge_rule_entry(rule, value, merged);
                }
            }
        }
        _ => {}
    }
}

/// The `analyzer.exclude` pattern strings (the `_ParsedExcludesData.parse`
/// shape: list-only, string entries only, everything else ignored).
fn exclude_patterns(map: &serde_yaml::Mapping) -> Option<impl Iterator<Item = String>> {
    let excludes = map.get(Value::from("analyzer"))?.as_mapping()?;
    match excludes.get(Value::from("exclude")) {
        Some(Value::Sequence(list)) => Some(
            list.iter()
                .filter_map(|pattern| pattern.as_str().map(str::to_owned)),
        ),
        _ => None,
    }
}

/// The `analyzer.errors` per-code actions (`_ParsedErrorProcessorsData.parse`,
/// `analysis_options_parse_model.dart`:541-631): a map of string code →
/// scalar action, the action stringified and lowercased
/// (`ErrorConfig._processMap`, `error_processor.dart`:36-54) —
/// `ignore`/`false` filters the code (`ignoreSynonyms`,
/// `analysis_options_file.dart`:46), `error`/`warning`/`info` re-levels it
/// (`severityMap`, `error_processor.dart`:14-18), and any other scalar
/// contributes nothing (the official reports and skips). The merged code is
/// lowercased (the `ErrorProcessor` constructor, `error_processor.dart`:71);
/// non-string keys and non-map sections are skipped/invalid.
fn severity_override_entries(map: &serde_yaml::Mapping) -> Vec<(String, Option<Severity>)> {
    let Some(entries) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("errors")))
        .and_then(|errors| errors.as_mapping())
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(key, value)| {
            let code = key.as_str()?.to_lowercase();
            let action = action_scalar(value)?;
            let action = match action.as_str() {
                "ignore" | "false" => None,
                "error" => Some(Severity::Error),
                "warning" => Some(Severity::Warning),
                "info" => Some(Severity::Info),
                _ => return None,
            };
            Some((code, action))
        })
        .collect()
}

/// One `errors` action scalar, stringified and lowercased the way
/// `valueNode.value.toString().toLowerCase()` does: strings verbatim, YAML
/// booleans as `true`/`false` (so bare `code: false` filters); everything
/// else (numbers, null, containers) is illegal.
fn action_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.to_lowercase()),
        Value::Bool(enabled) => Some(if *enabled { "true" } else { "false" }.to_owned()),
        _ => None,
    }
}

/// Applies one file's `analyzer.cannot-ignore` list (the official
/// `_applyCannotIgnore`, parser:173-197): a severity name marks every
/// registry code whose effective severity — `errors`, the overrides merged
/// through this file, else the registry severity — equals it; every other
/// name lowercases into the unignorable set (names are not
/// registry-validated there either).
fn apply_cannot_ignore(
    map: &serde_yaml::Mapping,
    unignorable: &mut FxHashSet<String>,
    errors: &FxHashMap<String, Option<Severity>>,
) {
    let Some(names) = cannot_ignore_names(map) else {
        return;
    };
    let arms: Vec<Severity> = names
        .iter()
        .filter_map(|name| severity_of_name(name))
        .collect();
    if !arms.is_empty() {
        let registry = CodeRegistry::builtin();
        for arm in arms {
            for code in registry.iter() {
                let effective = match errors.get(code.name) {
                    Some(Some(severity)) => Some(*severity),
                    Some(None) => None,
                    None => Some(code.dtype.severity()),
                };
                if effective == Some(arm) {
                    unignorable.insert(code.name.to_owned());
                }
            }
        }
    }
    for name in names {
        if severity_of_name(&name).is_none() {
            unignorable.insert(name.to_lowercase());
        }
    }
}

/// The `analyzer.cannot-ignore` name strings (`_ParsedCannotIgnoreData.parse`,
/// parse_model:346-405): list-only, string entries only, everything else
/// ignored.
fn cannot_ignore_names(map: &serde_yaml::Mapping) -> Option<Vec<String>> {
    let analyzer = map.get(Value::from("analyzer"))?.as_mapping()?;
    match analyzer.get(Value::from("cannot-ignore")) {
        Some(Value::Sequence(list)) => Some(
            list.iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// The severity a `cannot-ignore` arm names (`severityMap[name]`,
/// error_processor.dart:14-18 — case-sensitive, so `ERROR` is an explicit
/// code name, not an arm).
fn severity_of_name(name: &str) -> Option<Severity> {
    match name {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "info" => Some(Severity::Info),
        _ => None,
    }
}

/// The `analyzer.language` strictness booleans (`_ParsedLanguageData.parse`,
/// parse_model:904-998): map-only section; the three strict-* keys parse
/// through the `toBool` conversion (util/yaml.dart:156-169 — YAML booleans
/// or `true`/`false` strings, case-insensitive), non-boolean values and
/// unknown keys contribute nothing. `(casts, inference, raw_types)`.
fn language_scalars(map: &serde_yaml::Mapping) -> (Option<bool>, Option<bool>, Option<bool>) {
    let Some(language) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("language")))
        .and_then(|language| language.as_mapping())
    else {
        return (None, None, None);
    };
    let parse = |key: &str| -> Option<bool> {
        match language.get(Value::from(key)) {
            Some(Value::Bool(value)) => Some(*value),
            Some(Value::String(text)) => match text.to_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    };
    (
        parse("strict-casts"),
        parse("strict-inference"),
        parse("strict-raw-types"),
    )
}

// --- Local shape rows (the initial file's own sections, #457) ---

/// `Invalid option specified for '{0}': {1}` (registry :9521-9535 —
/// `STATIC_WARNING`; the detail message comes from the flag validation).
const INVALID_OPTION: CodeInfo = CodeInfo::new(
    "invalid_option",
    "INVALID_OPTION",
    DiagnosticType::StaticWarning,
    true,
);

/// The experimental flags the pinned SDK knows (`experiments.g.dart`,
/// dart 3.15 main — the `EnableString` table): the unknown-flag gate of
/// [`experiment_flag_issues`]. Version-pinned data — a flag outside this
/// list reports `unsupported_option` exactly as the official
/// `validateFlags` does for its `_knownFeatures` miss.
const KNOWN_EXPERIMENTS: [&str; 48] = [
    "anonymous-methods",
    "augmentations",
    "class-modifiers",
    "const-functions",
    "constant-update-2018",
    "constructor-tearoffs",
    "control-flow-collections",
    "data-assets",
    "digit-separators",
    "dot-shorthands",
    "enhanced-enums",
    "enhanced-parts",
    "extension-methods",
    "generic-metadata",
    "getter-setter-error",
    "inference-update-1",
    "inference-update-2",
    "inference-update-3",
    "inference-update-4",
    "inference-using-bounds",
    "inline-class",
    "macros",
    "named-arguments-anywhere",
    "native-assets",
    "non-nullable",
    "nonfunction-type-aliases",
    "null-aware-elements",
    "patterns",
    "primary-constructors",
    "private-named-parameters",
    "promotion-chain-intersection-join",
    "receiver-type-inference",
    "record-use",
    "records",
    "sealed-class",
    "set-literals",
    "single-combinators",
    "sound-flow-analysis",
    "spread-collections",
    "static-extensions",
    "super-parameters",
    "test-experiment",
    "this-promotion",
    "triple-shift",
    "unnamed-libraries",
    "unquoted-imports",
    "variance",
    "wildcard-variables",
];

/// The expired subset of [`KNOWN_EXPERIMENTS`] (`IsExpired`, same pinned
/// table — all 31 are enabled-by-default, their behavior hardcoded into the
/// toolchain): expired flags always report `invalid_option`, with the
/// detail choosing "no longer required" (request matches the hardcoded
/// state) vs "unconditionally enabled" (it contradicts it).
const EXPIRED_EXPERIMENTS: [&str; 31] = [
    "class-modifiers",
    "constant-update-2018",
    "constructor-tearoffs",
    "control-flow-collections",
    "digit-separators",
    "dot-shorthands",
    "enhanced-enums",
    "extension-methods",
    "generic-metadata",
    "getter-setter-error",
    "inference-update-1",
    "inference-update-2",
    "inference-update-3",
    "inference-using-bounds",
    "inline-class",
    "named-arguments-anywhere",
    "native-assets",
    "non-nullable",
    "nonfunction-type-aliases",
    "null-aware-elements",
    "patterns",
    "private-named-parameters",
    "records",
    "sealed-class",
    "set-literals",
    "sound-flow-analysis",
    "spread-collections",
    "super-parameters",
    "triple-shift",
    "unnamed-libraries",
    "wildcard-variables",
];

/// The `analyzer.enable-experiment` flag strings (the merge-side read of
/// `_ParsedEnableExperimentsData.parse`, parse_model:470-536): list-only,
/// string entries only — a non-list section is `None` (nothing applies),
/// non-string entries are skipped. `(flags, present)`.
fn experiment_flags(map: &serde_yaml::Mapping) -> Option<Vec<String>> {
    let experiments = map.get(Value::from("analyzer"))?.as_mapping()?;
    match experiments.get(Value::from("enable-experiment")) {
        Some(Value::Sequence(list)) => Some(
            list.iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// The `validateFlags` rows for one file's flag list (experiments_impl.dart
/// :183-222, reported per parse_model:510-524): an unknown flag — `no-`
/// stripped before the known-feature test — reports `unsupported_option`
/// (the without-values variant), an expired flag reports `invalid_option`
/// ("no longer required" when the request matches the hardcoded state,
/// "unconditionally enabled" otherwise), and an opposite second request for
/// an already-requested feature reports the conflict. Anchors at the flag
/// token's first occurrence.
fn experiment_flag_issues(flags: &[String], text: &str) -> Vec<OptionsIssue> {
    let mut issues = Vec::new();
    let mut requested: FxHashMap<&str, bool> = FxHashMap::default();
    for flag in flags {
        let (feature, value) = match flag.strip_prefix("no-") {
            Some(feature) => (feature, false),
            None => (flag.as_str(), true),
        };
        if !KNOWN_EXPERIMENTS.contains(&feature) {
            let (offset, length) = anchor_span(text, flag);
            issues.push(OptionsIssue {
                code: UNSUPPORTED_OPTION,
                message: format!("The option '{flag}' isn't supported by 'enable-experiment'."),
                correction: Some("Try removing the option.".to_owned()),
                offset,
                length,
            });
        } else if EXPIRED_EXPERIMENTS.contains(&feature) {
            let detail = if value {
                format!("Flag \"{flag}\" is no longer required.")
            } else {
                format!(
                    "Flag \"{flag}\" was supplied, but the feature is already \
                     unconditionally enabled."
                )
            };
            let (offset, length) = anchor_span(text, flag);
            issues.push(OptionsIssue {
                code: INVALID_OPTION,
                message: format!("Invalid option specified for 'enable-experiment': {detail}"),
                correction: None,
                offset,
                length,
            });
        } else if requested.get(feature) == Some(&!value) {
            let previous = if value {
                format!("no-{feature}")
            } else {
                feature.to_owned()
            };
            let (offset, length) = anchor_span(text, flag);
            issues.push(OptionsIssue {
                code: INVALID_OPTION,
                message: format!(
                    "Invalid option specified for 'enable-experiment': \
                     Flag \"{flag}\" conflicts with previous flag \"{previous}\""
                ),
                correction: None,
                offset,
                length,
            });
        } else {
            // Not a conflict (first request, or a same-value repeat): the
            // request records, matching the official else arm.
            requested.insert(feature, value);
        }
    }
    issues
}

/// The `analyzer` keys the official supports (`analyzerOptions`,
/// analysis_options_file.dart:99-108, in the set literal's insertion order —
/// the proposal text follows it).
const ANALYZER_OPTIONS: [&str; 8] = [
    "cannot-ignore",
    "enable-experiment",
    "errors",
    "exclude",
    "language",
    "optional-checks",
    "plugins",
    "strong-mode",
];

/// The `analyzer.language` keys (`languageOptions`, `analysis_options_file.dart`
/// :103-107).
const LANGUAGE_OPTIONS: [&str; 3] = ["strict-casts", "strict-inference", "strict-raw-types"];

/// The legal `errors` action values, in the official list order with its
/// duplicate preserved verbatim — `errorProcessorLegalValues` is
/// `[...ignoreSynonyms, ...trueOrFalse, ...severities]` (parse_model:55-59)
/// over `['ignore', 'false']` + `['true', 'false']` + `severityMap.keys`
/// (error_processor.dart:14-19: error, info, warning) — and the correction
/// text prints the list as-is.
const ERROR_ACTIONS: [&str; 7] = [
    "ignore", "false", "true", "false", "error", "info", "warning",
];

/// The lowercased actions that carry meaning (`ignoreSynonyms` filter,
/// `severityMap` re-levels; anything else contributes nothing and reports).
const EFFECTIVE_ERROR_ACTIONS: [&str; 5] = ["ignore", "false", "error", "info", "warning"];

/// The initial file's local shape rows — one walk over the sections this
/// engine consumes, mirroring the official shapes: `analyzer` (`parse_model`
/// :263-305: null-tolerant, non-map → row, unknown scalar keys →
/// `unsupported_option`), `errors` (:551-612: non-map or container
/// key/value → row, illegal action → `unsupported_option` with the legal
/// values), `cannot-ignore` (:356-397: non-list or non-string entry → row),
/// `exclude` (:644-670: same shape), `language` (:916-970: null-tolerant,
/// non-map → row, unknown key → `unsupported_option`, container value → row
/// naming the option), `linter` (:1130-1184: null-tolerant, non-map → row,
/// unknown key → singular `unsupported_option`). Scalar non-string `errors`
/// keys report `unrecognized_error_code`, non-boolean `language` values
/// report `unsupported_value` — faces dartq has not built, silent here; the
/// sections the engine does not consume (code-style, formatter, plugins,
/// strong-mode, optional-checks) are silent too. `enable-experiment`
/// (:470-536: non-list → row, non-string entry → row, then the
/// `validateFlags` rows) is pinned in [`experiment_flag_issues`].
fn local_issues(map: &serde_yaml::Mapping, text: &str) -> Vec<OptionsIssue> {
    let mut issues = Vec::new();
    let analyzer = match map.get(Value::from("analyzer")) {
        None | Some(Value::Null) => None,
        Some(Value::Mapping(analyzer)) => {
            for key in analyzer.keys() {
                if let Some(name) = key.as_str()
                    && !ANALYZER_OPTIONS.contains(&name)
                {
                    push_unsupported(&mut issues, text, name, "analyzer", &ANALYZER_OPTIONS);
                }
            }
            Some(analyzer)
        }
        Some(_) => {
            push_invalid_section(&mut issues, text, "analyzer", "analyzer");
            None
        }
    };
    if let Some(analyzer) = analyzer {
        // `errors` (:551-612).
        match analyzer.get(Value::from("errors")) {
            None => {}
            Some(Value::Mapping(entries)) => {
                for (key, value) in entries {
                    // A container key is an invalid section entry
                    // (:577-582); a scalar non-string key is
                    // `unrecognized_error_code` (silent).
                    if matches!(key, Value::Mapping(_) | Value::Sequence(_)) {
                        push_invalid_section(&mut issues, text, "errors", "errors");
                        continue;
                    }
                    // The action stringifies (`valueNode.value.toString()`,
                    // :600) — booleans, numbers, and null too.
                    let Some(action) = yaml_scalar_text(value) else {
                        push_invalid_section(&mut issues, text, "errors", "errors");
                        continue;
                    };
                    if !EFFECTIVE_ERROR_ACTIONS.contains(&action.to_lowercase().as_str()) {
                        push_unsupported(&mut issues, text, &action, "errors", &ERROR_ACTIONS);
                    }
                }
            }
            Some(_) => push_invalid_section(&mut issues, text, "errors", "errors"),
        }
        // `cannot-ignore` (:356-397) — not null-tolerant (`valueAt` returns
        // the node, so a present-but-empty entry is a scalar).
        match analyzer.get(Value::from("cannot-ignore")) {
            None => {}
            Some(Value::Sequence(list)) => {
                for entry in list {
                    if entry.as_str().is_none() {
                        push_invalid_section(&mut issues, text, "cannot-ignore", "cannot-ignore");
                    }
                }
            }
            Some(_) => push_invalid_section(&mut issues, text, "cannot-ignore", "cannot-ignore"),
        }
        // `exclude` (:644-670) — same not-null-tolerant shape.
        match analyzer.get(Value::from("exclude")) {
            None => {}
            Some(Value::Sequence(list)) => {
                for entry in list {
                    if entry.as_str().is_none() {
                        push_invalid_section(&mut issues, text, "exclude", "exclude");
                    }
                }
            }
            Some(_) => push_invalid_section(&mut issues, text, "exclude", "exclude"),
        }
        // `language` (:916-970) — null-tolerant.
        match analyzer.get(Value::from("language")) {
            None | Some(Value::Null) => {}
            Some(Value::Mapping(language)) => {
                for (key, value) in language {
                    let Some(name) = key.as_str() else {
                        continue;
                    };
                    if !LANGUAGE_OPTIONS.contains(&name) {
                        push_unsupported(&mut issues, text, name, "language", &LANGUAGE_OPTIONS);
                    } else if matches!(value, Value::Mapping(_) | Value::Sequence(_)) {
                        // A container value is an invalid section (:933-940,
                        // sectioned by the option name); a non-boolean
                        // scalar is `unsupported_value` (silent).
                        push_invalid_section(&mut issues, text, name, name);
                    }
                }
            }
            Some(_) => push_invalid_section(&mut issues, text, "language", "language"),
        }
        // `enable-experiment` (parse_model:470-536) — not null-tolerant
        // (`valueAt` returns the node, so a present-but-empty entry is a
        // scalar); the flag validation rides the shape rows.
        match analyzer.get(Value::from("enable-experiment")) {
            None => {}
            Some(Value::Sequence(list)) => {
                let mut flags = Vec::new();
                for entry in list {
                    match entry.as_str() {
                        Some(flag) => flags.push(flag.to_owned()),
                        None => push_invalid_section(
                            &mut issues,
                            text,
                            "enable-experiment",
                            "enable-experiment",
                        ),
                    }
                }
                issues.extend(experiment_flag_issues(&flags, text));
            }
            Some(_) => {
                push_invalid_section(&mut issues, text, "enable-experiment", "enable-experiment")
            }
        }
    }
    // `linter` (:1130-1184) — null-tolerant; the unknown-key proposal is
    // singular (`linterOptions` = {'rules'}).
    match map.get(Value::from("linter")) {
        None | Some(Value::Null) => {}
        Some(Value::Mapping(linter)) => {
            for key in linter.keys() {
                if let Some(name) = key.as_str()
                    && name != "rules"
                {
                    push_unsupported(&mut issues, text, name, "linter", &["rules"]);
                }
            }
        }
        Some(_) => push_invalid_section(&mut issues, text, "linter", "linter"),
    }
    issues
}

/// Records one `invalid_section_format` row (`Invalid format for the '{0}'
/// section.`, registry :9708-9722), anchored at `token`.
fn push_invalid_section(issues: &mut Vec<OptionsIssue>, text: &str, section: &str, token: &str) {
    let (offset, length) = anchor_span(text, token);
    issues.push(OptionsIssue {
        code: INVALID_SECTION_FORMAT,
        message: format!("Invalid format for the '{section}' section."),
        correction: None,
        offset,
        length,
    });
}

/// Records one `unsupported_option` row (`The option '{1}' isn't supported
/// by '{0}'.`, registry :17832-17905) with the correction matching the
/// legal-value count — singular (`unsupportedOptionWithLegalValue`) or the
/// `quotedAndCommaSeparatedWithAnd` proposal (`…WithLegalValues`).
fn push_unsupported(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    option: &str,
    section: &str,
    legal: &[&str],
) {
    let correction = match legal {
        [one] => format!("Try using the only supported option: '{one}', or removing the option."),
        _ => format!(
            "Try using one of the supported options: {}.",
            proposal(legal)
        ),
    };
    let (offset, length) = anchor_span(text, option);
    issues.push(OptionsIssue {
        code: UNSUPPORTED_OPTION,
        message: format!("The option '{option}' isn't supported by '{section}'."),
        correction: Some(correction),
        offset,
        length,
    });
}

/// The `quotedAndCommaSeparatedWithAnd` form (string.dart:72-107): one
/// element bare-quoted, two joined by ` and `, three or more comma-separated
/// with the Oxford comma before `and`.
fn proposal(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [one] => format!("'{one}'"),
        [first, second] => format!("'{first}' and '{second}'"),
        _ => {
            let mut parts: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
            let last = parts.pop().expect("non-empty above");
            format!("{}, and {}", parts.join(", "), last)
        }
    }
}

/// The `valueNode.value.toString()` form of one scalar: strings verbatim,
/// booleans as `true`/`false`, numbers in their decimal form, null as
/// `null`; containers are no scalar and report instead.
fn yaml_scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(enabled) => Some(if *enabled { "true" } else { "false" }.to_owned()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null => Some("null".to_owned()),
        _ => None,
    }
}

/// The lint names the initial file's own `linter.rules` declares
/// (`_parseLocalRules`, parse_model:1224-1378): list entries and map keys —
/// a `name:` entry with a null value returns before the registry check
/// (:1252-1253), group maps are not descended (the local parse validates
/// top-level entries only; the merge keeps its config.dart recursion), and
/// non-string names are skipped (:1238-1239).
fn declared_rule_names(rules: &Value, text: &str) -> Vec<DeclaredRule> {
    let mut names = Vec::new();
    let mut push = |name: &str| {
        let (offset, length) = anchor_span(text, name);
        names.push(DeclaredRule {
            name: name.to_owned(),
            offset,
            length,
        });
    };
    match rules {
        Value::Sequence(list) => {
            for entry in list {
                if let Some(name) = entry.as_str() {
                    push(name);
                }
            }
        }
        Value::Mapping(map) => {
            for (key, value) in map {
                if let Some(name) = key.as_str()
                    && !value.is_null()
                {
                    push(name);
                }
            }
        }
        _ => {}
    }
    names
}

/// UTF-16 `(offset, length)` of `token`'s first occurrence in `text` — the
/// span stand-in for the official node spans (`serde_yaml` keeps none; see
/// the module docs). `(0, 0)` when the token does not occur.
fn anchor_span(text: &str, token: &str) -> (u32, u32) {
    match text.find(token) {
        Some(byte) => (byte_offset_to_utf16(text, byte), utf16_len_of(token)),
        None => (0, 0),
    }
}

/// The `include:` URI strings of one options file: a single string or a list
/// whose string entries are taken in order (parser:589-602; non-string
/// entries are ignored there too).
fn include_uris(yaml: &Value) -> Vec<String> {
    let Some(map) = yaml.as_mapping() else {
        return Vec::new();
    };
    match map.get(Value::from("include")) {
        Some(Value::String(uri)) => vec![uri.clone()],
        Some(Value::Sequence(list)) => list
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Resolves one include URI the way `sourceFactory.resolveUri` does against
/// the including file: relative paths against its directory, `package:` URIs
/// through the package config, `file:` URIs absolute; `None` for schemes
/// with no filesystem target (the include is then skipped).
fn resolve_include(
    uri: &str,
    including_file: &Path,
    packages: &PackageConfig,
    config_dir: &Path,
) -> Option<PathBuf> {
    if let Some(rest) = uri.strip_prefix("package:") {
        let (name, subpath) = rest.split_once('/')?;
        if name.is_empty() || subpath.is_empty() {
            return None;
        }
        let entry = packages.entry(name)?;
        let root = entry.resolve_root(config_dir).ok()?;
        let path = root.join(&entry.package_uri).join(percent_decode(subpath));
        Some(dartq_pub::normalize(&path))
    } else if let Some(rest) = uri.strip_prefix("file:") {
        let path = rest.strip_prefix("//").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        Some(PathBuf::from(percent_decode(path)))
    } else if uri.contains(':') {
        // Any other scheme (`dart:`, `http:`, …) has no options-file target.
        None
    } else {
        let base = including_file.parent().unwrap_or(Path::new(""));
        Some(dartq_pub::normalize(&base.join(percent_decode(uri))))
    }
}

/// Decodes `%XX` escapes (the URI→path step of `FileSource` resolution);
/// text without `%` passes through untouched.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
