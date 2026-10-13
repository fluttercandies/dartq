//! `analysis_options.yaml` loading and include-chain merge (PRD §12.1/§12.2).
//!
//! Semantics are pinned to the official merge in the upstream Dart SDK
//! source (`pkg/analyzer/lib/src/analysis_options/`):
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
//!   enabled"), and an opposite re-request reports the conflict. The flags
//!   also reach the per-file feature sets: the grant-free package branch of
//!   `FeatureGrants` applies them (state.rs, #936 — the probe-pinned
//!   replace posture).
//! - **Self-diagnostics (#457)** — the options file's own problems surface
//!   instead of degrading silently. The official reports every options
//!   diagnostic on the *initial* file: include-chain rows directly at the
//!   initial include span (`_AnalysisOptionsIncludeWalker`, `include_walker.dart`
//!   :121-151 — a missing include reports `include_file_not_found` and stops
//!   that edge; a parsed include resolving back to the initial file reports
//!   `recursive_include_file`, :161-171; a chain cycle not through the initial
//!   file is the `included_file_warning` row "The file includes itself
//!   recursively.", :173-188), while the initial file's local shape problems
//!   report their raw codes (`invalid_section_format`, `unsupported_option`,
//!   `undefined_lint`, `analysis_options_parse_model.dart`) and *included*
//!   files' local problems wrap into `included_file_warning` — "Warning in
//!   the included options file {path}({start}..{end}): {message}",
//!   `include_walker.dart:210-248` (the inner offsets inclusive, the
//!   correction dropped). A malformed YAML file reports `parse_error` when
//!   initial — the registry `COMPILE_TIME_ERROR`, message = the YAML error's
//!   own text, exit tier 3 through the CLI severity arithmetic — and
//!   `included_file_parse_error` ("{message} in {path}({start}..{end})")
//!   when included (`include_walker.dart:48-66`). Codes and message templates
//!   are verbatim from the pinned registry (`diagnostic.g.dart`). `serde_yaml`
//!   keeps no spans, so rows anchor at the token text's occurrence in the
//!   initial file: a section's *value* tokens (include URIs, error actions,
//!   flag and rule names) search from their section key onward
//!   (`anchor_in_section`), keys at their first occurrence (`anchor_span`);
//!   the `(code, file)` differential face is exact regardless.
//!   `undefined_lint` needs the lint registry, which lives in dartq-lints (an
//!   engine dependency would cycle) — the engine records the declared rule
//!   names per file (the initial file in
//!   [`AnalysisOptions::declared_rules`], each included file in a
//!   [`DeclarationGroup`]) and the CLI face (dartq-commands) validates them
//!   against `rule_by_name`, wrapping the included files' rows. Rows ride the
//!   same severity face as every other output row (`Engine::analyze` appends
//!   them before `apply_severity_options`), and the options file — never
//!   resident as Dart — resolves its rows' positions through the engine's
//!   layout fallback (`Engine::content`/`line_index` over the text +
//!   [`LineIndex`] captured at load).
//! - The top-level `plugins:` section reports `invalid_section_format`
//!   (probed on the 3.13.1 oracle — the section's only CLI-observable faces
//!   are that row plus stderr load noise) and dartq loads no plugins: any
//!   declared section is the row, silent only when absent or null.
//! - Unreadable files are tolerated — the official reports unresolvable
//!   includes and still produces effective options (missing include →
//!   skipped).
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
// variants share one name/machine form; the variant is the correction text),
// unrecognized_error_code :17753-17762. All STATIC_WARNING rows, published
// docs — the severity is the single `DiagnosticType::severity` derivation at
// the emit sites.

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

/// `'{0}' isn't a recognized diagnostic code.` (registry :17753-17762;
/// emitted by the CLI face, which owns the code-registry seam.)
pub const UNRECOGNIZED_ERROR_CODE: CodeInfo = CodeInfo::new(
    "unrecognized_error_code",
    "UNRECOGNIZED_ERROR_CODE",
    DiagnosticType::StaticWarning,
    true,
);

/// `'{0}' was removed in Dart '{1}'` (registry :15098-15116; `{1}` is the
/// removal's `since` version — `"null"` for the unversioned state — and the
/// texts render from the lint registry's removed specs at the CLI face,
/// which owns that registry; the engine holds no lint registry).
pub const REMOVED_LINT: CodeInfo = CodeInfo::new(
    "removed_lint",
    "REMOVED_LINT",
    DiagnosticType::StaticWarning,
    true,
);

/// `The lint rule '{0}' is deprecated and shouldn't be enabled.`
/// (registry :3904-3925; emitted by the CLI face, which owns the
/// deprecated-state data the SDK registry attaches to the rule.)
pub const DEPRECATED_LINT: CodeInfo = CodeInfo::new(
    "deprecated_lint",
    "DEPRECATED_LINT",
    DiagnosticType::StaticWarning,
    true,
);

/// `{0}` — the YAML error's own text (registry :13901-13910,
/// `COMPILE_TIME_ERROR` → ERROR: the row drives the CLI's exit tier 3).
const PARSE_ERROR: CodeInfo = CodeInfo::new(
    "parse_error",
    "PARSE_ERROR",
    DiagnosticType::CompileTimeError,
    true,
);

/// `Warning in the included options file {0}({1}..{2}): {3}` (registry
/// :7728-7746). `{1}..{2}` is the inner row's inclusive span; the correction
/// is dropped (the official wrap carries the message only).
const INCLUDED_FILE_WARNING: CodeInfo = CodeInfo::new(
    "included_file_warning",
    "INCLUDED_FILE_WARNING",
    DiagnosticType::StaticWarning,
    true,
);

/// `{3} in {0}({1}..{2})` — the YAML error text, then the included file and
/// its failure span (registry :7696-7712, `COMPILE_TIME_ERROR` → ERROR).
const INCLUDED_FILE_PARSE_ERROR: CodeInfo = CodeInfo::new(
    "included_file_parse_error",
    "INCLUDED_FILE_PARSE_ERROR",
    DiagnosticType::CompileTimeError,
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

    /// The `unrecognized_error_code` row for one unknown `analyzer.errors`
    /// key (parse_model:569-583 — the registry check fires as the key is
    /// read, before the value is inspected, so any action spelling reports
    /// alongside; no correction — the registry entry carries none).
    #[must_use]
    pub fn unrecognized_error_code(error: &DeclaredError) -> Self {
        OptionsIssue {
            code: UNRECOGNIZED_ERROR_CODE,
            message: format!("'{}' isn't a recognized diagnostic code.", error.code),
            correction: None,
            offset: error.offset,
            length: error.length,
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

/// One `analyzer.errors` key declared by the initial file, in its original
/// spelling with its anchor span — the `unrecognized_error_code` candidate
/// list. Non-string scalar keys record their `value.toString()` form
/// (`parse_model:578-584` reports them unconditionally); validity is the
/// caller's seam, the same split that keeps the lint registry out of the
/// engine for [`DeclaredRule`].
#[derive(Debug, Clone)]
pub struct DeclaredError {
    pub code: String,
    pub offset: u32,
    pub length: u32,
}

/// One included options file's registry-candidate declarations: the file's
/// `linter.rules` names and `analyzer.errors` keys, with the inner-file
/// anchors and the precomputed initial-file span their wrapped rows anchor
/// at (the branch's first include directive — `initialIncludeSpan`,
/// `include_walker.dart:90`). The CLI face validates the names against its
/// registries and wraps every row into `included_file_warning`; the engine
/// holds no registry to do it at load.
#[derive(Debug, Clone)]
pub struct DeclarationGroup {
    /// The included file's normalized path (the wrapped message's `{0}`).
    pub file: PathBuf,
    /// The row anchor in the initial file's text: the branch's first include
    /// directive URI token, resolved to its `(offset, length)` (UTF-16).
    pub anchor_span: (u32, u32),
    /// The file's own `linter.rules` names (`undefined_lint` candidates,
    /// anchored in the included file's text).
    pub rules: Vec<DeclaredRule>,
    /// The file's own `analyzer.errors` keys (`unrecognized_error_code`
    /// candidates, anchored in the included file's text).
    pub errors: Vec<DeclaredError>,
}

impl DeclarationGroup {
    /// The official wrap face for one of this group's inner rows
    /// (`include_walker.dart:210-248`): the wrapped row anchors at the
    /// branch's first include directive in the initial file, the message
    /// names the included file and the inner span's inclusive end
    /// (`offset+length-1`), and the inner correction never survives the
    /// wrap (the official listener carries the message only).
    #[must_use]
    pub fn wrap_row(&self, inner: &OptionsIssue) -> OptionsIssue {
        OptionsIssue {
            code: INCLUDED_FILE_WARNING,
            message: format!(
                "Warning in the included options file {}({}..{}): {}",
                self.file.display(),
                inner.offset,
                inner.offset + inner.length.saturating_sub(1),
                inner.message
            ),
            correction: None,
            offset: self.anchor_span.0,
            length: self.anchor_span.1,
        }
    }
}

/// One governing directory's fully merged analysis options (the
/// `ResolvedAnalysisOptions` slice PRD §12.1 names, limited to the sections
/// this engine consumes: the lint rule set, the exclude globs, the
/// `errors`/`cannot-ignore` severity face, the language strictness scalars,
/// and the experiment flag list — the latter feeding the grant-free
/// package branch of the per-file feature sets, state.rs #936; #480
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
    /// `builder.contextFeatures`, parser:207-215). Read by the grant-free
    /// package branch of the per-file feature sets (state.rs #936, the
    /// probe-pinned replace posture).
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
    /// `analyzer.errors` keys the initial file's own section declares, in
    /// declaration order, original spellings (`unrecognized_error_code`
    /// candidates; validity is the caller's registry seam).
    pub declared_errors: Vec<DeclaredError>,
    /// Per-included-file registry candidates, in walk order — the CLI face's
    /// `undefined_lint`/`unrecognized_error_code` candidates for the files
    /// whose rows wrap into `included_file_warning`.
    pub included_groups: Vec<DeclarationGroup>,
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

    /// The lowercased `analyzer.cannot-ignore` set — the suppression-gate
    /// membership test the lint stage runs per file (`_filterIgnoredDiagnostics`)
    /// and the ignore-comment validator matches names against
    /// (`IgnoreValidator`, ignore_validator.dart:89, 113). Empty for the
    /// dominant workspace: the whole face short-circuits on it.
    #[must_use]
    pub fn unignorable(&self) -> &FxHashSet<String> {
        &self.unignorable
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
        let mut chain = FxHashMap::default();
        merge.apply_file(&normalized, None, None, &mut chain, packages, config_dir);
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
    /// Every parsed file's text, keyed by normalized path — the included
    /// files' inner spans (wrapped rows' `{start}..{end}`, the cycle row's
    /// include directive) anchor against their owning file's text.
    texts: FxHashMap<PathBuf, Arc<str>>,
    issues: Vec<OptionsIssue>,
    declared_rules: Vec<DeclaredRule>,
    declared_errors: Vec<DeclaredError>,
    included_groups: Vec<DeclarationGroup>,
    /// Each parsed file's first include URI — the cycle row's inner-span
    /// source (the official `spanInChain` = the repeated file's own
    /// outgoing directive, walker :170-188).
    first_includes: FxHashMap<PathBuf, String>,
}

impl MergeState {
    /// Records one row attributed to the initial file, anchored at `token`
    /// scoped to its `section` (the span stand-in — `serde_yaml` keeps no
    /// node spans; see the module docs).
    fn record(
        &mut self,
        code: CodeInfo,
        section: &str,
        token: &str,
        message: String,
        correction: Option<String>,
    ) {
        let (offset, length) = anchor_in_section(&self.initial_text, section, token);
        self.issues.push(OptionsIssue {
            code,
            message,
            correction,
            offset,
            length,
        });
    }

    /// Records one `included_file_warning` row: the wrapped message names the
    /// included file and the inner row's inclusive span (`{start}..{end}`,
    /// `include_walker.dart:238-245`), the row itself anchors at the branch's
    /// first include directive in the initial file, and the correction never
    /// survives the wrap (the official listener carries the message only).
    fn record_wrapped(
        &mut self,
        anchor: &str,
        included: &Path,
        inner_start: u32,
        inner_end: u32,
        inner: &str,
    ) {
        let (offset, length) = anchor_span(&self.initial_text, anchor);
        self.issues.push(OptionsIssue {
            code: INCLUDED_FILE_WARNING,
            message: format!(
                "Warning in the included options file {}({inner_start}..{inner_end}): {inner}",
                included.display()
            ),
            correction: None,
            offset,
            length,
        });
    }

    /// The official `_applyParsedFiles`: apply every resolvable include
    /// first, then this file's own data — depth-first, near data last.
    ///
    /// `included_from` carries the including file and the include URI for the
    /// cycle row's inner span (`None` for the initial file); `anchor` is the
    /// branch's first include directive URI token — every row this subtree
    /// records anchors at its initial-file occurrence (`initialIncludeSpan`,
    /// walker :90), the initial file's own edges at their own URI
    /// (`state.initialIncludeSpan ?? includeSpan`, :90).
    fn apply_file(
        &mut self,
        file: &Path,
        included_from: Option<(&Path, &str)>,
        anchor: Option<&str>,
        // The visited set: one contribution per file (the cycle row's span
        // comes from `first_includes`/`texts`, not this map).
        chain: &mut FxHashMap<PathBuf, ()>,
        packages: &PackageConfig,
        config_dir: &Path,
    ) {
        if included_from.is_some() && chain.insert(file.to_path_buf(), ()).is_some() {
            // Chain cycle not through the initial file: contributes once,
            // and the official wraps it naming the repeated file and the
            // span of its own outgoing include directive (`spanInChain`,
            // walker :173-188).
            let inner = match (self.first_includes.get(file), self.texts.get(file)) {
                (Some(first_uri), Some(text)) => anchor_in_section(text, "include", first_uri),
                _ => (0, 0),
            };
            let inner_start = inner.0;
            let inner_end = inner.0.saturating_add(inner.1.saturating_sub(1));
            let included = file.to_path_buf();
            let anchor = anchor.expect("a cycle row only exists under an include branch");
            self.record_wrapped(
                anchor,
                &included,
                inner_start,
                inner_end,
                "The file includes itself recursively.",
            );
            return;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            chain.remove(file);
            return;
        };
        self.texts
            .insert(file.to_path_buf(), Arc::from(text.as_str()));
        let yaml = match serde_yaml::from_str::<Value>(&text) {
            Ok(yaml) => {
                if let Some(first_uri) = include_uris(&yaml).into_iter().next() {
                    self.first_includes.insert(file.to_path_buf(), first_uri);
                }
                yaml
            }
            Err(error) => {
                // The YAML failure face: `parse_error` on the initial file
                // (its message drives the CLI's exit tier),
                // `included_file_parse_error` wrapped at the branch anchor
                // for an included one (walker :48-66). `serde_yaml` yields
                // one failure point, so the wrapped span shows it at both
                // ends; `yaml_failure` already converted it to UTF-16.
                let (message, at) = yaml_failure(&text, &error);
                if file == self.initial {
                    self.issues.push(OptionsIssue {
                        code: PARSE_ERROR,
                        message,
                        correction: None,
                        offset: at,
                        length: 0,
                    });
                } else {
                    let included = file.to_path_buf();
                    let anchor = anchor.expect("a wrapped row only exists under an include branch");
                    let (offset, length) = anchor_span(&self.initial_text, anchor);
                    self.issues.push(OptionsIssue {
                        code: INCLUDED_FILE_PARSE_ERROR,
                        message: format!("{message} in {}({at}..{at})", included.display()),
                        correction: None,
                        offset,
                        length,
                    });
                }
                chain.remove(file);
                return;
            }
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
                // Every row anchors at the branch's initial include span.
                Some(included) if included.is_file() => {
                    if included == self.initial {
                        self.record(
                            RECURSIVE_INCLUDE_FILE,
                            "include",
                            anchor.unwrap_or(&uri),
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
                        self.apply_file(
                            &included,
                            Some((file, uri.as_str())),
                            Some(anchor.unwrap_or(uri.as_str())),
                            chain,
                            packages,
                            config_dir,
                        );
                    }
                }
                _ => self.record(
                    INCLUDE_FILE_NOT_FOUND,
                    "include",
                    anchor.unwrap_or(&uri),
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
        self.apply_local(&yaml, &text, &declaring_dir, file, is_initial, anchor);
        chain.remove(file);
    }

    /// Applies one parsed file's own sections (the official
    /// `_ApplyState.apply`, the sections this engine consumes). The section
    /// order mirrors `_applyAnalyzer` (parser:164-171) where it matters:
    /// `errors` before `cannot-ignore`, so the severity arms see this
    /// file's re-levels.
    ///
    /// Local shape problems record raw for the initial file; an included
    /// file's wrap into `included_file_warning` at the branch anchor
    /// (`_IncludedDiagnosticListener`, walker :210-248), and its registry
    /// candidates land in a [`DeclarationGroup`] for the CLI face.
    fn apply_local(
        &mut self,
        yaml: &Value,
        text: &str,
        declaring_dir: &Path,
        file: &Path,
        is_initial: bool,
        anchor: Option<&str>,
    ) {
        let Some(map) = yaml.as_mapping() else {
            return;
        };
        // The included file's registry group registers up front: the linter
        // arm below fills its `rules` in place.
        if !is_initial {
            let group_anchor =
                anchor_span(&self.initial_text, anchor.expect("included under a branch"));
            self.included_groups.push(DeclarationGroup {
                file: file.to_path_buf(),
                anchor_span: group_anchor,
                rules: Vec::new(),
                errors: Vec::new(),
            });
        }
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
                        self.declared_rules.extend(declared_rule_names(rules, text));
                    } else {
                        self.included_groups
                            .last_mut()
                            .expect("the group registered above")
                            .rules = declared_rule_names(rules, text);
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
            self.issues.extend(local_issues(map, text));
            self.declared_errors.extend(declared_error_codes(map, text));
        } else {
            self.included_groups
                .last_mut()
                .expect("the group registered above")
                .errors = declared_error_codes(map, text);
            for issue in local_issues(map, text) {
                let inner_start = issue.offset;
                let inner_end = inner_start.saturating_add(issue.length.saturating_sub(1));
                let included = file.to_path_buf();
                let anchor = anchor.expect("included under a branch");
                self.record_wrapped(anchor, &included, inner_start, inner_end, &issue.message);
            }
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
            declared_errors: self.declared_errors,
            included_groups: self.included_groups,
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

/// The expired subset of `KNOWN_EXPERIMENTS` (`IsExpired`, same pinned
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
/// token's first occurrence after the `enable-experiment` key
/// ([`anchor_in_section`]).
fn experiment_flag_issues(flags: &[String], text: &str) -> Vec<OptionsIssue> {
    let mut issues = Vec::new();
    let mut requested: FxHashMap<&str, bool> = FxHashMap::default();
    let anchored = |text: &str, flag: &str| anchor_in_section(text, "enable-experiment", flag);
    for flag in flags {
        let (feature, value) = match flag.strip_prefix("no-") {
            Some(feature) => (feature, false),
            None => (flag.as_str(), true),
        };
        if !KNOWN_EXPERIMENTS.contains(&feature) {
            let (offset, length) = anchored(text, flag);
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
            let (offset, length) = anchored(text, flag);
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
            let (offset, length) = anchored(text, flag);
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
/// unknown key → singular `unsupported_option`). Scalar `errors` keys —
/// string or not — record as [`AnalysisOptions::declared_errors`] and report
/// `unrecognized_error_code` from the CLI face's registry seam; non-boolean
/// `language` values report `unsupported_value` — a face dartq has not
/// built, silent here; the
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
                    // (:577-582); every scalar key — string or not —
                    // records as an `unrecognized_error_code` candidate
                    // ([`AnalysisOptions::declared_errors`], reported from
                    // the CLI face's registry seam).
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
    // The top-level `plugins:` section: the 3.13.1 oracle's only
    // CLI-observable faces for it are the `invalid_section_format` row and
    // stderr plugin-load noise (probe, #930), and dartq loads no plugins —
    // any declared section is the row, silent only when absent or null.
    match map.get(Value::from("plugins")) {
        None | Some(Value::Null) => {}
        Some(_) => push_invalid_section(&mut issues, text, "plugins", "plugins"),
    }
    issues
}

/// Records one `invalid_section_format` row (`Invalid format for the '{0}'
/// section.`, registry :9708-9722), anchored at `token` (section-scoped —
/// [`anchor_in_section`]).
fn push_invalid_section(issues: &mut Vec<OptionsIssue>, text: &str, section: &str, token: &str) {
    let (offset, length) = anchor_in_section(text, section, token);
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
/// `quotedAndCommaSeparatedWithAnd` proposal (`…WithLegalValues`). The
/// anchor is section-scoped ([`anchor_in_section`]).
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
    let (offset, length) = anchor_in_section(text, section, option);
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
        let (offset, length) = anchor_in_section(text, "rules", name);
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

/// The `analyzer.errors` keys the initial file's own section declares
/// (`_ParsedErrorProcessorsData.parse`, parse_model:558-585): every scalar
/// key, in declaration order. String keys record their original spelling
/// (the registry check rides the uppercased form but the row names the
/// written one); any other scalar records its `valueNode.value.toString()`
/// form — those report unconditionally (the `case YamlScalar()` arm). The
/// anchor is the key token's first occurrence.
fn declared_error_codes(map: &serde_yaml::Mapping, text: &str) -> Vec<DeclaredError> {
    let mut codes = Vec::new();
    let Some(errors) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("errors")))
        .and_then(|errors| errors.as_mapping())
    else {
        return codes;
    };
    for (key, _) in errors {
        let code = match key {
            Value::String(code) => code.clone(),
            scalar @ (Value::Bool(_) | Value::Number(_) | Value::Null) => {
                yaml_scalar_text(scalar).expect("the matched arms are all scalars")
            }
            // Container keys are `invalid_section_format` (reported by
            // `local_issues`), never code candidates.
            Value::Mapping(_) | Value::Sequence(_) | Value::Tagged(_) => continue,
        };
        let (offset, length) = anchor_in_section(text, "errors", &code);
        codes.push(DeclaredError {
            code,
            offset,
            length,
        });
    }
    codes
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

/// `anchor_span` scoped to one section: the token searches from the section
/// key's first occurrence onward, so a value token (`info`, a flag name, a
/// URI) anchors inside its own section instead of at an earlier homograph in
/// a comment or another section — the anchoring convention for every
/// section's rows (a token equal to the key anchors at the key itself).
/// Falls back to the whole-text search (and then to `(0, 0)`) when either
/// miss.
fn anchor_in_section(text: &str, section: &str, token: &str) -> (u32, u32) {
    let from = text.find(section).unwrap_or(0);
    match text[from..].find(token) {
        Some(relative) => (
            byte_offset_to_utf16(text, from + relative),
            utf16_len_of(token),
        ),
        None => anchor_span(text, token),
    }
}

/// The `parse_error` face of one `serde_yaml` failure: the error's own text
/// with the located form's " at line L column C" suffix stripped (the
/// position rides the row's span, not the message — the oracle's
/// `error - analysis_options.yaml:3:1 - Expected node content. - parse_error`),
/// plus the failure point as a UTF-16 offset (`(0, 0)` when unlocated).
fn yaml_failure(text: &str, error: &serde_yaml::Error) -> (String, u32) {
    let display = error.to_string();
    match error.location() {
        Some(location) => {
            let suffix = format!(" at line {} column {}", location.line(), location.column());
            let message = display.strip_suffix(&suffix).unwrap_or(&display);
            let byte = location.index().min(text.len());
            (message.to_owned(), byte_offset_to_utf16(text, byte))
        }
        None => (display, 0),
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

// --- The pinned diagnostic-code name universe (`unrecognized_error_code`) ---

/// The diagnostic code names the pinned SDK's public registry knows — the
/// `codeNames` universe of `_DiagnosticOptions.isKnownDiagnosticOrLintCode`
/// (`analysis_options_parse_model.dart:55-97`): the `lowerCaseName` of every
/// entry in `diagnosticCodeValues` (`pkg/analyzer/lib/src/diagnostic/
/// diagnostic_code_values.g.dart`, dart 3.15 main — 1363 list entries, the
/// duplicates deduplicated; version-pinned data like
/// [`KNOWN_EXPERIMENTS`]). Sorted, lowercase: membership is a binary search
/// over the lowercased key. Codes removed from the registry but still
/// accepted live in `REMOVED_ERROR_CODES`, not here.
mod known_error_codes {
    pub const NAMES: &[&str] = &[
        "abi_specific_integer_invalid",
        "abi_specific_integer_mapping_extra",
        "abi_specific_integer_mapping_missing",
        "abi_specific_integer_mapping_unsupported",
        "abstract_class_member",
        "abstract_external_field",
        "abstract_field_constructor_initializer",
        "abstract_field_initializer",
        "abstract_final_base_class",
        "abstract_final_interface_class",
        "abstract_late_field",
        "abstract_sealed_class",
        "abstract_static_field",
        "abstract_static_method",
        "abstract_super_member_reference",
        "address_position",
        "address_receiver",
        "ambiguous_comment_reference_name",
        "ambiguous_comment_reference_prefix",
        "ambiguous_export",
        "ambiguous_extension_member_access_three_or_more",
        "ambiguous_extension_member_access_two",
        "ambiguous_import",
        "ambiguous_set_or_map_literal_both",
        "ambiguous_set_or_map_literal_either",
        "analysis_option_deprecated",
        "analysis_option_deprecated_with_replacement",
        "analysis_options_deprecated_plugins",
        "annotation_on_pointer_field",
        "annotation_on_type_argument",
        "annotation_space_before_parenthesis",
        "annotation_with_type_arguments",
        "annotation_with_type_arguments_uninstantiated",
        "anonymous_method_wrong_parameter_list",
        "anonymous_method_wrong_parameter_type",
        "argument_must_be_aconstant",
        "argument_must_be_native",
        "argument_type_not_assignable",
        "argument_type_not_assignable_to_error_handler",
        "assert_in_redirecting_constructor",
        "asset_directory_does_not_exist",
        "asset_does_not_exist",
        "asset_field_not_list",
        "asset_missing_path",
        "asset_not_string",
        "asset_not_string_or_map",
        "asset_path_not_string",
        "assignment_of_do_not_store",
        "assignment_to_const",
        "assignment_to_final",
        "assignment_to_final_local",
        "assignment_to_final_no_setter",
        "assignment_to_function",
        "assignment_to_method",
        "assignment_to_primary_constructor_parameter",
        "assignment_to_type",
        "async_for_in_wrong_context",
        "async_keyword_used_as_identifier",
        "augmentation_extends_clause_already_present",
        "augmentation_formal_parameter_modifier_extra",
        "augmentation_formal_parameter_modifier_missing",
        "augmentation_formal_parameter_type_mismatch",
        "augmentation_induced_getter_already_complete",
        "augmentation_induced_getter_return_type_mismatch",
        "augmentation_induced_setter_already_complete",
        "augmentation_modifier_extra",
        "augmentation_modifier_missing",
        "augmentation_named_formal_parameter_extra",
        "augmentation_named_formal_parameter_missing",
        "augmentation_of_different_declaration_kind",
        "augmentation_of_mixin_application_class",
        "augmentation_optional_positional_formal_parameter_count",
        "augmentation_positional_formal_parameter_name",
        "augmentation_required_positional_formal_parameter_count",
        "augmentation_return_type_mismatch",
        "augmentation_type_parameter_bound",
        "augmentation_type_parameter_count",
        "augmentation_type_parameter_name",
        "augmentation_variable_different_getter_setter_types",
        "augmentation_without_declaration",
        "augmentation_without_getter_declaration",
        "augmentation_without_setter_declaration",
        "augmented_expression_is_not_setter",
        "augmented_expression_is_setter",
        "augmented_expression_not_operator",
        "augments_constant_variable",
        "await_in_late_local_variable_initializer",
        "await_in_wrong_context",
        "await_of_incompatible_type",
        "base_class_implemented_outside_of_library",
        "base_enum",
        "base_mixin_implemented_outside_of_library",
        "binary_operator_written_out",
        "body_might_complete_normally",
        "body_might_complete_normally_catch_error",
        "body_might_complete_normally_nullable",
        "break_label_on_switch_member",
        "break_outside_of_loop",
        "built_in_identifier_as_extension_name",
        "built_in_identifier_as_extension_type_name",
        "built_in_identifier_as_prefix_name",
        "built_in_identifier_as_type",
        "built_in_identifier_as_type_name",
        "built_in_identifier_as_type_parameter_name",
        "built_in_identifier_as_typedef_name",
        "camera_permissions_incompatible",
        "case_expression_type_implements_equals",
        "case_expression_type_is_not_switch_expression_subtype",
        "cast_from_null_always_fails",
        "cast_from_nullable_always_fails",
        "cast_to_non_type",
        "catch_syntax",
        "catch_syntax_extra_parameters",
        "class_in_class",
        "class_instantiation_access_to_instance_member",
        "class_instantiation_access_to_static_member",
        "class_instantiation_access_to_unknown_member",
        "class_used_as_mixin",
        "class_used_as_mixin_declares_generative_constructor",
        "colon_in_place_of_in",
        "compound_implements_finalizable",
        "concrete_class_has_enum_superinterface",
        "concrete_class_with_abstract_member",
        "conflicting_constructor_and_static_field",
        "conflicting_constructor_and_static_getter",
        "conflicting_constructor_and_static_method",
        "conflicting_constructor_and_static_setter",
        "conflicting_field_and_method",
        "conflicting_generic_interfaces",
        "conflicting_inherited_method_and_setter",
        "conflicting_method_and_field",
        "conflicting_modifiers",
        "conflicting_static_and_instance",
        "conflicting_type_variable_and_class",
        "conflicting_type_variable_and_enum",
        "conflicting_type_variable_and_extension",
        "conflicting_type_variable_and_extension_type",
        "conflicting_type_variable_and_member_class",
        "conflicting_type_variable_and_member_enum",
        "conflicting_type_variable_and_member_extension",
        "conflicting_type_variable_and_member_extension_type",
        "conflicting_type_variable_and_member_mixin",
        "conflicting_type_variable_and_mixin",
        "const_and_final",
        "const_class",
        "const_constructor_constant_from_deferred_library",
        "const_constructor_field_type_mismatch",
        "const_constructor_param_type_mismatch",
        "const_constructor_throws_exception",
        "const_constructor_with_body",
        "const_constructor_with_field_initialized_by_non_const",
        "const_constructor_with_mixin_with_field",
        "const_constructor_with_mixin_with_fields",
        "const_constructor_with_non_const_super",
        "const_constructor_with_non_final_field",
        "const_deferred_class",
        "const_eval_assertion_failure",
        "const_eval_assertion_failure_with_message",
        "const_eval_extension_method",
        "const_eval_extension_type_method",
        "const_eval_for_element",
        "const_eval_method_invocation",
        "const_eval_primitive_equality",
        "const_eval_property_access",
        "const_eval_throws_exception",
        "const_eval_throws_idbze",
        "const_eval_type_bool",
        "const_eval_type_bool_int",
        "const_eval_type_bool_num_string",
        "const_eval_type_int",
        "const_eval_type_num",
        "const_eval_type_num_string",
        "const_eval_type_string",
        "const_eval_type_type",
        "const_factory",
        "const_field_initializer_not_assignable",
        "const_initialized_with_non_constant_value",
        "const_initialized_with_non_constant_value_from_deferred_library",
        "const_instance_field",
        "const_map_key_not_primitive_equality",
        "const_method",
        "const_not_initialized",
        "const_primary_constructor_with_block_body",
        "const_primary_constructor_with_expression_body",
        "const_set_element_not_primitive_equality",
        "const_spread_expected_list_or_set",
        "const_spread_expected_map",
        "const_type_parameter",
        "const_with_non_const",
        "const_with_non_constant_argument",
        "const_with_non_type",
        "const_with_type_parameters",
        "const_with_type_parameters_constructor_tearoff",
        "const_with_type_parameters_function_tearoff",
        "const_with_undefined_constructor",
        "const_with_undefined_constructor_default",
        "const_without_primary_constructor",
        "constant_pattern_never_matches_value_type",
        "constant_pattern_with_non_constant_expression",
        "constant_variable_augmentation",
        "constructor_already_complete",
        "constructor_with_return_type",
        "constructor_with_type_arguments",
        "continue_label_invalid",
        "continue_outside_of_loop",
        "continue_without_label_in_case",
        "could_not_infer",
        "covariant_and_static",
        "covariant_constructor",
        "covariant_member",
        "creation_of_struct_or_union",
        "dead_code",
        "dead_code_catch_following_catch",
        "dead_code_late_wildcard_variable_initializer",
        "dead_code_on_catch_subtype",
        "dead_null_aware_expression",
        "default_in_switch_expression",
        "default_value_already_specified_in_augmentation_chain",
        "default_value_in_function_type",
        "default_value_in_redirecting_factory_constructor",
        "default_value_on_required_parameter",
        "deferred_after_prefix",
        "deferred_import_of_extension",
        "definitely_unassigned_late_local_variable",
        "dependencies_field_not_map",
        "deprecated_colon_for_default_value",
        "deprecated_export_use",
        "deprecated_extend",
        "deprecated_extends_function",
        "deprecated_factory_method",
        "deprecated_field",
        "deprecated_implement",
        "deprecated_implements_function",
        "deprecated_instantiate",
        "deprecated_lint",
        "deprecated_lint_with_replacement",
        "deprecated_member_use",
        "deprecated_member_use_implicit_super_constructor_invocation",
        "deprecated_member_use_implicit_super_constructor_invocation_with_message",
        "deprecated_member_use_implicit_type",
        "deprecated_member_use_implicit_type_with_message",
        "deprecated_member_use_with_message",
        "deprecated_mixin",
        "deprecated_mixin_function",
        "deprecated_optional",
        "deprecated_subclass",
        "different_inherited_getter_and_setter_types",
        "directive_after_declaration",
        "disallowed_type_instantiation_expression",
        "doc_directive_argument_wrong_format",
        "doc_directive_has_extra_arguments",
        "doc_directive_has_unexpected_named_argument",
        "doc_directive_missing_closing_brace",
        "doc_directive_missing_closing_tag",
        "doc_directive_missing_one_argument",
        "doc_directive_missing_opening_tag",
        "doc_directive_missing_three_arguments",
        "doc_directive_missing_two_arguments",
        "doc_directive_unknown",
        "doc_import_cannot_be_deferred",
        "doc_import_cannot_have_configurations",
        "doc_import_in_part_file",
        "dot_shorthand_missing_context",
        "dot_shorthand_undefined_getter",
        "dot_shorthand_undefined_invocation",
        "duplicate_constructor_default",
        "duplicate_constructor_name",
        "duplicate_deferred",
        "duplicate_definition",
        "duplicate_export",
        "duplicate_field_formal_parameter",
        "duplicate_field_name",
        "duplicate_hidden_name",
        "duplicate_ignore",
        "duplicate_import",
        "duplicate_label_in_switch_statement",
        "duplicate_named_argument",
        "duplicate_part",
        "duplicate_pattern_assignment_variable",
        "duplicate_pattern_field",
        "duplicate_prefix",
        "duplicate_rest_element_in_pattern",
        "duplicate_rule",
        "duplicate_shown_name",
        "duplicate_variable_pattern",
        "duplicated_modifier",
        "empty_enum_body",
        "empty_map_pattern",
        "empty_record_literal_with_comma",
        "empty_record_type_named_fields_list",
        "empty_record_type_with_comma",
        "empty_struct",
        "encoding",
        "enum_constant_invokes_factory_constructor",
        "enum_constant_same_name_as_enclosing",
        "enum_in_class",
        "enum_instantiated_to_bounds_is_not_well_bounded",
        "enum_mixin_with_instance_variable",
        "enum_with_abstract_member",
        "enum_with_name_values",
        "enum_without_constants",
        "equal_elements_in_const_set",
        "equal_elements_in_set",
        "equal_keys_in_const_map",
        "equal_keys_in_map",
        "equal_keys_in_map_pattern",
        "equality_cannot_be_equality_operand",
        "exclude_invalid_glob",
        "expected_case_or_default",
        "expected_catch_clause_body",
        "expected_class_body",
        "expected_class_member",
        "expected_else_or_comma",
        "expected_executable",
        "expected_extension_body",
        "expected_extension_type_body",
        "expected_finally_clause_body",
        "expected_identifier_but_got_keyword",
        "expected_instead",
        "expected_list_or_map_literal",
        "expected_mixin_body",
        "expected_named_type_extends",
        "expected_named_type_implements",
        "expected_named_type_on",
        "expected_named_type_with",
        "expected_one_list_pattern_type_arguments",
        "expected_one_list_type_arguments",
        "expected_one_set_type_arguments",
        "expected_representation_field",
        "expected_representation_type",
        "expected_string_literal",
        "expected_switch_expression_body",
        "expected_switch_statement_body",
        "expected_token",
        "expected_try_statement_body",
        "expected_two_map_pattern_type_arguments",
        "expected_two_map_type_arguments",
        "expected_type_name",
        "experiment_not_enabled",
        "experiment_not_enabled_off_by_default",
        "experimental_member_use",
        "export_directive_after_part_directive",
        "export_internal_library",
        "export_of_non_library",
        "expression_in_map",
        "extends_deferred_class",
        "extends_disallowed_class",
        "extends_non_class",
        "extends_type_alias_expands_to_type_parameter",
        "extension_as_expression",
        "extension_augmentation_has_on_clause",
        "extension_augmentation_without_name",
        "extension_conflicting_static_and_instance",
        "extension_declares_abstract_member",
        "extension_declares_constructor",
        "extension_declares_instance_field",
        "extension_declares_member_of_object",
        "extension_override_access_to_static_member",
        "extension_override_argument_not_assignable",
        "extension_override_with_cascade",
        "extension_override_without_access",
        "extension_primary_constructor",
        "extension_type_augmentation_specifies_representation_field",
        "extension_type_constructor_with_super_formal_parameter",
        "extension_type_constructor_with_super_invocation",
        "extension_type_declares_instance_field",
        "extension_type_declares_member_of_object",
        "extension_type_extends",
        "extension_type_implements_disallowed_type",
        "extension_type_implements_itself",
        "extension_type_implements_not_supertype",
        "extension_type_implements_representation_not_supertype",
        "extension_type_inherited_member_conflict",
        "extension_type_representation_depends_on_itself",
        "extension_type_representation_type_bottom",
        "extension_type_with",
        "extension_type_with_abstract_member",
        "external_class",
        "external_constructor_with_field_initializers",
        "external_constructor_with_initializer",
        "external_enum",
        "external_factory_redirection",
        "external_factory_with_body",
        "external_field_constructor_initializer",
        "external_field_initializer",
        "external_getter_with_body",
        "external_late_field",
        "external_method_with_body",
        "external_operator_with_body",
        "external_setter_with_body",
        "external_typedef",
        "external_variable_initializer",
        "extra_annotation_on_struct_field",
        "extra_positional_arguments",
        "extra_positional_arguments_could_be_named",
        "extra_size_annotation_carray",
        "extraneous_modifier",
        "extraneous_modifier_in_extension_type",
        "extraneous_modifier_in_primary_constructor",
        "factory_constructor_new_name",
        "factory_not_complete_after_augmentations",
        "factory_top_level_declaration",
        "factory_with_initializers",
        "factory_without_body",
        "ffi_native_invalid_duplicate_default_asset",
        "ffi_native_invalid_multiple_annotations",
        "ffi_native_must_be_external",
        "ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer",
        "ffi_native_unexpected_number_of_parameters",
        "ffi_native_unexpected_number_of_parameters_with_receiver",
        "field_initialized_by_multiple_initializers",
        "field_initialized_in_declaration_and_initializer_of_primary_constructor",
        "field_initialized_in_declaration_and_parameter_of_primary_constructor",
        "field_initialized_in_initializer_and_declaration",
        "field_initialized_in_parameter_and_initializer",
        "field_initialized_outside_declaring_class",
        "field_initializer_factory_constructor",
        "field_initializer_not_assignable",
        "field_initializer_outside_constructor",
        "field_initializer_redirecting_constructor",
        "field_initializing_formal_not_assignable",
        "field_must_be_external_in_struct",
        "final_and_covariant",
        "final_and_covariant_late_with_initializer",
        "final_and_var",
        "final_class_extended_outside_of_library",
        "final_class_implemented_outside_of_library",
        "final_class_used_as_mixin_constraint_outside_of_library",
        "final_constructor",
        "final_enum",
        "final_initialized_in_declaration_and_constructor",
        "final_method",
        "final_mixin",
        "final_mixin_class",
        "final_not_initialized",
        "final_not_initialized_constructor1",
        "final_not_initialized_constructor2",
        "final_not_initialized_constructor3_plus",
        "fixme",
        "flutter_field_not_map",
        "for_in_of_invalid_element_type",
        "for_in_of_invalid_type",
        "for_in_with_const_variable",
        "function_already_complete",
        "function_not_complete_after_augmentations",
        "function_typed_parameter_var",
        "generic_function_type_cannot_be_bound",
        "generic_function_type_cannot_be_type_argument",
        "generic_method_type_instantiation_on_dynamic",
        "generic_struct_subclass",
        "getter_constructor",
        "getter_in_function",
        "getter_not_subtype_setter_types",
        "getter_with_parameters",
        "hack",
        "if_element_condition_from_deferred_library",
        "illegal_assignment_to_non_assignable",
        "illegal_async_generator_return_type",
        "illegal_async_return_type",
        "illegal_character",
        "illegal_concrete_enum_member_declaration",
        "illegal_concrete_enum_member_inheritance",
        "illegal_enum_values_declaration",
        "illegal_enum_values_inheritance",
        "illegal_language_version_override",
        "illegal_pattern_assignment_variable_name",
        "illegal_pattern_identifier_name",
        "illegal_pattern_variable_name",
        "illegal_sync_generator_return_type",
        "implements_before_extends",
        "implements_before_on",
        "implements_before_with",
        "implements_deferred_class",
        "implements_disallowed_class",
        "implements_non_class",
        "implements_repeated",
        "implements_super_class",
        "implements_super_class_constraint",
        "implements_type_alias_expands_to_type_parameter",
        "implicit_super_initializer_missing_arguments",
        "implicit_this_reference_in_initializer",
        "import_deferred_library_with_load_function",
        "import_directive_after_part_directive",
        "import_internal_library",
        "import_of_non_library",
        "include_file_not_found",
        "included_file_parse_error",
        "included_file_warning",
        "incompatible_lint",
        "incompatible_lint_files",
        "incompatible_lint_included",
        "incompatible_plugin_source",
        "inconsistent_inheritance",
        "inconsistent_inheritance_getter_and_method",
        "inconsistent_language_version_override",
        "inconsistent_pattern_variable_logical_or",
        "induced_getter_not_complete_after_augmentations",
        "induced_getter_without_body",
        "induced_setter_not_complete_after_augmentations",
        "induced_setter_without_body",
        "inference_failure_on_collection_literal",
        "inference_failure_on_function_invocation",
        "inference_failure_on_function_return_type",
        "inference_failure_on_generic_invocation",
        "inference_failure_on_instance_creation",
        "inference_failure_on_uninitialized_variable",
        "inference_failure_on_untyped_parameter",
        "initialized_variable_in_for_each",
        "initializer_for_non_existent_field",
        "initializer_for_static_field",
        "initializing_declaring_parameter",
        "initializing_formal_for_non_existent_field",
        "instance_access_to_static_member",
        "instance_access_to_static_member_of_unnamed_extension",
        "instance_member_access_from_factory",
        "instance_member_access_from_static",
        "instantiate_abstract_class",
        "instantiate_enum",
        "instantiate_type_alias_expands_to_type_parameter",
        "integer_literal_imprecise_as_double",
        "integer_literal_out_of_range",
        "interface_class_extended_outside_of_library",
        "interface_enum",
        "interface_mixin",
        "interface_mixin_class",
        "invalid_annotation",
        "invalid_annotation_constant_value_from_deferred_library",
        "invalid_annotation_from_deferred_library",
        "invalid_annotation_target",
        "invalid_assignment",
        "invalid_await_in_for",
        "invalid_await_not_required_annotation",
        "invalid_code_point",
        "invalid_comment_reference",
        "invalid_constant",
        "invalid_constant_const_prefix",
        "invalid_constant_pattern_binary",
        "invalid_constant_pattern_duplicate_const",
        "invalid_constant_pattern_empty_record_literal",
        "invalid_constant_pattern_generic",
        "invalid_constant_pattern_negation",
        "invalid_constant_pattern_unary",
        "invalid_constructor_name",
        "invalid_covariant_modifier_in_primary_constructor",
        "invalid_dependency",
        "invalid_deprecated_extend_annotation",
        "invalid_deprecated_implement_annotation",
        "invalid_deprecated_instantiate_annotation",
        "invalid_deprecated_mixin_annotation",
        "invalid_deprecated_optional_annotation",
        "invalid_deprecated_subclass_annotation",
        "invalid_exception_value",
        "invalid_export_of_internal_element",
        "invalid_export_of_internal_element_indirectly",
        "invalid_extension_argument_count",
        "invalid_factory_method_decl",
        "invalid_factory_method_impl",
        "invalid_factory_name_not_aclass",
        "invalid_field_name_from_object",
        "invalid_field_name_positional",
        "invalid_field_name_private",
        "invalid_field_type_in_struct",
        "invalid_generic_function_type",
        "invalid_hex_escape",
        "invalid_implementation_override",
        "invalid_implementation_override_setter",
        "invalid_initializer",
        "invalid_inline_function_type",
        "invalid_inside_unary_pattern",
        "invalid_internal_annotation",
        "invalid_language_version_override_at_sign",
        "invalid_language_version_override_equals",
        "invalid_language_version_override_greater",
        "invalid_language_version_override_location",
        "invalid_language_version_override_lower_case",
        "invalid_language_version_override_number",
        "invalid_language_version_override_prefix",
        "invalid_language_version_override_trailing_characters",
        "invalid_language_version_override_two_slashes",
        "invalid_literal_annotation",
        "invalid_literal_in_configuration",
        "invalid_modifier_on_constructor",
        "invalid_modifier_on_setter",
        "invalid_non_virtual_annotation",
        "invalid_null_aware_element",
        "invalid_null_aware_map_entry_key",
        "invalid_null_aware_map_entry_value",
        "invalid_null_aware_operator",
        "invalid_null_aware_operator_after_short_circuit",
        "invalid_operator",
        "invalid_operator_questionmark_period_for_super",
        "invalid_option",
        "invalid_override",
        "invalid_override_of_non_virtual_member",
        "invalid_override_setter",
        "invalid_platforms_field",
        "invalid_reference_to_generative_enum_constructor",
        "invalid_reference_to_generative_enum_constructor_tearoff",
        "invalid_reference_to_this",
        "invalid_reopen_annotation",
        "invalid_section_format",
        "invalid_star_after_async",
        "invalid_super_formal_parameter_location",
        "invalid_super_in_initializer",
        "invalid_sync",
        "invalid_this_in_initializer",
        "invalid_type_argument_in_const_list",
        "invalid_type_argument_in_const_map",
        "invalid_type_argument_in_const_set",
        "invalid_unicode_escape_started",
        "invalid_unicode_escape_ubracket",
        "invalid_unicode_escape_uno_bracket",
        "invalid_unicode_escape_ustarted",
        "invalid_uri",
        "invalid_use_of_covariant",
        "invalid_use_of_covariant_in_extension",
        "invalid_use_of_do_not_submit_member",
        "invalid_use_of_identifier_augmented",
        "invalid_use_of_internal_member",
        "invalid_use_of_null_value",
        "invalid_use_of_protected_member",
        "invalid_use_of_visible_for_overriding_member",
        "invalid_use_of_visible_for_template_member",
        "invalid_use_of_visible_for_testing_member",
        "invalid_visibility_annotation",
        "invalid_visible_outside_template_annotation",
        "invalid_widget_preview_application",
        "invalid_widget_preview_private_argument",
        "invocation_of_extension_without_call",
        "invocation_of_non_function",
        "invocation_of_non_function_expression",
        "js_interop_extension_constructor_js_annotation_has_no_effect",
        "label_in_outer_scope",
        "label_undefined",
        "late_final_field_with_const_constructor",
        "late_final_local_already_assigned",
        "late_pattern_variable_declaration",
        "leaf_call_must_not_return_handle",
        "leaf_call_must_not_take_handle",
        "library_directive_not_first",
        "list_element_type_not_assignable",
        "list_element_type_not_assignable_nullability",
        "literal_with_class",
        "literal_with_class_and_new",
        "literal_with_new",
        "local_function_declaration_modifier",
        "main_first_positional_parameter_type",
        "main_has_required_named_parameters",
        "main_has_too_many_required_positional_parameters",
        "main_is_not_function",
        "map_entry_not_in_map",
        "map_key_type_not_assignable",
        "map_key_type_not_assignable_nullability",
        "map_value_type_not_assignable",
        "map_value_type_not_assignable_nullability",
        "member_with_class_name",
        "mismatched_annotation_on_struct_field",
        "missing_annotation_on_struct_field",
        "missing_assignable_selector",
        "missing_assignment_in_initializer",
        "missing_catch_or_finally",
        "missing_closing_parenthesis",
        "missing_const_final_var_or_type",
        "missing_const_in_list_literal",
        "missing_const_in_map_literal",
        "missing_const_in_set_literal",
        "missing_default_value_for_parameter",
        "missing_default_value_for_parameter_positional",
        "missing_default_value_for_parameter_with_annotation",
        "missing_dependency",
        "missing_digit",
        "missing_enum_body",
        "missing_enum_constant_in_switch",
        "missing_exception_value",
        "missing_expression_in_initializer",
        "missing_expression_in_throw",
        "missing_field_type_in_struct",
        "missing_function_body",
        "missing_function_keyword",
        "missing_function_parameters",
        "missing_get",
        "missing_hex_digit",
        "missing_identifier",
        "missing_initializer",
        "missing_keyword_operator",
        "missing_method_parameters",
        "missing_name",
        "missing_name_for_named_parameter",
        "missing_name_in_library_directive",
        "missing_name_in_part_of_directive",
        "missing_named_pattern_field_name",
        "missing_override_of_must_be_overridden_one",
        "missing_override_of_must_be_overridden_three_plus",
        "missing_override_of_must_be_overridden_two",
        "missing_prefix_in_deferred_import",
        "missing_primary_constructor",
        "missing_primary_constructor_parameters",
        "missing_quote",
        "missing_required_argument",
        "missing_required_param",
        "missing_required_param_with_details",
        "missing_size_annotation_carray",
        "missing_star_after_sync",
        "missing_statement",
        "missing_typedef_parameters",
        "missing_variable_in_for_each",
        "missing_variable_pattern",
        "mixed_parameter_groups",
        "mixin_application_class_augmentation",
        "mixin_application_concrete_super_invoked_member_type",
        "mixin_application_no_concrete_super_invoked_member",
        "mixin_application_no_concrete_super_invoked_setter",
        "mixin_application_not_implemented_interface",
        "mixin_augmentation_has_on_clause",
        "mixin_class_declaration_extends_not_object",
        "mixin_class_declaration_with_clause",
        "mixin_class_declares_non_trivial_generative_constructor",
        "mixin_declares_constructor",
        "mixin_deferred_class",
        "mixin_inherits_from_not_object",
        "mixin_instantiate",
        "mixin_modifier_mixin_application_class_with_multiple_mixins",
        "mixin_of_disallowed_class",
        "mixin_of_non_class",
        "mixin_of_type_alias_expands_to_type_parameter",
        "mixin_on_sealed_class",
        "mixin_on_type_alias_expands_to_type_parameter",
        "mixin_primary_constructor",
        "mixin_subtype_of_base_is_not_base",
        "mixin_subtype_of_final_is_not_base",
        "mixin_super_class_constraint_deferred_class",
        "mixin_super_class_constraint_disallowed_class",
        "mixin_super_class_constraint_non_interface",
        "mixin_with_clause",
        "mixin_with_non_class_superclass",
        "mixins_super_class",
        "modifier_out_of_order",
        "multiple_clauses",
        "multiple_combinators",
        "multiple_combinators_deprecated",
        "multiple_extends_clauses",
        "multiple_implements_clauses",
        "multiple_library_directives",
        "multiple_named_parameter_groups",
        "multiple_on_clauses",
        "multiple_part_of_directives",
        "multiple_plugins",
        "multiple_positional_parameter_groups",
        "multiple_primary_constructor_body_declarations",
        "multiple_redirecting_constructor_invocations",
        "multiple_representation_fields",
        "multiple_super_initializers",
        "multiple_variance_modifiers",
        "multiple_with_clauses",
        "must_be_anative_function_type",
        "must_be_asubtype",
        "must_be_immutable",
        "must_call_super",
        "must_return_void",
        "name_not_string",
        "named_function_expression",
        "named_function_type",
        "named_parameter_outside_group",
        "native_clause_in_non_sdk_code",
        "native_clause_should_be_annotation",
        "native_field_invalid_type",
        "native_field_missing_type",
        "native_field_not_static",
        "native_function_body_in_non_sdk_code",
        "native_function_missing_type",
        "negative_variable_dimension",
        "new_constructor_dot_name",
        "new_constructor_new_name",
        "new_constructor_qualified_name",
        "new_with_non_type",
        "new_with_undefined_constructor",
        "new_with_undefined_constructor_default",
        "no_annotation_constructor_arguments",
        "no_combined_super_signature",
        "no_default_super_constructor_explicit",
        "no_default_super_constructor_implicit",
        "no_generative_constructors_in_superclass",
        "no_touchscreen_feature",
        "non_abstract_class_inherits_abstract_member_five_plus",
        "non_abstract_class_inherits_abstract_member_four",
        "non_abstract_class_inherits_abstract_member_one",
        "non_abstract_class_inherits_abstract_member_three",
        "non_abstract_class_inherits_abstract_member_two",
        "non_bool_condition",
        "non_bool_expression",
        "non_bool_negation_expression",
        "non_bool_operand",
        "non_const_argument_for_const_parameter",
        "non_const_call_to_literal_constructor",
        "non_const_call_to_literal_constructor_using_new",
        "non_const_generative_enum_constructor",
        "non_const_map_as_expression_statement",
        "non_constant_case_expression",
        "non_constant_case_expression_from_deferred_library",
        "non_constant_default_value",
        "non_constant_default_value_from_deferred_library",
        "non_constant_list_element",
        "non_constant_list_element_from_deferred_library",
        "non_constant_map_element",
        "non_constant_map_key",
        "non_constant_map_key_from_deferred_library",
        "non_constant_map_pattern_key",
        "non_constant_map_value",
        "non_constant_map_value_from_deferred_library",
        "non_constant_record_field",
        "non_constant_record_field_from_deferred_library",
        "non_constant_relational_pattern_expression",
        "non_constant_set_element",
        "non_constant_type_argument",
        "non_constructor_factory",
        "non_covariant_type_parameter_position_in_representation_type",
        "non_exhaustive_switch_expression",
        "non_exhaustive_switch_expression_private",
        "non_exhaustive_switch_statement",
        "non_exhaustive_switch_statement_private",
        "non_final_field_in_enum",
        "non_generative_constructor",
        "non_generative_implicit_constructor",
        "non_identifier_library_name",
        "non_native_function_type_argument_to_pointer",
        "non_nullable_equals_parameter",
        "non_part_of_directive_in_part",
        "non_positive_array_dimension",
        "non_redirecting_generative_constructor_with_primary",
        "non_resizable_activity",
        "non_sized_type_argument",
        "non_string_literal_as_uri",
        "non_sync_factory",
        "non_type_as_type_argument",
        "non_type_in_catch_clause",
        "non_void_return_for_operator",
        "non_void_return_for_setter",
        "normal_before_optional_parameters",
        "not_assigned_potentially_non_nullable_local_variable",
        "not_atype",
        "not_binary_operator",
        "not_enough_positional_arguments_name_plural",
        "not_enough_positional_arguments_name_singular",
        "not_enough_positional_arguments_plural",
        "not_enough_positional_arguments_singular",
        "not_initialized_non_nullable_instance_field",
        "not_initialized_non_nullable_instance_field_constructor",
        "not_initialized_non_nullable_variable",
        "not_instantiated_bound",
        "not_iterable_spread",
        "not_map_spread",
        "not_null_aware_null_spread",
        "null_argument_to_non_null_type",
        "null_aware_cascade_out_of_order",
        "null_check_always_fails",
        "nullable_type_in_catch_clause",
        "nullable_type_in_extends_clause",
        "nullable_type_in_implements_clause",
        "nullable_type_in_on_clause",
        "nullable_type_in_with_clause",
        "object_cannot_extend_another_class",
        "obsolete_colon_for_default_value",
        "on_repeated",
        "optional_parameter_in_operator",
        "out_of_order_clauses",
        "override_on_non_overriding_field",
        "override_on_non_overriding_getter",
        "override_on_non_overriding_method",
        "override_on_non_overriding_setter",
        "packed_annotation",
        "packed_annotation_alignment",
        "parse_error",
        "part_of_different_library",
        "part_of_name",
        "part_of_non_part",
        "part_of_unnamed_library",
        "path_does_not_exist",
        "path_not_posix",
        "path_pubspec_does_not_exist",
        "pattern_assignment_declares_variable",
        "pattern_assignment_not_local_variable",
        "pattern_constant_from_deferred_library",
        "pattern_never_matches_value_type",
        "pattern_type_mismatch_in_irrefutable_context",
        "pattern_variable_assignment_inside_guard",
        "pattern_variable_declaration_outside_function_or_method",
        "pattern_variable_shared_case_scope_different_finality_or_type",
        "pattern_variable_shared_case_scope_has_label",
        "pattern_variable_shared_case_scope_not_all_cases",
        "permission_implies_unsupported_hardware",
        "platform_value_disallowed",
        "positional_after_named_argument",
        "positional_field_in_object_pattern",
        "positional_parameter_outside_group",
        "positional_super_formal_parameter_with_positional_argument",
        "prefix_after_combinator",
        "prefix_collides_with_top_level_member",
        "prefix_identifier_not_followed_by_dot",
        "prefix_shadowed_by_local_declaration",
        "primary_constructor_body_with_expression_body",
        "primary_constructor_body_with_modifier",
        "primary_constructor_body_without_declaration",
        "primary_constructor_cannot_redirect",
        "private_collision_in_mixin_application",
        "private_named_non_field_parameter",
        "private_named_parameter_duplicate_public_name",
        "private_named_parameter_without_public_name",
        "private_optional_parameter",
        "private_setter",
        "read_potentially_unassigned_final",
        "receiver_of_type_never",
        "record_literal_one_positional_no_trailing_comma",
        "record_literal_one_positional_no_trailing_comma_by_type",
        "record_type_one_positional_no_trailing_comma",
        "recursive_compile_time_constant",
        "recursive_constant_constructor",
        "recursive_constructor_redirect",
        "recursive_factory_redirect",
        "recursive_include_file",
        "recursive_interface_inheritance",
        "recursive_interface_inheritance_extends",
        "recursive_interface_inheritance_implements",
        "recursive_interface_inheritance_on",
        "recursive_interface_inheritance_with",
        "redeclare_on_non_redeclaring_member",
        "redirect_generative_to_missing_constructor",
        "redirect_generative_to_non_generative_constructor",
        "redirect_to_abstract_class_constructor",
        "redirect_to_invalid_function_type",
        "redirect_to_invalid_return_type",
        "redirect_to_missing_constructor",
        "redirect_to_non_class",
        "redirect_to_non_const_constructor",
        "redirect_to_type_alias_expands_to_type_parameter",
        "redirecting_constructor_with_body",
        "redirection_in_non_factory_constructor",
        "referenced_before_declaration",
        "refutable_pattern_in_irrefutable_context",
        "relational_pattern_operand_type_not_assignable",
        "relational_pattern_operator_return_type_not_assignable_to_bool",
        "removed_lint",
        "removed_lint_use",
        "replaced_lint",
        "replaced_lint_use",
        "representation_field_modifier",
        "representation_field_trailing_comma",
        "rest_element_in_map_pattern",
        "rethrow_outside_catch",
        "return_in_generative_constructor",
        "return_in_generator",
        "return_of_do_not_store",
        "return_of_invalid_type_from_catch_error",
        "return_of_invalid_type_from_closure",
        "return_of_invalid_type_from_constructor",
        "return_of_invalid_type_from_function",
        "return_of_invalid_type_from_method",
        "return_of_invalid_type_from_then",
        "return_type_invalid_for_catch_error",
        "return_type_invalid_for_then",
        "return_without_value",
        "sdk_version_constructor_tearoffs",
        "sdk_version_gt_gt_gt_operator",
        "sdk_version_since",
        "sealed_class_subtype_outside_of_library",
        "sealed_enum",
        "sealed_mixin",
        "sealed_mixin_class",
        "set_element_from_deferred_library",
        "set_element_type_not_assignable",
        "set_element_type_not_assignable_nullability",
        "setter_constructor",
        "setter_in_function",
        "setting_orientation_on_activity",
        "shared_deferred_prefix",
        "size_annotation_dimensions",
        "spread_expression_from_deferred_library",
        "stack_overflow",
        "static_access_to_instance_member",
        "static_constructor",
        "static_getter_without_body",
        "static_operator",
        "static_setter_without_body",
        "strict_raw_type",
        "subtype_of_base_is_not_base_final_or_sealed",
        "subtype_of_final_is_not_base_final_or_sealed",
        "subtype_of_sealed_class",
        "subtype_of_struct_class_in_extends",
        "subtype_of_struct_class_in_implements",
        "subtype_of_struct_class_in_with",
        "super_formal_parameter_type_is_not_subtype_of_associated",
        "super_formal_parameter_without_associated_named",
        "super_formal_parameter_without_associated_positional",
        "super_in_enum_constructor",
        "super_in_extension",
        "super_in_extension_type",
        "super_in_invalid_context",
        "super_in_redirecting_constructor",
        "super_initializer_in_object",
        "super_initializing_declaring_parameter",
        "super_invocation_not_last",
        "switch_case_completes_normally",
        "switch_has_case_after_default_case",
        "switch_has_multiple_default_cases",
        "tearoff_of_generative_constructor_of_abstract_class",
        "tearoff_with_must_be_const_parameter",
        "text_direction_code_point_in_comment",
        "text_direction_code_point_in_literal",
        "throw_of_invalid_type",
        "todo",
        "top_level_cycle",
        "top_level_operator",
        "type_alias_cannot_reference_itself",
        "type_annotation_deferred_class",
        "type_argument_not_matching_bounds",
        "type_arguments_on_type_variable",
        "type_before_factory",
        "type_check_is_not_null",
        "type_check_is_null",
        "type_parameter_on_constructor",
        "type_parameter_on_operator",
        "type_parameter_referenced_by_static",
        "type_parameter_supertype_of_its_bound",
        "type_test_with_non_type",
        "type_test_with_undefined_name",
        "typedef_augmentation",
        "typedef_in_class",
        "unawaited_return_in_try_block",
        "unchecked_invocation_of_nullable_value",
        "unchecked_method_invocation_of_nullable_value",
        "unchecked_operator_invocation_of_nullable_value",
        "unchecked_property_access_of_nullable_value",
        "unchecked_use_of_nullable_value_as_condition",
        "unchecked_use_of_nullable_value_as_iterator",
        "unchecked_use_of_nullable_value_in_spread",
        "unchecked_use_of_nullable_value_in_yield_each",
        "undefined_class",
        "undefined_class_boolean",
        "undefined_constructor_in_initializer",
        "undefined_constructor_in_initializer_default",
        "undefined_enum_constant",
        "undefined_enum_constructor_named",
        "undefined_enum_constructor_unnamed",
        "undefined_extension_getter",
        "undefined_extension_method",
        "undefined_extension_operator",
        "undefined_extension_setter",
        "undefined_function",
        "undefined_getter",
        "undefined_getter_on_function_type",
        "undefined_hidden_name",
        "undefined_identifier",
        "undefined_identifier_await",
        "undefined_lint",
        "undefined_method",
        "undefined_method_on_function_type",
        "undefined_method_on_type_literal",
        "undefined_named_parameter",
        "undefined_operator",
        "undefined_prefixed_name",
        "undefined_referenced_parameter",
        "undefined_setter",
        "undefined_setter_on_function_type",
        "undefined_shown_name",
        "undefined_super_getter",
        "undefined_super_method",
        "undefined_super_operator",
        "undefined_super_setter",
        "undone",
        "unexpected_dollar_in_string",
        "unexpected_separator_in_number",
        "unexpected_token",
        "unexpected_tokens",
        "unignorable_ignore",
        "unknown_platform",
        "unnecessary_cast",
        "unnecessary_cast_pattern",
        "unnecessary_dev_dependency",
        "unnecessary_final",
        "unnecessary_import",
        "unnecessary_nan_comparison_false",
        "unnecessary_nan_comparison_true",
        "unnecessary_no_such_method",
        "unnecessary_non_null_assertion",
        "unnecessary_null_assert_pattern",
        "unnecessary_null_check_pattern",
        "unnecessary_null_comparison_always_null_false",
        "unnecessary_null_comparison_always_null_true",
        "unnecessary_null_comparison_never_null_false",
        "unnecessary_null_comparison_never_null_true",
        "unnecessary_question_mark",
        "unnecessary_set_literal",
        "unnecessary_type_check_false",
        "unnecessary_type_check_true",
        "unnecessary_wildcard_pattern",
        "unqualified_reference_to_non_local_static_member",
        "unqualified_reference_to_static_member_of_extended_type",
        "unreachable_switch_case",
        "unreachable_switch_default",
        "unrecognized_error_code",
        "unsupported_chrome_os_feature",
        "unsupported_chrome_os_hardware",
        "unsupported_operator",
        "unsupported_option_with_legal_value",
        "unsupported_option_with_legal_values",
        "unsupported_option_without_values",
        "unsupported_value",
        "unterminated_multi_line_comment",
        "unterminated_string_literal",
        "unused_catch_clause",
        "unused_catch_stack",
        "unused_element",
        "unused_element_parameter",
        "unused_field",
        "unused_field_from_primary_constructor",
        "unused_import",
        "unused_label",
        "unused_local_variable",
        "unused_result",
        "unused_result_with_message",
        "unused_shown_name",
        "uri_does_not_exist",
        "uri_does_not_exist_in_doc_import",
        "uri_has_not_been_generated",
        "uri_with_interpolation",
        "use_of_native_extension",
        "use_of_private_parameter_name",
        "use_of_void_result",
        "values_declaration_in_enum",
        "var_and_type",
        "var_as_type_name",
        "var_class",
        "var_enum",
        "var_return_type",
        "var_typedef",
        "variable_length_array_not_last",
        "variable_pattern_keyword_in_declaration_context",
        "variable_type_mismatch",
        "void_with_type_arguments",
        "with_before_extends",
        "workspace_field_not_list",
        "workspace_value_not_string",
        "workspace_value_not_subdirectory",
        "wrong_explicit_type_parameter_variance_in_superinterface",
        "wrong_number_of_parameters_for_operator",
        "wrong_number_of_parameters_for_operator_minus",
        "wrong_number_of_parameters_for_setter",
        "wrong_number_of_type_arguments",
        "wrong_number_of_type_arguments_constructor",
        "wrong_number_of_type_arguments_dot_shorthand_constructor",
        "wrong_number_of_type_arguments_element",
        "wrong_number_of_type_arguments_enum",
        "wrong_number_of_type_arguments_extension",
        "wrong_number_of_type_arguments_function",
        "wrong_separator_for_positional_parameter",
        "wrong_type_parameter_variance_in_superinterface",
        "wrong_type_parameter_variance_position",
        "yield_each_in_non_generator",
        "yield_each_of_invalid_type",
        "yield_in_non_generator",
        "yield_of_invalid_type",
    ];
}

/// The registry names that existed but were removed, still accepted as
/// `analyzer.errors` keys so existing options files keep working
/// (`removedCodeNames`, parse_model:66-76 — "we don't want to report these,
/// this breaks clients").
const REMOVED_ERROR_CODES: [&str; 3] = [
    "missing_return",
    "plugins_in_inner_options",
    "undefined_annotation",
];

/// Whether `name` — an `analyzer.errors` key in its written spelling — names
/// a diagnostic code the pinned registry knows (the diagnostic universe of
/// `known_error_codes::NAMES` plus the `REMOVED_ERROR_CODES` compat set;
/// case-insensitive, the official test rides the uppercased key). Lint-rule
/// names are a separate registry (dartq-lints) — the CLI face's seam ORs
/// them in.
#[must_use]
pub fn is_known_diagnostic_code(name: &str) -> bool {
    let lowered = name.to_lowercase();
    REMOVED_ERROR_CODES.contains(&lowered.as_str())
        || known_error_codes::NAMES
            .binary_search(&lowered.as_str())
            .is_ok()
}
