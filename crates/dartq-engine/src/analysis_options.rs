//! `analysis_options.yaml` loading and include-chain merge.
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
//!   that is not a string is ignored, a leading-`/` pattern reports
//!   `exclude_invalid_glob` (:660-668), and a non-list `exclude:` section is
//!   ignored (`_ParsedExcludesData.parse`, `analysis_options_parse_model.dart`
//!   :634-681).
//! - **Linter rule entries** — `parseLinterSection`/`_parseRuleConfig`
//!   (lint/config.dart:20-104): a list of names, or a map of `name: bool`,
//!   `name: severity-string` (`disable` disables, any other string enables),
//!   or a nested group map recursed exactly one level; unknown shapes
//!   ignored. Every entry name lowercases at the section boundary
//!   (`entry.key.toLowerCase()`, config.dart:86-90), so a mixed-case
//!   spelling resolves the snake_case rule downstream. A `linter:` key that
//!   is present but not a map marks the section invalid, which *clears* the
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
//!   util/yaml.dart:156-169 — a non-boolean scalar reports
//!   `unsupported_value`), set-if-present across the chain
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
//!   `unsupported_value`, `exclude_invalid_glob`, `undefined_lint`,
//!   `analysis_options_parse_model.dart`) and *included*
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
    CodeInfo, ContextMessage, Diagnostic, DiagnosticType, FileId, Severity, SourceRange,
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

/// `Support for legacy plugins is deprecated, and will be removed in an
/// upcoming version of Dart.` — the unconditional row on any
/// `analyzer.plugins` key (upstream `_ParsedAnalyzerData.parse`,
/// `analysis_options_parse_model.dart`:285-289, reported at the key node;
/// registry `diagnostic.g.dart`:488-500, `STATIC_WARNING`, no published
/// docs; the correction points at the new plugin system docs).
const ANALYSIS_OPTIONS_DEPRECATED_PLUGINS: CodeInfo = CodeInfo::new(
    "analysis_options_deprecated_plugins",
    "ANALYSIS_OPTIONS_DEPRECATED_PLUGINS",
    DiagnosticType::StaticWarning,
    false,
);

/// `The rule '{0}' is already enabled and doesn't need to be enabled again.`
/// (registry :4742-4756; emitted by the CLI face's enabled pass, which owns
/// the lint registry the pass resolves through.)
const DUPLICATE_RULE: CodeInfo = CodeInfo::new(
    "duplicate_rule",
    "DUPLICATE_RULE",
    DiagnosticType::StaticWarning,
    true,
);

/// `The rule '{0}' is incompatible with '{1}'.` (registry :7785-7800; the
/// second argument arrives pre-quoted — the official message nests the
/// `quotedAndCommaSeparatedWithAnd` join inside the template's own quotes;
/// emitted by the CLI face's enabled pass, which owns the incompatible-rule
/// data the registry attaches to the rule.)
const INCOMPATIBLE_LINT: CodeInfo = CodeInfo::new(
    "incompatible_lint",
    "INCOMPATIBLE_LINT",
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

    /// The `unsupported_value` row for one lint rule's illegal scalar value
    /// (`parseRule`'s fall-through arm, parse_model.dart:1272-1284 — the
    /// value is neither a bool nor a legal severity string; the legal-values
    /// proposal is the `_validLintValues` or-join, `'true', 'false',
    /// 'ignore', 'info', 'warning', or 'error'`). The caller has already
    /// gated the name on the registry (the check fires first, so an
    /// unregistered name reports `undefined_lint` and its value is never
    /// inspected). Anchored at the declared name's span — the facts face
    /// carries the name anchor; the official row anchors at the value node.
    #[must_use]
    pub fn unsupported_lint_value(rule: &DeclaredRule, illegal: &str) -> Self {
        OptionsIssue {
            code: UNSUPPORTED_VALUE,
            message: format!("The value '{illegal}' isn't supported by '{}'.", rule.name),
            correction: Some(format!(
                "Try using one of the supported values: {}.",
                proposal_with(
                    "or",
                    &["true", "false", "ignore", "info", "warning", "error"]
                )
            )),
            offset: rule.offset,
            length: rule.length,
        }
    }

    /// The `duplicate_rule` row for one enabled declaration repeating a name
    /// the pass has already taken (`_processEnabledRule`, parse_model.dart
    /// :1397-1404 — first occurrence wins, every later enabled one reports;
    /// the message names the written spelling).
    #[must_use]
    pub fn duplicate_rule(rule: &DeclaredRule) -> Self {
        OptionsIssue {
            code: DUPLICATE_RULE,
            message: format!(
                "The rule '{}' is already enabled and doesn't need to be enabled again.",
                rule.name
            ),
            correction: Some("Try removing all but one occurrence of the rule.".to_owned()),
            offset: rule.offset,
            length: rule.length,
        }
    }

    /// The `incompatible_lint` row for one enabled declaration conflicting
    /// with earlier enabled ones (`_processEnabledRule`, parse_model.dart
    /// :1385-1395). `conflicting` carries the earlier rules' written
    /// spellings in the declared rule's `incompatibleRules` order; the
    /// official message nests the quoted join inside the template's own
    /// quotes (`... incompatible with ''a' and 'b''.` — the parser test's
    /// pinned rendering).
    #[must_use]
    pub fn incompatible_lint(rule: &DeclaredRule, conflicting: &[&str]) -> Self {
        OptionsIssue {
            code: INCOMPATIBLE_LINT,
            message: format!(
                "The rule '{}' is incompatible with '{}'.",
                rule.name,
                proposal(conflicting)
            ),
            correction: Some("Try removing all but one of the incompatible rules.".to_owned()),
            offset: rule.offset,
            length: rule.length,
        }
    }

    /// The `incompatible_lint_files` row (registry `name` is the shared
    /// `incompatible_lint` — the printed code, `lowerCaseName`, is identical
    /// for the whole family; only the message shape differs): a *local*
    /// enabled rule conflicts with the include chain's effective enabled set
    /// (`reportIncompatibleWithIncluded`, linter_rule_options_validator.dart
    /// :188-225). `spelling` is the local rule's written name; `conflicting`
    /// the conflicting included rules' written spellings in the declared
    /// `incompatibleRules` order — the join is quoted inside the template's
    /// bare `{1}` (diagnostic_factory.dart:319-347). `span` is the local
    /// rule token's `(offset, length)` in the reporting file's text.
    #[must_use]
    pub fn incompatible_lint_files(spelling: &str, conflicting: &[&str], span: (u32, u32)) -> Self {
        OptionsIssue {
            code: INCOMPATIBLE_LINT,
            message: format!(
                "The rule '{spelling}' is incompatible with {}.",
                proposal(conflicting)
            ),
            correction: Some(
                "Try locally disabling all but one of the conflicting rules or \
                 removing one of the incompatible files."
                    .to_owned(),
            ),
            offset: span.0,
            length: span.1,
        }
    }

    /// The `incompatible_lint_included` row (same shared `incompatible_lint`
    /// code): incompatible rules enabled through *different include entries*
    /// of the reporting file (`reportIncompatibleIncluded`,
    /// linter_rule_options_validator.dart:99-186). `{0}` is the *include
    /// URI* — the include node's written value, not a rule name — `span` its
    /// token's `(offset, length)` in the reporting file's text; `{2}`/`{3}`
    /// count the distinct include directives the conflicting rules came in
    /// through (diagnostic_factory.dart:351-382).
    #[must_use]
    pub fn incompatible_lint_included(
        uri: &str,
        conflicting: &[&str],
        file_count: usize,
        span: (u32, u32),
    ) -> Self {
        OptionsIssue {
            code: INCOMPATIBLE_LINT,
            message: format!(
                "The rule '{uri}' is incompatible with {}, which is included from \
                 {file_count} file{}.",
                proposal(conflicting),
                if file_count == 1 { "" } else { "s" }
            ),
            correction: Some(
                "Try locally disabling all but one of the conflicting rules or \
                 removing one of the incompatible files."
                    .to_owned(),
            ),
            offset: span.0,
            length: span.1,
        }
    }
}

/// One context row the INCOMPATIBLE_LINT family attaches to its diagnostic
/// (the census W3 face): the message plus the conflicting rule's location —
/// `file: None` anchors at the reporting file itself (the local list's
/// rules), `Some` at the included options file the rule was enabled in,
/// resolved to a `FileId` at emission. The span is the name token's
/// `(offset, length)` in that file's text, UTF-16.
struct ContextRow {
    message: String,
    file: Option<PathBuf>,
    offset: u32,
    length: u32,
}

/// An [`OptionsIssue`] plus its `contextMessages` payload — the
/// INCOMPATIBLE_LINT family's context producers. The plain factories build
/// the row alone; these constructors thread each conflicting rule's file
/// and span alongside it so the row emits with its context messages
/// attached, one per conflicting rule anchor.
///
/// Upstream shape (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/diagnostic/diagnostic_factory.dart:288-382`):
/// every factory folds its conflicting declarations into a
/// `Map<file.path, node>` — one entry per file, the first occurrence keeps
/// the position, a later same-file declaration replaces the anchor — and
/// reports one context message per map entry at the node's span:
/// `"The rule '<name>' is enabled here."`, with an `" in the file '<path>'."`
/// suffix on the files variant only (the included variant names the file in
/// the message body's URI, not in the context row).
#[must_use]
pub struct ContextualIssue {
    issue: OptionsIssue,
    context: Vec<ContextRow>,
}

impl ContextualIssue {
    /// The `incompatible_lint` row plus its context face — the plain
    /// [`OptionsIssue::incompatible_lint`] message (the caller's conflicting
    /// spellings in declaration order, unchanged) and one context row per
    /// upstream's map fold: every local conflicting rule was enabled in the
    /// reporting file itself, so the `file.path` key collapses to its last
    /// entry — one row anchored at that rule's name token
    /// (diagnostic_factory.dart:302-313).
    pub fn incompatible_lint(rule: &DeclaredRule, conflicting: &[&DeclaredRule]) -> Self {
        let spellings: Vec<&str> = conflicting
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let context = conflicting
            .last()
            .map(|entry| ContextRow {
                message: format!("The rule '{}' is enabled here.", entry.name),
                file: None,
                offset: entry.offset,
                length: entry.length,
            })
            .into_iter()
            .collect();
        Self {
            issue: OptionsIssue::incompatible_lint(rule, &spellings),
            context,
        }
    }

    /// The `incompatible_lint_files` row plus its context face: one context
    /// row per entry — `"The rule '<name>' is enabled here in the file
    /// '<path>'."` at the rule's span in its options file
    /// (diagnostic_factory.dart:333-345). `conflicting` arrives per-file
    /// folded (first occurrence keeps the position, the last same-file entry
    /// wins) — the caller's existing `Map<file.path, node>` fold — so each
    /// entry is one map entry upstream.
    pub fn incompatible_lint_files(
        spelling: &str,
        conflicting: &[&RuleData],
        span: (u32, u32),
    ) -> Self {
        let spellings: Vec<&str> = conflicting
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let context = conflicting
            .iter()
            .map(|entry| ContextRow {
                message: format!(
                    "The rule '{}' is enabled here in the file '{}'.",
                    entry.name,
                    entry.file.display()
                ),
                file: Some(entry.file.clone()),
                offset: entry.offset,
                length: entry.length,
            })
            .collect();
        Self {
            issue: OptionsIssue::incompatible_lint_files(spelling, &spellings, span),
            context,
        }
    }

    /// The `incompatible_lint_included` row plus its context face — the
    /// files variant's shape with the plain context message (no file
    /// suffix, diagnostic_factory.dart:369-380): one row per folded entry,
    /// anchored at the rule's span in the included file it was enabled in.
    pub fn incompatible_lint_included(
        uri: &str,
        conflicting: &[&RuleData],
        file_count: usize,
        span: (u32, u32),
    ) -> Self {
        let spellings: Vec<&str> = conflicting
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let context = conflicting
            .iter()
            .map(|entry| ContextRow {
                message: format!("The rule '{}' is enabled here.", entry.name),
                file: Some(entry.file.clone()),
                offset: entry.offset,
                length: entry.length,
            })
            .collect();
        Self {
            issue: OptionsIssue::incompatible_lint_included(uri, &spellings, file_count, span),
            context,
        }
    }

    /// The wrapped row, for callers that re-anchor an included file's row
    /// into `included_file_warning` (the wrap carries the message only —
    /// the context face is the unwrapped row's).
    pub fn inner(&self) -> &OptionsIssue {
        &self.issue
    }

    /// The engine diagnostic row with its `contextMessages` attached.
    /// `context_file` resolves a conflicting rule's file to its `FileId`; a
    /// file the caller cannot resolve drops its row — under-report, never a
    /// crash (the persist decode's posture), and a fully-dropped context
    /// degrades to the plain no-context row (`with_context`'s empty face).
    pub fn to_diagnostic(
        &self,
        file: FileId,
        context_file: impl FnMut(&Path) -> Option<FileId>,
    ) -> Diagnostic {
        let mut context_file = context_file;
        let context: Vec<ContextMessage> = self
            .context
            .iter()
            .filter_map(|row| {
                Some(ContextMessage {
                    message: row.message.clone().into(),
                    file: match &row.file {
                        Some(path) => context_file(path)?,
                        None => file,
                    },
                    range: SourceRange {
                        offset: row.offset,
                        length: row.length,
                    },
                })
            })
            .collect();
        self.issue.to_diagnostic(file).with_context(context)
    }
}

/// A lint name declared by the initial file's own `linter.rules`, with its
/// anchor span — the `undefined_lint` candidate list. The engine holds no
/// lint registry (that lives in dartq-lints, and an engine dependency would
/// cycle), so validity is the caller's seam — and so are the value rows
/// (`unsupported_value`, `duplicate_rule`): upstream gates both on the
/// registry check in `parseRule` firing first (parse_model.dart:1255-1262 —
/// an unregistered name reports `undefined_lint` and its value is never
/// inspected), so this struct records the value facts and the caller
/// combines them with its registry.
#[derive(Debug, Clone)]
pub struct DeclaredRule {
    pub name: String,
    pub offset: u32,
    pub length: u32,
    /// Whether the entry is enabled after the parse model's value
    /// processing (parse_model.dart:1266-1283): a YAML bool as-is, a legal
    /// severity string (`ignore`/`info`/`warning`/`error`,
    /// case-sensitive) as `!= "ignore"`, everything else — including a
    /// group map — disabled.
    pub enabled: bool,
    /// The text of a value that is neither a bool nor a legal severity
    /// string — the `invalidValue` argument of the `unsupported_value` row
    /// (`rule: 42` → `"42"`; a group map or list renders through Dart's
    /// container `toString`, `{a: b}`/`[x]`). `None` for bool and
    /// legal-string values.
    pub illegal_value: Option<String>,
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

/// A linter-rule occurrence's cross-file identity (the official `_RuleData`):
/// the written spelling (the rows name it, and the registry lookup is
/// case-insensitive), the owning options file, and the name token's span in
/// that file's text — the wrapped inner span when the row reports inside an
/// included file.
#[derive(Debug, Clone)]
pub struct RuleData {
    /// The name as written in the owning file (message text renders it
    /// verbatim — the mixed-case parser tests pin that).
    pub name: String,
    /// The options file the declaration lives in (normalized path).
    pub file: PathBuf,
    /// The name token's `(offset, length)` in `file`'s text, UTF-16.
    pub offset: u32,
    pub length: u32,
}

/// One resolved `include:` edge inside one options file (the official
/// `_IncludedLinterRules` — the include node's role, not the target's local
/// data): `uri` is the directive's written value — the
/// `incompatible_lint_included` row's `{0}` — and `(offset, length)` its span
/// in *this* file's text; `group` indexes the target's entry in
/// [`AnalysisOptions::included_groups`], whose `enabled_rules`/`disabled_rules`
/// carry the subtree's effective rule state.
#[derive(Debug, Clone)]
pub struct IncludeEdge {
    /// The include URI as written in the including file.
    pub uri: String,
    /// The URI token's `(offset, length)` in the including file's text, UTF-16.
    pub offset: u32,
    pub length: u32,
    /// The target file's [`DeclarationGroup`] index in
    /// [`AnalysisOptions::included_groups`].
    pub group: usize,
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
    /// This file's resolved include edges, in declaration order — the
    /// `incompatible_lint_included` walk's per-edge entries at this level
    /// (`reportIncompatibleIncluded`, linter_rule_options_validator.dart
    /// :105-186; missing or cyclic edges never register one).
    pub includes: Vec<IncludeEdge>,
    /// The file's effective enabled/disabled rule state — its own
    /// `includes`' subtrees folded in order, then its own `rules` applied
    /// (the official `_EffectiveLinterRules`, insertion-ordered per name):
    /// a disabled entry hides a lower-priority enabled same-named rule.
    pub enabled_rules: Vec<RuleData>,
    /// The disabled side of the effective state — see `enabled_rules`.
    pub disabled_rules: Vec<RuleData>,
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
/// `ResolvedAnalysisOptions` slice names, limited to the sections
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
    /// The initial file's resolved include edges, in declaration order — the
    /// `incompatible_lint_included` walk's top-level entries and, folded, the
    /// `includedEffectiveLinterRules` the `incompatible_lint_files` check
    /// compares the local [`AnalysisOptions::declared_rules`] against
    /// (linter_rule_options_validator.dart:188-225). The initial file never
    /// carries a group entry — it is not included.
    pub included_edges: Vec<IncludeEdge>,
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
    /// (`IgnoreValidator`, `ignore_validator.dart:89`, 113). Empty for the
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
        let mut chain = FxHashSet::default();
        merge.apply_file(&normalized, None, None, &mut chain, packages, config_dir);
        merge.build()
    }

    /// One merged [`AnalysisOptions`] per governing directory, sharing
    /// loaded files through `loaded` (workspace members under one repo-root
    /// options file parse it once). Every package root maps to its entry:
    /// the nearest `analysis_options.yaml` in its ancestry, or the
    /// synthesized default (no file at all). Nested options files inside
    /// each root's discovery surfaces are located and mapped to their own
    /// directories — the official `optionsFileMap` (`context_locator.dart`
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
    /// The initial file's resolved include edges — built into
    /// [`AnalysisOptions::included_edges`].
    included_edges: Vec<IncludeEdge>,
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
    /// Returns the included file's [`DeclarationGroup`] index (`Some` only
    /// for a parsed, non-cyclic *included* file — the include-edge child the
    /// caller records; the initial file and skipped edges yield `None`).
    ///
    /// `included_from` carries the including file and the include URI for the
    /// cycle row's inner span (`None` for the initial file); `anchor` is the
    /// branch's first include directive URI token — every row this subtree
    /// records anchors at its initial-file occurrence (`initialIncludeSpan`,
    /// walker :90), the initial file's own edges at their own URI
    /// (`state.initialIncludeSpan ?? includeSpan`, :90).
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn apply_file(
        &mut self,
        file: &Path,
        included_from: Option<(&Path, &str)>,
        anchor: Option<&str>,
        // The visited set: one contribution per file (the cycle row's span
        // comes from `first_includes`/`texts`, not this set).
        chain: &mut FxHashSet<PathBuf>,
        packages: &PackageConfig,
        config_dir: &Path,
    ) -> Option<usize> {
        if included_from.is_some() && !chain.insert(file.to_path_buf()) {
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
            // The walk pairs `included_from` with a branch anchor (the
            // recursive call passes both, the initial call neither), so the
            // anchor is always `Some` here; a divergent walk degrades the
            // row's anchor to `(0, 0)` (`anchor_span`'s miss face) — a
            // diagnostic row, never a panic.
            debug_assert!(anchor.is_some(), "cycle row without its include anchor");
            let anchor = anchor.unwrap_or_default();
            self.record_wrapped(
                anchor,
                &included,
                inner_start,
                inner_end,
                "The file includes itself recursively.",
            );
            return None;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            chain.remove(file);
            return None;
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
                    // The same branch-anchor pairing as the cycle row: an
                    // included file's wrap always carries the anchor; a
                    // divergent walk degrades the row's anchor to `(0, 0)`.
                    debug_assert!(anchor.is_some(), "wrapped row without its include anchor");
                    let anchor = anchor.unwrap_or_default();
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
                return None;
            }
        };
        let is_initial = file == self.initial;

        // This file's resolved include edges — the per-file entries the
        // `incompatible_lint_included` walk iterates at this level. Only
        // parsed, non-cyclic targets register one (the official
        // `includedLinterRules` collects `_ParsedInclude` alone,
        // parser:509-537).
        let mut edges = Vec::new();
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
                    } else if let Some(child) = self.apply_file(
                        &included,
                        Some((file, uri.as_str())),
                        Some(anchor.unwrap_or(uri.as_str())),
                        chain,
                        packages,
                        config_dir,
                    ) {
                        // The edge's own span is the URI token's occurrence
                        // inside *this* file's `include` section — the
                        // `incompatible_lint_included` row's anchor when the
                        // conflict sits at this level.
                        let (offset, length) = anchor_in_section(&text, "include", &uri);
                        edges.push(IncludeEdge {
                            uri,
                            offset,
                            length,
                            group: child,
                        });
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

        if is_initial {
            self.included_edges = edges;
            return None;
        }
        // `apply_local` registered this file's group (the empty one for a
        // non-map document registers there too); fold the effective rule
        // state over its edges + local declarations.
        let group_index = self.included_groups.len() - 1;
        self.included_groups[group_index].includes = edges;
        let (enabled_rules, disabled_rules) =
            effective_rules(&self.included_groups, group_index, file);
        let group = &mut self.included_groups[group_index];
        group.enabled_rules = enabled_rules;
        group.disabled_rules = disabled_rules;
        Some(group_index)
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
        // The included file's registry group registers up front — before the
        // mapping gate, so a valid-but-non-map document (a scalar or list
        // YAML) still yields the empty entry upstream's `_IncludedLinterRules`
        // contributes; the linter arm below fills its `rules` in place.
        if !is_initial {
            // Only the include walk calls with `!is_initial`, always
            // carrying the branch anchor; a divergent walk degrades the
            // group's wrap anchor to `(0, 0)`.
            debug_assert!(anchor.is_some(), "included group without its branch anchor");
            let group_anchor = anchor_span(&self.initial_text, anchor.unwrap_or_default());
            self.included_groups.push(DeclarationGroup {
                file: file.to_path_buf(),
                anchor_span: group_anchor,
                rules: Vec::new(),
                errors: Vec::new(),
                includes: Vec::new(),
                enabled_rules: Vec::new(),
                disabled_rules: Vec::new(),
            });
        }
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
                // The branch-anchor pairing again (see the group anchor
                // above); a divergent walk degrades the wrap's anchor to
                // `(0, 0)`, the row itself still records.
                debug_assert!(anchor.is_some(), "wrapped row without its branch anchor");
                let anchor = anchor.unwrap_or_default();
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
            included_edges: self.included_edges,
        }
    }
}

/// One file's effective rule state (the official `_EffectiveLinterRules`
/// fold, linter_rule_options_validator.dart:14-48): the group's include
/// edges apply their targets' effective state in declaration order, then the
/// file's own `rules` overlay — each side applying disabled entries before
/// enabled so a disable hides a lower-priority enable of the same rule
/// (`apply`/`applyLocal`, :25-45). Identity is the registry's
/// case-insensitive name (the registry lowercases every spelling); order is
/// first-declaration insertion — the `LinkedHashMap` position rule.
///
/// Registry membership is *not* filtered here — the engine holds no lint
/// registry; the CLI face filters when it runs the validator
/// (`is_known_lint_name` gates the same candidates upstream's `parseRule`
/// registry check admits).
fn effective_rules(
    groups: &[DeclarationGroup],
    group_index: usize,
    file: &Path,
) -> (Vec<RuleData>, Vec<RuleData>) {
    /// Removes `key`'s entry (`_EffectiveLinterRules`' `enabled.remove` /
    /// `disabled.remove` — registry identity is case-insensitive).
    fn drop_key(list: &mut Vec<RuleData>, key: &str) {
        if let Some(position) = list.iter().position(|r| r.name.eq_ignore_ascii_case(key)) {
            list.remove(position);
        }
    }
    /// Inserts `item`, replacing a same-named entry in place (the
    /// `LinkedHashMap` overwrite keeps the first position).
    fn set(list: &mut Vec<RuleData>, item: RuleData) {
        match list
            .iter_mut()
            .find(|r| r.name.eq_ignore_ascii_case(&item.name))
        {
            Some(slot) => *slot = item,
            None => list.push(item),
        }
    }
    /// `_EffectiveLinterRules.apply`: the incoming disabled entries settle
    /// first, then the enabled ones.
    fn apply(enabled: &mut Vec<RuleData>, disabled: &mut Vec<RuleData>, other: &DeclarationGroup) {
        for rule in &other.disabled_rules {
            drop_key(enabled, &rule.name);
            set(disabled, rule.clone());
        }
        for rule in &other.enabled_rules {
            drop_key(disabled, &rule.name);
            set(enabled, rule.clone());
        }
    }

    let group = &groups[group_index];
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();
    for edge in &group.includes {
        apply(&mut enabled, &mut disabled, &groups[edge.group]);
    }
    // `applyLocal` — the file's own declarations overlay the included fold,
    // disabled first.
    for rule in group.rules.iter().filter(|rule| !rule.enabled) {
        drop_key(&mut enabled, &rule.name);
        set(
            &mut disabled,
            RuleData {
                name: rule.name.clone(),
                file: file.to_path_buf(),
                offset: rule.offset,
                length: rule.length,
            },
        );
    }
    for rule in group.rules.iter().filter(|rule| rule.enabled) {
        drop_key(&mut disabled, &rule.name);
        set(
            &mut enabled,
            RuleData {
                name: rule.name.clone(),
                file: file.to_path_buf(),
                offset: rule.offset,
                length: rule.length,
            },
        );
    }
    (enabled, disabled)
}

/// Merges one file's `linter.rules` value into the rule map
/// (`parseLinterSection` + `parseDiagnosticsSection`, lint/config.dart
/// :68-105): a list of names, or a map of per-rule entries; unknown shapes
/// contribute nothing. Every entry name lowercases at this boundary
/// (`entry.key.toLowerCase()`, config.dart:86-90 — the registry lookups
/// downstream are case-insensitive, so a mixed-case spelling resolves the
/// snake_case rule).
fn apply_rules(rules: &Value, merged: &mut FxHashMap<String, bool>) {
    /// One entry's boolean (`_parseRuleConfig`, config.dart:83-104): a YAML
    /// bool, or a severity string — `ConfiguredSeverity` has no `ignore`
    /// member (config.dart:116-131), so only the exact `disable` disables
    /// and every other string enables.
    fn entry_bool(value: &Value) -> Option<bool> {
        match value {
            Value::Bool(enabled) => Some(*enabled),
            Value::String(severity) => Some(severity != "disable"),
            _ => None,
        }
    }
    match rules {
        Value::Sequence(list) => {
            for item in list {
                if let Some(name) = item.as_str() {
                    merged.insert(name.to_lowercase(), true);
                }
            }
        }
        Value::Mapping(map) => {
            for (key, value) in map {
                let Some(name) = key.as_str() else {
                    continue;
                };
                if let Some(enabled) = entry_bool(value) {
                    merged.insert(name.to_lowercase(), enabled);
                } else if let Value::Mapping(group) = value {
                    // The group form recurses exactly one level
                    // (config.dart:92-103 — the inner forEach parses scalar
                    // entries only; a deeper map drops, it does not
                    // descend).
                    for (group_key, group_value) in group {
                        if let Some(group_name) = group_key.as_str()
                            && let Some(enabled) = entry_bool(group_value)
                        {
                            merged.insert(group_name.to_lowercase(), enabled);
                        }
                    }
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
        // The official iterates `diagnosticCodeValues` — the whole pinned
        // registry universe, not dartq's own emit table — matching each
        // diagnostic's *effective* severity against the arm: this file's
        // `errors` override first (`ErrorProcessor.code` compares against
        // `diagnostic.lowerCaseName`; a `null`-severity ignore processor
        // filters the code out of the arm entirely), else the registry
        // `type.severity`.
        for arm in arms {
            for &(name, declared) in known_error_codes::SEVERITIES {
                let effective = match errors.get(name) {
                    Some(Some(severity)) => Some(*severity),
                    Some(None) => None,
                    None => Some(declared),
                };
                if effective == Some(arm) {
                    unignorable.insert(name.to_owned());
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

/// The `toBool` conversion (util/yaml.dart:156-169): a YAML boolean, or a
/// `true`/`false` string case-insensitively; anything else fails (and
/// reports `unsupported_value` from [`local_issues`]).
fn to_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(enabled) => Some(*enabled),
        Value::String(text) => match text.to_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// The `analyzer.language` strictness booleans (`_ParsedLanguageData.parse`,
/// parse_model:904-998): map-only section; the three strict-* keys parse
/// through the `toBool` conversion ([`to_bool`]), non-boolean values and
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
    let parse = |key: &str| language.get(Value::from(key)).and_then(to_bool);
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

/// `The exclude glob pattern '{0}' shouldn't start with a forward slash.`
/// (registry :5057-5069, no published docs; the correction is `Remove the
/// leading forward slash.`.)
const EXCLUDE_INVALID_GLOB: CodeInfo = CodeInfo::new(
    "exclude_invalid_glob",
    "EXCLUDE_INVALID_GLOB",
    DiagnosticType::StaticWarning,
    false,
);

/// `The value '{1}' isn't supported by '{0}'.` (registry :17914-17929; the
/// correction is `Try using one of the supported values: {2}.`.)
const UNSUPPORTED_VALUE: CodeInfo = CodeInfo::new(
    "unsupported_value",
    "UNSUPPORTED_VALUE",
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

/// The `validateFlags` rows for one file's flag list (`experiments_impl.dart`
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

/// The `analyzer.strong-mode` keys the 3.15 parse model accepts
/// (parse_model:1888-1899 — `declaration-casts` is in the file-keys set but
/// not in the parser's switch, so it reports too).
const STRONG_MODE_OPTIONS: [&str; 2] = ["implicit-casts", "implicit-dynamic"];

/// The `analyzer.optional-checks` keys (`optionalChecksOptions`,
/// analysis_options_file.dart:120-124).
const OPTIONAL_CHECKS_OPTIONS: [&str; 2] =
    ["chrome-os-manifest-checks", "propagate-linter-exceptions"];

/// The legal `errors` action values, in the official list order with its
/// duplicate preserved verbatim — `errorProcessorLegalValues` is
/// `[...ignoreSynonyms, ...trueOrFalse, ...severities]` (parse_model:55-59)
/// over `['ignore', 'false']` + `['true', 'false']` + `severityMap.keys`
/// (error_processor.dart:14-19: error, info, warning) — and the correction
/// text prints the list as-is.
const ERROR_ACTIONS: [&str; 7] = [
    "ignore", "false", "true", "false", "error", "info", "warning",
];

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
/// `language` values report `unsupported_value` and leading-`/` exclude
/// patterns `exclude_invalid_glob`. The shape-only sections ride the same
/// anchors: `strong-mode` (:1841-1911, not null-tolerant) and
/// `optional-checks` (:1399-1499) report their section/key/value arms with
/// no merge behavior, and the top-level `code-style` (:407-468) and
/// `formatter` (:813-895) report theirs; `plugins` stays the probed 3.13.1
/// row. `enable-experiment`
/// (:470-536: non-list → row, non-string entry → row, then the
/// `validateFlags` rows) is pinned in [`experiment_flag_issues`].
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
                    if !ERROR_ACTIONS.contains(&action.to_lowercase().as_str()) {
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
        // `exclude` (:644-670) — same not-null-tolerant shape; a
        // leading-`/` string entry reports `exclude_invalid_glob`
        // (:660-668) while still contributing its never-matching glob.
        match analyzer.get(Value::from("exclude")) {
            None => {}
            Some(Value::Sequence(list)) => {
                for entry in list {
                    match entry.as_str() {
                        Some(pattern) if pattern.starts_with('/') => {
                            push_exclude_invalid_glob(&mut issues, text, pattern);
                        }
                        Some(_) => {}
                        None => push_invalid_section(&mut issues, text, "exclude", "exclude"),
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
                        // sectioned by the option name).
                        push_invalid_section(&mut issues, text, name, name);
                    } else if to_bool(value).is_none() {
                        // A non-boolean scalar is `unsupported_value`
                        // (:944-956, the `toBool` miss) with the
                        // `trueOrFalseProposal` legal values; a tagged node
                        // has no scalar text and stays silent.
                        if let Some(invalid) = yaml_scalar_text(value) {
                            push_unsupported_value(&mut issues, text, name, &invalid);
                        }
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
                push_invalid_section(&mut issues, text, "enable-experiment", "enable-experiment");
            }
        }
        // `strong-mode` (parse_model:1841-1911) — NOT null-tolerant (only
        // an absent key is silent; a bare `strong-mode:` is a scalar node
        // → row). Only `implicit-casts`/`implicit-dynamic` parse; every
        // other key reports `unsupported_option` with their AND-proposal.
        match analyzer.get(Value::from("strong-mode")) {
            None => {}
            Some(Value::Mapping(strong_mode)) => {
                for (key, value) in strong_mode {
                    // Non-scalar keys skip (`keyNode is! YamlScalar` →
                    // continue); non-string scalar keys render their text
                    // and fall to the default arm.
                    let Some(name) = yaml_scalar_text(key) else {
                        continue;
                    };
                    match name.as_str() {
                        "implicit-casts" | "implicit-dynamic" => {
                            push_bool_value(&mut issues, text, &name, value);
                        }
                        other => push_unsupported(
                            &mut issues,
                            text,
                            other,
                            "strong-mode",
                            &STRONG_MODE_OPTIONS,
                        ),
                    }
                }
            }
            Some(_) => push_invalid_section(&mut issues, text, "strong-mode", "strong-mode"),
        }
        // `analyzer.plugins` — the legacy plugins key reports
        // `analysis_options_deprecated_plugins` unconditionally at the key
        // node (upstream `_ParsedAnalyzerData.parse`,
        // parse_model:285-289: `analyzer.getKey(plugins)` fires on the
        // key's mere presence — a bare `plugins:` null value reports too,
        // and the row precedes the remaining section parses).
        if analyzer.keys().any(|key| key.as_str() == Some("plugins")) {
            let (offset, length) = anchor_in_section(text, "analyzer", "plugins");
            issues.push(OptionsIssue {
                code: ANALYSIS_OPTIONS_DEPRECATED_PLUGINS,
                message: "Support for legacy plugins is deprecated, and will be removed in an \
                          upcoming version of Dart."
                    .to_owned(),
                correction: Some(
                    "See https://dart.dev/tools/analyzer-plugins for documentation regarding \
                     the new analyzer plugin system."
                        .to_owned(),
                ),
                offset,
                length,
            });
        }
        // `optional-checks` (:1399-1499) — the same parseBool shape as
        // strong-mode under the two check keys; the unknown-key row is the
        // odd face whose section-name argument is the legal-key list
        // itself (`reportUnsupportedKey` :1423-1433), and the scalar form
        // enables the legal names silently while any other scalar — a
        // bare null rendering `null` too — reports.
        match analyzer.get(Value::from("optional-checks")) {
            None => {}
            Some(Value::Mapping(section)) => {
                for (key, value) in section {
                    let Some(name) = yaml_scalar_text(key) else {
                        continue;
                    };
                    match name.as_str() {
                        "chrome-os-manifest-checks" | "propagate-linter-exceptions" => {
                            push_bool_value(&mut issues, text, &name, value);
                        }
                        other => push_optional_checks_key(&mut issues, text, other),
                    }
                }
            }
            Some(Value::Sequence(_) | Value::Tagged(_)) => {
                push_invalid_section(&mut issues, text, "optional-checks", "optional-checks");
            }
            Some(scalar) => {
                let name = yaml_scalar_text(scalar).expect("the matched arms are all scalars");
                if !OPTIONAL_CHECKS_OPTIONS.contains(&name.as_str()) {
                    push_optional_checks_key(&mut issues, text, &name);
                }
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
    // The top-level `code-style` (:407-468) and `formatter` (:813-895)
    // sections — both null-tolerant; unknown keys report the
    // without-values `unsupported_option` variant.
    match map.get(Value::from("code-style")) {
        None | Some(Value::Null) => {}
        Some(Value::Mapping(section)) => {
            for (key, value) in section {
                let Some(name) = yaml_scalar_text(key) else {
                    // Container keys stay silent: the official renders the
                    // key with Dart's map/list `toString`, which the
                    // `serde_yaml` Value cannot reproduce.
                    continue;
                };
                match name.as_str() {
                    "format" => match value {
                        Value::Mapping(_) | Value::Sequence(_) | Value::Tagged(_) => {
                            push_invalid_section(&mut issues, text, "format", "format");
                        }
                        scalar => {
                            if to_bool(scalar).is_none()
                                && let Some(invalid) = yaml_scalar_text(scalar)
                            {
                                push_unsupported_value(&mut issues, text, "format", &invalid);
                            }
                        }
                    },
                    other => push_unsupported_no_values(&mut issues, text, other, "code-style"),
                }
            }
        }
        Some(_) => push_invalid_section(&mut issues, text, "code-style", "code-style"),
    }
    match map.get(Value::from("formatter")) {
        None | Some(Value::Null) => {}
        Some(Value::Mapping(section)) => {
            for (key, value) in section {
                let Some(name) = yaml_scalar_text(key) else {
                    continue;
                };
                match name.as_str() {
                    "page_width" => {
                        // `value case int value when value > 0`: integers
                        // only, positive.
                        let positive = match value {
                            Value::Number(number) => number.as_i64().is_some_and(|v| v > 0),
                            _ => false,
                        };
                        if !positive {
                            push_invalid_option(
                                &mut issues,
                                text,
                                "page_width",
                                "\"page_width\" must be a positive integer.",
                                &yaml_scalar_text(value).unwrap_or_default(),
                            );
                        }
                    }
                    "trailing_commas" => {
                        if !matches!(value.as_str(), Some("automate" | "preserve")) {
                            push_invalid_option(
                                &mut issues,
                                text,
                                "trailing_commas",
                                "\"trailing_commas\" must be \"automate\" or \"preserve\".",
                                &yaml_scalar_text(value).unwrap_or_default(),
                            );
                        }
                    }
                    other => push_unsupported_no_values(&mut issues, text, other, "formatter"),
                }
            }
        }
        Some(_) => push_invalid_section(&mut issues, text, "formatter", "formatter"),
    }
    // The top-level `plugins:` section reports the official
    // `_ParsedPluginsData` faces (upstream Dart SDK source,
    // `analysis_options_parse_model.dart`:1529-1837 — VALIDATION ONLY:
    // dartq loads no plugins, so the parsed `PluginConfiguration`s are never
    // consumed; only the reported rows matter).
    plugin_issues(map, text, &mut issues);
    issues
}

/// The legal `plugins` sub-keys (`pluginsOptions`, upstream Dart SDK source,
/// `analysis_options_file.dart`:73-79 — insertion order is the Dart set
/// literal's iteration order, which the legal-values proposal prints).
const PLUGINS_OPTIONS: [&str; 5] = ["diagnostics", "git", "path", "version", "hosted"];

/// The legal `plugins/<name>/git` sub-keys (`gitOptions`,
/// `analysis_options_file.dart`:82 — same insertion-order note as
/// [`PLUGINS_OPTIONS`]).
const GIT_OPTIONS: [&str; 4] = ["url", "ref", "path", "tag_pattern"];

/// The top-level `plugins:` section's row faces — a direct mirror of
/// `_ParsedPluginsData.parse` and its `_validate*` helpers
/// (`analysis_options_parse_model.dart`:1529-1837), reporting rows only:
///
/// - a non-map section reports `invalid_section_format` on `plugins`
///   (a bare `plugins:` null scalar is silent);
/// - `dependency_overrides` maps each entry through the same per-entry
///   validation under the `plugins/dependency_overrides` section name, and
///   a non-map `dependency_overrides` value reports
///   `invalid_section_format` on that name;
/// - a plugin entry whose value is neither a string scalar (a version
///   constraint — always valid) nor a map reports `invalid_section_format`
///   on `plugins/<name>`; non-string scalar keys are skipped entirely
///   (upstream gates validation on `nameNode.value is String`);
/// - a plugin map's keys validate against `pluginsOptions`: `diagnostics`
///   and `git` recurse into their own validators, every other key reports
///   `unsupported_option` (with-values variant) under `plugins/<name>`;
/// - `plugins/<name>/diagnostics` must be a map of code → legal action:
///   a non-map value reports `invalid_section_format`, a non-scalar value
///   reports and returns, and a scalar outside `errorProcessorLegalValues`
///   reports `unsupported_option` (with-values);
/// - `plugins/<name>/git` accepts a string scalar or a map of `gitOptions`
///   keys with string values — anything else reports
///   `invalid_section_format` on `plugins/<name>/git` (or the
///   per-key `plugins/<name>/git/<key>` for a non-string value), and an
///   unknown key reports `unsupported_option`.
fn plugin_issues(map: &serde_yaml::Mapping, text: &str, issues: &mut Vec<OptionsIssue>) {
    let Some(plugins) = map.get(Value::from("plugins")) else {
        return;
    };
    let Some(entries) = plugins.as_mapping() else {
        // A null scalar is silent (`YamlScalar(value: null)` breaks the
        // switch without reporting); every other non-map shape reports.
        if !plugins.is_null() {
            push_invalid_section(issues, text, "plugins", "plugins");
        }
        return;
    };
    for (name_node, plugin_node) in entries {
        let Some(name_scalar) = yaml_scalar_text(name_node) else {
            // `nameNode is! YamlScalar` — a container key is skipped.
            continue;
        };
        if name_scalar == "dependency_overrides" {
            match plugin_node.as_mapping() {
                Some(overrides) => {
                    for (override_key, override_value) in overrides {
                        let Some(override_name) = yaml_scalar_text(override_key) else {
                            continue;
                        };
                        // Upstream validates only string-valued keys
                        // (`overrideNameNode.value is String`).
                        if override_key.is_string() {
                            validate_plugin_entry(
                                issues,
                                text,
                                "plugins/dependency_overrides",
                                &override_name,
                                override_value,
                            );
                        }
                    }
                }
                None => push_plugin_invalid_section(
                    issues,
                    text,
                    "plugins/dependency_overrides",
                    "dependency_overrides",
                ),
            }
            continue;
        }
        // `_validatePlugin` fires only for string-valued keys
        // (`nameNode.value is String`, parse_model:1617).
        if name_node.is_string() {
            validate_plugin_entry(issues, text, "plugins", &name_scalar, plugin_node);
        }
    }
}

/// `_validatePlugin` (`analysis_options_parse_model.dart`:1787-1808): a
/// string scalar is a version constraint (always valid), a map goes through
/// `_validatePluginMap`, and anything else — a non-string scalar, a list —
/// reports `invalid_section_format` under `plugins/<name>` (or
/// `plugins/dependency_overrides/<name>`).
fn validate_plugin_entry(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    parent: &str,
    name: &str,
    value: &Value,
) {
    let section = format!("{parent}/{name}");
    match value {
        Value::String(_) => {}
        Value::Mapping(plugin_map) => {
            for (key_node, value_node) in plugin_map {
                let Some(key) = key_node.as_str() else {
                    continue;
                };
                match key {
                    "diagnostics" => {
                        validate_plugin_diagnostics(issues, text, &section, value_node)
                    }
                    "git" => validate_plugin_git(issues, text, &section, value_node),
                    _ if !PLUGINS_OPTIONS.contains(&key) => {
                        push_plugin_unsupported(issues, text, key, &section, &PLUGINS_OPTIONS)
                    }
                    _ => {}
                }
            }
        }
        _ => {
            push_plugin_invalid_section(issues, text, &section, name);
        }
    }
}

/// `_validateDiagnostics` (`analysis_options_parse_model.dart`:1695-1740):
/// the section name is `plugins/<name>/diagnostics`; a non-map value
/// reports `invalid_section_format`, a non-scalar entry value reports it
/// and *returns* (the upstream `return` stops the loop), and a scalar
/// action outside `errorProcessorLegalValues` reports `unsupported_option`
/// with the legal-values proposal.
fn validate_plugin_diagnostics(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    plugin_path: &str,
    value: &Value,
) {
    let section = format!("{plugin_path}/diagnostics");
    let Some(entries) = value.as_mapping() else {
        push_plugin_invalid_section(issues, text, &section, "diagnostics");
        return;
    };
    for (_, severity_node) in entries {
        let Some(action) = yaml_scalar_text(severity_node) else {
            push_plugin_invalid_section(issues, text, &section, "diagnostics");
            return;
        };
        if !ERROR_ACTIONS.contains(&action.to_lowercase().as_str()) {
            push_plugin_unsupported(issues, text, &action, &section, &ERROR_ACTIONS);
        }
    }
}

/// `_validateGit` (`analysis_options_parse_model.dart`:1742-1785): the
/// section name is `plugins/<name>/git`; a non-string scalar or a non-map
/// value reports `invalid_section_format` under it, a map's unknown key
/// reports `unsupported_option` with the `gitOptions` proposal, and a legal
/// key whose value is not a string scalar reports `invalid_section_format`
/// under `plugins/<name>/git/<key>`.
fn validate_plugin_git(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    plugin_path: &str,
    value: &Value,
) {
    let section = format!("{plugin_path}/git");
    match value {
        Value::Mapping(entries) => {
            for (key_node, value_node) in entries {
                let Some(key) = key_node.as_str() else {
                    continue;
                };
                if !GIT_OPTIONS.contains(&key) {
                    push_plugin_unsupported(issues, text, key, &section, &GIT_OPTIONS);
                } else if !value_node.is_string() {
                    let sub = format!("{section}/{key}");
                    let token = yaml_scalar_text(value_node).unwrap_or_else(|| key.to_owned());
                    push_plugin_invalid_section(issues, text, &sub, &token);
                }
            }
        }
        Value::String(_) => {}
        _ => {
            push_plugin_invalid_section(issues, text, &section, "git");
        }
    }
}

/// The plugins rows' anchor: upstream spans every row at the offending YAML
/// node; serde_yaml keeps no spans, so the row anchors at `token`'s first
/// occurrence inside the `plugins` section (the established
/// [`anchor_in_section`] convention — the `(code, file)` face is exact).
fn plugin_anchor(text: &str, token: &str) -> (u32, u32) {
    anchor_in_section(text, "plugins", token)
}

/// One `invalid_section_format` row whose message carries the nested
/// section name (`plugins/<name>` / `plugins/<name>/git/…`) and whose
/// anchor is `token`'s occurrence inside `plugins` ([`plugin_anchor`]).
fn push_plugin_invalid_section(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    section: &str,
    token: &str,
) {
    let (offset, length) = plugin_anchor(text, token);
    issues.push(OptionsIssue {
        code: INVALID_SECTION_FORMAT,
        message: format!("Invalid format for the '{section}' section."),
        correction: None,
        offset,
        length,
    });
}

/// One `unsupported_option` row under a nested plugin section — the
/// with-values variant whose legal list is the section's own key set
/// ([`proposal`]'s AND-join).
fn push_plugin_unsupported(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    option: &str,
    section: &str,
    legal: &[&str],
) {
    let (offset, length) = plugin_anchor(text, option);
    issues.push(OptionsIssue {
        code: UNSUPPORTED_OPTION,
        message: format!("The option '{option}' isn't supported by '{section}'."),
        correction: Some(format!(
            "Try using one of the supported options: {}.",
            proposal(legal)
        )),
        offset,
        length,
    });
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

/// Records one `exclude_invalid_glob` row (`The exclude glob pattern '{0}'
/// shouldn't start with a forward slash.`, registry :5057-5069; correction
/// `Remove the leading forward slash.`), anchored at the pattern's own
/// occurrence inside the `exclude` section.
fn push_exclude_invalid_glob(issues: &mut Vec<OptionsIssue>, text: &str, pattern: &str) {
    let (offset, length) = anchor_in_section(text, "exclude", pattern);
    issues.push(OptionsIssue {
        code: EXCLUDE_INVALID_GLOB,
        message: format!(
            "The exclude glob pattern '{pattern}' shouldn't start with a forward slash."
        ),
        correction: Some("Remove the leading forward slash.".to_owned()),
        offset,
        length,
    });
}

/// Records one `unsupported_value` row (`The value '{1}' isn't supported by
/// '{0}'.`, registry :17914-17929) with the `trueOrFalse` proposal
/// (`'true' and 'false'`, string.dart:72-107), anchored at the value's
/// occurrence after its option key.
fn push_unsupported_value(issues: &mut Vec<OptionsIssue>, text: &str, option: &str, invalid: &str) {
    let (offset, length) = anchor_in_section(text, option, invalid);
    issues.push(OptionsIssue {
        code: UNSUPPORTED_VALUE,
        message: format!("The value '{invalid}' isn't supported by '{option}'."),
        correction: Some(format!(
            "Try using one of the supported values: {}.",
            proposal(&["true", "false"])
        )),
        offset,
        length,
    });
}

/// Records one `unsupported_option` row in the without-values variant
/// (`unsupportedOptionWithoutValues`, registry :17889-17898 — the
/// correction is `Try removing the option.`), anchored at the option key.
fn push_unsupported_no_values(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    option: &str,
    section: &str,
) {
    let (offset, length) = anchor_in_section(text, section, option);
    issues.push(OptionsIssue {
        code: UNSUPPORTED_OPTION,
        message: format!("The option '{option}' isn't supported by '{section}'."),
        correction: Some("Try removing the option.".to_owned()),
        offset,
        length,
    });
}

/// Records one `invalid_option` row (`Invalid option specified for '{0}':
/// {1}`, registry :9521-9535, no correction), anchored at the value token
/// `at` searched from the option key onward.
fn push_invalid_option(
    issues: &mut Vec<OptionsIssue>,
    text: &str,
    option: &str,
    detail: &str,
    at: &str,
) {
    let (offset, length) = anchor_in_section(text, option, at);
    issues.push(OptionsIssue {
        code: INVALID_OPTION,
        message: format!("Invalid option specified for '{option}': {detail}"),
        correction: None,
        offset,
        length,
    });
}

/// The `parseBool` shape shared by strong-mode and optional-checks
/// (parse_model:1405-1421, :1859-1875): a container value is an
/// `invalid_section_format` row sectioned by the option name, a scalar
/// failing the `toBool` conversion reports `unsupported_value` with the
/// `trueOrFalse` proposal, and a legal boolean stays silent.
fn push_bool_value(issues: &mut Vec<OptionsIssue>, text: &str, name: &str, value: &Value) {
    match value {
        Value::Mapping(_) | Value::Sequence(_) | Value::Tagged(_) => {
            push_invalid_section(issues, text, name, name);
        }
        scalar => {
            if to_bool(scalar).is_none()
                && let Some(invalid) = yaml_scalar_text(scalar)
            {
                push_unsupported_value(issues, text, name, &invalid);
            }
        }
    }
}

/// The optional-checks unknown-key row — the odd face whose section-name
/// argument is the legal-key list itself (`reportUnsupportedKey`,
/// parse_model:1423-1433 passes `optionalChecksOptions
/// .quotedAndCommaSeparatedWithOr`), so the rendered message carries the
/// doubled quotes verbatim; the correction is the builder's AND-form
/// proposal (_ErrorBuilder :133-144).
fn push_optional_checks_key(issues: &mut Vec<OptionsIssue>, text: &str, key: &str) {
    let (offset, length) = anchor_in_section(text, "optional-checks", key);
    issues.push(OptionsIssue {
        code: UNSUPPORTED_OPTION,
        message: format!(
            "The option '{key}' isn't supported by ''chrome-os-manifest-checks' \
             or 'propagate-linter-exceptions'."
        ),
        correction: Some(
            "Try using one of the supported options: \
             'chrome-os-manifest-checks' and 'propagate-linter-exceptions'."
                .to_owned(),
        ),
        offset,
        length,
    });
}

/// The `quotedAndCommaSeparatedWithAnd` form (string.dart:72-107): one
/// element bare-quoted, two joined by ` and `, three or more comma-separated
/// with the Oxford comma before `and`.
fn proposal(values: &[&str]) -> String {
    proposal_with("and", values)
}

/// The [`proposal`] join with the trailing conjunction spelled — the same
/// extension family's `quotedAndCommaSeparatedWithOr` (string.dart:79-83)
/// reads through `"or"`.
fn proposal_with(conjunction: &str, values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [one] => format!("'{one}'"),
        [first, second] => format!("'{first}' {conjunction} '{second}'"),
        _ => {
            let mut parts: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
            let last = parts.pop().expect("non-empty above");
            format!("{}, {} {}", parts.join(", "), conjunction, last)
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

/// The `ruleValue.toString()` form of a container node — the
/// `unsupported_value` row's `invalidValue` argument for a group-map or
/// list rule value (`parseRule`'s fall-through arm, parse_model.dart
/// :1272-1284). Upstream's YamlMap/YamlList render through Dart's
/// `Map`/`List` `toString` — `{k: v, k: v}` / `[a, b]` in insertion order,
/// scalars bare (`a`, `42`, `true`, `null`), nested containers recursing.
/// A tagged node renders its inner value; an empty map is `{}` and an
/// empty list `[]`.
fn yaml_container_text(value: &Value) -> String {
    match value {
        Value::Mapping(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}: {}",
                        yaml_container_text(key),
                        yaml_container_text(value)
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
        Value::Sequence(list) => {
            let entries: Vec<String> = list.iter().map(yaml_container_text).collect();
            format!("[{}]", entries.join(", "))
        }
        Value::Tagged(tagged) => yaml_container_text(&tagged.value),
        scalar => yaml_scalar_text(scalar).unwrap_or_default(),
    }
}

/// The lint names the initial file's own `linter.rules` declares
/// (`_parseLocalRules`, parse_model:1224-1378): list entries and map keys —
/// a `name:` entry with a null value returns before the registry check
/// (:1252-1253), group maps are not descended (the local parse validates
/// top-level entries only; the merge keeps its config.dart recursion), and
/// non-string names are skipped (:1238-1239). Names keep their original
/// spelling — the registry lookups downstream are case-insensitive and the
/// rows name the written form. The value facts follow `parseRule`
/// (:1266-1283): a bool enables/disables, a legal severity string enables
/// unless exactly `ignore`, any other scalar records its text as
/// [`DeclaredRule::illegal_value`] and disables, and a group map disables
/// with no value text.
fn declared_rule_names(rules: &Value, text: &str) -> Vec<DeclaredRule> {
    /// The legal severity strings (`_validLintStringValues`,
    /// parse_model:1097-1102 — the match is case-sensitive).
    const VALID_STRING_VALUES: [&str; 4] = ["ignore", "info", "warning", "error"];
    let mut names = Vec::new();
    // Per-occurrence anchoring: every string entry — recorded or skipped —
    // consumes its own text occurrence, so a repeated name anchors at the
    // occurrence it was declared at (the `duplicate_rule` row's reported
    // span, upstream's per-node anchoring) instead of collapsing onto the
    // first. `Cell` keeps `anchor` a shared-borrow closure so both `push`
    // and the skip arm below can advance the cursor.
    let search_from = std::cell::Cell::new(text.find("rules").unwrap_or(0));
    let anchor = |name: &str| anchor_occurrence(text, "rules", &search_from, name);
    let mut push = |name: &str, value: &Value| {
        let (enabled, illegal_value) = match value {
            Value::Bool(enabled) => (*enabled, None),
            Value::String(severity) if VALID_STRING_VALUES.contains(&severity.as_str()) => {
                (severity != "ignore", None)
            }
            scalar @ (Value::String(_) | Value::Number(_)) => (false, yaml_scalar_text(scalar)),
            // A container value still reports `unsupported_value` upstream —
            // `ruleValue.toString()` renders the YamlMap/YamlList through
            // Dart's `Map`/`List` `toString` (`{a: b}`, `[x]`); `Tagged`
            // unwraps to its inner value first. A bare null value returns
            // before the registry check upstream — the caller's
            // `!value.is_null()` gate already kept it out.
            Value::Mapping(_) | Value::Sequence(_) | Value::Tagged(_) => {
                (false, Some(yaml_container_text(value)))
            }
            Value::Null => (false, None),
        };
        let (offset, length) = anchor(name);
        names.push(DeclaredRule {
            name: name.to_owned(),
            offset,
            length,
            enabled,
            illegal_value,
        });
    };
    match rules {
        Value::Sequence(list) => {
            for entry in list {
                if let Some(name) = entry.as_str() {
                    push(name, &Value::Bool(true));
                }
            }
        }
        Value::Mapping(map) => {
            for (key, value) in map {
                if let Some(name) = key.as_str() {
                    if value.is_null() {
                        // A null-valued key returns before the registry
                        // check upstream, but its token still occupies an
                        // occurrence — consume it so a later same-named
                        // entry anchors at its own span.
                        anchor(name);
                    } else {
                        push(name, value);
                    }
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
    // Per-occurrence anchoring like `declared_rule_names`: each key
    // consumes its own occurrence, so a repeated key anchors at the
    // occurrence being reported.
    let search_from = std::cell::Cell::new(text.find("errors").unwrap_or(0));
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
        let (offset, length) = anchor_occurrence(text, "errors", &search_from, &code);
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

/// [`anchor_in_section`]'s per-occurrence variant for a declaration stream:
/// `search_from` carries the byte offset the next search starts at and is
/// advanced past each found token, so successive calls on a section's
/// entries anchor each occurrence at its own span — the per-node anchoring
/// upstream's `YamlNode` spans give every repeated entry (a duplicated
/// rule name reports at the repeated occurrence, never the first).
/// `search_from` starts at the section key's occurrence; a token missing
/// past the cursor falls back to [`anchor_in_section`]'s whole-section
/// search (the miss posture, same rows).
fn anchor_occurrence(
    text: &str,
    section: &str,
    search_from: &std::cell::Cell<usize>,
    token: &str,
) -> (u32, u32) {
    match text[search_from.get()..].find(token) {
        Some(relative) => {
            let byte = search_from.get() + relative;
            search_from.set(byte + token.len());
            (byte_offset_to_utf16(text, byte), utf16_len_of(token))
        }
        None => anchor_in_section(text, section, token),
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

/// The diagnostic code universe the pinned SDK's public registry knows — the
/// `codeNames` set of `_DiagnosticOptions.isKnownDiagnosticOrLintCode`
/// (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/analysis_options/analysis_options_parse_model.dart`
/// :56-99 — built as `diagnosticCodeValues.map((code) =>
/// code.lowerCaseName.toUpperCase())`, i.e. the registry's `name` field
/// lowercased, NOT `uniqueName`; the two spellings diverge on 225 registry
/// entries) crossed with the `(lowerCaseName, default severity)` pairs
/// `_applyCannotIgnore`'s severity arms iterate (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/analysis_options/analysis_options_parser.dart`
/// :173-197). One table serves both faces: [`SEVERITIES`]' name column is the
/// membership universe — a name-sharing family collapses to one row per
/// (name, severity) pair, so `multiple_combinators` appears twice, once per
/// `Error`/`Warning` registry code that shares it.
///
/// The single fact source is `diagnosticCodeValues` (upstream Dart SDK
/// source, `pkg/analyzer/lib/src/diagnostic/diagnostic_code_values.g.dart`,
/// dart 3.15 main — 1363 list entries resolving to 982 distinct names / 983
/// pairs; version-pinned data like [`KNOWN_EXPERIMENTS`]). Codes removed from
/// the registry but still accepted live in `REMOVED_ERROR_CODES`, not here;
/// lint-rule names are a separate registry (dartq-lints) — the CLI face's
/// seam ORs them in.
mod known_error_codes {
    use dartq_diagnostics::Severity;

    /// Whether `lowered` — an already-lowercased `analyzer.errors` key —
    /// names a code in the registry universe. Binary search over
    /// [`SEVERITIES`]' name column (sorted by name; the one duplicated name
    /// `multiple_combinators` sits on adjacent rows).
    pub fn contains(lowered: &str) -> bool {
        SEVERITIES
            .binary_search_by(|&(name, _)| name.cmp(lowered))
            .is_ok()
    }

    /// The `(lowerCaseName, default severity)` pairs of `diagnosticCodeValues`
    /// — see the module doc for the provenance and the two upstream faces the
    /// one table serves. Sorted by name then severity.
    pub const SEVERITIES: &[(&str, Severity)] = &[
        ("abi_specific_integer_invalid", Severity::Error),
        ("abi_specific_integer_mapping_extra", Severity::Error),
        ("abi_specific_integer_mapping_missing", Severity::Error),
        ("abi_specific_integer_mapping_unsupported", Severity::Error),
        ("abstract_class_member", Severity::Error),
        ("abstract_external_field", Severity::Error),
        ("abstract_field_initializer", Severity::Error),
        ("abstract_final_base_class", Severity::Error),
        ("abstract_final_interface_class", Severity::Error),
        ("abstract_late_field", Severity::Error),
        ("abstract_sealed_class", Severity::Error),
        ("abstract_static_field", Severity::Error),
        ("abstract_static_method", Severity::Error),
        ("abstract_super_member_reference", Severity::Error),
        ("address_position", Severity::Error),
        ("address_receiver", Severity::Error),
        ("ambiguous_comment_reference", Severity::Warning),
        ("ambiguous_export", Severity::Error),
        ("ambiguous_extension_member_access", Severity::Error),
        ("ambiguous_import", Severity::Error),
        ("ambiguous_set_or_map_literal_both", Severity::Error),
        ("ambiguous_set_or_map_literal_either", Severity::Error),
        ("analysis_option_deprecated", Severity::Warning),
        ("analysis_options_deprecated_plugins", Severity::Warning),
        ("annotation_on_pointer_field", Severity::Error),
        ("annotation_on_type_argument", Severity::Error),
        ("annotation_space_before_parenthesis", Severity::Error),
        ("annotation_with_type_arguments", Severity::Error),
        (
            "annotation_with_type_arguments_uninstantiated",
            Severity::Error,
        ),
        ("anonymous_method_wrong_parameter_list", Severity::Error),
        ("anonymous_method_wrong_parameter_type", Severity::Error),
        ("argument_must_be_a_constant", Severity::Error),
        ("argument_must_be_native", Severity::Error),
        ("argument_type_not_assignable", Severity::Error),
        (
            "argument_type_not_assignable_to_error_handler",
            Severity::Warning,
        ),
        ("assert_in_redirecting_constructor", Severity::Error),
        ("asset_directory_does_not_exist", Severity::Warning),
        ("asset_does_not_exist", Severity::Warning),
        ("asset_field_not_list", Severity::Warning),
        ("asset_missing_path", Severity::Warning),
        ("asset_not_string", Severity::Warning),
        ("asset_not_string_or_map", Severity::Warning),
        ("asset_path_not_string", Severity::Warning),
        ("assignment_of_do_not_store", Severity::Warning),
        ("assignment_to_const", Severity::Error),
        ("assignment_to_final", Severity::Error),
        ("assignment_to_final_local", Severity::Error),
        ("assignment_to_final_no_setter", Severity::Error),
        ("assignment_to_function", Severity::Error),
        ("assignment_to_method", Severity::Error),
        (
            "assignment_to_primary_constructor_parameter",
            Severity::Error,
        ),
        ("assignment_to_type", Severity::Error),
        ("async_for_in_wrong_context", Severity::Error),
        ("async_keyword_used_as_identifier", Severity::Error),
        (
            "augmentation_extends_clause_already_present",
            Severity::Error,
        ),
        (
            "augmentation_formal_parameter_modifier_extra",
            Severity::Error,
        ),
        (
            "augmentation_formal_parameter_modifier_missing",
            Severity::Error,
        ),
        (
            "augmentation_formal_parameter_type_mismatch",
            Severity::Error,
        ),
        ("augmentation_modifier_extra", Severity::Error),
        ("augmentation_modifier_missing", Severity::Error),
        ("augmentation_named_formal_parameter_extra", Severity::Error),
        (
            "augmentation_named_formal_parameter_missing",
            Severity::Error,
        ),
        (
            "augmentation_of_different_declaration_kind",
            Severity::Error,
        ),
        ("augmentation_of_mixin_application_class", Severity::Error),
        (
            "augmentation_optional_positional_formal_parameter_count",
            Severity::Error,
        ),
        (
            "augmentation_positional_formal_parameter_name",
            Severity::Error,
        ),
        (
            "augmentation_required_positional_formal_parameter_count",
            Severity::Error,
        ),
        ("augmentation_return_type_mismatch", Severity::Error),
        ("augmentation_type_parameter_bound", Severity::Error),
        ("augmentation_type_parameter_count", Severity::Error),
        ("augmentation_type_parameter_name", Severity::Error),
        (
            "augmentation_variable_different_getter_setter_types",
            Severity::Error,
        ),
        ("augmentation_without_declaration", Severity::Error),
        ("augmented_expression_is_not_setter", Severity::Error),
        ("augmented_expression_is_setter", Severity::Error),
        ("augmented_expression_not_operator", Severity::Error),
        ("augments_constant_variable", Severity::Error),
        ("await_in_late_local_variable_initializer", Severity::Error),
        ("await_in_wrong_context", Severity::Error),
        ("await_of_incompatible_type", Severity::Error),
        ("base_enum", Severity::Error),
        ("binary_operator_written_out", Severity::Error),
        ("body_might_complete_normally", Severity::Error),
        (
            "body_might_complete_normally_catch_error",
            Severity::Warning,
        ),
        ("body_might_complete_normally_nullable", Severity::Warning),
        ("break_label_on_switch_member", Severity::Error),
        ("break_outside_of_loop", Severity::Error),
        ("built_in_identifier_as_type", Severity::Error),
        ("built_in_identifier_in_declaration", Severity::Error),
        ("camera_permissions_incompatible", Severity::Warning),
        ("case_expression_type_implements_equals", Severity::Error),
        (
            "case_expression_type_is_not_switch_expression_subtype",
            Severity::Error,
        ),
        ("cast_from_null_always_fails", Severity::Warning),
        ("cast_from_nullable_always_fails", Severity::Warning),
        ("cast_to_non_type", Severity::Error),
        ("catch_syntax", Severity::Error),
        ("catch_syntax_extra_parameters", Severity::Error),
        ("class_in_class", Severity::Error),
        ("class_instantiation_access_to_member", Severity::Error),
        ("class_used_as_mixin", Severity::Error),
        (
            "class_used_as_mixin_declares_generative_constructor",
            Severity::Error,
        ),
        ("collection_element_from_deferred_library", Severity::Error),
        ("colon_in_place_of_in", Severity::Error),
        ("compound_implements_finalizable", Severity::Error),
        ("concrete_class_has_enum_superinterface", Severity::Error),
        ("concrete_class_with_abstract_member", Severity::Error),
        ("conflicting_constructor_and_static_member", Severity::Error),
        ("conflicting_field_and_method", Severity::Error),
        ("conflicting_generic_interfaces", Severity::Error),
        ("conflicting_inherited_method_and_setter", Severity::Error),
        ("conflicting_method_and_field", Severity::Error),
        ("conflicting_modifiers", Severity::Error),
        ("conflicting_static_and_instance", Severity::Error),
        ("conflicting_type_variable_and_container", Severity::Error),
        ("conflicting_type_variable_and_member", Severity::Error),
        ("const_and_final", Severity::Error),
        ("const_class", Severity::Error),
        ("const_constructor_field_type_mismatch", Severity::Error),
        ("const_constructor_param_type_mismatch", Severity::Error),
        ("const_constructor_throws_exception", Severity::Error),
        ("const_constructor_with_body", Severity::Error),
        (
            "const_constructor_with_field_initialized_by_non_const",
            Severity::Error,
        ),
        ("const_constructor_with_mixin_with_field", Severity::Error),
        ("const_constructor_with_non_const_super", Severity::Error),
        ("const_constructor_with_non_final_field", Severity::Error),
        ("const_deferred_class", Severity::Error),
        ("const_eval_assertion_failure", Severity::Error),
        ("const_eval_assertion_failure_with_message", Severity::Error),
        ("const_eval_extension_method", Severity::Error),
        ("const_eval_extension_type_method", Severity::Error),
        ("const_eval_for_element", Severity::Error),
        ("const_eval_method_invocation", Severity::Error),
        ("const_eval_primitive_equality", Severity::Error),
        ("const_eval_property_access", Severity::Error),
        ("const_eval_throws_exception", Severity::Error),
        ("const_eval_throws_idbze", Severity::Error),
        ("const_eval_type_bool", Severity::Error),
        ("const_eval_type_bool_int", Severity::Error),
        ("const_eval_type_bool_num_string", Severity::Error),
        ("const_eval_type_int", Severity::Error),
        ("const_eval_type_num", Severity::Error),
        ("const_eval_type_num_string", Severity::Error),
        ("const_eval_type_string", Severity::Error),
        ("const_eval_type_type", Severity::Error),
        ("const_factory", Severity::Error),
        ("const_initialized_with_non_constant_value", Severity::Error),
        (
            "const_initialized_with_non_constant_value_from_deferred_library",
            Severity::Error,
        ),
        ("const_instance_field", Severity::Error),
        ("const_map_key_not_primitive_equality", Severity::Error),
        ("const_method", Severity::Error),
        ("const_not_initialized", Severity::Error),
        ("const_primary_constructor_with_body", Severity::Error),
        ("const_set_element_not_primitive_equality", Severity::Error),
        ("const_spread_expected_list_or_set", Severity::Error),
        ("const_spread_expected_map", Severity::Error),
        ("const_type_parameter", Severity::Error),
        ("const_with_non_const", Severity::Error),
        ("const_with_non_constant_argument", Severity::Error),
        ("const_with_type_parameters", Severity::Error),
        ("const_with_undefined_constructor", Severity::Error),
        ("const_with_undefined_constructor_default", Severity::Error),
        ("const_without_primary_constructor", Severity::Error),
        (
            "constant_pattern_never_matches_value_type",
            Severity::Warning,
        ),
        (
            "constant_pattern_with_non_constant_expression",
            Severity::Error,
        ),
        ("constant_variable_augmentation", Severity::Error),
        ("constructor_with_return_type", Severity::Error),
        ("constructor_with_type_arguments", Severity::Error),
        ("continue_label_invalid", Severity::Error),
        ("continue_outside_of_loop", Severity::Error),
        ("continue_without_label_in_case", Severity::Error),
        ("could_not_infer", Severity::Error),
        ("covariant_and_static", Severity::Error),
        ("covariant_constructor", Severity::Error),
        ("covariant_member", Severity::Error),
        ("creation_of_struct_or_union", Severity::Error),
        ("creation_with_non_type", Severity::Error),
        ("dead_code", Severity::Warning),
        ("dead_code_catch_following_catch", Severity::Warning),
        ("dead_code_on_catch_subtype", Severity::Warning),
        ("dead_null_aware_expression", Severity::Warning),
        ("declaration_already_complete", Severity::Error),
        ("default_in_switch_expression", Severity::Error),
        (
            "default_value_already_specified_in_augmentation_chain",
            Severity::Error,
        ),
        ("default_value_in_function_type", Severity::Error),
        (
            "default_value_in_redirecting_factory_constructor",
            Severity::Error,
        ),
        ("default_value_on_required_parameter", Severity::Error),
        ("deferred_after_prefix", Severity::Error),
        ("deferred_import_of_extension", Severity::Error),
        ("definitely_unassigned_late_local_variable", Severity::Error),
        ("dependencies_field_not_map", Severity::Warning),
        ("deprecated_colon_for_default_value", Severity::Info),
        ("deprecated_export_use", Severity::Warning),
        ("deprecated_extend", Severity::Warning),
        ("deprecated_factory_method", Severity::Warning),
        ("deprecated_field", Severity::Warning),
        ("deprecated_implement", Severity::Warning),
        ("deprecated_instantiate", Severity::Warning),
        ("deprecated_lint", Severity::Warning),
        ("deprecated_lint_with_replacement", Severity::Warning),
        ("deprecated_member_use", Severity::Info),
        ("deprecated_mixin", Severity::Warning),
        ("deprecated_optional", Severity::Warning),
        ("deprecated_subclass", Severity::Warning),
        ("deprecated_subtype_of_function", Severity::Warning),
        (
            "different_inherited_getter_and_setter_types",
            Severity::Error,
        ),
        ("directive_after_declaration", Severity::Error),
        ("disallowed_type_instantiation_expression", Severity::Error),
        ("doc_directive_argument_wrong_format", Severity::Warning),
        ("doc_directive_has_extra_arguments", Severity::Warning),
        (
            "doc_directive_has_unexpected_named_argument",
            Severity::Warning,
        ),
        ("doc_directive_missing_argument", Severity::Warning),
        ("doc_directive_missing_closing_brace", Severity::Warning),
        ("doc_directive_missing_closing_tag", Severity::Warning),
        ("doc_directive_missing_opening_tag", Severity::Warning),
        ("doc_directive_unknown", Severity::Warning),
        ("doc_import_cannot_be_deferred", Severity::Warning),
        ("doc_import_cannot_have_configurations", Severity::Warning),
        ("doc_import_in_part_file", Severity::Warning),
        ("dot_shorthand_missing_context", Severity::Error),
        ("dot_shorthand_undefined_member", Severity::Error),
        ("duplicate_constructor", Severity::Error),
        ("duplicate_deferred", Severity::Error),
        ("duplicate_definition", Severity::Error),
        ("duplicate_export", Severity::Warning),
        ("duplicate_field_formal_parameter", Severity::Error),
        ("duplicate_field_name", Severity::Error),
        ("duplicate_hidden_name", Severity::Warning),
        ("duplicate_ignore", Severity::Warning),
        ("duplicate_import", Severity::Warning),
        ("duplicate_label_in_switch_statement", Severity::Error),
        ("duplicate_named_argument", Severity::Error),
        ("duplicate_part", Severity::Error),
        ("duplicate_pattern_assignment_variable", Severity::Error),
        ("duplicate_pattern_field", Severity::Error),
        ("duplicate_prefix", Severity::Error),
        ("duplicate_rest_element_in_pattern", Severity::Error),
        ("duplicate_rule", Severity::Warning),
        ("duplicate_shown_name", Severity::Warning),
        ("duplicate_variable_pattern", Severity::Error),
        ("duplicated_modifier", Severity::Error),
        ("empty_enum_body", Severity::Error),
        ("empty_map_pattern", Severity::Error),
        ("empty_record_literal_with_comma", Severity::Error),
        ("empty_record_type_named_fields_list", Severity::Error),
        ("empty_record_type_with_comma", Severity::Error),
        ("empty_struct", Severity::Error),
        ("encoding", Severity::Error),
        ("enum_constant_invokes_factory_constructor", Severity::Error),
        ("enum_constant_same_name_as_enclosing", Severity::Error),
        ("enum_in_class", Severity::Error),
        (
            "enum_instantiated_to_bounds_is_not_well_bounded",
            Severity::Error,
        ),
        ("enum_mixin_with_instance_variable", Severity::Error),
        ("enum_with_abstract_member", Severity::Error),
        ("enum_with_name_values", Severity::Error),
        ("enum_without_constants", Severity::Error),
        ("equal_elements_in_const_set", Severity::Error),
        ("equal_elements_in_set", Severity::Warning),
        ("equal_keys_in_const_map", Severity::Error),
        ("equal_keys_in_map", Severity::Warning),
        ("equal_keys_in_map_pattern", Severity::Error),
        ("equality_cannot_be_equality_operand", Severity::Error),
        ("exclude_invalid_glob", Severity::Warning),
        ("expected_body", Severity::Error),
        ("expected_case_or_default", Severity::Error),
        ("expected_class_member", Severity::Error),
        ("expected_else_or_comma", Severity::Error),
        ("expected_executable", Severity::Error),
        ("expected_identifier_but_got_keyword", Severity::Error),
        ("expected_instead", Severity::Error),
        ("expected_list_or_map_literal", Severity::Error),
        ("expected_named_type", Severity::Error),
        ("expected_one_list_pattern_type_arguments", Severity::Error),
        ("expected_one_list_type_arguments", Severity::Error),
        ("expected_one_set_type_arguments", Severity::Error),
        ("expected_representation_field", Severity::Error),
        ("expected_representation_type", Severity::Error),
        ("expected_string_literal", Severity::Error),
        ("expected_token", Severity::Error),
        ("expected_two_map_pattern_type_arguments", Severity::Error),
        ("expected_two_map_type_arguments", Severity::Error),
        ("expected_type_name", Severity::Error),
        ("experiment_not_enabled", Severity::Error),
        ("experiment_not_enabled_off_by_default", Severity::Error),
        ("experimental_member_use", Severity::Warning),
        ("export_directive_after_part_directive", Severity::Error),
        ("export_internal_library", Severity::Error),
        ("export_of_non_library", Severity::Error),
        ("expression_in_map", Severity::Error),
        ("extends_non_class", Severity::Error),
        ("extension_as_expression", Severity::Error),
        ("extension_augmentation_has_on_clause", Severity::Error),
        ("extension_augmentation_without_name", Severity::Error),
        ("extension_conflicting_static_and_instance", Severity::Error),
        ("extension_declares_abstract_member", Severity::Error),
        ("extension_declares_constructor", Severity::Error),
        ("extension_declares_instance_field", Severity::Error),
        ("extension_declares_member_of_object", Severity::Error),
        (
            "extension_override_access_to_static_member",
            Severity::Error,
        ),
        (
            "extension_override_argument_not_assignable",
            Severity::Error,
        ),
        ("extension_override_with_cascade", Severity::Error),
        ("extension_override_without_access", Severity::Error),
        ("extension_primary_constructor", Severity::Error),
        (
            "extension_type_augmentation_specifies_representation_field",
            Severity::Error,
        ),
        (
            "extension_type_constructor_with_super_formal_parameter",
            Severity::Error,
        ),
        (
            "extension_type_constructor_with_super_invocation",
            Severity::Error,
        ),
        ("extension_type_declares_instance_field", Severity::Error),
        ("extension_type_declares_member_of_object", Severity::Error),
        ("extension_type_extends", Severity::Error),
        ("extension_type_implements_disallowed_type", Severity::Error),
        ("extension_type_implements_itself", Severity::Error),
        ("extension_type_implements_not_supertype", Severity::Error),
        (
            "extension_type_implements_representation_not_supertype",
            Severity::Error,
        ),
        ("extension_type_inherited_member_conflict", Severity::Error),
        (
            "extension_type_representation_depends_on_itself",
            Severity::Error,
        ),
        ("extension_type_representation_type_bottom", Severity::Error),
        ("extension_type_with", Severity::Error),
        ("extension_type_with_abstract_member", Severity::Error),
        ("external_class", Severity::Error),
        (
            "external_constructor_with_field_initializers",
            Severity::Error,
        ),
        ("external_constructor_with_initializer", Severity::Error),
        ("external_enum", Severity::Error),
        ("external_factory_redirection", Severity::Error),
        ("external_factory_with_body", Severity::Error),
        ("external_getter_with_body", Severity::Error),
        ("external_late_field", Severity::Error),
        ("external_method_with_body", Severity::Error),
        ("external_operator_with_body", Severity::Error),
        ("external_setter_with_body", Severity::Error),
        ("external_typedef", Severity::Error),
        ("external_with_initializer", Severity::Error),
        ("extra_annotation_on_struct_field", Severity::Error),
        ("extra_positional_arguments", Severity::Error),
        ("extra_positional_arguments_could_be_named", Severity::Error),
        ("extra_size_annotation_carray", Severity::Error),
        ("extraneous_modifier", Severity::Error),
        ("extraneous_modifier_in_extension_type", Severity::Error),
        (
            "extraneous_modifier_in_primary_constructor",
            Severity::Error,
        ),
        ("factory_constructor_new_name", Severity::Error),
        ("factory_not_complete_after_augmentations", Severity::Error),
        ("factory_top_level_declaration", Severity::Error),
        ("factory_with_initializers", Severity::Error),
        ("factory_without_body", Severity::Error),
        (
            "ffi_native_invalid_duplicate_default_asset",
            Severity::Error,
        ),
        ("ffi_native_invalid_multiple_annotations", Severity::Error),
        ("ffi_native_must_be_external", Severity::Error),
        (
            "ffi_native_only_classes_extending_nativefieldwrapperclass1_can_be_pointer",
            Severity::Error,
        ),
        (
            "ffi_native_unexpected_number_of_parameters",
            Severity::Error,
        ),
        (
            "ffi_native_unexpected_number_of_parameters_with_receiver",
            Severity::Error,
        ),
        (
            "field_initialized_by_multiple_initializers",
            Severity::Error,
        ),
        (
            "field_initialized_in_declaration_and_initializer_of_primary_constructor",
            Severity::Error,
        ),
        (
            "field_initialized_in_declaration_and_parameter_of_primary_constructor",
            Severity::Error,
        ),
        (
            "field_initialized_in_initializer_and_declaration",
            Severity::Error,
        ),
        (
            "field_initialized_in_parameter_and_initializer",
            Severity::Error,
        ),
        ("field_initialized_outside_declaring_class", Severity::Error),
        ("field_initializer_factory_constructor", Severity::Error),
        ("field_initializer_not_assignable", Severity::Error),
        ("field_initializer_outside_constructor", Severity::Error),
        ("field_initializer_redirecting_constructor", Severity::Error),
        ("field_initializing_formal_not_assignable", Severity::Error),
        ("field_must_be_external_in_struct", Severity::Error),
        ("final_and_covariant", Severity::Error),
        ("final_and_covariant_late_with_initializer", Severity::Error),
        ("final_and_var", Severity::Error),
        ("final_constructor", Severity::Error),
        ("final_enum", Severity::Error),
        (
            "final_initialized_in_declaration_and_constructor",
            Severity::Error,
        ),
        ("final_method", Severity::Error),
        ("final_mixin", Severity::Error),
        ("final_mixin_class", Severity::Error),
        ("final_not_initialized", Severity::Error),
        ("final_not_initialized_constructor", Severity::Error),
        ("fixme", Severity::Info),
        ("flutter_field_not_map", Severity::Warning),
        ("for_in_of_invalid_element_type", Severity::Error),
        ("for_in_of_invalid_type", Severity::Error),
        ("for_in_with_const_variable", Severity::Error),
        ("function_not_complete_after_augmentations", Severity::Error),
        ("function_typed_parameter_var", Severity::Error),
        ("generic_function_type_cannot_be_bound", Severity::Error),
        (
            "generic_function_type_cannot_be_type_argument",
            Severity::Error,
        ),
        (
            "generic_method_type_instantiation_on_dynamic",
            Severity::Error,
        ),
        ("generic_struct_subclass", Severity::Error),
        ("getter_constructor", Severity::Error),
        ("getter_in_function", Severity::Error),
        ("getter_not_subtype_setter_types", Severity::Error),
        ("getter_with_parameters", Severity::Error),
        ("hack", Severity::Info),
        (
            "if_element_condition_from_deferred_library",
            Severity::Error,
        ),
        ("illegal_assignment_to_non_assignable", Severity::Error),
        ("illegal_async_generator_return_type", Severity::Error),
        ("illegal_async_return_type", Severity::Error),
        ("illegal_character", Severity::Error),
        ("illegal_concrete_enum_member", Severity::Error),
        ("illegal_enum_values", Severity::Error),
        ("illegal_language_version_override", Severity::Error),
        ("illegal_pattern_assignment_variable_name", Severity::Error),
        ("illegal_pattern_identifier_name", Severity::Error),
        ("illegal_pattern_variable_name", Severity::Error),
        ("illegal_sync_generator_return_type", Severity::Error),
        ("implements_before_extends", Severity::Error),
        ("implements_before_on", Severity::Error),
        ("implements_before_with", Severity::Error),
        ("implements_non_class", Severity::Error),
        ("implements_repeated", Severity::Error),
        ("implements_super_class", Severity::Error),
        ("implements_super_class_constraint", Severity::Error),
        (
            "implicit_super_initializer_missing_arguments",
            Severity::Error,
        ),
        ("implicit_this_reference_in_initializer", Severity::Error),
        ("import_deferred_library_with_load_function", Severity::Info),
        ("import_directive_after_part_directive", Severity::Error),
        ("import_internal_library", Severity::Error),
        ("import_of_non_library", Severity::Error),
        ("include_file_not_found", Severity::Warning),
        ("included_file_parse_error", Severity::Error),
        ("included_file_warning", Severity::Warning),
        ("incompatible_lint", Severity::Warning),
        ("incompatible_plugin_source", Severity::Warning),
        ("inconsistent_inheritance", Severity::Error),
        (
            "inconsistent_inheritance_getter_and_method",
            Severity::Error,
        ),
        ("inconsistent_language_version_override", Severity::Error),
        ("inconsistent_pattern_variable_logical_or", Severity::Error),
        (
            "induced_getter_not_complete_after_augmentations",
            Severity::Error,
        ),
        ("induced_getter_without_body", Severity::Error),
        (
            "induced_setter_not_complete_after_augmentations",
            Severity::Error,
        ),
        ("induced_setter_without_body", Severity::Error),
        ("inference_failure_on_collection_literal", Severity::Warning),
        (
            "inference_failure_on_function_invocation",
            Severity::Warning,
        ),
        (
            "inference_failure_on_function_return_type",
            Severity::Warning,
        ),
        ("inference_failure_on_generic_invocation", Severity::Warning),
        ("inference_failure_on_instance_creation", Severity::Warning),
        (
            "inference_failure_on_uninitialized_variable",
            Severity::Warning,
        ),
        ("inference_failure_on_untyped_parameter", Severity::Warning),
        ("initialized_variable_in_for_each", Severity::Error),
        ("initializer_for_non_existent_field", Severity::Error),
        ("initializer_for_static_field", Severity::Error),
        ("initializing_declaring_parameter", Severity::Error),
        (
            "initializing_formal_for_non_existent_field",
            Severity::Error,
        ),
        ("instance_access_to_static_member", Severity::Error),
        ("instance_member_access_from_factory", Severity::Error),
        ("instance_member_access_from_static", Severity::Error),
        ("instantiate_abstract_class", Severity::Error),
        ("instantiate_enum", Severity::Error),
        (
            "instantiate_type_alias_expands_to_type_parameter",
            Severity::Error,
        ),
        ("integer_literal_imprecise_as_double", Severity::Error),
        ("integer_literal_out_of_range", Severity::Error),
        ("interface_enum", Severity::Error),
        ("interface_mixin", Severity::Error),
        ("interface_mixin_class", Severity::Error),
        ("invalid_annotation", Severity::Error),
        (
            "invalid_annotation_constant_value_from_deferred_library",
            Severity::Error,
        ),
        ("invalid_annotation_from_deferred_library", Severity::Error),
        ("invalid_annotation_target", Severity::Warning),
        ("invalid_assignment", Severity::Error),
        ("invalid_await_in_for", Severity::Error),
        ("invalid_await_not_required_annotation", Severity::Warning),
        ("invalid_code_point", Severity::Error),
        ("invalid_comment_reference", Severity::Error),
        ("invalid_constant", Severity::Error),
        ("invalid_constant_const_prefix", Severity::Error),
        ("invalid_constant_pattern_binary", Severity::Error),
        ("invalid_constant_pattern_duplicate_const", Severity::Error),
        (
            "invalid_constant_pattern_empty_record_literal",
            Severity::Error,
        ),
        ("invalid_constant_pattern_generic", Severity::Error),
        ("invalid_constant_pattern_negation", Severity::Error),
        ("invalid_constant_pattern_unary", Severity::Error),
        ("invalid_constructor_name", Severity::Error),
        (
            "invalid_covariant_modifier_in_primary_constructor",
            Severity::Error,
        ),
        ("invalid_dependency", Severity::Warning),
        ("invalid_deprecated_extend_annotation", Severity::Warning),
        ("invalid_deprecated_implement_annotation", Severity::Warning),
        (
            "invalid_deprecated_instantiate_annotation",
            Severity::Warning,
        ),
        ("invalid_deprecated_mixin_annotation", Severity::Warning),
        ("invalid_deprecated_optional_annotation", Severity::Warning),
        ("invalid_deprecated_subclass_annotation", Severity::Warning),
        ("invalid_exception_value", Severity::Error),
        ("invalid_export_of_internal_element", Severity::Warning),
        (
            "invalid_export_of_internal_element_indirectly",
            Severity::Warning,
        ),
        ("invalid_extension_argument_count", Severity::Error),
        ("invalid_factory_method_decl", Severity::Warning),
        ("invalid_factory_method_impl", Severity::Warning),
        ("invalid_factory_name_not_a_class", Severity::Error),
        ("invalid_field_name", Severity::Error),
        ("invalid_field_type_in_struct", Severity::Error),
        ("invalid_generic_function_type", Severity::Error),
        ("invalid_hex_escape", Severity::Error),
        ("invalid_implementation_override", Severity::Error),
        ("invalid_initializer", Severity::Error),
        ("invalid_inline_function_type", Severity::Error),
        ("invalid_inside_unary_pattern", Severity::Error),
        ("invalid_internal_annotation", Severity::Warning),
        ("invalid_language_version_override", Severity::Warning),
        ("invalid_literal_annotation", Severity::Warning),
        ("invalid_literal_in_configuration", Severity::Error),
        ("invalid_modifier_on_constructor", Severity::Error),
        ("invalid_modifier_on_setter", Severity::Error),
        ("invalid_non_virtual_annotation", Severity::Warning),
        ("invalid_null_aware_operator", Severity::Warning),
        ("invalid_operator", Severity::Error),
        (
            "invalid_operator_questionmark_period_for_super",
            Severity::Error,
        ),
        ("invalid_option", Severity::Warning),
        ("invalid_override", Severity::Error),
        ("invalid_override_of_non_virtual_member", Severity::Warning),
        (
            "invalid_pattern_variable_in_shared_case_scope",
            Severity::Error,
        ),
        ("invalid_platforms_field", Severity::Warning),
        (
            "invalid_reference_to_generative_enum_constructor",
            Severity::Error,
        ),
        ("invalid_reference_to_this", Severity::Error),
        ("invalid_reopen_annotation", Severity::Warning),
        ("invalid_return_type_for_catch_error", Severity::Warning),
        ("invalid_return_type_for_then", Severity::Warning),
        ("invalid_section_format", Severity::Warning),
        ("invalid_star_after_async", Severity::Error),
        ("invalid_super_formal_parameter_location", Severity::Error),
        ("invalid_super_in_initializer", Severity::Error),
        ("invalid_sync", Severity::Error),
        ("invalid_this_in_initializer", Severity::Error),
        ("invalid_type_argument_in_const_literal", Severity::Error),
        ("invalid_unicode_escape_started", Severity::Error),
        ("invalid_unicode_escape_u_bracket", Severity::Error),
        ("invalid_unicode_escape_u_no_bracket", Severity::Error),
        ("invalid_unicode_escape_u_started", Severity::Error),
        ("invalid_uri", Severity::Error),
        ("invalid_use_of_covariant", Severity::Error),
        ("invalid_use_of_covariant_in_extension", Severity::Error),
        ("invalid_use_of_do_not_submit_member", Severity::Warning),
        ("invalid_use_of_identifier_augmented", Severity::Error),
        ("invalid_use_of_internal_member", Severity::Warning),
        ("invalid_use_of_null_value", Severity::Error),
        ("invalid_use_of_protected_member", Severity::Warning),
        ("invalid_use_of_type_outside_library", Severity::Error),
        (
            "invalid_use_of_visible_for_overriding_member",
            Severity::Warning,
        ),
        (
            "invalid_use_of_visible_for_template_member",
            Severity::Warning,
        ),
        (
            "invalid_use_of_visible_for_testing_member",
            Severity::Warning,
        ),
        ("invalid_visibility_annotation", Severity::Warning),
        (
            "invalid_visible_outside_template_annotation",
            Severity::Warning,
        ),
        ("invalid_widget_preview_application", Severity::Warning),
        ("invalid_widget_preview_private_argument", Severity::Warning),
        ("invocation_of_extension_without_call", Severity::Error),
        ("invocation_of_non_function", Severity::Error),
        ("invocation_of_non_function_expression", Severity::Error),
        (
            "js_interop_extension_constructor_js_annotation_has_no_effect",
            Severity::Error,
        ),
        ("label_in_outer_scope", Severity::Error),
        ("label_undefined", Severity::Error),
        ("late_final_field_with_const_constructor", Severity::Error),
        ("late_final_local_already_assigned", Severity::Error),
        ("late_pattern_variable_declaration", Severity::Error),
        ("leaf_call_must_not_return_handle", Severity::Error),
        ("leaf_call_must_not_take_handle", Severity::Error),
        ("library_directive_not_first", Severity::Error),
        ("list_element_type_not_assignable", Severity::Error),
        ("literal_with_class", Severity::Error),
        ("literal_with_class_and_new", Severity::Error),
        ("literal_with_new", Severity::Error),
        ("local_function_declaration_modifier", Severity::Error),
        ("main_first_positional_parameter_type", Severity::Error),
        ("main_has_required_named_parameters", Severity::Error),
        (
            "main_has_too_many_required_positional_parameters",
            Severity::Error,
        ),
        ("main_is_not_function", Severity::Error),
        ("map_entry_not_in_map", Severity::Error),
        ("map_key_type_not_assignable", Severity::Error),
        ("map_value_type_not_assignable", Severity::Error),
        ("member_with_class_name", Severity::Error),
        ("mismatched_annotation_on_struct_field", Severity::Error),
        ("missing_annotation_on_struct_field", Severity::Error),
        ("missing_assignable_selector", Severity::Error),
        ("missing_assignment_in_initializer", Severity::Error),
        ("missing_catch_or_finally", Severity::Error),
        ("missing_closing_parenthesis", Severity::Error),
        ("missing_const_final_var_or_type", Severity::Error),
        ("missing_const_in_list_literal", Severity::Error),
        ("missing_const_in_map_literal", Severity::Error),
        ("missing_const_in_set_literal", Severity::Error),
        ("missing_default_value_for_parameter", Severity::Error),
        ("missing_dependency", Severity::Warning),
        ("missing_digit", Severity::Error),
        ("missing_enum_body", Severity::Error),
        ("missing_enum_constant_in_switch", Severity::Warning),
        ("missing_exception_value", Severity::Error),
        ("missing_expression_in_initializer", Severity::Error),
        ("missing_expression_in_throw", Severity::Error),
        ("missing_field_type_in_struct", Severity::Error),
        ("missing_function_body", Severity::Error),
        ("missing_function_keyword", Severity::Error),
        ("missing_function_parameters", Severity::Error),
        ("missing_get", Severity::Error),
        ("missing_hex_digit", Severity::Error),
        ("missing_identifier", Severity::Error),
        ("missing_initializer", Severity::Error),
        ("missing_keyword_operator", Severity::Error),
        ("missing_method_parameters", Severity::Error),
        ("missing_name", Severity::Warning),
        ("missing_name_for_named_parameter", Severity::Error),
        ("missing_name_in_library_directive", Severity::Error),
        ("missing_name_in_part_of_directive", Severity::Error),
        ("missing_named_pattern_field_name", Severity::Error),
        ("missing_override_of_must_be_overridden", Severity::Warning),
        ("missing_prefix_in_deferred_import", Severity::Error),
        ("missing_primary_constructor", Severity::Error),
        ("missing_primary_constructor_parameters", Severity::Error),
        ("missing_quote", Severity::Error),
        ("missing_required_argument", Severity::Error),
        ("missing_required_param", Severity::Warning),
        ("missing_size_annotation_carray", Severity::Error),
        ("missing_star_after_sync", Severity::Error),
        ("missing_statement", Severity::Error),
        ("missing_typedef_parameters", Severity::Error),
        ("missing_variable_in_for_each", Severity::Error),
        ("missing_variable_pattern", Severity::Error),
        ("mixed_parameter_groups", Severity::Error),
        ("mixin_application_class_augmentation", Severity::Error),
        (
            "mixin_application_concrete_super_invoked_member_type",
            Severity::Error,
        ),
        (
            "mixin_application_no_concrete_super_invoked_member",
            Severity::Error,
        ),
        (
            "mixin_application_not_implemented_interface",
            Severity::Error,
        ),
        ("mixin_augmentation_has_on_clause", Severity::Error),
        (
            "mixin_class_declaration_extends_not_object",
            Severity::Error,
        ),
        ("mixin_class_declaration_with_clause", Severity::Error),
        (
            "mixin_class_declares_non_trivial_generative_constructor",
            Severity::Error,
        ),
        ("mixin_declares_constructor", Severity::Error),
        ("mixin_inherits_from_not_object", Severity::Error),
        ("mixin_instantiate", Severity::Error),
        (
            "mixin_modifier_mixin_application_class_with_multiple_mixins",
            Severity::Error,
        ),
        ("mixin_of_non_class", Severity::Error),
        ("mixin_on_sealed_class", Severity::Warning),
        ("mixin_primary_constructor", Severity::Error),
        (
            "mixin_super_class_constraint_deferred_class",
            Severity::Error,
        ),
        (
            "mixin_super_class_constraint_non_interface",
            Severity::Error,
        ),
        ("mixin_with_clause", Severity::Error),
        ("mixin_with_non_class_superclass", Severity::Error),
        ("modifier_out_of_order", Severity::Error),
        ("multiple_clauses", Severity::Error),
        ("multiple_combinators", Severity::Error),
        ("multiple_combinators", Severity::Warning),
        ("multiple_extends_clauses", Severity::Error),
        ("multiple_implements_clauses", Severity::Error),
        ("multiple_library_directives", Severity::Error),
        ("multiple_named_parameter_groups", Severity::Error),
        ("multiple_on_clauses", Severity::Error),
        ("multiple_part_of_directives", Severity::Error),
        ("multiple_plugins", Severity::Warning),
        ("multiple_positional_parameter_groups", Severity::Error),
        (
            "multiple_primary_constructor_body_declarations",
            Severity::Error,
        ),
        (
            "multiple_redirecting_constructor_invocations",
            Severity::Error,
        ),
        ("multiple_representation_fields", Severity::Error),
        ("multiple_super_initializers", Severity::Error),
        ("multiple_variance_modifiers", Severity::Error),
        ("multiple_with_clauses", Severity::Error),
        ("must_be_a_native_function_type", Severity::Error),
        ("must_be_a_subtype", Severity::Error),
        ("must_be_immutable", Severity::Warning),
        ("must_call_super", Severity::Warning),
        ("must_return_void", Severity::Error),
        ("name_not_string", Severity::Warning),
        ("named_function_expression", Severity::Error),
        ("named_function_type", Severity::Error),
        ("named_parameter_outside_group", Severity::Error),
        ("native_clause_in_non_sdk_code", Severity::Error),
        ("native_clause_should_be_annotation", Severity::Error),
        ("native_field_invalid_type", Severity::Error),
        ("native_field_missing_type", Severity::Error),
        ("native_field_not_static", Severity::Error),
        ("native_function_body_in_non_sdk_code", Severity::Error),
        ("native_function_missing_type", Severity::Error),
        ("negative_variable_dimension", Severity::Error),
        ("new_constructor_dot_name", Severity::Error),
        ("new_constructor_new_name", Severity::Error),
        ("new_constructor_qualified_name", Severity::Error),
        ("new_with_undefined_constructor", Severity::Error),
        ("new_with_undefined_constructor_default", Severity::Error),
        ("no_annotation_constructor_arguments", Severity::Error),
        ("no_combined_super_signature", Severity::Error),
        ("no_default_super_constructor", Severity::Error),
        ("no_generative_constructors_in_superclass", Severity::Error),
        ("no_touchscreen_feature", Severity::Warning),
        (
            "non_abstract_class_inherits_abstract_member",
            Severity::Error,
        ),
        ("non_bool_condition", Severity::Error),
        ("non_bool_expression", Severity::Error),
        ("non_bool_negation_expression", Severity::Error),
        ("non_bool_operand", Severity::Error),
        ("non_const_argument_for_const_parameter", Severity::Warning),
        ("non_const_call_to_literal_constructor", Severity::Warning),
        ("non_const_generative_enum_constructor", Severity::Error),
        ("non_const_map_as_expression_statement", Severity::Error),
        ("non_constant_case_expression", Severity::Error),
        (
            "non_constant_case_expression_from_deferred_library",
            Severity::Error,
        ),
        ("non_constant_default_value", Severity::Error),
        (
            "non_constant_default_value_from_deferred_library",
            Severity::Error,
        ),
        ("non_constant_list_element", Severity::Error),
        ("non_constant_map_element", Severity::Error),
        ("non_constant_map_key", Severity::Error),
        ("non_constant_map_pattern_key", Severity::Error),
        ("non_constant_map_value", Severity::Error),
        ("non_constant_record_field", Severity::Error),
        (
            "non_constant_record_field_from_deferred_library",
            Severity::Error,
        ),
        (
            "non_constant_relational_pattern_expression",
            Severity::Error,
        ),
        ("non_constant_set_element", Severity::Error),
        ("non_constant_type_argument", Severity::Error),
        ("non_constructor_factory", Severity::Error),
        (
            "non_covariant_type_parameter_position_in_representation_type",
            Severity::Error,
        ),
        ("non_exhaustive_switch_expression", Severity::Error),
        ("non_exhaustive_switch_statement", Severity::Error),
        ("non_final_field_in_enum", Severity::Error),
        ("non_generative_constructor", Severity::Error),
        ("non_generative_implicit_constructor", Severity::Error),
        ("non_identifier_library_name", Severity::Error),
        (
            "non_native_function_type_argument_to_pointer",
            Severity::Error,
        ),
        ("non_nullable_equals_parameter", Severity::Warning),
        ("non_part_of_directive_in_part", Severity::Error),
        ("non_positive_array_dimension", Severity::Error),
        (
            "non_redirecting_generative_constructor_with_primary",
            Severity::Error,
        ),
        ("non_resizable_activity", Severity::Warning),
        ("non_sized_type_argument", Severity::Error),
        ("non_string_literal_as_uri", Severity::Error),
        ("non_sync_factory", Severity::Error),
        ("non_type_as_type_argument", Severity::Error),
        ("non_type_in_catch_clause", Severity::Error),
        ("non_void_return_for_operator", Severity::Error),
        ("non_void_return_for_setter", Severity::Error),
        ("normal_before_optional_parameters", Severity::Error),
        ("not_a_type", Severity::Error),
        (
            "not_assigned_potentially_non_nullable_local_variable",
            Severity::Error,
        ),
        ("not_binary_operator", Severity::Error),
        ("not_enough_positional_arguments", Severity::Error),
        (
            "not_initialized_non_nullable_instance_field",
            Severity::Error,
        ),
        ("not_initialized_non_nullable_variable", Severity::Error),
        ("not_instantiated_bound", Severity::Error),
        ("not_iterable_spread", Severity::Error),
        ("not_map_spread", Severity::Error),
        ("not_null_aware_null_spread", Severity::Error),
        ("null_argument_to_non_null_type", Severity::Warning),
        ("null_aware_cascade_out_of_order", Severity::Error),
        ("null_check_always_fails", Severity::Warning),
        ("nullable_type_in_catch_clause", Severity::Warning),
        ("nullable_type_in_extends_clause", Severity::Error),
        ("nullable_type_in_implements_clause", Severity::Error),
        ("nullable_type_in_on_clause", Severity::Error),
        ("nullable_type_in_with_clause", Severity::Error),
        ("object_cannot_extend_another_class", Severity::Error),
        ("obsolete_colon_for_default_value", Severity::Error),
        ("on_repeated", Severity::Error),
        ("optional_parameter_in_operator", Severity::Error),
        ("out_of_order_clauses", Severity::Error),
        ("override_on_non_overriding_member", Severity::Warning),
        ("packed_annotation", Severity::Error),
        ("packed_annotation_alignment", Severity::Error),
        ("parse_error", Severity::Error),
        ("part_of_different_library", Severity::Error),
        ("part_of_name", Severity::Error),
        ("part_of_non_part", Severity::Error),
        ("part_of_unnamed_library", Severity::Error),
        ("path_does_not_exist", Severity::Warning),
        ("path_not_posix", Severity::Warning),
        ("path_pubspec_does_not_exist", Severity::Warning),
        ("pattern_assignment_declares_variable", Severity::Error),
        ("pattern_assignment_not_local_variable", Severity::Error),
        ("pattern_constant_from_deferred_library", Severity::Error),
        ("pattern_never_matches_value_type", Severity::Warning),
        (
            "pattern_type_mismatch_in_irrefutable_context",
            Severity::Error,
        ),
        ("pattern_variable_assignment_inside_guard", Severity::Error),
        (
            "pattern_variable_declaration_outside_function_or_method",
            Severity::Error,
        ),
        ("permission_implies_unsupported_hardware", Severity::Warning),
        ("platform_value_disallowed", Severity::Warning),
        ("positional_after_named_argument", Severity::Error),
        ("positional_field_in_object_pattern", Severity::Error),
        ("positional_parameter_outside_group", Severity::Error),
        (
            "positional_super_formal_parameter_with_positional_argument",
            Severity::Error,
        ),
        ("prefix_after_combinator", Severity::Error),
        ("prefix_collides_with_top_level_member", Severity::Error),
        ("prefix_identifier_not_followed_by_dot", Severity::Error),
        ("prefix_shadowed_by_local_declaration", Severity::Error),
        (
            "primary_constructor_body_with_expression_body",
            Severity::Error,
        ),
        ("primary_constructor_body_with_modifier", Severity::Error),
        (
            "primary_constructor_body_without_declaration",
            Severity::Error,
        ),
        ("primary_constructor_cannot_redirect", Severity::Error),
        ("private_collision_in_mixin_application", Severity::Error),
        ("private_named_non_field_parameter", Severity::Error),
        (
            "private_named_parameter_duplicate_public_name",
            Severity::Error,
        ),
        (
            "private_named_parameter_without_public_name",
            Severity::Error,
        ),
        ("private_optional_parameter", Severity::Error),
        ("private_setter", Severity::Error),
        ("read_potentially_unassigned_final", Severity::Error),
        ("receiver_of_type_never", Severity::Warning),
        (
            "record_literal_one_positional_no_trailing_comma",
            Severity::Error,
        ),
        (
            "record_type_one_positional_no_trailing_comma",
            Severity::Error,
        ),
        ("recursive_compile_time_constant", Severity::Error),
        ("recursive_constant_constructor", Severity::Error),
        ("recursive_constructor_redirect", Severity::Error),
        ("recursive_include_file", Severity::Warning),
        ("recursive_interface_inheritance", Severity::Error),
        ("redeclare_on_non_redeclaring_member", Severity::Warning),
        (
            "redirect_generative_to_missing_constructor",
            Severity::Error,
        ),
        (
            "redirect_generative_to_non_generative_constructor",
            Severity::Error,
        ),
        ("redirect_to_abstract_class_constructor", Severity::Error),
        ("redirect_to_invalid_function_type", Severity::Error),
        ("redirect_to_invalid_return_type", Severity::Error),
        ("redirect_to_missing_constructor", Severity::Error),
        ("redirect_to_non_class", Severity::Error),
        ("redirect_to_non_const_constructor", Severity::Error),
        (
            "redirect_to_type_alias_expands_to_type_parameter",
            Severity::Error,
        ),
        ("redirecting_constructor_with_body", Severity::Error),
        ("redirection_in_non_factory_constructor", Severity::Error),
        ("referenced_before_declaration", Severity::Error),
        ("refutable_pattern_in_irrefutable_context", Severity::Error),
        (
            "relational_pattern_operand_type_not_assignable",
            Severity::Error,
        ),
        (
            "relational_pattern_operator_return_type_not_assignable_to_bool",
            Severity::Error,
        ),
        ("removed_lint", Severity::Warning),
        ("removed_lint_use", Severity::Warning),
        ("replaced_lint", Severity::Warning),
        ("replaced_lint_use", Severity::Warning),
        ("representation_field_modifier", Severity::Error),
        ("representation_field_trailing_comma", Severity::Error),
        ("rest_element_in_map_pattern", Severity::Error),
        ("rethrow_outside_catch", Severity::Error),
        ("return_in_generative_constructor", Severity::Error),
        ("return_in_generator", Severity::Error),
        ("return_of_do_not_store", Severity::Warning),
        ("return_of_invalid_type", Severity::Error),
        ("return_of_invalid_type_from_closure", Severity::Error),
        ("return_without_value", Severity::Error),
        ("sdk_version_constructor_tearoffs", Severity::Warning),
        ("sdk_version_gt_gt_gt_operator", Severity::Warning),
        ("sdk_version_since", Severity::Warning),
        ("sealed_enum", Severity::Error),
        ("sealed_mixin", Severity::Error),
        ("sealed_mixin_class", Severity::Error),
        ("set_element_type_not_assignable", Severity::Error),
        ("setter_constructor", Severity::Error),
        ("setter_in_function", Severity::Error),
        ("setting_orientation_on_activity", Severity::Warning),
        ("shared_deferred_prefix", Severity::Error),
        ("size_annotation_dimensions", Severity::Error),
        ("spread_expression_from_deferred_library", Severity::Error),
        ("stack_overflow", Severity::Error),
        ("static_access_to_instance_member", Severity::Error),
        ("static_constructor", Severity::Error),
        ("static_getter_without_body", Severity::Error),
        ("static_operator", Severity::Error),
        ("static_setter_without_body", Severity::Error),
        ("strict_raw_type", Severity::Warning),
        (
            "subtype_of_base_or_final_is_not_base_final_or_sealed",
            Severity::Error,
        ),
        ("subtype_of_deferred_class", Severity::Error),
        ("subtype_of_disallowed_type", Severity::Error),
        ("subtype_of_sealed_class", Severity::Warning),
        ("subtype_of_struct_class", Severity::Error),
        (
            "super_formal_parameter_type_is_not_subtype_of_associated",
            Severity::Error,
        ),
        (
            "super_formal_parameter_without_associated_named",
            Severity::Error,
        ),
        (
            "super_formal_parameter_without_associated_positional",
            Severity::Error,
        ),
        ("super_in_enum_constructor", Severity::Error),
        ("super_in_extension", Severity::Error),
        ("super_in_extension_type", Severity::Error),
        ("super_in_invalid_context", Severity::Error),
        ("super_in_redirecting_constructor", Severity::Error),
        ("super_initializer_in_object", Severity::Error),
        ("super_initializing_declaring_parameter", Severity::Error),
        ("super_invocation_not_last", Severity::Error),
        ("supertype_expands_to_type_parameter", Severity::Error),
        ("switch_case_completes_normally", Severity::Error),
        ("switch_has_case_after_default_case", Severity::Error),
        ("switch_has_multiple_default_cases", Severity::Error),
        (
            "tearoff_of_generative_constructor_of_abstract_class",
            Severity::Error,
        ),
        ("tearoff_with_must_be_const_parameter", Severity::Warning),
        ("text_direction_code_point_in_comment", Severity::Warning),
        ("text_direction_code_point_in_literal", Severity::Warning),
        ("throw_of_invalid_type", Severity::Error),
        ("todo", Severity::Info),
        ("top_level_cycle", Severity::Error),
        ("top_level_operator", Severity::Error),
        ("type_alias_cannot_reference_itself", Severity::Error),
        ("type_annotation_deferred_class", Severity::Error),
        ("type_argument_not_matching_bounds", Severity::Error),
        ("type_arguments_on_type_variable", Severity::Error),
        ("type_before_factory", Severity::Error),
        ("type_check_with_null", Severity::Warning),
        ("type_parameter_on_constructor", Severity::Error),
        ("type_parameter_on_operator", Severity::Error),
        ("type_parameter_referenced_by_static", Severity::Error),
        ("type_parameter_supertype_of_its_bound", Severity::Error),
        ("type_test_with_non_type", Severity::Error),
        ("type_test_with_undefined_name", Severity::Error),
        ("typedef_augmentation", Severity::Error),
        ("typedef_in_class", Severity::Error),
        ("unawaited_return_in_try_block", Severity::Warning),
        ("unchecked_use_of_nullable_value", Severity::Error),
        ("undefined_class", Severity::Error),
        ("undefined_constructor_in_initializer", Severity::Error),
        ("undefined_enum_constant", Severity::Error),
        ("undefined_enum_constructor", Severity::Error),
        ("undefined_extension_getter", Severity::Error),
        ("undefined_extension_method", Severity::Error),
        ("undefined_extension_operator", Severity::Error),
        ("undefined_extension_setter", Severity::Error),
        ("undefined_function", Severity::Error),
        ("undefined_getter", Severity::Error),
        ("undefined_hidden_name", Severity::Warning),
        ("undefined_identifier", Severity::Error),
        ("undefined_identifier_await", Severity::Error),
        ("undefined_lint", Severity::Warning),
        ("undefined_method", Severity::Error),
        ("undefined_named_parameter", Severity::Error),
        ("undefined_operator", Severity::Error),
        ("undefined_prefixed_name", Severity::Error),
        ("undefined_referenced_parameter", Severity::Warning),
        ("undefined_setter", Severity::Error),
        ("undefined_shown_name", Severity::Warning),
        ("undefined_super_member", Severity::Error),
        ("undone", Severity::Info),
        ("unexpected_dollar_in_string", Severity::Error),
        ("unexpected_separator_in_number", Severity::Error),
        ("unexpected_token", Severity::Error),
        ("unexpected_tokens", Severity::Error),
        ("unignorable_ignore", Severity::Warning),
        ("unknown_platform", Severity::Warning),
        ("unnecessary_cast", Severity::Warning),
        ("unnecessary_cast_pattern", Severity::Warning),
        ("unnecessary_dev_dependency", Severity::Warning),
        ("unnecessary_final", Severity::Warning),
        ("unnecessary_import", Severity::Info),
        ("unnecessary_nan_comparison", Severity::Warning),
        ("unnecessary_no_such_method", Severity::Warning),
        ("unnecessary_non_null_assertion", Severity::Warning),
        ("unnecessary_null_assert_pattern", Severity::Warning),
        ("unnecessary_null_check_pattern", Severity::Warning),
        ("unnecessary_null_comparison", Severity::Warning),
        ("unnecessary_question_mark", Severity::Warning),
        ("unnecessary_set_literal", Severity::Warning),
        ("unnecessary_type_check", Severity::Warning),
        ("unnecessary_wildcard_pattern", Severity::Warning),
        (
            "unqualified_reference_to_non_local_static_member",
            Severity::Error,
        ),
        (
            "unqualified_reference_to_static_member_of_extended_type",
            Severity::Error,
        ),
        ("unreachable_switch_case", Severity::Warning),
        ("unreachable_switch_default", Severity::Warning),
        ("unrecognized_error_code", Severity::Warning),
        ("unsupported_chrome_os_feature", Severity::Warning),
        ("unsupported_chrome_os_hardware", Severity::Warning),
        ("unsupported_operator", Severity::Error),
        ("unsupported_option", Severity::Warning),
        ("unsupported_value", Severity::Warning),
        ("unterminated_multi_line_comment", Severity::Error),
        ("unterminated_string_literal", Severity::Error),
        ("unused_catch_clause", Severity::Warning),
        ("unused_catch_stack", Severity::Warning),
        ("unused_element", Severity::Warning),
        ("unused_element_parameter", Severity::Warning),
        ("unused_field", Severity::Warning),
        ("unused_field_from_primary_constructor", Severity::Warning),
        ("unused_import", Severity::Warning),
        ("unused_label", Severity::Warning),
        ("unused_local_variable", Severity::Warning),
        ("unused_result", Severity::Warning),
        ("unused_shown_name", Severity::Warning),
        ("uri_does_not_exist", Severity::Error),
        ("uri_does_not_exist_in_doc_import", Severity::Warning),
        ("uri_has_not_been_generated", Severity::Error),
        ("uri_with_interpolation", Severity::Error),
        ("use_of_native_extension", Severity::Error),
        ("use_of_void_result", Severity::Error),
        ("values_declaration_in_enum", Severity::Error),
        ("var_and_type", Severity::Error),
        ("var_as_type_name", Severity::Error),
        ("var_class", Severity::Error),
        ("var_enum", Severity::Error),
        ("var_return_type", Severity::Error),
        ("var_typedef", Severity::Error),
        ("variable_length_array_not_last", Severity::Error),
        (
            "variable_pattern_keyword_in_declaration_context",
            Severity::Error,
        ),
        ("variable_type_mismatch", Severity::Error),
        ("void_with_type_arguments", Severity::Error),
        ("with_before_extends", Severity::Error),
        ("workspace_field_not_list", Severity::Warning),
        ("workspace_value_not_string", Severity::Warning),
        ("workspace_value_not_subdirectory", Severity::Warning),
        (
            "wrong_explicit_type_parameter_variance_in_superinterface",
            Severity::Error,
        ),
        ("wrong_number_of_parameters_for_operator", Severity::Error),
        ("wrong_number_of_parameters_for_setter", Severity::Error),
        ("wrong_number_of_type_arguments", Severity::Error),
        (
            "wrong_number_of_type_arguments_constructor",
            Severity::Error,
        ),
        ("wrong_number_of_type_arguments_element", Severity::Error),
        ("wrong_number_of_type_arguments_enum", Severity::Error),
        ("wrong_number_of_type_arguments_extension", Severity::Error),
        ("wrong_number_of_type_arguments_function", Severity::Error),
        ("wrong_separator_for_positional_parameter", Severity::Error),
        (
            "wrong_type_parameter_variance_in_superinterface",
            Severity::Error,
        ),
        ("wrong_type_parameter_variance_position", Severity::Error),
        ("yield_in_non_generator", Severity::Error),
        ("yield_of_invalid_type", Severity::Error),
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
/// a diagnostic code the pinned registry knows (the `name`-field universe of
/// `known_error_codes` plus the `REMOVED_ERROR_CODES` compat set;
/// case-insensitive, the official test rides the uppercased key). Lint-rule
/// names are a separate registry (dartq-lints) — the CLI face's seam ORs
/// them in.
#[must_use]
pub fn is_known_diagnostic_code(name: &str) -> bool {
    let lowered = name.to_lowercase();
    REMOVED_ERROR_CODES.contains(&lowered.as_str()) || known_error_codes::contains(&lowered)
}
