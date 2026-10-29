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
//!   (`anchor_in_section`), keys at their first occurrence (`anchor_span`),
//!   and section-level rows (`invalid_section_format`) at the section's own
//!   *value node* (`section_value_anchor` — the upstream `analyzer.span`
//!   convention, the pinned 2026-10-29 probe's `2:3|11`); the
//!   `(code, file)` differential face is exact regardless.
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
//!   declared section is the row, silent only when absent or null. The
//!   3.13.7 verifier adds `plugins_in_inner_options`
//!   (parse_model:1538-1545): an *initial* options file outside its context
//!   root declaring a top-level `plugins:` map — the
//!   [`OptionsIssue::plugins_in_inner_options`] row, anchored at the
//!   section's value node; the emitting caller is the CLI face's
//!   loaded-options walk, which owns the initial/context-root knowledge.
//! - [`text_direction_issues`] is the unicode-text verifier
//!   (unicode_text_verifier.dart) over one Dart source: every bidi code
//!   point reports `text_direction_code_point_in_literal` inside a string
//!   literal region, `text_direction_code_point_in_comment` anywhere else.
//!   It rides the per-file Dart diagnostic pass (its mounting face).
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

/// `The Unicode code point 'U+{0}' changes the appearance of text from how
/// it's interpreted by the compiler.` (registry :15936-15951 — the shared
/// template of the `in_comment`/`in_literal` pair; `{0}` is the code
/// point's uppercase hex with no zero padding).
const TEXT_DIRECTION_CODE_POINT_IN_COMMENT: CodeInfo = CodeInfo::new(
    "text_direction_code_point_in_comment",
    "TEXT_DIRECTION_CODE_POINT_IN_COMMENT",
    DiagnosticType::StaticWarning,
    true,
);

/// Same template as [`TEXT_DIRECTION_CODE_POINT_IN_COMMENT`] — the pair
/// splits only on where the code point sits (string literal vs anywhere
/// else; `unicode_text_verifier.dart`:23-34).
const TEXT_DIRECTION_CODE_POINT_IN_LITERAL: CodeInfo = CodeInfo::new(
    "text_direction_code_point_in_literal",
    "TEXT_DIRECTION_CODE_POINT_IN_LITERAL",
    DiagnosticType::StaticWarning,
    true,
);

/// `Plugins can only be specified in the root of a pub workspace or the
/// root of a package that isn't in a workspace.` (registry :13996-14007;
/// emitted by the caller that owns the context-root knowledge — the CLI
/// face's loaded-options walk; the row anchors at the top-level `plugins`
/// section's value node, `parse_model`:1538-1545).
const PLUGINS_IN_INNER_OPTIONS: CodeInfo = CodeInfo::new(
    "plugins_in_inner_options",
    "PLUGINS_IN_INNER_OPTIONS",
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

/// `Multiple plugins can't be enabled.` — one row per
/// `analyzer.plugins` entry that names a plugin different from the
/// section's first (upstream `_ParsedLegacyPluginsData.reportLocalMultiplePlugins`,
/// analysis_options_parse_model.dart:1057-1063; registry
/// `diagnostic.g.dart`:11886-11898, `STATIC_WARNING`, no published docs;
/// the correction names the first plugin, `Remove all plugins following
/// the first, '{0}'.`).
const MULTIPLE_PLUGINS: CodeInfo = CodeInfo::new(
    "multiple_plugins",
    "MULTIPLE_PLUGINS",
    DiagnosticType::StaticWarning,
    false,
);

/// `The plugin '{0}' is specified with a different source in '{1}'.`
/// (registry :7870-7881, `STATIC_WARNING`; the correction is `Ensure that
/// all specifications of the same plugin use an identical source.`). The
/// cross-file conflict row of `_computePluginConflicts`
/// (context_manager.dart:606-685) — emitted by the CLI face, which owns the
/// whole loaded-options set the pairwise comparison scans.
pub const INCOMPATIBLE_PLUGIN_SOURCE: CodeInfo = CodeInfo::new(
    "incompatible_plugin_source",
    "INCOMPATIBLE_PLUGIN_SOURCE",
    DiagnosticType::StaticWarning,
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

    /// The `incompatible_plugin_source` row (`_computePluginConflicts`,
    /// context_manager.dart:664-684): `name` is the conflicting plugin's
    /// name, `other_file` the *other* options file's path (`{1}`), `span`
    /// the reporting file's plugin-key anchor ([`AnalysisOptions::plugin_name_anchor`]).
    #[must_use]
    pub fn incompatible_plugin_source(name: &str, other_file: &Path, span: (u32, u32)) -> Self {
        OptionsIssue {
            code: INCOMPATIBLE_PLUGIN_SOURCE,
            message: format!(
                "The plugin '{name}' is specified with a different source in '{}'.",
                other_file.display()
            ),
            correction: Some(
                "Ensure that all specifications of the same plugin use an identical source."
                    .to_owned(),
            ),
            offset: span.0,
            length: span.1,
        }
    }

    /// The `plugins_in_inner_options` row (`_ParsedPluginsData.parse`,
    /// parse_model:1538-1545): an *initial* options file whose directory is
    /// not its context root declaring a top-level `plugins:` map reports at
    /// the section's value node (`plugins.span` — the whole map block;
    /// [`section_value_anchor`] derives it). `context_root` renders in the
    /// correction verbatim. The non-map section reports
    /// `invalid_section_format` instead and never reaches here.
    #[must_use]
    pub fn plugins_in_inner_options(context_root: &str, span: (u32, u32)) -> Self {
        OptionsIssue {
            code: PLUGINS_IN_INNER_OPTIONS,
            message: "Plugins can only be specified in the root of a pub workspace or the root \
                      of a package that isn't in a workspace."
                .to_owned(),
            correction: Some(format!(
                "Try specifying plugins in an analysis options file at '{context_root}'."
            )),
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
    /// The file's own `analyzer.cannot-ignore` names, non-severity spellings
    /// only (`unrecognized_error_code` candidates, anchored in the included
    /// file's text).
    pub cannot_ignore: Vec<DeclaredError>,
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
    /// `analyzer.optional-checks.chrome-os-manifest-checks`, merged
    /// set-if-present across the include chain (the official
    /// `AnalysisOptionsImpl.chromeOsManifestChecks` — a nearer file's map
    /// value replaces the farther's; the scalar form `optional-checks:
    /// chrome-os-manifest-checks` enables the check whole, parse_model
    /// :1399-1499). `None` (never declared) is the official `false`
    /// default — the manifest pass stays silent.
    pub chrome_os_manifest_checks: Option<bool>,
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
    /// `analyzer.cannot-ignore` names the initial file's own list declares,
    /// in declaration order, non-severity spellings only
    /// (`unrecognized_error_code` candidates; validity is the caller's
    /// registry seam).
    pub declared_cannot_ignore: Vec<DeclaredError>,
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
    /// The include-chain-merged top-level `plugins:` configurations — the
    /// upstream `pluginConfigurations` (the `_applyPlugins` merge,
    /// analysis_options_parser.dart:300-328): per name, a nearer file's
    /// configuration replaces the farther one's whole entry; an invalid or
    /// null-valued `plugins:` section in a nearer file clears everything the
    /// chain merged before it (`clearsExisting`). Insertion-ordered far to
    /// near — the upstream `configurations.values` list order.
    pub plugins: Vec<PluginConfig>,
    /// The merged `plugins/dependency_overrides` map (`_applyPlugins`
    /// `addAll` — a nearer entry replaces per package name); `None` when no
    /// file in the chain declares one or a nearer invalid section cleared it.
    pub plugin_dependency_overrides: Option<FxHashMap<String, PluginSource>>,
    /// The initial file's YAML parse failure, when any — the upstream
    /// `parse_error` shape kept as data. The oracle emits no machine row for
    /// a malformed options file (the pinned 2026-10-18 oracle probe: silent
    /// on every output face), so the failure rides the faces' stderr
    /// advisory channel — recorded here, never pushed into [`Self::issues`].
    /// An *included* file's failure still reports `included_file_parse_error`
    /// (a different, probed-as-emitted code) through `issues` unchanged.
    pub parse_failure: Option<OptionsIssue>,
    /// Whether the initial file declares a top-level `plugins:` key at all —
    /// the upstream `_computePluginConflicts` membership gate (only files
    /// whose own `plugins` node is a `YamlMap` contribute specs,
    /// context_manager.dart:625-626).
    pub has_plugins_map: bool,
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

/// The configured severity of one `plugins/<name>/diagnostics` entry — the
/// upstream `ConfiguredSeverity` (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/lint/config.dart`: disable/enable/info/warning/
/// error). A bool parses to enable/disable; a string must match the enum
/// name *verbatim* (case-sensitive `asNameMap`, `_parseRuleConfig` — a miss,
/// including `"false"` or `"Error"`, degrades to enable); any other scalar
/// or container shape contributes no entry at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfiguredSeverity {
    /// `false` or `disable` — disables the *rule* when the key names one;
    /// naming a code it degrades to an INFO emit upstream (the closed
    /// {INFO, WARNING, ERROR} protocol enum misses `DISABLE`, and the emit
    /// site's `?? INFO` completes the fall — plugin_server.dart:624-648).
    Disable,
    /// `true` or `enable` — the declared severity stands.
    Enable,
    /// Re-level to info.
    Info,
    /// Re-level to warning.
    Warning,
    /// Re-level to error.
    Error,
}

impl ConfiguredSeverity {
    /// Upstream `RuleConfig.isEnabled` — every severity but `Disable`.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Disable)
    }

    /// The re-leveling severity of a code-level entry, mirroring the
    /// upstream `_configuredSeverity` emit (plugin_server.dart:624-648):
    /// `Enable` leaves the declared severity standing (`None`), `Disable`
    /// coerces to INFO through the closed protocol enum's miss +
    /// `?? INFO` fallback, the named tiers re-level.
    #[must_use]
    pub const fn severity(self) -> Option<Severity> {
        match self {
            Self::Enable => None,
            Self::Disable | Self::Info => Some(Severity::Info),
            Self::Warning => Some(Severity::Warning),
            Self::Error => Some(Severity::Error),
        }
    }
}

/// A plugin's declared source — the upstream `PluginSource` sealed family
/// (`_parsePluginSource`, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/analysis_options/analysis_options_parse_model.dart`
/// :1639-1693): a scalar is a version constraint; a map prefers
/// `version`/`hosted`, then `git`, then `path` — first match wins.
/// `PartialEq` is the upstream `==` that the cross-file conflict check
/// compares (`_computePluginConflicts`, context_manager.dart:606-685).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// `name: <constraint>` or `name: {version: <constraint>, hosted: <url>}`.
    Versioned {
        /// The version constraint string.
        constraint: String,
        /// The `hosted:` URL, when present.
        hosted: Option<String>,
    },
    /// `name: {git: <url>}` or `name: {git: {url:…, ref:…, path:…, tag_pattern:…}}`.
    Git {
        /// The repository URL.
        url: String,
        /// `git.path` — the in-repo subdirectory.
        path: Option<String>,
        /// `git.ref` — the pinned ref.
        reference: Option<String>,
        /// `git.tag_pattern`.
        tag_pattern: Option<String>,
    },
    /// `name: {path: <dir>}` — the plugin package's directory; a relative
    /// value joins and normalizes against the declaring options file's
    /// directory (upstream `pathContext.join` + `normalize`).
    Path(PathBuf),
}

/// One entry of the merged top-level `plugins:` section — the upstream
/// `PluginConfiguration` slice this engine consumes (name, source, the
/// `diagnostics` map of configured severities keyed lowercased — the
/// upstream `LinkedHashMap` is case-insensitive). `isEnabled` has no parse
/// path upstream (`enabled` is unimplemented — parse_model:1612) and is not
/// modeled.
#[derive(Debug)]
pub struct PluginConfig {
    /// The plugin name as written (`nameNode.toString()` — any scalar key
    /// renders; the section's key case is preserved).
    pub name: String,
    /// The declared source.
    pub source: PluginSource,
    /// The `diagnostics:` map, lowercased code/rule names to their
    /// configured severities (`parseDiagnosticsSection`, config.dart:11-64).
    pub diagnostics: FxHashMap<String, ConfiguredSeverity>,
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

    /// The initial options file's text and line index — the text every
    /// recorded issue's span resolves against (the file is never resident
    /// as Dart). `None` for the synthesized default (no options file).
    #[must_use]
    pub fn layout(&self) -> Option<(&str, &LineIndex)> {
        self.layout
            .as_ref()
            .map(|(text, index)| (text.as_ref(), index))
    }

    /// Whether the YAML ignore-comment face suppresses one row anchored at
    /// `offset` (UTF-16, the initial file's text) with the code `name`: the
    /// file's `# ignore:` / `# ignore_for_file:` entries name it. The
    /// shared application point for every options-file row face — the
    /// `IgnoreInfo.forYaml` filter the oracle's YAML validators run
    /// (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/pubspec/pubspec_validator.dart` :80-83); the
    /// row's line resolves through the captured layout.
    #[must_use]
    pub fn ignored_at(&self, offset: u32, name: &str) -> bool {
        let Some((text, index)) = self.layout() else {
            return false;
        };
        let ignores = crate::YamlIgnores::parse(text);
        let (line, _) = index.line_col_utf16(text, offset);
        ignores.ignored(line, name)
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

    /// The span the cross-file plugin-conflict row anchors at for `name`
    /// (the upstream `_computePluginConflicts` span: `configurationNode` —
    /// the plugin's *key* node in this file's `plugins:` map — falling back
    /// to the section's value node when the merged configuration arrived
    /// through the include chain and no local key names it,
    /// context_manager.dart:632-636). `(0, 0)` when the file's text carries
    /// neither — a diagnostic row, never a panic.
    #[must_use]
    pub fn plugin_name_anchor(&self, name: &str) -> (u32, u32) {
        let Some((text, _)) = &self.layout else {
            return (0, 0);
        };
        let Some(section_at) = text.find("plugins") else {
            return (0, 0);
        };
        if text[section_at..].find(name).is_some() {
            return anchor_in_section(text, "plugins", name);
        }
        plugin_value_anchor(text, "plugins")
    }

    /// The top-level `plugins` section's whole value-node span — the
    /// `plugins_in_inner_options` anchor (`plugins.span`, parse_model:1543;
    /// [`OptionsIssue::plugins_in_inner_options`] renders the row, the CLI
    /// face's loaded-options walk decides the initial/context-root
    /// condition that fires it). `(0, 0)` when the section or its value
    /// cannot be located.
    pub fn plugins_section_anchor(&self) -> (u32, u32) {
        let Some((text, _)) = &self.layout else {
            return (0, 0);
        };
        section_value_anchor(text, "plugins").unwrap_or((0, 0))
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
        let all_roots: rustc_hash::FxHashSet<PathBuf> = roots.iter().cloned().collect();
        for root in roots {
            let base = per_root[root].clone();
            Self::locate_nested(
                root,
                base,
                &mut loaded,
                &mut per_root,
                packages,
                config_dir,
                &all_roots,
            );
        }
        per_root
    }

    /// Walks `root` whole (the same dot rule and exclude pruning discovery
    /// applies; a nested package root is its own walk's, never descended
    /// here) registering every nested options file at
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
        all_roots: &rustc_hash::FxHashSet<PathBuf>,
    ) {
        // The accumulated governing entries: the root's own first, each
        // nested file's as it registers. Exclusion tests every entry's
        // globs, each anchored to its declaring directory.
        let mut governing: Vec<Arc<AnalysisOptions>> = vec![base];
        let excluded = |path: &Path, governing: &[Arc<AnalysisOptions>]| {
            governing.iter().any(|options| options.is_excluded(path))
        };
        // The whole root, like discovery's walk (every non-dot child
        // directory; the root's own options file is `base`).
        let mut queue: Vec<PathBuf> = std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                    .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"."))
                    .map(|entry| entry.path())
                    .filter(|path| !all_roots.contains(path))
                    .collect()
            })
            .unwrap_or_default();
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
                    if !name.starts_with('.') && !all_roots.contains(&path) {
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
    /// The `chromeOsManifestChecks` accumulator — same set-if-present
    /// merge as the strictness scalars (a nearer file's declared value
    /// replaces the farther's).
    chrome_os_manifest_checks: Option<bool>,
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
    declared_cannot_ignore: Vec<DeclaredError>,
    included_groups: Vec<DeclarationGroup>,
    /// The initial file's resolved include edges — built into
    /// [`AnalysisOptions::included_edges`].
    included_edges: Vec<IncludeEdge>,
    /// Each parsed file's first include URI — the cycle row's inner-span
    /// source (the official `spanInChain` = the repeated file's own
    /// outgoing directive, walker :170-188).
    first_includes: FxHashMap<PathBuf, String>,
    /// The `_applyPlugins` accumulator: per-name merged configurations,
    /// insertion-ordered far to near (a nearer same-name entry replaces in
    /// place — the upstream `configurations` map's `values` order).
    plugins: Vec<PluginConfig>,
    /// The `plugins/dependency_overrides` accumulator (addAll-merge).
    plugin_overrides: Option<FxHashMap<String, PluginSource>>,
    /// The initial file's YAML parse failure — rides
    /// [`AnalysisOptions::parse_failure`], never `issues` (oracle-silent on
    /// the machine face; stderr advisory only).
    parse_failure: Option<OptionsIssue>,
    /// The initial file declares a top-level `plugins:` *map* — the
    /// `_computePluginConflicts` spec-membership gate.
    has_plugins_map: bool,
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
                // The YAML failure face: the initial file records
                // [`AnalysisOptions::parse_failure`] — the oracle emits no
                // machine row for a malformed options file (pinned 2026-10-18
                // probe: silent), so the failure rides the faces' stderr
                // advisory channel and never joins `issues`. An included
                // file's failure reports `included_file_parse_error` wrapped
                // at the branch anchor (walker :48-66) as before. `serde_yaml`
                // yields one failure point, so the wrapped span shows it at
                // both ends; `yaml_failure` already converted it to UTF-16.
                let (message, at) = yaml_failure(&text, &error);
                if file == self.initial {
                    self.parse_failure = Some(OptionsIssue {
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
                cannot_ignore: Vec::new(),
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
        if let Some(checks) = chrome_os_manifest_checks(map) {
            self.chrome_os_manifest_checks = Some(checks);
        }
        // `_applyPlugins` (analysis_options_parser.dart:300-328) — merged
        // per file like the other sections (nearer wins per name, an
        // invalid or null-valued section clears the chain's merge). The
        // section's row faces stay in `local_issues`/`plugin_issues`.
        self.apply_plugins(map, declaring_dir);
        if is_initial {
            self.has_plugins_map =
                matches!(map.get(Value::from("plugins")), Some(Value::Mapping(_)));
        }
        if is_initial {
            self.issues.extend(local_issues(map, text));
            self.declared_errors.extend(declared_error_codes(map, text));
            self.declared_cannot_ignore
                .extend(declared_cannot_ignore_names(map, text));
        } else {
            let group = self
                .included_groups
                .last_mut()
                .expect("the group registered above");
            group.errors = declared_error_codes(map, text);
            group.cannot_ignore = declared_cannot_ignore_names(map, text);
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
            chrome_os_manifest_checks: self.chrome_os_manifest_checks,
            severity_overrides: self.errors,
            unignorable: self.unignorable,
            excludes: self.excludes,
            file_path: self.initial,
            layout: Some((self.initial_text, line_index)),
            issues: self.issues,
            declared_rules: self.declared_rules,
            declared_errors: self.declared_errors,
            declared_cannot_ignore: self.declared_cannot_ignore,
            included_groups: self.included_groups,
            included_edges: self.included_edges,
            plugins: self.plugins,
            plugin_dependency_overrides: self.plugin_overrides,
            parse_failure: self.parse_failure,
            has_plugins_map: self.has_plugins_map,
        }
    }

    /// The `_applyPlugins` merge arm (analysis_options_parser.dart:300-328):
    /// a `plugins:` section whose value is not a map — *including the bare
    /// `plugins:` null* — clears everything the include chain merged before
    /// this file (`clearsExisting`); a map merges per name (nearer replaces
    /// the farther entry whole, keeping its position) and
    /// `dependency_overrides` addAll-merges per package name.
    fn apply_plugins(&mut self, map: &serde_yaml::Mapping, declaring_dir: &Path) {
        let parsed = parse_plugins_section(map, declaring_dir);
        if parsed.clears_existing {
            self.plugins.clear();
            self.plugin_overrides = None;
            return;
        }
        for config in parsed.configurations {
            match self.plugins.iter_mut().find(|c| c.name == config.name) {
                Some(existing) => *existing = config,
                None => self.plugins.push(config),
            }
        }
        if let Some(overrides) = parsed.dependency_overrides {
            self.plugin_overrides
                .get_or_insert_default()
                .extend(overrides);
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

/// The `analyzer.optional-checks.chrome-os-manifest-checks` value
/// (`_ParsedOptionalChecksData.parse`, parse_model:1399-1499): the map
/// form's `toBool` conversion on the key's value (a non-boolean value
/// contributes nothing — the shape row reports separately in
/// [`local_issues`]); the scalar form names the check directly and
/// enables it (`optional-checks: chrome-os-manifest-checks` —
/// `enabledChecks` adds the legal names the scalar text matches).
fn chrome_os_manifest_checks(map: &serde_yaml::Mapping) -> Option<bool> {
    let section = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("optional-checks")))?;
    match section {
        Value::Mapping(section) => section
            .get(Value::from("chrome-os-manifest-checks"))
            .and_then(to_bool),
        Value::Null | Value::Sequence(_) | Value::Tagged(_) => None,
        scalar => yaml_scalar_text(scalar)
            .filter(|name| name == "chrome-os-manifest-checks")
            .map(|_| true),
    }
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
        // `analyzer.plugins` conflicts — `_ParsedLegacyPluginsData`
        // (parse_model:1007-1086): the section's entry stream is the
        // scalar itself (one entry), the list's nodes, or the map's
        // keys; each string-valued entry is one plugin name. The first
        // name wins; every later *different* name reports
        // `multiple_plugins` at its own node with the first name as the
        // correction argument. A repeated same-name entry is not a
        // conflict (the /2 case: `- aaa`, `- aaa` stays silent).
        if let Some(plugins) = analyzer.get(Value::from("plugins")) {
            let entries: Vec<&Value> = match plugins {
                Value::Sequence(nodes) => nodes.iter().collect(),
                Value::Mapping(entries) => entries.keys().collect(),
                Value::Null | Value::Tagged(_) => Vec::new(),
                scalar => vec![scalar],
            };
            let mut first_name: Option<&str> = None;
            let cursor = std::cell::Cell::new(text.find("plugins").unwrap_or(0));
            for entry in entries {
                let Some(name) = entry.as_str() else {
                    continue;
                };
                match first_name {
                    None => first_name = Some(name),
                    Some(first) if name != first => {
                        let (offset, length) = anchor_occurrence(text, "analyzer", &cursor, name);
                        issues.push(OptionsIssue {
                            code: MULTIPLE_PLUGINS,
                            message: "Multiple plugins can't be enabled.".to_owned(),
                            correction: Some(format!(
                                "Remove all plugins following the first, '{first}'."
                            )),
                            offset,
                            length,
                        });
                    }
                    Some(_) => {}
                }
            }
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
        // switch without reporting); every other non-map shape reports at
        // the *value* node's span (`plugins.span`, parse_model:1543-1547 —
        // the scalar or the block list itself).
        if !plugins.is_null() {
            push_plugin_invalid_value(issues, text, "plugins", "plugins");
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
                // `pluginNode.span` — the value under `dependency_overrides:`.
                None => push_plugin_invalid_value(
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
        // `pluginValue.span` — the value node under `<name>:`.
        _ => {
            push_plugin_invalid_value(issues, text, &section, name);
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
        // `diagnosticsValue.span` — the value node under `diagnostics:`.
        push_plugin_invalid_value(issues, text, &section, "diagnostics");
        return;
    };
    for (code_node, severity_node) in entries {
        let Some(action) = yaml_scalar_text(severity_node) else {
            // `severityNode.span` — the entry's value node (its key locates
            // the value span when the key is a scalar).
            let token = yaml_scalar_text(code_node).unwrap_or_else(|| "diagnostics".to_owned());
            push_plugin_invalid_value(issues, text, &section, &token);
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
                    // `valueNode.span` — the value node under `git/<key>:`.
                    let sub = format!("{section}/{key}");
                    push_plugin_invalid_value(issues, text, &sub, key);
                }
            }
        }
        Value::String(_) => {}
        // `gitValue.span` — the value node under `git:`.
        _ => {
            push_plugin_invalid_value(issues, text, &section, "git");
        }
    }
}

/// The data half of one file's `plugins:` section — the upstream
/// `_ParsedPluginsData` (parse_model:1529-1631): `clears_existing` mirrors
/// `clearsExisting` (a non-map section — including the bare null scalar —
/// clears the include chain's merge), `configurations` the parsed
/// `PluginConfiguration`s, `dependency_overrides` the override map. The
/// row faces stay in [`plugin_issues`]; this is the merge-time parse.
struct PluginsSection {
    clears_existing: bool,
    configurations: Vec<PluginConfig>,
    dependency_overrides: Option<FxHashMap<String, PluginSource>>,
}

/// Parses one file's top-level `plugins:` section into [`PluginsSection`]
/// — `_ParsedPluginsData.parse` without the reporter (rows are
/// [`plugin_issues`]'s): every scalar key contributes a configuration when
/// its `_parsePluginSource` arm yields a source (name = the scalar's text);
/// `dependency_overrides` parses each map entry's source; a non-map section
/// (or the bare null) clears the chain's merge.
fn parse_plugins_section(map: &serde_yaml::Mapping, declaring_dir: &Path) -> PluginsSection {
    let mut parsed = PluginsSection {
        clears_existing: false,
        configurations: Vec::new(),
        dependency_overrides: None,
    };
    let Some(plugins) = map.get(Value::from("plugins")) else {
        return parsed;
    };
    let Some(entries) = plugins.as_mapping() else {
        // `YamlScalar(value: null)` reports nothing but still clears.
        parsed.clears_existing = true;
        return parsed;
    };
    for (name_node, plugin_node) in entries {
        let Some(name) = yaml_scalar_text(name_node) else {
            continue;
        };
        if name == "dependency_overrides" {
            if let Value::Mapping(overrides) = plugin_node {
                for (override_key, override_value) in overrides {
                    let Some(override_name) = yaml_scalar_text(override_key) else {
                        continue;
                    };
                    if let Some(source) = parse_plugin_source(override_value, declaring_dir) {
                        parsed
                            .dependency_overrides
                            .get_or_insert_default()
                            .insert(override_name, source);
                    }
                }
            }
            continue;
        }
        let Some(source) = parse_plugin_source(plugin_node, declaring_dir) else {
            continue;
        };
        // `diagnostics` parses whenever the entry is a map carrying the key
        // (any node shape — `parseDiagnosticsSection` itself gates forms).
        let diagnostics = match plugin_node {
            Value::Mapping(plugin_map) => plugin_map
                .get(Value::from("diagnostics"))
                .map(parse_diagnostics_section)
                .unwrap_or_default(),
            _ => FxHashMap::default(),
        };
        parsed.configurations.push(PluginConfig {
            name,
            source,
            diagnostics,
        });
    }
    parsed
}

/// `_parsePluginSource` (parse_model:1639-1693) — the source arm order is
/// the upstream decision order: string scalar → version constraint; map →
/// `version`(+`hosted`) → `git` → `path`, first match wins. A relative
/// `path` joins the declaring file's directory and normalizes
/// (`pathContext.join` + `normalize` — the source enters the synthetic
/// pub package, so it stores absolute before leaving options parsing).
fn parse_plugin_source(node: &Value, declaring_dir: &Path) -> Option<PluginSource> {
    if let Value::String(constraint) = node {
        return Some(PluginSource::Versioned {
            constraint: constraint.clone(),
            hosted: None,
        });
    }
    let Value::Mapping(map) = node else {
        return None;
    };
    let string_at = |key: &str| match map.get(Value::from(key)) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    if let Some(constraint) = string_at("version") {
        return Some(PluginSource::Versioned {
            constraint,
            hosted: string_at("hosted"),
        });
    }
    match map.get(Value::from("git")) {
        Some(Value::String(url)) => {
            return Some(PluginSource::Git {
                url: url.clone(),
                path: None,
                reference: None,
                tag_pattern: None,
            });
        }
        Some(Value::Mapping(git)) => {
            let git_at = |key: &str| match git.get(Value::from(key)) {
                Some(Value::String(text)) => Some(text.clone()),
                _ => None,
            };
            if let Some(url) = git_at("url") {
                return Some(PluginSource::Git {
                    url,
                    path: git_at("path"),
                    reference: git_at("ref"),
                    tag_pattern: git_at("tag_pattern"),
                });
            }
        }
        _ => {}
    }
    if let Some(path) = string_at("path") {
        let path = PathBuf::from(path);
        return Some(PluginSource::Path(if path.is_relative() {
            dartq_pub::normalize(&declaring_dir.join(path))
        } else {
            dartq_pub::normalize(&path)
        }));
    }
    None
}

/// `parseDiagnosticsSection` (lint/config.dart:11-64): a list enables each
/// string entry; a map parses each entry through `_parseRuleConfig`
/// (config.dart:83-104 — string keys only: a bool yields enable/disable, a
/// string must name a `ConfiguredSeverity` *verbatim* — a miss degrades to
/// enable — and any other shape contributes nothing), and a map *value*
/// recurses one level as a named group. Keys lowercase into the map (the
/// upstream `LinkedHashMap` is case-insensitive).
fn parse_diagnostics_section(node: &Value) -> FxHashMap<String, ConfiguredSeverity> {
    /// `_parseRuleConfig`: `Some(severity)` for the two parseable shapes.
    fn rule_config(value: &Value) -> Option<ConfiguredSeverity> {
        match value {
            Value::Bool(enabled) => Some(if *enabled {
                ConfiguredSeverity::Enable
            } else {
                ConfiguredSeverity::Disable
            }),
            Value::String(name) => Some(match name.as_str() {
                "disable" => ConfiguredSeverity::Disable,
                "enable" => ConfiguredSeverity::Enable,
                "info" => ConfiguredSeverity::Info,
                "warning" => ConfiguredSeverity::Warning,
                "error" => ConfiguredSeverity::Error,
                _ => ConfiguredSeverity::Enable,
            }),
            _ => None,
        }
    }
    let mut configs = FxHashMap::default();
    match node {
        Value::Sequence(list) => {
            for entry in list {
                if let Value::String(name) = entry {
                    configs.insert(name.to_lowercase(), ConfiguredSeverity::Enable);
                }
            }
        }
        Value::Mapping(map) => {
            for (key_node, value) in map {
                let Some(name) = key_node.as_str() else {
                    continue;
                };
                if let Some(severity) = rule_config(value) {
                    configs.insert(name.to_lowercase(), severity);
                } else if let Value::Mapping(group) = value {
                    for (inner_key, inner_value) in group {
                        let Some(inner_name) = inner_key.as_str() else {
                            continue;
                        };
                        if let Some(severity) = rule_config(inner_value) {
                            configs.insert(inner_name.to_lowercase(), severity);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    configs
}

/// The plugins rows' anchor: upstream spans every row at the offending YAML
/// node; serde_yaml keeps no spans, so the row anchors at `token`'s first
/// occurrence inside the `plugins` section (the established
/// [`anchor_in_section`] convention — the `(code, file)` face is exact).
fn plugin_anchor(text: &str, token: &str) -> (u32, u32) {
    anchor_in_section(text, "plugins", token)
}

/// The UTF-16 `(offset, length)` of the *value* node of the `key` entry
/// inside the `plugins` section — the upstream anchor of every
/// `invalid_section_format` row in the plugins face (`plugins.span`,
/// `pluginNode.span`, `pluginValue.span` — the span of the offending
/// value, never the key; the pinned 2026-10-18 probe confirms the oracle
/// anchors the scalar value / entry value / list block). `serde_yaml`
/// keeps no spans, so the value's extent is re-derived from the text:
/// same-line content after `key:` anchors the trimmed token (a trailing
/// ` #` comment excluded); otherwise the block collection's span — the
/// first non-blank, non-comment line more indented than the key (or a `-`
/// item at the key's indentation, the same-indent sequence form) through
/// the last line still inside the block. `(0, 0)` when the key or a value
/// cannot be located.
fn plugin_value_anchor(text: &str, key: &str) -> (u32, u32) {
    // The key locates inside the `plugins` section (the
    // [`anchor_in_section`] convention — the section key's first occurrence
    // scopes the search); successive occurrences are tried until one reads
    // as a real `key:` mapping entry (a `git` inside `gitlab:` is no hit).
    let from = text.find("plugins").unwrap_or(0);
    let mut scan = from;
    while let Some(relative) = text[scan..].find(key) {
        let key_at = scan + relative;
        if let Some((start, end)) = value_span(text.as_bytes(), key, key_at) {
            return (
                byte_offset_to_utf16(text, start),
                byte_offset_to_utf16(text, end).saturating_sub(byte_offset_to_utf16(text, start)),
            );
        }
        scan = key_at + key.len();
    }
    (0, 0)
}

/// The UTF-16 `(offset, length)` of the *value* node of the mapping key
/// `key`'s first occurrence — the upstream anchor of every section-level
/// row (`analyzer.span` on a non-map `analyzer:`, `plugins.span` on the
/// plugins face). The same span machinery as [`plugin_value_anchor`] with
/// the whole-text first-occurrence scoping [`anchor_in_section`] uses for
/// a token equal to its section key: successive occurrences are tried
/// until one reads as a real `key:` entry (a homograph inside a comment or
/// a longer key's head is no hit). `None` when no entry locates.
fn section_value_anchor(text: &str, key: &str) -> Option<(u32, u32)> {
    let mut scan = 0;
    while let Some(relative) = text[scan..].find(key) {
        let key_at = scan + relative;
        if let Some((start, end)) = value_span(text.as_bytes(), key, key_at) {
            return Some((
                byte_offset_to_utf16(text, start),
                byte_offset_to_utf16(text, end).saturating_sub(byte_offset_to_utf16(text, start)),
            ));
        }
        scan = key_at + key.len();
    }
    None
}

/// The byte `(start, end)` of the value node of the mapping key `key` at
/// `key_at` — `None` when `key_at` does not sit on a `key:` entry (a token
/// inside a scalar or a longer key's head). The span machinery of
/// [`plugin_value_anchor`], reading bytes (UTF-8 input is ASCII-safe for
/// the ws/indent scans).
fn value_span(bytes: &[u8], key: &str, key_at: usize) -> Option<(usize, usize)> {
    let mut at = key_at + key.len();
    // Skip to the `:` separating key from value; a `key` hit without one
    // (inside a scalar or comment) is no key — no value.
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    if bytes.get(at) != Some(&b':') {
        return None;
    }
    at += 1;
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let key_line_start = bytes[..key_at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let key_indent = indent_of(&bytes[key_line_start..key_at]);
    match bytes.get(at) {
        // Same-line content: the value token runs to the trimmed line end,
        // a ` #` comment excluded; a flow collection continues across lines
        // until its brackets balance.
        Some(&c) if c != b'\n' && c != b'#' => Some(flow_span(bytes, at)),
        // A `#` comment or bare EOL: the block collection (if any) lives on
        // the deeper-indented lines below.
        _ => block_span(bytes, at, key_indent),
    }
}

/// The trailing-comment-trimmed same-line value span, extended across lines
/// while a flow collection (`[`/`{`) stays unbalanced. `start` is the first
/// non-ws byte after `key:`.
fn flow_span(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut depth = 0_i32;
    let mut quote = None;
    let mut at = start;
    while at < bytes.len() {
        let c = bytes[at];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == b'\\' && q == b'"' {
                at += 1;
            }
        } else {
            match c {
                b'\'' | b'"' => quote = Some(c),
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                b'\n' if depth <= 0 => break,
                b'#' if depth <= 0 && at > start && matches!(bytes[at - 1], b' ' | b'\t') => {
                    break;
                }
                _ => {}
            }
        }
        at += 1;
    }
    let mut end = at.min(bytes.len());
    while end > start && matches!(bytes[end - 1], b' ' | b'\t' | b'\r' | b'\n') {
        end -= 1;
    }
    (start, end.max(start))
}

/// The block-collection value span below a `key:` whose line carries no
/// content: the first non-blank, non-comment line more indented than the
/// key (or a `- ` item at the key's own indentation — YAML's same-indent
/// sequence form) starts the block; it ends before the first content line
/// that is neither deeper than the key nor a `- ` item at the block's
/// sequence indentation. `at` sits at the line tail after `key:`.
fn block_span(bytes: &[u8], mut at: usize, key_indent: usize) -> Option<(usize, usize)> {
    let mut start = None;
    let mut end = None;
    let mut seq_indent = usize::MAX;
    while at < bytes.len() {
        // Step to the next line's start and measure it.
        let line_start = bytes[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| at + i + 1)?;
        at = line_start;
        let mut cursor = at;
        while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
        let indent = cursor - line_start;
        let content_end = bytes[cursor..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| cursor + i);
        let first = bytes.get(cursor).copied();
        // Blank and comment-only lines ride inside the block's span but
        // neither open nor close it.
        if first.is_none() || first == Some(b'#') || cursor == content_end {
            continue;
        }
        let dash_item = first == Some(b'-')
            && matches!(bytes.get(cursor + 1), Some(b' ' | b'\t' | b'\n') | None);
        let inside = indent > key_indent
            || (dash_item && indent == key_indent)
            || (dash_item && indent == seq_indent);
        if !inside {
            break;
        }
        if start.is_none() {
            start = Some(cursor);
            if dash_item {
                seq_indent = indent;
            }
        }
        // The content's own end — a trailing ` #` comment stays inside the
        // node's span upstream (the comment is part of the collection's
        // extent); only trailing ws trims.
        let mut line_end = content_end;
        while line_end > cursor && matches!(bytes[line_end - 1], b' ' | b'\t' | b'\r') {
            line_end -= 1;
        }
        end = Some(line_end);
    }
    start.zip(end)
}

/// The leading-space/tab count of a line head — indentation for the block
/// scan (YAML bans tabs in indentation; a tab is still counted here, the
/// malformed shape only shifts the span).
fn indent_of(line_head: &[u8]) -> usize {
    line_head
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count()
}

/// One `invalid_section_format` row whose message carries the nested
/// section name (`plugins/<name>` / `plugins/<name>/git/…`) and whose
/// anchor is the *value* node's span under `key` — upstream's anchor for
/// every plugins-face `invalid_section_format` is the offending value's
/// `YamlNode.span`, never the key (the pinned 2026-10-18 probe: scalar →
/// the value token, map entry → the entry value, list → the block's span).
/// `key` is the entry's key token, resolved to its value's extent by
/// [`plugin_value_anchor`].
fn push_plugin_invalid_value(issues: &mut Vec<OptionsIssue>, text: &str, section: &str, key: &str) {
    let (offset, length) = plugin_value_anchor(text, key);
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
/// section.`, registry :9708-9722), anchored at the section's own value
/// node ([`section_value_anchor`] — the upstream `analyzer.span`/`errors.span`
/// convention: the pinned 2026-10-29 probe reports the block-list value of
/// `analyzer:` at `2:3|11`, never the key), falling back to the
/// section-scoped key anchor ([`anchor_in_section`]) when no `key:` entry
/// locates (a bare key with nothing below it).
fn push_invalid_section(issues: &mut Vec<OptionsIssue>, text: &str, section: &str, token: &str) {
    let (offset, length) = section_value_anchor(text, section)
        .unwrap_or_else(|| anchor_in_section(text, section, token));
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

/// The `analyzer.cannot-ignore` string entries a file's own list declares
/// (`_ParsedCannotIgnoreData.parse`, parse_model:346-405): string entries
/// only, in declaration order, with the severity names dropped — the
/// `error`/`info`/`warning` arms (case-sensitive, `severityMap` keys) skip
/// the registry check upstream and can never report. The rest are
/// `unrecognized_error_code` candidates, validity the caller's registry
/// seam; non-string entries are `invalid_section_format` (reported by
/// `local_issues`), never candidates.
fn declared_cannot_ignore_names(map: &serde_yaml::Mapping, text: &str) -> Vec<DeclaredError> {
    let mut names = Vec::new();
    let Some(entries) = map
        .get(Value::from("analyzer"))
        .and_then(|analyzer| analyzer.as_mapping())
        .and_then(|analyzer| analyzer.get(Value::from("cannot-ignore")))
        .and_then(|entries| entries.as_sequence())
    else {
        return names;
    };
    // Per-occurrence anchoring like `declared_error_codes`: every string
    // entry — recorded or severity-skipped — consumes its own occurrence, so
    // a repeated name anchors at the occurrence being reported (every
    // occurrence of an unknown name reports upstream).
    let search_from = std::cell::Cell::new(text.find("cannot-ignore").unwrap_or(0));
    for entry in entries {
        let Some(name) = entry.as_str() else {
            continue;
        };
        let (offset, length) = anchor_occurrence(text, "cannot-ignore", &search_from, name);
        if severity_of_name(name).is_some() {
            continue;
        }
        names.push(DeclaredError {
            code: name.to_owned(),
            offset,
            length,
        });
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
        // `packageUri` absent anchors the source root at `rootUri` itself —
        // the reader contract (package_config_json.dart:152), matching
        // `PackageConfig::resolve`'s `lib_dir`.
        let path = match &entry.package_uri {
            Some(uri) => root.join(uri).join(percent_decode(subpath)),
            None => root.join(percent_decode(subpath)),
        };
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

// --- Pubspec and Android manifest validation faces (#2757) ---
//
// The oracle's driver validates every `pubspec.yaml` and (under
// `analyzer.optional-checks.chrome-os-manifest-checks`) every
// `android/AndroidManifest.xml` it discovers in the workspace's packages —
// upstream Dart SDK source, `pkg/analyzer/lib/src/pubspec/pubspec_validator.dart`
// (the seven per-file validators run in declaration order:
// dependency, field, flutter, name, screenshots, platforms, workspace) and
// `pkg/analyzer/lib/src/manifest/manifest_validator.dart`. Neither file is
// YAML/XML-parsed by `serde_yaml` here: rows anchor at the official
// `YamlNode`/`SourceFile` spans, so this section carries its own
// span-tracking readers (the options face's anchoring convention cannot pin
// the exact `4:7|12` row positions these faces report). Node spans are raw
// source extents — quoted scalars include the quotes — while message
// arguments use the cooked scalar text, both exactly as the upstream node
// model reports them. Every judgment, iteration order, early return, and
// message template mirrors the pinned 3.13.7 validators verbatim.

const ASSET_DIRECTORY_DOES_NOT_EXIST: CodeInfo = CodeInfo::new(
    "asset_directory_does_not_exist",
    "ASSET_DIRECTORY_DOES_NOT_EXIST",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_DOES_NOT_EXIST: CodeInfo = CodeInfo::new(
    "asset_does_not_exist",
    "ASSET_DOES_NOT_EXIST",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_FIELD_NOT_LIST: CodeInfo = CodeInfo::new(
    "asset_field_not_list",
    "ASSET_FIELD_NOT_LIST",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_MISSING_PATH: CodeInfo = CodeInfo::new(
    "asset_missing_path",
    "ASSET_MISSING_PATH",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_NOT_STRING: CodeInfo = CodeInfo::new(
    "asset_not_string",
    "ASSET_NOT_STRING",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_NOT_STRING_OR_MAP: CodeInfo = CodeInfo::new(
    "asset_not_string_or_map",
    "ASSET_NOT_STRING_OR_MAP",
    DiagnosticType::StaticWarning,
    true,
);

const ASSET_PATH_NOT_STRING: CodeInfo = CodeInfo::new(
    "asset_path_not_string",
    "ASSET_PATH_NOT_STRING",
    DiagnosticType::StaticWarning,
    true,
);

const CAMERA_PERMISSIONS_INCOMPATIBLE: CodeInfo = CodeInfo::new(
    "camera_permissions_incompatible",
    "CAMERA_PERMISSIONS_INCOMPATIBLE",
    DiagnosticType::StaticWarning,
    false,
);

const DEPENDENCIES_FIELD_NOT_MAP: CodeInfo = CodeInfo::new(
    "dependencies_field_not_map",
    "DEPENDENCIES_FIELD_NOT_MAP",
    DiagnosticType::StaticWarning,
    true,
);

const DEPRECATED_FIELD: CodeInfo = CodeInfo::new(
    "deprecated_field",
    "DEPRECATED_FIELD",
    DiagnosticType::StaticWarning,
    true,
);

const FLUTTER_FIELD_NOT_MAP: CodeInfo = CodeInfo::new(
    "flutter_field_not_map",
    "FLUTTER_FIELD_NOT_MAP",
    DiagnosticType::StaticWarning,
    true,
);

const INVALID_DEPENDENCY: CodeInfo = CodeInfo::new(
    "invalid_dependency",
    "INVALID_DEPENDENCY",
    DiagnosticType::StaticWarning,
    true,
);

const INVALID_PLATFORMS_FIELD: CodeInfo = CodeInfo::new(
    "invalid_platforms_field",
    "INVALID_PLATFORMS_FIELD",
    DiagnosticType::StaticWarning,
    true,
);

const MISSING_NAME: CodeInfo = CodeInfo::new(
    "missing_name",
    "MISSING_NAME",
    DiagnosticType::StaticWarning,
    true,
);

const NAME_NOT_STRING: CodeInfo = CodeInfo::new(
    "name_not_string",
    "NAME_NOT_STRING",
    DiagnosticType::StaticWarning,
    true,
);

const NO_TOUCHSCREEN_FEATURE: CodeInfo = CodeInfo::new(
    "no_touchscreen_feature",
    "NO_TOUCHSCREEN_FEATURE",
    DiagnosticType::StaticWarning,
    false,
);

const NON_RESIZABLE_ACTIVITY: CodeInfo = CodeInfo::new(
    "non_resizable_activity",
    "NON_RESIZABLE_ACTIVITY",
    DiagnosticType::StaticWarning,
    false,
);

const PATH_DOES_NOT_EXIST: CodeInfo = CodeInfo::new(
    "path_does_not_exist",
    "PATH_DOES_NOT_EXIST",
    DiagnosticType::StaticWarning,
    true,
);

const PATH_NOT_POSIX: CodeInfo = CodeInfo::new(
    "path_not_posix",
    "PATH_NOT_POSIX",
    DiagnosticType::StaticWarning,
    true,
);

const PATH_PUBSPEC_DOES_NOT_EXIST: CodeInfo = CodeInfo::new(
    "path_pubspec_does_not_exist",
    "PATH_PUBSPEC_DOES_NOT_EXIST",
    DiagnosticType::StaticWarning,
    true,
);

const PERMISSION_IMPLIES_UNSUPPORTED_HARDWARE: CodeInfo = CodeInfo::new(
    "permission_implies_unsupported_hardware",
    "PERMISSION_IMPLIES_UNSUPPORTED_HARDWARE",
    DiagnosticType::StaticWarning,
    false,
);

const PLATFORM_VALUE_DISALLOWED: CodeInfo = CodeInfo::new(
    "platform_value_disallowed",
    "PLATFORM_VALUE_DISALLOWED",
    DiagnosticType::StaticWarning,
    true,
);

const SETTING_ORIENTATION_ON_ACTIVITY: CodeInfo = CodeInfo::new(
    "setting_orientation_on_activity",
    "SETTING_ORIENTATION_ON_ACTIVITY",
    DiagnosticType::StaticWarning,
    false,
);

const UNKNOWN_PLATFORM: CodeInfo = CodeInfo::new(
    "unknown_platform",
    "UNKNOWN_PLATFORM",
    DiagnosticType::StaticWarning,
    true,
);

const UNNECESSARY_DEV_DEPENDENCY: CodeInfo = CodeInfo::new(
    "unnecessary_dev_dependency",
    "UNNECESSARY_DEV_DEPENDENCY",
    DiagnosticType::StaticWarning,
    true,
);

const UNSUPPORTED_CHROME_OS_FEATURE: CodeInfo = CodeInfo::new(
    "unsupported_chrome_os_feature",
    "UNSUPPORTED_CHROME_OS_FEATURE",
    DiagnosticType::StaticWarning,
    false,
);

const UNSUPPORTED_CHROME_OS_HARDWARE: CodeInfo = CodeInfo::new(
    "unsupported_chrome_os_hardware",
    "UNSUPPORTED_CHROME_OS_HARDWARE",
    DiagnosticType::StaticWarning,
    false,
);

const WORKSPACE_FIELD_NOT_LIST: CodeInfo = CodeInfo::new(
    "workspace_field_not_list",
    "WORKSPACE_FIELD_NOT_LIST",
    DiagnosticType::StaticWarning,
    true,
);

const WORKSPACE_VALUE_NOT_STRING: CodeInfo = CodeInfo::new(
    "workspace_value_not_string",
    "WORKSPACE_VALUE_NOT_STRING",
    DiagnosticType::StaticWarning,
    true,
);

const WORKSPACE_VALUE_NOT_SUBDIRECTORY: CodeInfo = CodeInfo::new(
    "workspace_value_not_subdirectory",
    "WORKSPACE_VALUE_NOT_SUBDIRECTORY",
    DiagnosticType::StaticWarning,
    true,
);

/// A span-tracking YAML node — the `yaml` package's node model reduced to
/// the pubspec grammar: block and flow mappings/sequences plus typed
/// scalars. `raw` is the byte extent the upstream `YamlNode.span` reports —
/// the raw source text, quoted scalars' quotes included, trailing
/// whitespace and ` #` comments trimmed; message arguments carry the cooked
/// scalar text instead. Flow collections parse one level deep (their items
/// as scalar nodes), the shape the pubspec validators read; YAML features
/// the pubspec grammar never uses (anchors, block scalars, multi-line plain
/// scalars) are outside this reader and its callers.
enum PubspecNode {
    Scalar {
        raw: (usize, usize),
        text: String,
        kind: ScalarKind,
    },
    Seq {
        raw: (usize, usize),
        items: Vec<PubspecNode>,
    },
    Map {
        raw: (usize, usize),
        entries: Vec<(PubspecNode, PubspecNode)>,
    },
}

/// The `yaml` package's implicit scalar resolution (`_tryParseScalar`,
/// `yaml` loader.dart:214-257): the type distinction every `is String` /
/// `== null` judgment in the validators reads — `42` is not a string,
/// an unquoted `null` is null, a quoted `"null"` is a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    Null,
    Bool,
    Number,
    Str,
}

impl PubspecNode {
    /// The node's raw source extent (bytes).
    fn raw(&self) -> (usize, usize) {
        match self {
            PubspecNode::Scalar { raw, .. }
            | PubspecNode::Seq { raw, .. }
            | PubspecNode::Map { raw, .. } => *raw,
        }
    }

    /// The cooked scalar text when the node is a *string* scalar
    /// (`PubspecValidationContext.asString` — a non-string or non-scalar
    /// node is no string).
    fn as_str(&self) -> Option<&str> {
        match self {
            PubspecNode::Scalar {
                text,
                kind: ScalarKind::Str,
                ..
            } => Some(text),
            _ => None,
        }
    }

    /// Whether the node resolves to the YAML null (a missing value, `null`,
    /// `Null`, `NULL`, `~`).
    fn is_null(&self) -> bool {
        matches!(
            self,
            PubspecNode::Scalar {
                kind: ScalarKind::Null,
                ..
            }
        )
    }

    /// The block-or-flow mapping's entries, in document order.
    fn as_map(&self) -> Option<&[(PubspecNode, PubspecNode)]> {
        match self {
            PubspecNode::Map { entries, .. } => Some(entries),
            _ => None,
        }
    }

    /// The block-or-flow sequence's items, in document order.
    fn as_seq(&self) -> Option<&[PubspecNode]> {
        match self {
            PubspecNode::Seq { items, .. } => Some(items),
            _ => None,
        }
    }

    /// The value node of the mapping entry keyed `key` (keys compare by
    /// their cooked scalar text, the `contents.nodes[key]` face).
    fn get(&self, key: &str) -> Option<&PubspecNode> {
        self.get_entry(key).map(|(_, value)| value)
    }

    /// Both nodes of the mapping entry keyed `key`.
    fn get_entry(&self, key: &str) -> Option<(&PubspecNode, &PubspecNode)> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(k, v)| (k, v))
    }
}

/// A line-indexed reader over one pubspec text producing [`PubspecNode`]s.
/// Line-oriented like the official scanner is token-oriented: blank and
/// comment-only lines open no node and belong to no collection's extent;
/// indentation decides containment exactly as [`block_span`] judges it
/// (deeper lines ride under the open entry, `- ` items may sit at their
/// key's own indentation).
struct PubspecYaml<'a> {
    text: &'a str,
    bytes: &'a [u8],
}

impl<'a> PubspecYaml<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
        }
    }

    /// The document root: the first content line's node; an empty (or
    /// comment-only) document is a null scalar at the origin — the
    /// `contents is! YamlMap` face the validators gate on.
    fn parse_document(&self) -> PubspecNode {
        match self.next_content_line(0) {
            Some(i) => self.parse_node(i).0,
            None => PubspecNode::Scalar {
                raw: (0, 0),
                text: String::new(),
                kind: ScalarKind::Null,
            },
        }
    }

    /// The first content (non-blank, non-comment-only) line at or after
    /// `from` — the range stops at the line count, so an all-blank tail
    /// yields `None`.
    fn next_content_line(&self, from: usize) -> Option<usize> {
        (from..self.line_count()).find(|&i| self.line_content(i).is_some())
    }

    /// One line's `(indent, content start, content end)` — `None` for a
    /// blank or comment-only line; the content end trims trailing
    /// whitespace.
    fn line_content(&self, i: usize) -> Option<(usize, usize, usize)> {
        let bounds = self.line_bounds(i)?;
        let head = &self.bytes[bounds.0..bounds.1];
        let indent = indent_of(head);
        if indent == head.len() || head[indent] == b'#' {
            return None;
        }
        Some((indent, bounds.0 + indent, bounds.1))
    }

    /// The total number of lines (a trailing newline closes one line).
    fn line_count(&self) -> usize {
        1 + self.bytes.iter().filter(|&&b| b == b'\n').count()
    }

    /// The `(start, end)` byte bounds of line `i`'s text, newline excluded.
    fn line_bounds(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.line_count() {
            return None;
        }
        let start = if i == 0 {
            0
        } else {
            let mut newlines = 0;
            let mut start = 0;
            for (offset, b) in self.bytes.iter().enumerate() {
                if *b == b'\n' {
                    newlines += 1;
                    if newlines == i {
                        start = offset + 1;
                        break;
                    }
                }
            }
            start
        };
        let end = self.bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(self.bytes.len(), |at| start + at);
        Some((start, end))
    }

    /// The byte offset of line `i`'s start.
    fn line_start(&self, i: usize) -> usize {
        self.line_bounds(i).map_or(0, |(start, _)| start)
    }

    /// Parses the node opening at content line `i`: a sequence (dash item),
    /// a mapping (`key:`), or an inline scalar/flow value.
    fn parse_node(&self, i: usize) -> (PubspecNode, usize) {
        let (indent, content, _) = self.line_content(i).expect("content line");
        if self.bytes[content] == b'-'
            && matches!(self.bytes.get(content + 1), Some(b' ' | b'\t') | None)
        {
            self.parse_seq(i, indent)
        } else if self.key_colon(i, content).is_some() {
            self.parse_map(i, content)
        } else {
            self.parse_inline(i, content)
        }
    }

    /// Parses a block sequence whose items sit at column `indent`.
    fn parse_seq(&self, i: usize, indent: usize) -> (PubspecNode, usize) {
        let mut items = Vec::new();
        let start = self.line_start(i) + indent;
        let mut end = start;
        let mut cur = i;
        while cur < self.line_count() {
            let Some((ind, content, content_end)) = self.line_content(cur) else {
                cur += 1;
                continue;
            };
            if ind != indent
                || !(self.bytes[content] == b'-'
                    && matches!(self.bytes.get(content + 1), Some(b' ' | b'\t') | None))
            {
                break;
            }
            let value_col = content
                + 1
                + self.bytes[content + 1..content_end]
                    .iter()
                    .take_while(|&&b| b == b' ' || b == b'\t')
                    .count();
            let (node, next) = if value_col >= content_end {
                self.parse_block_below(cur, indent)
            } else if self.key_colon(cur, value_col).is_some() {
                self.parse_map(cur, value_col)
            } else {
                self.parse_inline(cur, value_col)
            };
            end = node.raw().1;
            items.push(node);
            cur = next;
        }
        (
            PubspecNode::Seq {
                raw: (start, end),
                items,
            },
            cur,
        )
    }

    /// Parses a block mapping whose entries' keys sit at the byte offset
    /// `key_col` on line `i` — the map's entries share one *column* (the
    /// first key's line-relative column: a sequence item's map starts
    /// mid-line, its continuations at that same column), and each entry's
    /// child block opens past that column.
    fn parse_map(&self, i: usize, key_col: usize) -> (PubspecNode, usize) {
        let mut entries = Vec::new();
        let column = key_col - self.line_start(i);
        let start = key_col;
        let mut end = start;
        let mut cur = i;
        while cur < self.line_count() {
            let Some((ind, _, content_end)) = self.line_content(cur) else {
                cur += 1;
                continue;
            };
            if (if cur == i { column } else { ind }) != column {
                break;
            }
            let at = self.line_start(cur) + column;
            let Some(colon) = self.key_colon(cur, at) else {
                break;
            };
            let key = self.quoted_or_plain_scalar(at, self.scalar_end(at, colon));
            let after = colon
                + 1
                + self.bytes[colon + 1..content_end]
                    .iter()
                    .take_while(|&&b| b == b' ' || b == b'\t')
                    .count();
            let (value, next) = if after < content_end {
                self.parse_inline(cur, after)
            } else {
                self.parse_block_below(cur, column)
            };
            end = end.max(value.raw().1);
            entries.push((key, value));
            cur = next;
        }
        (
            PubspecNode::Map {
                raw: (start, end),
                entries,
            },
            cur,
        )
    }

    /// The value block below a `key:` (or bare `-`) whose line carries no
    /// content: the first content line deeper than `key_indent` — or a
    /// `- ` item at `key_indent` itself, YAML's same-indent sequence form —
    /// opens it; anything shallower leaves a null scalar at the key line's
    /// tail.
    fn parse_block_below(&self, i: usize, key_indent: usize) -> (PubspecNode, usize) {
        let tail = self.line_bounds(i).map_or(0, |(_, end)| end);
        let null_node = || PubspecNode::Scalar {
            raw: (tail, tail),
            text: String::new(),
            kind: ScalarKind::Null,
        };
        let Some(next) = self.next_content_line(i + 1) else {
            return (null_node(), i + 1);
        };
        let Some((ind, content, _)) = self.line_content(next) else {
            return (null_node(), next);
        };
        let dash = self.bytes[content] == b'-'
            && matches!(self.bytes.get(content + 1), Some(b' ' | b'\t') | None);
        if ind > key_indent || (ind == key_indent && dash) {
            self.parse_node(next)
        } else {
            (null_node(), next)
        }
    }

    /// Parses an inline value at byte `col` on line `i`: a flow collection
    /// (possibly across lines) or a one-line scalar.
    fn parse_inline(&self, i: usize, col: usize) -> (PubspecNode, usize) {
        match self.bytes.get(col) {
            Some(b'[') | Some(b'{') => self.parse_flow(col),
            _ => {
                let (_, _, content_end) = self.line_content(i).expect("content line");
                (
                    self.quoted_or_plain_scalar(col, self.scalar_end(col, content_end)),
                    i + 1,
                )
            }
        }
    }

    /// A scalar token's end at byte `start`: a quoted token ends after its
    /// closing quote, a plain token at the trailing-whitespace / ` #`
    /// comment trim of the line's content end.
    fn scalar_end(&self, start: usize, content_end: usize) -> usize {
        match self.bytes.get(start) {
            Some(q @ (b'\'' | b'"')) => {
                let mut at = start + 1;
                while at < content_end {
                    if self.bytes[at] == *q && !(*q == b'"' && self.bytes[at - 1] == b'\\') {
                        return at + 1;
                    }
                    at += 1;
                }
                content_end
            }
            _ => {
                let mut end = content_end;
                let mut at = start;
                while at < end {
                    if self.bytes[at] == b'#'
                        && at > start
                        && matches!(self.bytes[at - 1], b' ' | b'\t')
                    {
                        end = at;
                        break;
                    }
                    at += 1;
                }
                while end > start && matches!(self.bytes[end - 1], b' ' | b'\t' | b'\r') {
                    end -= 1;
                }
                end
            }
        }
    }

    /// The scalar node over bytes `start..end`: quoted forms cook with
    /// YAML's folding rules, plain forms resolve through
    /// [`ScalarKind::resolve`].
    fn quoted_or_plain_scalar(&self, start: usize, end: usize) -> PubspecNode {
        let slice = &self.text[start..end];
        match self.bytes.get(start) {
            Some(b'\'') if end >= start + 2 => {
                let inner = &slice[1..slice.len().saturating_sub(1)];
                PubspecNode::Scalar {
                    raw: (start, end),
                    text: inner.replace("''", "'"),
                    kind: ScalarKind::Str,
                }
            }
            Some(b'"') if end >= start + 2 => {
                let inner = &slice[1..slice.len().saturating_sub(1)];
                PubspecNode::Scalar {
                    raw: (start, end),
                    text: unescape_double_quoted(inner),
                    kind: ScalarKind::Str,
                }
            }
            _ => PubspecNode::Scalar {
                raw: (start, end),
                text: slice.to_owned(),
                kind: ScalarKind::resolve(slice),
            },
        }
    }

    /// Parses a flow collection opening at byte `start` (`[` or `{`),
    /// splitting its interior at depth-one commas; items parse as scalars
    /// or nested flows, map entries at their depth-one colon.
    fn parse_flow(&self, start: usize) -> (PubspecNode, usize) {
        let is_map = self.bytes[start] == b'{';
        let mut depth = 0_i32;
        let mut quote = None;
        let mut at = start;
        let mut end = self.bytes.len();
        while at < self.bytes.len() {
            let c = self.bytes[at];
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                } else if c == b'\\' && q == b'"' {
                    at += 1;
                }
            } else {
                match c {
                    b'\'' | b'"' => quote = Some(c),
                    b'[' | b'{' => depth += 1,
                    b']' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = at + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            at += 1;
        }
        // Interior chunks at depth one, commas splitting.
        let interior_start = start + 1;
        let interior_end = end.saturating_sub(1);
        let mut chunks: Vec<(usize, usize)> = Vec::new();
        let mut chunk = interior_start;
        let mut depth = 0_i32;
        let mut quote = None;
        let mut at = interior_start;
        while at < interior_end {
            let c = self.bytes[at];
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                } else if c == b'\\' && q == b'"' {
                    at += 1;
                }
            } else {
                match c {
                    b'\'' | b'"' => quote = Some(c),
                    b'[' | b'{' => depth += 1,
                    b']' | b'}' => depth -= 1,
                    b',' if depth == 0 => {
                        chunks.push((chunk, at));
                        chunk = at + 1;
                    }
                    _ => {}
                }
            }
            at += 1;
        }
        chunks.push((chunk, interior_end));
        let mut entries: Vec<(PubspecNode, PubspecNode)> = Vec::new();
        let mut items: Vec<PubspecNode> = Vec::new();
        for (from, to) in chunks {
            let head = from
                + self.bytes[from..to]
                    .iter()
                    .take_while(|&&b| b == b' ' || b == b'\t')
                    .count();
            let tail = {
                let mut t = to;
                while t > head && matches!(self.bytes[t - 1], b' ' | b'\t') {
                    t -= 1;
                }
                t
            };
            if head >= tail {
                continue;
            }
            if is_map {
                let mut at = head;
                let mut quote = None;
                let mut colon = None;
                while at < tail {
                    let c = self.bytes[at];
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        }
                    } else {
                        match c {
                            b'\'' | b'"' => quote = Some(c),
                            b':' => {
                                colon = Some(at);
                                break;
                            }
                            _ => {}
                        }
                    }
                    at += 1;
                }
                let Some(colon) = colon else {
                    continue;
                };
                let key = self.quoted_or_plain_scalar(head, self.scalar_end(head, colon));
                let mut value_at = colon + 1;
                while value_at < tail && matches!(self.bytes[value_at], b' ' | b'\t') {
                    value_at += 1;
                }
                let value = match self.bytes.get(value_at) {
                    Some(b'[') | Some(b'{') => self.parse_flow(value_at).0,
                    _ => self.quoted_or_plain_scalar(value_at, self.scalar_end(value_at, tail)),
                };
                entries.push((key, value));
            } else {
                items.push(match self.bytes.get(head) {
                    Some(b'[') | Some(b'{') => self.parse_flow(head).0,
                    _ => self.quoted_or_plain_scalar(head, self.scalar_end(head, tail)),
                });
            }
        }
        let raw = (start, end);
        let node = if is_map {
            PubspecNode::Map { raw, entries }
        } else {
            PubspecNode::Seq { raw, items }
        };
        // The line after the closing bracket's line resumes the walk.
        let line = self.text[..end].bytes().filter(|&b| b == b'\n').count();
        (node, line + 1)
    }

    /// The byte of the `:` that ends the mapping key starting at `col` on
    /// line `i` — `None` when the token reads as something else (a scalar
    /// head, a comment): the key runs to a `:` followed by whitespace, line
    /// end, or the line's end (quoted keys first close their quotes).
    fn key_colon(&self, i: usize, col: usize) -> Option<usize> {
        let (_, _, content_end) = self.line_content(i)?;
        let mut at = col;
        let mut quote = None;
        while at < content_end {
            let c = self.bytes[at];
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else {
                match c {
                    b'\'' | b'"' => quote = Some(c),
                    b':' => {
                        let after = self.bytes.get(at + 1);
                        if matches!(after, None | Some(b' ' | b'\t' | b'\n' | b'\r')) {
                            return Some(at);
                        }
                    }
                    b'#' if at > col && matches!(self.bytes[at - 1], b' ' | b'\t') => {
                        return None;
                    }
                    _ => {}
                }
            }
            at += 1;
        }
        None
    }
}

/// YAML double-quoted scalar unescaping — the escape set the `yaml`
/// scanner folds (`\"`, `\\`, the C0 named escapes, `\xXX`/`\uXXXX`/
/// `\UXXXXXXXX`); unknown escapes keep the escaped character.
fn unescape_double_quoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(escaped) = chars.next() else {
            out.push('\\');
            break;
        };
        match escaped {
            '0' => out.push('\0'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            't' | '\t' => out.push('\t'),
            'n' => out.push('\n'),
            'v' => out.push('\u{b}'),
            'f' => out.push('\u{c}'),
            'r' => out.push('\r'),
            'e' => out.push('\u{1b}'),
            ' ' => out.push(' '),
            '"' => out.push('"'),
            '/' => out.push('/'),
            '\\' => out.push('\\'),
            'N' => out.push('\u{85}'),
            '_' => out.push('\u{a0}'),
            'L' => out.push('\u{2028}'),
            'P' => out.push('\u{2029}'),
            'x' | 'u' | 'U' => {
                let width = match escaped {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut value = 0_u32;
                for _ in 0..width {
                    match chars.next().and_then(|c| c.to_digit(16)) {
                        Some(digit) => value = value * 16 + digit,
                        None => break,
                    }
                }
                out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
            }
            other => out.push(other),
        }
    }
    out
}

impl ScalarKind {
    /// The implicit resolution of one plain scalar (`_tryParseScalar`,
    /// `yaml` loader.dart:214-257, plus `_parseNumberValue` :268-334):
    /// the null/bool spellings first, then the Dart int/double grammars
    /// (`0x`/`0o` radix forms included, underscores never), everything
    /// else a string.
    fn resolve(text: &str) -> Self {
        match text {
            "" | "null" | "Null" | "NULL" | "~" => return ScalarKind::Null,
            "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => {
                return ScalarKind::Bool;
            }
            _ => {}
        }
        if is_yaml_number(text) {
            ScalarKind::Number
        } else {
            ScalarKind::Str
        }
    }
}

/// Whether `text` resolves to a YAML number under the Dart loader's
/// grammar: `.inf`/`.nan` spellings, `[+-]?0x[hex]+`, `[+-]?0o[0-7]+`,
/// `[+-]?[0-9]+`, or a Dart `double.tryParse` decimal.
fn is_yaml_number(text: &str) -> bool {
    if matches!(
        text,
        ".inf"
            | ".Inf"
            | ".INF"
            | "+.inf"
            | "+.Inf"
            | "+.INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) {
        return true;
    }
    let digits = |bytes: &[u8]| bytes.iter().all(u8::is_ascii_digit) && !bytes.is_empty();
    let rest: &[u8] = match text.as_bytes().first() {
        Some(b'+') | Some(b'-') => &text.as_bytes()[1..],
        _ => text.as_bytes(),
    };
    if rest.len() >= 2 && rest[0] == b'0' && (rest[1] | 0x20) == b'x' {
        return rest[2..].iter().all(u8::is_ascii_hexdigit) && rest.len() > 2;
    }
    if rest.len() >= 2 && rest[0] == b'0' && (rest[1] | 0x20) == b'o' {
        return rest[2..].iter().all(|b| (b'0'..=b'7').contains(b)) && rest.len() > 2;
    }
    if digits(rest) {
        return true;
    }
    dart_double(text)
}

/// The Dart `double.tryParse` decimal grammar: an optional sign, digits
/// with an optional fraction and/or exponent, or a fraction-only form.
fn dart_double(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        at = 1;
    }
    let int_start = at;
    while at < bytes.len() && bytes[at].is_ascii_digit() {
        at += 1;
    }
    let int_digits = at > int_start;
    let mut fraction = false;
    if at < bytes.len() && bytes[at] == b'.' {
        at += 1;
        let frac_start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        fraction = at > frac_start;
    }
    if !int_digits && !fraction {
        return false;
    }
    if at < bytes.len() && (bytes[at] | 0x20) == b'e' {
        at += 1;
        if matches!(bytes.get(at), Some(b'+') | Some(b'-')) {
            at += 1;
        }
        let exp_start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at == exp_start {
            return false;
        }
    }
    at == bytes.len()
}

/// The field names the validators read (`PubspecField`, pubspec_validator.dart
/// :90-132).
const FLUTTER_FIELD: &str = "flutter";
const ASSETS_FIELD: &str = "assets";
const ASSET_PATH_FIELD: &str = "path";
const PATH_FIELD: &str = "path";
const GIT_FIELD: &str = "git";
const NAME_FIELD: &str = "name";
const VERSION_FIELD: &str = "version";
const PUBLISH_TO_FIELD: &str = "publish_to";
const DEPENDENCIES_FIELD: &str = "dependencies";
const DEV_DEPENDENCIES_FIELD: &str = "dev_dependencies";
const SCREENSHOTS_FIELD: &str = "screenshots";
const PLATFORMS_FIELD: &str = "platforms";
const WORKSPACE_FIELD: &str = "workspace";

/// The fields `fieldValidator` reports (`_deprecatedFields`,
/// field_validator.dart:9).
const DEPRECATED_FIELDS: [&str; 4] = ["author", "authors", "transformers", "web"];

/// The platforms `platformsValidator` accepts (`_knownPlatforms`,
/// platforms_validator.dart:12-19).
const KNOWN_PLATFORMS: [&str; 6] = ["android", "ios", "linux", "macos", "web", "windows"];

/// Validates one `pubspec.yaml`: the seven upstream validators in their
/// declaration order, each judgment mirroring the pinned 3.13.7 validator
/// verbatim (early returns included). `text` is the file's content, `dir`
/// the directory containing it (the `packageRoot` every relative path
/// resolves against).
pub fn pubspec_issues(text: &str, dir: &Path) -> Vec<OptionsIssue> {
    let yaml = PubspecYaml::new(text);
    let root = yaml.parse_document();
    let mut issues = Vec::new();
    dependency_validator(&yaml, &root, dir, &mut issues);
    field_validator(&yaml, &root, &mut issues);
    flutter_validator(&yaml, &root, dir, &mut issues);
    name_validator(&yaml, &root, &mut issues);
    screenshots_validator(&yaml, &root, dir, &mut issues);
    platforms_validator(&yaml, &root, &mut issues);
    workspace_validator(&yaml, &root, dir, &mut issues);
    issues
}

/// One row at a raw byte span, converted to the diagnostic's UTF-16 face.
fn pubspec_row(
    code: CodeInfo,
    message: String,
    correction: Option<String>,
    text: &str,
    raw: (usize, usize),
) -> OptionsIssue {
    let start = byte_offset_to_utf16(text, raw.0);
    OptionsIssue {
        code,
        message,
        correction,
        offset: start,
        length: byte_offset_to_utf16(text, raw.1).saturating_sub(start),
    }
}

/// `dependencyValidator` (dependency_validator.dart:13-129): the `path:` /
/// `git:` entries of both dependency sections judged against the resolved
/// folder's existence facts, publishability gating `invalid_dependency`, and
/// dev dependencies duplicated in `dependencies` reporting
/// `unnecessary_dev_dependency`.
fn dependency_validator(
    yaml: &PubspecYaml<'_>,
    root: &PubspecNode,
    dir: &Path,
    issues: &mut Vec<OptionsIssue>,
) {
    let Some(map) = root.as_map() else {
        return;
    };
    let dependencies = declared_dependencies(yaml, map, DEPENDENCIES_FIELD, issues);
    let dev_dependencies = declared_dependencies(yaml, map, DEV_DEPENDENCIES_FIELD, issues);
    // Publishability (:102-111): a version — any non-null scalar — and a
    // `publish_to` other than `none`.
    let publishable = root
        .get(VERSION_FIELD)
        .is_some_and(|version| !version.is_null())
        && root.get(PUBLISH_TO_FIELD).and_then(PubspecNode::as_str) != Some("none");
    for (_, spec) in &dependencies {
        validate_path_entries(yaml, dir, spec, publishable, issues);
    }
    for (key, spec) in &dev_dependencies {
        let name = key.as_str();
        if name.is_some()
            && dependencies
                .iter()
                .any(|(dep_key, _)| dep_key.as_str() == name)
        {
            issues.push(pubspec_row(
                UNNECESSARY_DEV_DEPENDENCY,
                format!(
                    "The dev dependency on {} is unnecessary because there is also a \
                     normal dependency on that package.",
                    name.unwrap_or_default()
                ),
                Some("Try removing the dev dependency.".to_owned()),
                yaml.text,
                key.raw(),
            ));
        }
        validate_path_entries(yaml, dir, spec, false, issues);
    }
}

/// `getDeclaredDependencies` (dependency_validator.dart:17-31): one
/// section's named entries in document order — a missing or null field
/// contributes none, a non-map field reports `dependencies_field_not_map`
/// once and contributes none.
fn declared_dependencies<'a>(
    yaml: &PubspecYaml<'_>,
    map: &'a [(PubspecNode, PubspecNode)],
    field: &str,
    issues: &mut Vec<OptionsIssue>,
) -> Vec<(&'a PubspecNode, &'a PubspecNode)> {
    let Some(node) = map
        .iter()
        .find(|(key, _)| key.as_str() == Some(field))
        .map(|(_, value)| value)
    else {
        return Vec::new();
    };
    if node.is_null() {
        return Vec::new();
    }
    match node.as_map() {
        Some(entries) => entries.iter().map(|(key, value)| (key, value)).collect(),
        None => {
            issues.push(pubspec_row(
                DEPENDENCIES_FIELD_NOT_MAP,
                format!("The value of the '{field}' field is expected to be a map."),
                Some("Try converting the value to be a map.".to_owned()),
                yaml.text,
                node.raw(),
            ));
            Vec::new()
        }
    }
}

/// `validatePathEntries` (dependency_validator.dart:42-93): one dependency
/// spec's `path:` value judged — not-POSIX first, then the folder's and its
/// pubspec's existence — and, for publishable packages, the `path:`/`git:`
/// forms reported at their keys.
fn validate_path_entries(
    yaml: &PubspecYaml<'_>,
    dir: &Path,
    spec: &PubspecNode,
    check_for_path_and_git_deps: bool,
    issues: &mut Vec<OptionsIssue>,
) {
    if spec.as_map().is_none() {
        return;
    }
    if let Some((key, value)) = spec.get_entry(PATH_FIELD) {
        if let Some(path) = value.as_str() {
            if path.contains('\\') {
                issues.push(pubspec_row(
                    PATH_NOT_POSIX,
                    format!("The path '{path}' isn't a POSIX-style path."),
                    Some("Try converting the value to a POSIX-style path.".to_owned()),
                    yaml.text,
                    value.raw(),
                ));
                return;
            }
            let resolved = resolve_dependency_path(dir, path);
            let folder_exists = resolved.is_dir();
            if !folder_exists {
                issues.push(pubspec_row(
                    PATH_DOES_NOT_EXIST,
                    format!("The path '{path}' doesn't exist."),
                    Some(
                        "Try creating the referenced path or using a path that exists.".to_owned(),
                    ),
                    yaml.text,
                    value.raw(),
                ));
            } else if !resolved.join("pubspec.yaml").is_file() {
                issues.push(pubspec_row(
                    PATH_PUBSPEC_DOES_NOT_EXIST,
                    format!("The directory '{path}' doesn't contain a pubspec."),
                    Some(
                        "Try creating a pubspec in the referenced directory or using a path \
                         that has a pubspec."
                            .to_owned(),
                    ),
                    yaml.text,
                    value.raw(),
                ));
            }
            if check_for_path_and_git_deps {
                issues.push(invalid_dependency_row(yaml, key.raw(), PATH_FIELD));
            }
        }
    }
    if let Some((key, _)) = spec.get_entry(GIT_FIELD)
        && check_for_path_and_git_deps
    {
        issues.push(invalid_dependency_row(yaml, key.raw(), GIT_FIELD));
    }
}

/// The `invalid_dependency` row at one form's key span.
fn invalid_dependency_row(
    yaml: &PubspecYaml<'_>,
    key_raw: (usize, usize),
    kind: &str,
) -> OptionsIssue {
    pubspec_row(
        INVALID_DEPENDENCY,
        format!("Publishable packages can't have '{kind}' dependencies."),
        Some(format!(
            "Try adding a 'publish_to: none' entry to mark the package as not for \
             publishing or remove the {kind} dependency."
        )),
        yaml.text,
        key_raw,
    )
}

/// `fieldValidator` (field_validator.dart:12-26): every top-level key whose
/// name is one of the deprecated spellings, reported at the key.
fn field_validator(yaml: &PubspecYaml<'_>, root: &PubspecNode, issues: &mut Vec<OptionsIssue>) {
    let Some(entries) = root.as_map() else {
        return;
    };
    for (key, _) in entries {
        let Some(name) = key.as_str() else {
            continue;
        };
        if DEPRECATED_FIELDS.contains(&name) {
            issues.push(pubspec_row(
                DEPRECATED_FIELD,
                format!("The '{name}' field is no longer used and can be removed."),
                Some("Try removing the field.".to_owned()),
                yaml.text,
                key.raw(),
            ));
        }
    }
}

/// `flutterValidator` (flutter_validator.dart:12-67): the `flutter` section
/// must be a map (an empty section is allowed), its `assets` a list, and
/// every entry a string path or a map with a string `path:`.
fn flutter_validator(
    yaml: &PubspecYaml<'_>,
    root: &PubspecNode,
    dir: &Path,
    issues: &mut Vec<OptionsIssue>,
) {
    let Some(map) = root.as_map() else {
        return;
    };
    let Some(flutter) = map
        .iter()
        .find(|(key, _)| key.as_str() == Some(FLUTTER_FIELD))
        .map(|(_, value)| value)
    else {
        return;
    };
    let Some(flutter_entries) = flutter.as_map() else {
        if !flutter.is_null() {
            issues.push(pubspec_row(
                FLUTTER_FIELD_NOT_MAP,
                "The value of the 'flutter' field is expected to be a map.".to_owned(),
                Some("Try converting the value to be a map.".to_owned()),
                yaml.text,
                flutter.raw(),
            ));
        }
        return;
    };
    let Some(assets) = flutter_entries
        .iter()
        .find(|(key, _)| key.as_str() == Some(ASSETS_FIELD))
        .map(|(_, value)| value)
    else {
        return;
    };
    let Some(items) = assets.as_seq() else {
        issues.push(pubspec_row(
            ASSET_FIELD_NOT_LIST,
            "The value of the 'assets' field is expected to be a list of relative file paths."
                .to_owned(),
            Some("Try converting the value to be a list of relative file paths.".to_owned()),
            yaml.text,
            assets.raw(),
        ));
        return;
    };
    for item in items {
        match item {
            PubspecNode::Scalar { .. } => match item.as_str() {
                Some(entry) => validate_asset_path(yaml, dir, entry, item.raw(), issues),
                None => {
                    issues.push(pubspec_row(
                        ASSET_NOT_STRING_OR_MAP,
                        "An asset value is required to be a file path (string) or map.".to_owned(),
                        Some("Try converting the value to be a string or map.".to_owned()),
                        yaml.text,
                        item.raw(),
                    ));
                    return;
                }
            },
            PubspecNode::Map { .. } => match item.get(ASSET_PATH_FIELD) {
                None => issues.push(pubspec_row(
                    ASSET_MISSING_PATH,
                    "Asset map entry must contain a 'path' field.".to_owned(),
                    Some("Try adding a 'path' field.".to_owned()),
                    yaml.text,
                    item.raw(),
                )),
                Some(path) if !matches!(path, PubspecNode::Scalar { .. }) => {
                    issues.push(pubspec_row(
                        ASSET_PATH_NOT_STRING,
                        "Asset paths are required to be file paths (strings).".to_owned(),
                        Some("Try converting the value to be a string.".to_owned()),
                        yaml.text,
                        path.raw(),
                    ))
                }
                Some(path) => match path.as_str() {
                    Some(entry) => validate_asset_path(yaml, dir, entry, path.raw(), issues),
                    None => {
                        issues.push(pubspec_row(
                            ASSET_NOT_STRING,
                            "Assets are required to be file paths (strings).".to_owned(),
                            Some("Try converting the value to be a string.".to_owned()),
                            yaml.text,
                            path.raw(),
                        ));
                        return;
                    }
                },
            },
            _ => issues.push(pubspec_row(
                ASSET_NOT_STRING_OR_MAP,
                "An asset value is required to be a file path (string) or map.".to_owned(),
                Some("Try converting the value to be a string or map.".to_owned()),
                yaml.text,
                item.raw(),
            )),
        }
    }
}

/// `_validateAssetPath` (flutter_validator.dart:101-124): `packages/`
/// references go unvalidated, a trailing `/` demands a directory, and the
/// row anchors at the entry's own node.
fn validate_asset_path(
    yaml: &PubspecYaml<'_>,
    dir: &Path,
    path: &str,
    raw: (usize, usize),
    issues: &mut Vec<OptionsIssue>,
) {
    if path.starts_with("packages/") {
        return;
    }
    let resolved = dir.join(path);
    if asset_exists_at_path(resolved.as_path()) {
        return;
    }
    let (code, message) = if path.ends_with('/') {
        (
            ASSET_DIRECTORY_DOES_NOT_EXIST,
            format!("The asset directory '{path}' doesn't exist."),
        )
    } else {
        (
            ASSET_DOES_NOT_EXIST,
            format!("The asset file '{path}' doesn't exist."),
        )
    };
    issues.push(pubspec_row(
        code,
        message,
        Some(
            if path.ends_with('/') {
                "Try creating the directory or fixing the path to the directory."
            } else {
                "Try creating the file or fixing the path to the file."
            }
            .to_owned(),
        ),
        yaml.text,
        raw,
    ));
}

/// `_assetExistsAtPath` (flutter_validator.dart:71-97): the path itself as
/// folder or file, else the same file name inside any sibling folder of its
/// parent.
fn asset_exists_at_path(path: &Path) -> bool {
    if path.is_dir() || path.is_file() {
        return true;
    }
    let Some(name) = path.file_name() else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    if !parent.is_dir() {
        return false;
    }
    let Ok(children) = std::fs::read_dir(parent) else {
        return false;
    };
    children
        .flatten()
        .any(|child| child.path().is_dir() && child.path().join(name).is_file())
}

/// `nameValidator` (name_validator.dart:12-26): the document must be a map
/// carrying a string `name:`; a missing field reports at the origin, a
/// non-string value at its own node.
fn name_validator(yaml: &PubspecYaml<'_>, root: &PubspecNode, issues: &mut Vec<OptionsIssue>) {
    let Some(entries) = root.as_map() else {
        issues.push(OptionsIssue {
            code: MISSING_NAME,
            message: "The 'name' field is required but missing.".to_owned(),
            correction: Some("Try adding a field named 'name'.".to_owned()),
            offset: 0,
            length: 0,
        });
        return;
    };
    match entries
        .iter()
        .find(|(key, _)| key.as_str() == Some(NAME_FIELD))
        .map(|(_, value)| value)
    {
        None => issues.push(OptionsIssue {
            code: MISSING_NAME,
            message: "The 'name' field is required but missing.".to_owned(),
            correction: Some("Try adding a field named 'name'.".to_owned()),
            offset: 0,
            length: 0,
        }),
        Some(value) if value.as_str().is_none() => issues.push(pubspec_row(
            NAME_NOT_STRING,
            "The value of the 'name' field is required to be a string.".to_owned(),
            Some("Try converting the value to be a string.".to_owned()),
            yaml.text,
            value.raw(),
        )),
        Some(_) => {}
    }
}

/// `screenshotsValidator` (screenshot_validator.dart:12-38): each
/// `screenshots:` entry's `path:` must reference an existing *file*.
fn screenshots_validator(
    yaml: &PubspecYaml<'_>,
    root: &PubspecNode,
    dir: &Path,
    issues: &mut Vec<OptionsIssue>,
) {
    let Some(items) = root.get(SCREENSHOTS_FIELD).and_then(PubspecNode::as_seq) else {
        return;
    };
    for item in items {
        let Some((_, path)) = item.get_entry(PATH_FIELD) else {
            continue;
        };
        let Some(entry) = path.as_str() else {
            continue;
        };
        if !dir.join(entry).is_file() {
            issues.push(pubspec_row(
                PATH_DOES_NOT_EXIST,
                format!("The path '{entry}' doesn't exist."),
                Some("Try creating the referenced path or using a path that exists.".to_owned()),
                yaml.text,
                path.raw(),
            ));
        }
    }
}

/// `platformsValidator` (platforms_validator.dart:22-53): the `platforms`
/// field must be a map whose keys name known platforms and whose values are
/// empty; keys report at their key nodes, values at theirs.
fn platforms_validator(yaml: &PubspecYaml<'_>, root: &PubspecNode, issues: &mut Vec<OptionsIssue>) {
    let Some(platforms) = root.get(PLATFORMS_FIELD) else {
        return;
    };
    let Some(entries) = platforms.as_map() else {
        issues.push(pubspec_row(
            INVALID_PLATFORMS_FIELD,
            "The 'platforms' field must be a map with platforms as keys.".to_owned(),
            Some("Try changing the 'platforms' field to a map with platforms as keys.".to_owned()),
            yaml.text,
            platforms.raw(),
        ));
        return;
    };
    for (key, _) in entries {
        let known = key
            .as_str()
            .is_some_and(|name| KNOWN_PLATFORMS.contains(&name));
        if !known {
            let argument = match key {
                PubspecNode::Scalar { text, .. } => text.clone(),
                PubspecNode::Seq { .. } => "[]".to_owned(),
                PubspecNode::Map { .. } => "{}".to_owned(),
            };
            issues.push(pubspec_row(
                UNKNOWN_PLATFORM,
                format!("The platform '{argument}' is not a recognized platform."),
                Some("Try correcting the platform name or removing it.".to_owned()),
                yaml.text,
                key.raw(),
            ));
        }
    }
    for (_, value) in entries {
        if !value.is_null() {
            issues.push(pubspec_row(
                PLATFORM_VALUE_DISALLOWED,
                "Keys in the `platforms` field can't have values.".to_owned(),
                Some("Try removing the value, while keeping the key.".to_owned()),
                yaml.text,
                value.raw(),
            ));
        }
    }
}

/// `workspaceValidator` (workspace_validator.dart:24-55): the `workspace`
/// field must be a list of subdirectory paths (glob syntax allowed, its
/// non-glob prefix must exist).
fn workspace_validator(
    yaml: &PubspecYaml<'_>,
    root: &PubspecNode,
    dir: &Path,
    issues: &mut Vec<OptionsIssue>,
) {
    let Some(workspace) = root.get(WORKSPACE_FIELD) else {
        return;
    };
    let Some(items) = workspace.as_seq() else {
        issues.push(pubspec_row(
            WORKSPACE_FIELD_NOT_LIST,
            "The value of the 'workspace' field is required to be a list of relative file paths."
                .to_owned(),
            Some("Try converting the value to be a list of relative file paths.".to_owned()),
            yaml.text,
            workspace.raw(),
        ));
        return;
    };
    for item in items {
        match item.as_str() {
            Some(entry) => {
                validate_directory_path(yaml, dir, entry, item.raw(), issues);
            }
            None => {
                issues.push(pubspec_row(
                    WORKSPACE_VALUE_NOT_STRING,
                    "Workspace entries are required to be directory paths (strings).".to_owned(),
                    Some("Try converting the value to be a string.".to_owned()),
                    yaml.text,
                    item.raw(),
                ));
                if matches!(item, PubspecNode::Scalar { .. }) {
                    return;
                }
            }
        }
    }
}

/// `_validateDirectoryPath` (workspace_validator.dart:88-140): the value
/// must stay inside the pubspec's directory; glob values judge their
/// non-glob prefix, plain values the directory itself.
fn validate_directory_path(
    yaml: &PubspecYaml<'_>,
    dir: &Path,
    value: &str,
    raw: (usize, usize),
    issues: &mut Vec<OptionsIssue>,
) {
    let joined = dir.join(value);
    if !path_is_within(dir, &joined) {
        issues.push(pubspec_row(
            WORKSPACE_VALUE_NOT_SUBDIRECTORY,
            format!(
                "Workspace values must be a relative path of a subdirectory of '{}'.",
                dir.display()
            ),
            Some(
                "Try using a subdirectory of the directory containing the 'pubspec.yaml' file."
                    .to_owned(),
            ),
            yaml.text,
            raw,
        ));
        return;
    }
    if is_glob_pattern(value) {
        if let Some(base) = glob_base_path(value) {
            let base_dir = dir.join(&base);
            if !base_dir.is_dir() {
                issues.push(pubspec_row(
                    PATH_DOES_NOT_EXIST,
                    format!("The path '{base}' doesn't exist."),
                    Some(
                        "Try creating the referenced path or using a path that exists.".to_owned(),
                    ),
                    yaml.text,
                    raw,
                ));
            }
        }
        return;
    }
    if !joined.is_dir() {
        issues.push(pubspec_row(
            PATH_DOES_NOT_EXIST,
            format!("The path '{value}' doesn't exist."),
            Some("Try creating the referenced path or using a path that exists.".to_owned()),
            yaml.text,
            raw,
        ));
    }
}

/// Whether `path` sits inside `base` — the `Folder.contains` face
/// (`isWithin`): equality counts, escaping `..` segments do not.
fn path_is_within(base: &Path, path: &Path) -> bool {
    if path == base {
        return true;
    }
    let normalized = normalize_path(path);
    let normalized_base = normalize_path(base);
    normalized.starts_with(&normalized_base)
}

/// Lexical path normalization — `context.normalize`: `.` segments drop,
/// `..` segments pop, the root stays rooted.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `resolve_dependency_path`'s join — `context.joinAll(path.posix.split(..))`
/// followed by `absolute` + `normalize` (dependency_validator.dart:59-63).
fn resolve_dependency_path(dir: &Path, entry: &str) -> PathBuf {
    let joined = if entry.starts_with('/') {
        PathBuf::from(entry)
    } else {
        dir.join(entry)
    };
    normalize_path(&joined)
}

/// `_isGlobPattern` (workspace_validator.dart:66-68): `Glob.quote` changes
/// the text — any of the quote set `* { [ ? \ } ] , - ( )` present.
fn is_glob_pattern(value: &str) -> bool {
    value.bytes().any(|b| b"*{[?\\}],-()".contains(&b))
}

/// `_globBasePath` (workspace_validator.dart:70-78): the non-glob prefix of
/// the path's `/`-split parts, joined; `None` when the first part is
/// already a pattern.
fn glob_base_path(value: &str) -> Option<String> {
    let mut non_glob = Vec::new();
    for part in value.split('/') {
        if is_glob_pattern(part) {
            break;
        }
        non_glob.push(part);
    }
    if non_glob.is_empty() {
        None
    } else {
        Some(non_glob.join("/"))
    }
}

// --- Android manifest validation (manifest_validator.dart) ---

/// The tags the manifest parser retains (`_manifestValidationElements`,
/// manifest_validator.dart:40-46); everything else parses through with its
/// subtree discarded.
const MANIFEST_ELEMENTS: [&str; 5] = [
    "activity",
    "application",
    "manifest",
    "uses-feature",
    "uses-permission",
];

/// The `uses-feature` names Chrome OS cannot support
/// (`unsupportedHardwareFeatures`, manifest_values.dart:38-78).
const UNSUPPORTED_HARDWARE_FEATURES: [&str; 37] = [
    "android.hardware.camera",
    "android.hardware.camera.autofocus",
    "android.hardware.camera.capability.manual_post_processing",
    "android.hardware.camera.capability.manual_sensor",
    "android.hardware.camera.capability.raw",
    "android.hardware.camera.flash",
    "android.hardware.camera.level.full",
    "android.hardware.consumerir",
    "android.hardware.location.gps",
    "android.hardware.nfc",
    "android.hardware.nfc.hce",
    "android.hardware.sensor.barometer",
    "android.hardware.telephony",
    "android.hardware.telephony.cdma",
    "android.hardware.telephony.gsm",
    "android.hardware.type.automotive",
    "android.hardware.type.television",
    "android.hardware.usb.accessory",
    "android.hardware.usb.host",
    // Partially-supported, only on some Chrome OS devices.
    "android.hardware.sensor.accelerometer",
    "android.hardware.sensor.compass",
    "android.hardware.sensor.gyroscope",
    "android.hardware.sensor.light",
    "android.hardware.sensor.proximity",
    "android.hardware.sensor.stepcounter",
    "android.hardware.sensor.stepdetector",
    // Software features that are not supported.
    "android.software.app_widgets",
    "android.software.device_admin",
    "android.software.home_screen",
    "android.software.input_methods",
    "android.software.leanback",
    "android.software.live_wallpaper",
    "android.software.live_tv",
    "android.software.managed_users",
    "android.software.midi",
    "android.software.sip",
    "android.software.sip.voip",
];

/// One parsed manifest element: the lowercased tag name, its attributes
/// (names lowercased, values verbatim, each with its source span), direct
/// children, and the element's own span.
struct ManifestElement {
    name: String,
    attributes: Vec<(String, String, (usize, usize))>,
    children: Vec<ManifestElement>,
    span: (usize, usize),
}

impl ManifestElement {
    /// The value (and span) of one attribute — later duplicates win, the
    /// parser's map-overwrite face.
    fn attribute(&self, name: &str) -> Option<&(String, String, (usize, usize))> {
        self.attributes.iter().rev().find(|(key, _, _)| key == name)
    }

    fn attribute_value(&self, name: &str) -> Option<&str> {
        self.attribute(name).map(|(_, value, _)| value.as_str())
    }
}

/// The Android manifest row set, gated on the
/// `analyzer.optional-checks.chrome-os-manifest-checks` option (`check_manifest`
/// — `ManifestValidator.validate`, manifest_validator.dart:412-424). `text`
/// is the `AndroidManifest.xml` content; a malformed document parses to no
/// rows (the parser never throws, its error result ends the walk silently).
pub fn manifest_issues(text: &str, check_manifest: bool) -> Vec<OptionsIssue> {
    if !check_manifest {
        return Vec::new();
    }
    let mut parser = ManifestParser::new(text);
    // The `manifest` root element, found past any leading comments and
    // declarations; a stray end tag's element carries its name too, so only
    // the retained-element results with the wrong name loop onward.
    let manifest = loop {
        match parser.parse_xml_tag() {
            TagResult::Element(element) => {
                if element.name == "manifest" {
                    break element;
                }
            }
            TagResult::Eof | TagResult::Error => return Vec::new(),
            _ => {}
        }
    };
    let features: Vec<&ManifestElement> = manifest
        .children
        .iter()
        .filter(|child| child.name == "uses-feature")
        .collect();
    let permissions: Vec<&ManifestElement> = manifest
        .children
        .iter()
        .filter(|child| child.name == "uses-permission")
        .collect();
    let mut issues = Vec::new();
    validate_touchscreen_feature(text, &features, &manifest, &mut issues);
    validate_features(text, &features, &mut issues);
    validate_permissions(text, &permissions, &features, &mut issues);
    if let Some(application) = manifest
        .children
        .iter()
        .find(|child| child.name == "application")
    {
        for activity in application
            .children
            .iter()
            .filter(|child| child.name == "activity")
        {
            validate_activity(text, activity, &mut issues);
        }
    }
    issues
}

/// One manifest row at a byte span, converted to UTF-16.
fn manifest_row(
    code: CodeInfo,
    message: String,
    correction: Option<String>,
    text: &str,
    raw: (usize, usize),
) -> OptionsIssue {
    let start = byte_offset_to_utf16(text, raw.0);
    OptionsIssue {
        code,
        message,
        correction,
        offset: start,
        length: byte_offset_to_utf16(text, raw.1).saturating_sub(start),
    }
}

/// `_validateTouchScreenFeature` (manifest_validator.dart:594-626): a
/// `touchscreen` feature without `android:required` or with `true` reports
/// per its shape, its absence reports on the manifest element itself.
fn validate_touchscreen_feature(
    text: &str,
    features: &[&ManifestElement],
    manifest: &ManifestElement,
    issues: &mut Vec<OptionsIssue>,
) {
    let feature = features.iter().copied().find(|element| {
        element.attribute_value("android:name") == Some("android.hardware.touchscreen")
    });
    match feature {
        Some(feature) => match feature.attribute_value("android:required") {
            None => issues.push(unsupported_hardware_row(
                text,
                feature.attribute("android:name").map(|(_, _, span)| *span),
                "android.hardware.touchscreen",
            )),
            Some("true") => issues.push(unsupported_feature_row(
                text,
                feature.attribute("android:name").map(|(_, _, span)| *span),
                "android.hardware.touchscreen",
            )),
            Some(_) => {}
        },
        None => issues.push(manifest_row(
            NO_TOUCHSCREEN_FEATURE,
            "The default \"android.hardware.touchscreen\" needs to be optional for Chrome OS."
                .to_owned(),
            Some(
                "Consider adding <uses-feature \
                 android:name=\"android.hardware.touchscreen\" android:required=\"false\" /> \
                 to the manifest."
                    .to_owned(),
            ),
            text,
            manifest.span,
        )),
    }
}

/// `_validateFeatures` (manifest_validator.dart:515-555): every retained
/// `uses-feature` naming an unsupported hardware feature reports per its
/// `android:required` shape.
fn validate_features(text: &str, features: &[&ManifestElement], issues: &mut Vec<OptionsIssue>) {
    for element in features {
        let Some(name) = element.attribute_value("android:name") else {
            continue;
        };
        let in_unsupported = UNSUPPORTED_HARDWARE_FEATURES.contains(&name);
        if !in_unsupported {
            continue;
        }
        match element.attribute_value("android:required") {
            None => issues.push(unsupported_hardware_row(
                text,
                element.attribute("android:name").map(|(_, _, span)| *span),
                name,
            )),
            Some("true") => issues.push(unsupported_feature_row(
                text,
                element.attribute("android:name").map(|(_, _, span)| *span),
                name,
            )),
            Some(_) => {}
        }
    }
}

/// `_validatePermissions` (manifest_validator.dart:558-591): the CAMERA
/// permission demands the camera feature pair, every other permission is
/// checked against the hardware it implies.
fn validate_permissions(
    text: &str,
    permissions: &[&ManifestElement],
    features: &[&ManifestElement],
    issues: &mut Vec<OptionsIssue>,
) {
    let has_camera = features
        .iter()
        .any(|f| f.attribute_value("android:name") == Some("android.hardware.camera"));
    let has_autofocus = features
        .iter()
        .any(|f| f.attribute_value("android:name") == Some("android.hardware.camera.autofocus"));
    for permission in permissions {
        let name_span = permission
            .attribute("android:name")
            .map(|(_, _, span)| *span);
        if permission.attribute_value("android:name") == Some("android.permission.CAMERA") {
            if !has_camera || !has_autofocus {
                issues.push(manifest_row(
                    CAMERA_PERMISSIONS_INCOMPATIBLE,
                    "Camera permissions make app incompatible for Chrome OS, consider adding \
                     optional features \"android.hardware.camera\" and \
                     \"android.hardware.camera.autofocus\"."
                        .to_owned(),
                    Some(
                        "Try adding `<uses-feature \
                         android:name=\"android.hardware.camera\"  \
                         android:required=\"false\">` `<uses-feature \
                         android:name=\"android.hardware.camera.autofocus\"  \
                         android:required=\"false\">`."
                            .to_owned(),
                    ),
                    text,
                    name_span.unwrap_or((0, 0)),
                ));
            }
        } else if let Some(feature) = permission
            .attribute_value("android:name")
            .and_then(implied_unsupported_hardware)
        {
            issues.push(manifest_row(
                PERMISSION_IMPLIES_UNSUPPORTED_HARDWARE,
                format!(
                    "Permission makes app incompatible for Chrome OS, consider adding \
                     optional {feature} feature tag,"
                ),
                Some(format!(
                    " Try adding `<uses-feature android:name=\"{feature}\"  \
                     android:required=\"false\">`."
                )),
                text,
                name_span.unwrap_or((0, 0)),
            ));
        }
    }
}

/// `_validateActivity` (manifest_validator.dart:488-512): locked
/// orientations and `resizeableActivity="false"` report at their
/// attributes.
fn validate_activity(text: &str, activity: &ManifestElement, issues: &mut Vec<OptionsIssue>) {
    if let Some((_, value, span)) = activity.attribute("android:screenorientation")
        && matches!(
            value.as_str(),
            "landscape"
                | "portrait"
                | "reverseLandscape"
                | "reversePortrait"
                | "sensorLandscape"
                | "sensorPortrait"
                | "userLandscape"
                | "userPortrait"
        )
    {
        issues.push(manifest_row(
            SETTING_ORIENTATION_ON_ACTIVITY,
            "The `<activity>` element should not be locked to any orientation so that \
             users can take advantage of the multi-window environments and larger \
             screens on Chrome OS"
                .to_owned(),
            Some(
                "Consider declaring the corresponding activity element with \
                 `screenOrientation=\"unspecified\"` or `\"fullSensor\"` attribute."
                    .to_owned(),
            ),
            text,
            *span,
        ));
    }
    if let Some((_, value, span)) = activity.attribute("android:resizeableactivity")
        && value == "false"
    {
        issues.push(manifest_row(
            NON_RESIZABLE_ACTIVITY,
            "The `<activity>` element should be allowed to be resized to allow users \
             to take advantage of the multi-window environment on Chrome OS"
                .to_owned(),
            Some(
                "Consider declaring the corresponding activity element with \
                 `resizableActivity=\"true\"` attribute."
                    .to_owned(),
            ),
            text,
            *span,
        ));
    }
}

/// The `unsupported_chrome_os_hardware` row at one `android:name` span.
fn unsupported_hardware_row(text: &str, span: Option<(usize, usize)>, name: &str) -> OptionsIssue {
    manifest_row(
        UNSUPPORTED_CHROME_OS_HARDWARE,
        format!("The feature {name} isn't supported on Chrome OS, consider making it optional."),
        Some("Try adding `android:required=\"false\"` for this feature.".to_owned()),
        text,
        span.unwrap_or((0, 0)),
    )
}

/// The `unsupported_chrome_os_feature` row at one `android:name` span.
fn unsupported_feature_row(text: &str, span: Option<(usize, usize)>, name: &str) -> OptionsIssue {
    manifest_row(
        UNSUPPORTED_CHROME_OS_FEATURE,
        format!("The feature {name} isn't supported on Chrome OS, consider making it optional."),
        Some("Try changing to `android:required=\"false\"` for this feature.".to_owned()),
        text,
        span.unwrap_or((0, 0)),
    )
}

/// `getImpliedUnsupportedHardware` (manifest_values.dart:95-124): the
/// hardware feature a permission's presence implies.
fn implied_unsupported_hardware(permission: &str) -> Option<&'static str> {
    match permission {
        "android.permission.CAMERA" => Some("android.hardware.camera"),
        "android.permission.CALL_PHONE"
        | "android.permission.CALL_PRIVILEGED"
        | "android.permission.MODIFY_PHONE_STATE"
        | "android.permission.PROCESS_OUTGOING_CALLS"
        | "android.permission.READ_SMS"
        | "android.permission.RECEIVE_SMS"
        | "android.permission.RECEIVE_MMS"
        | "android.permission.RECEIVE_WAP_PUSH"
        | "android.permission.SEND_SMS"
        | "android.permission.WRITE_APN_SETTINGS"
        | "android.permission.WRITE_SMS" => Some("android.hardware.telephony"),
        _ => None,
    }
}

/// One parse outcome of [`ManifestParser::parse_xml_tag`].
enum TagResult {
    /// A retained element (its subtree in tow).
    Element(ManifestElement),
    /// An end tag.
    EndTag,
    /// A parsed non-retained element (its subtree discarded) — comments and
    /// declarations ride here too.
    Other,
    /// End of input.
    Eof,
    /// A parse error; the walk ends silently.
    Error,
}

/// The rudimentary XML reader of `ManifestParser`
/// (manifest_validator.dart:38-401), byte-faithful: element and attribute
/// names lowercase, attribute spans run from the name's first byte to the
/// closing quote's index (opening quote included), element spans run from
/// the start tag's `<` through the end tag's (or `/>`'s) `>` inclusive.
/// Malformed input yields [`TagResult::Error`] instead of throwing.
struct ManifestParser<'a> {
    content: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ManifestParser<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            content,
            bytes: content.as_bytes(),
            pos: 0,
        }
    }

    fn at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn is_ws(&self) -> bool {
        self.bytes
            .get(self.pos)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    }

    fn is_closing(&self) -> bool {
        self.at(0) == Some(b'>')
    }

    /// The `/>` and `?>` two-character closings.
    fn is_two_char_closing(&self) -> bool {
        matches!(self.at(0), Some(b'?' | b'/')) && self.at(1) == Some(b'>')
    }

    /// Walks to the next tag and parses it.
    fn parse_xml_tag(&mut self) -> TagResult {
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'<' {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return TagResult::Eof;
        }
        // `<!--` opens a comment, any other `<!` a declaration; both parse
        // through unretained.
        if self.at(1) == Some(b'!') {
            return if self.at(2) == Some(b'-') && self.at(3) == Some(b'-') {
                self.parse_comment()
            } else {
                self.parse_declaration()
            };
        }
        self.parse_normal_tag()
    }

    fn parse_comment(&mut self) -> TagResult {
        self.pos += 4;
        while !(self.at(0) == Some(b'-') && self.at(1) == Some(b'-') && self.at(2) == Some(b'>')) {
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return TagResult::Error;
            }
        }
        self.pos += 2;
        TagResult::Other
    }

    fn parse_declaration(&mut self) -> TagResult {
        self.pos += 2;
        while !self.is_closing() {
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return TagResult::Error;
            }
        }
        TagResult::Other
    }

    fn parse_normal_tag(&mut self) -> TagResult {
        let start = self.pos;
        self.pos += 1;
        if self.pos >= self.bytes.len() || self.is_ws() {
            // A tag cannot begin with whitespace.
            return TagResult::Error;
        }
        let is_end_tag = self.at(0) == Some(b'/');
        if is_end_tag {
            self.pos += 1;
        }
        let name_start = self.pos;
        let mut closed_empty = false;
        while !self.is_closing() && !self.is_two_char_closing() && !self.is_ws() {
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return TagResult::Error;
            }
        }
        let name = self.content[name_start..self.pos.min(self.content.len())].to_lowercase();
        if self.is_two_char_closing() {
            closed_empty = true;
            self.pos += 1;
        }
        if is_end_tag {
            // End tags carry no attributes; the `>` must follow.
            return match self.parse_any_whitespace() {
                false => TagResult::Error,
                true if self.is_closing() => TagResult::EndTag,
                true => TagResult::Error,
            };
        }
        let is_relevant = MANIFEST_ELEMENTS.contains(&name.as_str());
        let mut attributes = Vec::new();
        let mut empty = closed_empty;
        if !closed_empty && !self.is_closing() {
            match self.parse_attributes(is_relevant) {
                None => return TagResult::Error,
                Some((parsed, was_empty)) => {
                    attributes = parsed;
                    empty = was_empty;
                }
            }
        }
        let mut children = Vec::new();
        if !empty {
            self.pos += 1;
            loop {
                match self.parse_xml_tag() {
                    TagResult::EndTag => break,
                    TagResult::Eof | TagResult::Error => return TagResult::Eof,
                    TagResult::Other => {}
                    TagResult::Element(child) => {
                        children.push(child);
                        self.pos += 1;
                    }
                }
            }
        }
        if is_relevant {
            TagResult::Element(ManifestElement {
                name,
                attributes,
                children,
                span: (start, self.pos + 1),
            })
        } else {
            TagResult::Other
        }
    }

    /// Skips whitespace, `false` at end of input (the
    /// `_parseAnyWhitespace` error face).
    fn parse_any_whitespace(&mut self) -> bool {
        if self.pos >= self.bytes.len() {
            return false;
        }
        while self.is_ws() {
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return false;
            }
        }
        true
    }

    /// Parses attributes up to the tag's close; `None` on a malformed
    /// attribute. Attribute names lowercase, values keep their case, the
    /// span runs to the closing quote's index.
    fn parse_attributes(
        &mut self,
        is_relevant: bool,
    ) -> Option<(Vec<(String, String, (usize, usize))>, bool)> {
        let mut attributes = Vec::new();
        loop {
            if self.pos >= self.bytes.len() {
                return None;
            }
            if self.is_closing() {
                return Some((attributes, false));
            } else if self.is_two_char_closing() {
                self.pos += 1;
                return Some((attributes, true));
            } else if !self.is_ws() {
                return None;
            }
            if !self.parse_any_whitespace() {
                return None;
            }
            if self.is_closing() {
                return Some((attributes, false));
            } else if self.is_two_char_closing() {
                self.pos += 1;
                return Some((attributes, true));
            }
            // The attribute name runs to `=`; whitespace, `>`, or `/>`
            // inside is the HTML-style valueless form XML rejects.
            let name_start = self.pos;
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return None;
            }
            while self.at(0) != Some(b'=') {
                if self.is_ws() || self.is_closing() || self.is_two_char_closing() {
                    return None;
                }
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return None;
                }
            }
            let name = if is_relevant {
                self.content[name_start..self.pos].to_lowercase()
            } else {
                String::new()
            };
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return None;
            }
            let quote = match self.at(0) {
                Some(q @ (b'\'' | b'"')) => q,
                _ => return None,
            };
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return None;
            }
            let value_start = self.pos;
            while self.at(0) != Some(quote) {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return None;
                }
            }
            if is_relevant {
                let value = self.content[value_start..self.pos].to_owned();
                let span = (name_start, self.pos);
                attributes.push((name, value, span));
            }
            self.pos += 1;
        }
    }
}

// --- The unicode-text face (`text_direction_*`) ---

/// Whether `c` is one of the nine bidirectional code points the verifier
/// flags — U+202A..U+202E and U+2066..U+2069 (`_isCodeUnitMatch`,
/// unicode_text_verifier.dart:53-62).
fn is_direction_code_point(c: char) -> bool {
    matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069)
}

/// One region frame of the source scan ([`text_direction_issues`]): code
/// (top level or an interpolation body, brace-depth carried for the `}`
/// that closes the interpolation), the two comment forms (Dart block
/// comments nest), and a string literal — `quote`/`triple` pick the
/// terminator, `raw` disables escapes and interpolation.
#[derive(Debug, Clone, Copy)]
enum TextRegion {
    Code { braces: u32 },
    LineComment,
    BlockComment { depth: u32 },
    Str { quote: u8, triple: bool, raw: bool },
}

/// The `text_direction_code_point_in_comment` / `..._in_literal` rows for
/// one Dart source (`UnicodeTextVerifier.verify`, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/error/unicode_text_verifier.dart`:17-40): every
/// bidi code point (U+202A..U+202E, U+2066..U+2069) reports at its own
/// UTF-16 `(offset, 1)` — `in_literal` when the covering node is a string
/// literal (`SimpleStringLiteral`/`InterpolationString`), `in_comment`
/// otherwise ("if it's not in a string literal, we assume we're in a
/// comment" — the verifier's own approximation, over-reporting kept).
///
/// The covering-node question reduces to a source-region scan: a nesting
/// stack tracks comment and string-literal regions, interpolation `${...}`
/// re-entering code (which may open strings and comments of its own), the
/// escape and `$identifier` forms of non-raw strings, and the triple/raw
/// terminators. A code point inside any string region reports `in_literal`,
/// everywhere else `in_comment`, in source order. Rides the per-file Dart
/// diagnostic pass (the engine face that attributes rows to each source
/// file); the options module carries it as the auxiliary-verifier home
/// until that face mounts it.
#[must_use]
pub fn text_direction_issues(text: &str) -> Vec<OptionsIssue> {
    // The upstream pre-pass: the region walk only runs when a flagged code
    // point exists at all (unicode_text_verifier.dart:20-22, 43-51).
    if !text.chars().any(is_direction_code_point) {
        return Vec::new();
    }
    let mut issues = Vec::new();
    let mut stack = vec![TextRegion::Code { braces: 0 }];
    let (mut at, mut utf16) = (0_usize, 0_u32);
    while let Some(c) = text[at..].chars().next() {
        let (size, units) = (c.len_utf8(), c.len_utf16() as u32);
        if is_direction_code_point(c) {
            let hex = format!("{:X}", c as u32);
            let code_info = match stack.last() {
                // The string-literal arm; everywhere else — the comments and
                // plain code — is the in_comment arm (the verifier's
                // covering-node assumption).
                Some(TextRegion::Str { .. }) => TEXT_DIRECTION_CODE_POINT_IN_LITERAL,
                _ => TEXT_DIRECTION_CODE_POINT_IN_COMMENT,
            };
            issues.push(OptionsIssue {
                code: code_info,
                message: format!(
                    "The Unicode code point 'U+{hex}' changes the appearance of text from how \
                     it's interpreted by the compiler."
                ),
                correction: Some(format!(
                    "Try removing the code point or using the Unicode escape sequence '\\u{hex}'."
                )),
                offset: utf16,
                length: 1,
            });
        }
        match *stack.last().expect("the stack never empties") {
            TextRegion::Str { quote, triple, raw } => {
                if !raw && c == '\\' {
                    // The escape consumes the next char too (a raw newline
                    // continuation included).
                    at += size;
                    utf16 += units;
                    if let Some(escaped) = text[at..].chars().next() {
                        at += escaped.len_utf8();
                        utf16 += escaped.len_utf16() as u32;
                    }
                } else if !raw && c == '$' {
                    if text[at + size..].as_bytes().first() == Some(&b'{') {
                        at += size + 1;
                        utf16 += units + 1;
                        stack.push(TextRegion::Code { braces: 0 });
                    } else {
                        // `$identifier`: the identifier's chars ride the
                        // literal region (no bidi char is an identifier
                        // char, so none of them match).
                        at += size;
                        utf16 += units;
                        while let Some(next) = text[at..].chars().next() {
                            if next.is_ascii_alphanumeric() || next == '_' || next == '$' {
                                at += next.len_utf8();
                                utf16 += next.len_utf16() as u32;
                            } else {
                                break;
                            }
                        }
                    }
                } else if c as u32 == u32::from(quote) {
                    let closing = if triple {
                        let rest = text[at + size..].as_bytes();
                        rest.first() == Some(&quote) && rest.get(1) == Some(&quote)
                    } else {
                        true
                    };
                    let span = if triple { 3 * size } else { size };
                    let span_units = if triple { 3 * units } else { units };
                    at += span;
                    utf16 += span_units;
                    if closing {
                        stack.pop();
                    }
                } else {
                    at += size;
                    utf16 += units;
                }
            }
            TextRegion::LineComment => {
                if c == '\n' {
                    stack.pop();
                }
                at += size;
                utf16 += units;
            }
            TextRegion::BlockComment { depth } => {
                let rest = text[at + size..].as_bytes();
                let (opener, closer) = (
                    c == '/' && rest.first() == Some(&b'*'),
                    c == '*' && rest.first() == Some(&b'/'),
                );
                if opener || closer {
                    at += 2 * size;
                    utf16 += 2 * units;
                    match (opener, depth) {
                        (true, _) => stack.push(TextRegion::BlockComment { depth: depth + 1 }),
                        (false, 1) => {
                            stack.pop();
                        }
                        (false, _) => {
                            stack.pop();
                            stack.push(TextRegion::BlockComment { depth: depth - 1 });
                        }
                    }
                } else {
                    at += size;
                    utf16 += units;
                }
            }
            TextRegion::Code { braces } => {
                let next = text[at + size..].as_bytes().first().copied();
                match c {
                    '/' if next == Some(b'/') => {
                        at += 2 * size;
                        utf16 += 2 * units;
                        stack.push(TextRegion::LineComment);
                    }
                    '/' if next == Some(b'*') => {
                        at += 2 * size;
                        utf16 += 2 * units;
                        stack.push(TextRegion::BlockComment { depth: 1 });
                    }
                    '\'' | '"' => {
                        let quote = c as u8;
                        let rest = text[at + size..].as_bytes();
                        let triple = rest.first() == Some(&quote) && rest.get(1) == Some(&quote);
                        let span = if triple { 3 * size } else { size };
                        let span_units = if triple { 3 * units } else { units };
                        at += span;
                        utf16 += span_units;
                        stack.push(TextRegion::Str {
                            quote,
                            triple,
                            raw: false,
                        });
                    }
                    // The raw prefix only opens a literal when a quote
                    // follows directly (`r'...'`); a bare `r` identifier
                    // falls through the catch-all.
                    'r' if matches!(next, Some(b'\'' | b'"')) => {
                        let quote = next.expect("matched above");
                        let rest = text[at + 2 * size..].as_bytes();
                        let triple = rest.first() == Some(&quote) && rest.get(1) == Some(&quote);
                        let span = if triple { 3 * size } else { 2 * size };
                        let span_units = if triple { 3 * units } else { 2 * units };
                        at += span;
                        utf16 += span_units;
                        stack.push(TextRegion::Str {
                            quote,
                            triple,
                            raw: true,
                        });
                    }
                    '{' => {
                        if let Some(TextRegion::Code { braces }) = stack.last_mut() {
                            *braces += 1;
                        }
                        at += size;
                        utf16 += units;
                    }
                    '}' => {
                        match (braces, stack.len()) {
                            // The `}` closing an interpolation pops back to
                            // the owning string literal.
                            (0, n) if n > 1 => {
                                stack.pop();
                            }
                            (0, _) => {}
                            _ => {
                                if let Some(TextRegion::Code { braces }) = stack.last_mut() {
                                    *braces -= 1;
                                }
                            }
                        }
                        at += size;
                        utf16 += units;
                    }
                    _ => {
                        at += size;
                        utf16 += units;
                    }
                }
            }
        }
    }
    issues
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
pub(crate) mod known_error_codes {
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
        ("deprecated_new_in_comment_reference", Severity::Warning),
        ("deprecated_optional", Severity::Warning),
        ("deprecated_subclass", Severity::Warning),
        ("deprecated_subtype_of_function", Severity::Warning),
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
        ("doc_import_cannot_have_combinators", Severity::Warning),
        ("doc_import_cannot_have_configurations", Severity::Warning),
        ("doc_import_cannot_have_prefix", Severity::Warning),
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
        ("non_constant_annotation_constructor", Severity::Error),
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
        ("plugins_in_inner_options", Severity::Warning),
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
        ("undefined_annotation", Severity::Error),
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

    /// The registry's `uniqueName` spellings that diverge from the `name`
    /// field — 225 `(uniqueName, name)` rows over the same 1160-constant
    /// registry the severity table folds (version-pinned data like
    /// [`SEVERITIES`]; the two spellings agree on the other 935 constants).
    /// Sorted by `uniqueName`; every `name` column value is a
    /// [`SEVERITIES`] name.
    ///
    /// These are NOT `analyzer.errors` keys (the `codeNames` universe is the
    /// `name` field only — see [`is_known_diagnostic_code`]'s pins). The
    /// consumer is the ignore-comment citation match: a citation suppresses
    /// a diagnostic whose `lowerCaseName` OR `lowerCaseUniqueName` it equals
    /// (`IgnoredDiagnosticName._matches`, upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/ignore_comments/ignore_info.dart`:39-52) —
    /// e.g. `invalid_null_aware_operator_after_short_circuit` (the
    /// `uniqueName` of the second `invalid_null_aware_operator` constant)
    /// suppresses and is never `unnecessary_ignore`-reported
    /// (`_validDiagnosticCodeNames`, ignore_validator.dart:19-25, is the
    /// name universe — :208-213 skips such citations at report time), while
    /// the same spelling is rejected in `analyzer.errors`.
    pub const UNIQUE_NAME_ALIASES: &[(&str, &str)] = &[
        (
            "abstract_field_constructor_initializer",
            "abstract_field_initializer",
        ),
        (
            "ambiguous_extension_member_access_three_or_more",
            "ambiguous_extension_member_access",
        ),
        (
            "ambiguous_extension_member_access_two",
            "ambiguous_extension_member_access",
        ),
        (
            "analysis_option_deprecated_with_replacement",
            "analysis_option_deprecated",
        ),
        (
            "augmentation_induced_getter_already_complete",
            "declaration_already_complete",
        ),
        (
            "augmentation_induced_getter_return_type_mismatch",
            "augmentation_return_type_mismatch",
        ),
        (
            "augmentation_induced_setter_already_complete",
            "declaration_already_complete",
        ),
        (
            "augmentation_without_getter_declaration",
            "augmentation_without_declaration",
        ),
        (
            "augmentation_without_setter_declaration",
            "augmentation_without_declaration",
        ),
        (
            "base_class_implemented_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "base_mixin_implemented_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "built_in_identifier_as_extension_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "built_in_identifier_as_extension_type_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "built_in_identifier_as_prefix_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "built_in_identifier_as_type_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "built_in_identifier_as_type_parameter_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "built_in_identifier_as_typedef_name",
            "built_in_identifier_in_declaration",
        ),
        (
            "class_instantiation_access_to_instance_member",
            "class_instantiation_access_to_member",
        ),
        (
            "class_instantiation_access_to_static_member",
            "class_instantiation_access_to_member",
        ),
        (
            "class_instantiation_access_to_unknown_member",
            "class_instantiation_access_to_member",
        ),
        (
            "conflicting_constructor_and_static_field",
            "conflicting_constructor_and_static_member",
        ),
        (
            "conflicting_constructor_and_static_getter",
            "conflicting_constructor_and_static_member",
        ),
        (
            "conflicting_constructor_and_static_method",
            "conflicting_constructor_and_static_member",
        ),
        (
            "conflicting_constructor_and_static_setter",
            "conflicting_constructor_and_static_member",
        ),
        (
            "conflicting_type_variable_and_class",
            "conflicting_type_variable_and_container",
        ),
        (
            "conflicting_type_variable_and_enum",
            "conflicting_type_variable_and_container",
        ),
        (
            "conflicting_type_variable_and_extension",
            "conflicting_type_variable_and_container",
        ),
        (
            "conflicting_type_variable_and_extension_type",
            "conflicting_type_variable_and_container",
        ),
        (
            "conflicting_type_variable_and_member_class",
            "conflicting_type_variable_and_member",
        ),
        (
            "conflicting_type_variable_and_member_enum",
            "conflicting_type_variable_and_member",
        ),
        (
            "conflicting_type_variable_and_member_extension",
            "conflicting_type_variable_and_member",
        ),
        (
            "conflicting_type_variable_and_member_extension_type",
            "conflicting_type_variable_and_member",
        ),
        (
            "conflicting_type_variable_and_member_mixin",
            "conflicting_type_variable_and_member",
        ),
        (
            "conflicting_type_variable_and_mixin",
            "conflicting_type_variable_and_container",
        ),
        (
            "const_constructor_constant_from_deferred_library",
            "collection_element_from_deferred_library",
        ),
        (
            "const_constructor_with_mixin_with_fields",
            "const_constructor_with_mixin_with_field",
        ),
        (
            "const_field_initializer_not_assignable",
            "field_initializer_not_assignable",
        ),
        (
            "const_primary_constructor_with_block_body",
            "const_primary_constructor_with_body",
        ),
        (
            "const_primary_constructor_with_expression_body",
            "const_primary_constructor_with_body",
        ),
        ("const_with_non_type", "creation_with_non_type"),
        (
            "const_with_type_parameters_constructor_tearoff",
            "const_with_type_parameters",
        ),
        (
            "const_with_type_parameters_function_tearoff",
            "const_with_type_parameters",
        ),
        (
            "constructor_already_complete",
            "declaration_already_complete",
        ),
        ("dead_code_late_wildcard_variable_initializer", "dead_code"),
        (
            "deprecated_extends_function",
            "deprecated_subtype_of_function",
        ),
        (
            "deprecated_implements_function",
            "deprecated_subtype_of_function",
        ),
        (
            "deprecated_member_use_implicit_super_constructor_invocation",
            "deprecated_member_use",
        ),
        (
            "deprecated_member_use_implicit_super_constructor_invocation_with_message",
            "deprecated_member_use",
        ),
        (
            "deprecated_member_use_implicit_type",
            "deprecated_member_use",
        ),
        (
            "deprecated_member_use_implicit_type_with_message",
            "deprecated_member_use",
        ),
        (
            "deprecated_member_use_with_message",
            "deprecated_member_use",
        ),
        (
            "deprecated_mixin_function",
            "deprecated_subtype_of_function",
        ),
        (
            "doc_directive_missing_one_argument",
            "doc_directive_missing_argument",
        ),
        (
            "doc_directive_missing_three_arguments",
            "doc_directive_missing_argument",
        ),
        (
            "doc_directive_missing_two_arguments",
            "doc_directive_missing_argument",
        ),
        (
            "dot_shorthand_undefined_getter",
            "dot_shorthand_undefined_member",
        ),
        (
            "dot_shorthand_undefined_invocation",
            "dot_shorthand_undefined_member",
        ),
        ("duplicate_constructor_default", "duplicate_constructor"),
        ("duplicate_constructor_name", "duplicate_constructor"),
        ("expected_catch_clause_body", "expected_body"),
        ("expected_class_body", "expected_body"),
        ("expected_extension_body", "expected_body"),
        ("expected_extension_type_body", "expected_body"),
        ("expected_finally_clause_body", "expected_body"),
        ("expected_mixin_body", "expected_body"),
        ("expected_named_type_extends", "expected_named_type"),
        ("expected_named_type_implements", "expected_named_type"),
        ("expected_named_type_on", "expected_named_type"),
        ("expected_named_type_with", "expected_named_type"),
        ("expected_switch_expression_body", "expected_body"),
        ("expected_switch_statement_body", "expected_body"),
        ("expected_try_statement_body", "expected_body"),
        ("extends_deferred_class", "subtype_of_deferred_class"),
        ("extends_disallowed_class", "subtype_of_disallowed_type"),
        (
            "extends_type_alias_expands_to_type_parameter",
            "supertype_expands_to_type_parameter",
        ),
        (
            "external_field_constructor_initializer",
            "external_with_initializer",
        ),
        ("external_field_initializer", "external_with_initializer"),
        ("external_variable_initializer", "external_with_initializer"),
        (
            "final_class_extended_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "final_class_implemented_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "final_class_used_as_mixin_constraint_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "final_not_initialized_constructor_1",
            "final_not_initialized_constructor",
        ),
        (
            "final_not_initialized_constructor_2",
            "final_not_initialized_constructor",
        ),
        (
            "final_not_initialized_constructor_3_plus",
            "final_not_initialized_constructor",
        ),
        ("function_already_complete", "declaration_already_complete"),
        (
            "illegal_concrete_enum_member_declaration",
            "illegal_concrete_enum_member",
        ),
        (
            "illegal_concrete_enum_member_inheritance",
            "illegal_concrete_enum_member",
        ),
        ("illegal_enum_values_declaration", "illegal_enum_values"),
        ("illegal_enum_values_inheritance", "illegal_enum_values"),
        ("implements_deferred_class", "subtype_of_deferred_class"),
        ("implements_disallowed_class", "subtype_of_disallowed_type"),
        (
            "implements_type_alias_expands_to_type_parameter",
            "supertype_expands_to_type_parameter",
        ),
        ("incompatible_lint_files", "incompatible_lint"),
        ("incompatible_lint_included", "incompatible_lint"),
        (
            "instance_access_to_static_member_of_unnamed_extension",
            "instance_access_to_static_member",
        ),
        (
            "interface_class_extended_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        ("invalid_field_name_from_object", "invalid_field_name"),
        ("invalid_field_name_positional", "invalid_field_name"),
        ("invalid_field_name_private", "invalid_field_name"),
        (
            "invalid_implementation_override_setter",
            "invalid_implementation_override",
        ),
        (
            "invalid_language_version_override_at_sign",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_equals",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_greater",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_location",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_lower_case",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_number",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_prefix",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_trailing_characters",
            "invalid_language_version_override",
        ),
        (
            "invalid_language_version_override_two_slashes",
            "invalid_language_version_override",
        ),
        ("invalid_null_aware_element", "invalid_null_aware_operator"),
        (
            "invalid_null_aware_map_entry_key",
            "invalid_null_aware_operator",
        ),
        (
            "invalid_null_aware_map_entry_value",
            "invalid_null_aware_operator",
        ),
        (
            "invalid_null_aware_operator_after_short_circuit",
            "invalid_null_aware_operator",
        ),
        ("invalid_override_setter", "invalid_override"),
        (
            "invalid_reference_to_generative_enum_constructor_tearoff",
            "invalid_reference_to_generative_enum_constructor",
        ),
        (
            "invalid_type_argument_in_const_list",
            "invalid_type_argument_in_const_literal",
        ),
        (
            "invalid_type_argument_in_const_map",
            "invalid_type_argument_in_const_literal",
        ),
        (
            "invalid_type_argument_in_const_set",
            "invalid_type_argument_in_const_literal",
        ),
        (
            "list_element_type_not_assignable_nullability",
            "list_element_type_not_assignable",
        ),
        (
            "map_key_type_not_assignable_nullability",
            "map_key_type_not_assignable",
        ),
        (
            "map_value_type_not_assignable_nullability",
            "map_value_type_not_assignable",
        ),
        (
            "missing_default_value_for_parameter_positional",
            "missing_default_value_for_parameter",
        ),
        (
            "missing_default_value_for_parameter_with_annotation",
            "missing_default_value_for_parameter",
        ),
        (
            "missing_override_of_must_be_overridden_one",
            "missing_override_of_must_be_overridden",
        ),
        (
            "missing_override_of_must_be_overridden_three_plus",
            "missing_override_of_must_be_overridden",
        ),
        (
            "missing_override_of_must_be_overridden_two",
            "missing_override_of_must_be_overridden",
        ),
        (
            "missing_required_param_with_details",
            "missing_required_param",
        ),
        (
            "mixin_application_no_concrete_super_invoked_setter",
            "mixin_application_no_concrete_super_invoked_member",
        ),
        ("mixin_deferred_class", "subtype_of_deferred_class"),
        ("mixin_of_disallowed_class", "subtype_of_disallowed_type"),
        (
            "mixin_of_type_alias_expands_to_type_parameter",
            "supertype_expands_to_type_parameter",
        ),
        (
            "mixin_on_type_alias_expands_to_type_parameter",
            "supertype_expands_to_type_parameter",
        ),
        (
            "mixin_subtype_of_base_is_not_base",
            "subtype_of_base_or_final_is_not_base_final_or_sealed",
        ),
        (
            "mixin_subtype_of_final_is_not_base",
            "subtype_of_base_or_final_is_not_base_final_or_sealed",
        ),
        (
            "mixin_super_class_constraint_disallowed_class",
            "subtype_of_disallowed_type",
        ),
        ("mixins_super_class", "implements_super_class"),
        ("multiple_combinators_deprecated", "multiple_combinators"),
        ("new_with_non_type", "creation_with_non_type"),
        (
            "no_default_super_constructor_explicit",
            "no_default_super_constructor",
        ),
        (
            "no_default_super_constructor_implicit",
            "no_default_super_constructor",
        ),
        (
            "non_abstract_class_inherits_abstract_member_five_plus",
            "non_abstract_class_inherits_abstract_member",
        ),
        (
            "non_abstract_class_inherits_abstract_member_four",
            "non_abstract_class_inherits_abstract_member",
        ),
        (
            "non_abstract_class_inherits_abstract_member_one",
            "non_abstract_class_inherits_abstract_member",
        ),
        (
            "non_abstract_class_inherits_abstract_member_three",
            "non_abstract_class_inherits_abstract_member",
        ),
        (
            "non_abstract_class_inherits_abstract_member_two",
            "non_abstract_class_inherits_abstract_member",
        ),
        (
            "non_const_call_to_literal_constructor_using_new",
            "non_const_call_to_literal_constructor",
        ),
        (
            "non_constant_list_element_from_deferred_library",
            "collection_element_from_deferred_library",
        ),
        (
            "non_constant_map_key_from_deferred_library",
            "collection_element_from_deferred_library",
        ),
        (
            "non_constant_map_value_from_deferred_library",
            "collection_element_from_deferred_library",
        ),
        (
            "non_exhaustive_switch_expression_private",
            "non_exhaustive_switch_expression",
        ),
        (
            "non_exhaustive_switch_statement_private",
            "non_exhaustive_switch_statement",
        ),
        (
            "not_enough_positional_arguments_name_plural",
            "not_enough_positional_arguments",
        ),
        (
            "not_enough_positional_arguments_name_singular",
            "not_enough_positional_arguments",
        ),
        (
            "not_enough_positional_arguments_plural",
            "not_enough_positional_arguments",
        ),
        (
            "not_enough_positional_arguments_singular",
            "not_enough_positional_arguments",
        ),
        (
            "not_initialized_non_nullable_instance_field_constructor",
            "not_initialized_non_nullable_instance_field",
        ),
        (
            "override_on_non_overriding_field",
            "override_on_non_overriding_member",
        ),
        (
            "override_on_non_overriding_getter",
            "override_on_non_overriding_member",
        ),
        (
            "override_on_non_overriding_method",
            "override_on_non_overriding_member",
        ),
        (
            "override_on_non_overriding_setter",
            "override_on_non_overriding_member",
        ),
        (
            "pattern_variable_shared_case_scope_different_finality_or_type",
            "invalid_pattern_variable_in_shared_case_scope",
        ),
        (
            "pattern_variable_shared_case_scope_has_label",
            "invalid_pattern_variable_in_shared_case_scope",
        ),
        (
            "pattern_variable_shared_case_scope_not_all_cases",
            "invalid_pattern_variable_in_shared_case_scope",
        ),
        (
            "record_literal_one_positional_no_trailing_comma_by_type",
            "record_literal_one_positional_no_trailing_comma",
        ),
        (
            "recursive_factory_redirect",
            "recursive_constructor_redirect",
        ),
        (
            "recursive_interface_inheritance_extends",
            "recursive_interface_inheritance",
        ),
        (
            "recursive_interface_inheritance_implements",
            "recursive_interface_inheritance",
        ),
        (
            "recursive_interface_inheritance_on",
            "recursive_interface_inheritance",
        ),
        (
            "recursive_interface_inheritance_with",
            "recursive_interface_inheritance",
        ),
        (
            "return_of_invalid_type_from_catch_error",
            "invalid_return_type_for_catch_error",
        ),
        (
            "return_of_invalid_type_from_constructor",
            "return_of_invalid_type",
        ),
        (
            "return_of_invalid_type_from_function",
            "return_of_invalid_type",
        ),
        (
            "return_of_invalid_type_from_method",
            "return_of_invalid_type",
        ),
        (
            "return_of_invalid_type_from_then",
            "invalid_return_type_for_then",
        ),
        (
            "return_type_invalid_for_catch_error",
            "invalid_return_type_for_catch_error",
        ),
        (
            "return_type_invalid_for_then",
            "invalid_return_type_for_then",
        ),
        (
            "sealed_class_subtype_outside_of_library",
            "invalid_use_of_type_outside_library",
        ),
        (
            "set_element_from_deferred_library",
            "collection_element_from_deferred_library",
        ),
        (
            "set_element_type_not_assignable_nullability",
            "set_element_type_not_assignable",
        ),
        (
            "subtype_of_base_is_not_base_final_or_sealed",
            "subtype_of_base_or_final_is_not_base_final_or_sealed",
        ),
        (
            "subtype_of_final_is_not_base_final_or_sealed",
            "subtype_of_base_or_final_is_not_base_final_or_sealed",
        ),
        (
            "subtype_of_struct_class_in_extends",
            "subtype_of_struct_class",
        ),
        (
            "subtype_of_struct_class_in_implements",
            "subtype_of_struct_class",
        ),
        ("subtype_of_struct_class_in_with", "subtype_of_struct_class"),
        ("type_check_is_not_null", "type_check_with_null"),
        ("type_check_is_null", "type_check_with_null"),
        (
            "unchecked_invocation_of_nullable_value",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_method_invocation_of_nullable_value",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_operator_invocation_of_nullable_value",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_property_access_of_nullable_value",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_use_of_nullable_value_as_condition",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_use_of_nullable_value_as_iterator",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_use_of_nullable_value_in_spread",
            "unchecked_use_of_nullable_value",
        ),
        (
            "unchecked_use_of_nullable_value_in_yield_each",
            "unchecked_use_of_nullable_value",
        ),
        ("undefined_class_boolean", "undefined_class"),
        (
            "undefined_constructor_in_initializer_default",
            "undefined_constructor_in_initializer",
        ),
        (
            "undefined_enum_constructor_named",
            "undefined_enum_constructor",
        ),
        (
            "undefined_enum_constructor_unnamed",
            "undefined_enum_constructor",
        ),
        ("undefined_getter_on_function_type", "undefined_getter"),
        ("undefined_method_on_function_type", "undefined_method"),
        ("undefined_method_on_type_literal", "undefined_method"),
        ("undefined_setter_on_function_type", "undefined_setter"),
        ("undefined_super_getter", "undefined_super_member"),
        ("undefined_super_method", "undefined_super_member"),
        ("undefined_super_operator", "undefined_super_member"),
        ("undefined_super_setter", "undefined_super_member"),
        (
            "unnecessary_nan_comparison_false",
            "unnecessary_nan_comparison",
        ),
        (
            "unnecessary_nan_comparison_true",
            "unnecessary_nan_comparison",
        ),
        (
            "unnecessary_null_comparison_always_null_false",
            "unnecessary_null_comparison",
        ),
        (
            "unnecessary_null_comparison_always_null_true",
            "unnecessary_null_comparison",
        ),
        (
            "unnecessary_null_comparison_never_null_false",
            "unnecessary_null_comparison",
        ),
        (
            "unnecessary_null_comparison_never_null_true",
            "unnecessary_null_comparison",
        ),
        ("unnecessary_type_check_false", "unnecessary_type_check"),
        ("unnecessary_type_check_true", "unnecessary_type_check"),
        ("unsupported_option_with_legal_value", "unsupported_option"),
        ("unsupported_option_with_legal_values", "unsupported_option"),
        ("unsupported_option_without_values", "unsupported_option"),
        ("unused_result_with_message", "unused_result"),
        ("use_of_private_parameter_name", "undefined_named_parameter"),
        (
            "wrong_number_of_parameters_for_operator_minus",
            "wrong_number_of_parameters_for_operator",
        ),
        (
            "wrong_number_of_type_arguments_dot_shorthand_constructor",
            "wrong_number_of_type_arguments_constructor",
        ),
        ("yield_each_in_non_generator", "yield_in_non_generator"),
        ("yield_each_of_invalid_type", "yield_of_invalid_type"),
    ];

    /// The registry `name` a `uniqueName` spelling resolves to, or `None`
    /// when the spelling is not a registry `uniqueName`. Binary search over
    /// [`UNIQUE_NAME_ALIASES`] (sorted by the first column).
    pub fn canonical_name(unique: &str) -> Option<&'static str> {
        UNIQUE_NAME_ALIASES
            .binary_search_by(|&(spelling, _)| spelling.cmp(unique))
            .ok()
            .map(|index| UNIQUE_NAME_ALIASES[index].1)
    }
}

/// The registry names that existed but were removed, still accepted as
/// `analyzer.errors` keys so existing options files keep working
/// (`removedCodeNames`, parse_model:66-76 — "we don't want to report these,
/// this breaks clients"). Exactly the 3.13.7 pin's set; the two names the
/// 3.15-main face carried here (`plugins_in_inner_options`,
/// `undefined_annotation`) are live registry codes of the pin.
const REMOVED_ERROR_CODES: [&str; 1] = ["missing_return"];

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

/// The registry `name` a `uniqueName` spelling (lowercased, the citation
/// match's spelling) resolves to — `None` when it names nothing. The ignore
/// faces' companion to [`is_known_diagnostic_code`]: that one answers the
/// `analyzer.errors` key universe (the `name` field only), this one answers
/// the citation match's second leg (see
/// [`known_error_codes::UNIQUE_NAME_ALIASES`]).
#[must_use]
pub fn canonical_diagnostic_name(unique: &str) -> Option<&'static str> {
    known_error_codes::canonical_name(&unique.to_lowercase())
}

#[cfg(test)]
mod unique_name_alias_pins {
    use super::known_error_codes::{SEVERITIES, UNIQUE_NAME_ALIASES};

    /// The alias table walks clean against its upstream derivation: sorted
    /// by the `uniqueName` column (the `canonical_name` binary search's
    /// precondition), row count pinned at the registry's divergent
    /// constants (the 3.13.7 pin dropped the two
    /// `ambiguous_comment_reference` spellings with their code), and every
    /// canonical `name` resolves in the severity table's name column — an
    /// alias into a dead name would admit citations for codes the
    /// registry never declared.
    #[test]
    fn the_unique_name_alias_table_walks_upstream_clean() {
        assert_eq!(UNIQUE_NAME_ALIASES.len(), 223);
        let mut names = SEVERITIES.iter().map(|&(name, _)| name).collect::<Vec<_>>();
        names.sort_unstable();
        for window in UNIQUE_NAME_ALIASES.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "alias column unsorted at {}: {}",
                window[0].0,
                window[1].0
            );
        }
        for &(unique, name) in UNIQUE_NAME_ALIASES {
            assert_ne!(unique, name, "self-alias row {unique}");
            assert!(
                names.binary_search(&name).is_ok(),
                "alias {unique} resolves to unlisted name {name}"
            );
        }
    }

    /// The dispatch's case (#2131): the second `invalid_null_aware_operator`
    /// constant's `uniqueName` spelling resolves to the shared `name` — the
    /// spelling the unnecessary_ignore probe must admit and the citation
    /// match must canonicalize — while the name-universe face keeps
    /// rejecting it as an `analyzer.errors` key.
    #[test]
    fn the_after_short_circuit_spelling_resolves_to_its_shared_name() {
        use super::{canonical_diagnostic_name, is_known_diagnostic_code};
        assert_eq!(
            canonical_diagnostic_name("invalid_null_aware_operator_after_short_circuit"),
            Some("invalid_null_aware_operator")
        );
        assert!(!is_known_diagnostic_code(
            "invalid_null_aware_operator_after_short_circuit"
        ));
        assert!(is_known_diagnostic_code("invalid_null_aware_operator"));
    }
}

#[cfg(test)]
mod pubspec_manifest_pins {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway workspace root — the facade fixture's temp-dir posture,
    /// one unique directory per call, cleaned by the OS.
    fn temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-pubspec-pin-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root dir");
        root
    }

    /// Writes one fixture file (and its parent directories) under `root`.
    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, content).expect("file");
    }

    /// The UTF-16 offset of one 1-based line/column position (the
    /// expected.tsv face; every pin fixture is ASCII, so bytes and code
    /// units agree).
    fn at(text: &str, line: usize, col: usize) -> u32 {
        let mut offset = 0_u32;
        for (index, line_text) in text.split('\n').enumerate() {
            if index + 1 == line {
                return offset + col as u32 - 1;
            }
            offset += line_text.len() as u32 + 1;
        }
        unreachable!("line {line} past the fixture");
    }

    /// One row's full pin: code, message, span.
    fn assert_row(issue: &OptionsIssue, code: &str, message: &str, offset: u32, length: u32) {
        assert_eq!(issue.code.name, code, "code of {message}");
        assert_eq!(issue.code.dtype, DiagnosticType::StaticWarning);
        assert_eq!(issue.message, message);
        assert_eq!(issue.offset, offset, "offset of {code}");
        assert_eq!(issue.length, length, "length of {code}");
    }

    fn codes(issues: &[OptionsIssue]) -> Vec<&str> {
        issues.iter().map(|issue| issue.code.name).collect()
    }

    #[test]
    fn the_dependency_validator_pins_its_six_codes() {
        // path_does_not_exist /1 — `missing_pkg` resolves nowhere.
        let text = "name: test\ndependencies:\n  foo:\n    path: missing_pkg\n";
        let issues = pubspec_issues(text, &temp_root("path-miss"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "path_does_not_exist",
            "The path 'missing_pkg' doesn't exist.",
            at(text, 4, 11),
            11,
        );
        // path_does_not_exist /2 — the folder exists with a pubspec.
        let root = temp_root("path-hit");
        write(&root, "real_dep/pubspec.yaml", "name: real_dep\n");
        let text = "name: test\ndependencies:\n  foo:\n    path: real_dep\n";
        assert!(pubspec_issues(text, &root).is_empty());
        // path_not_posix /1 — the backslash form reports at the raw span
        // (quotes included) and skips the existence walk.
        let text = "name: test\ndependencies:\n  foo:\n    path: 'sub\\dir'\n";
        let issues = pubspec_issues(text, &temp_root("posix"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "path_not_posix",
            "The path 'sub\\dir' isn't a POSIX-style path.",
            at(text, 4, 11),
            9,
        );
        // path_pubspec_does_not_exist /1 — the folder exists bare.
        let root = temp_root("pubspec-miss");
        std::fs::create_dir_all(root.join("bare_dir")).expect("bare dir");
        let text = "name: test\ndependencies:\n  foo:\n    path: bare_dir\n";
        let issues = pubspec_issues(text, &root);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "path_pubspec_does_not_exist",
            "The directory 'bare_dir' doesn't contain a pubspec.",
            at(text, 4, 11),
            8,
        );
        // dependencies_field_not_map /1 — a scalar section; /2 an empty
        // flow map stays silent.
        let text = "name: test\ndependencies: not_a_map\n";
        let issues = pubspec_issues(text, &temp_root("deps-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "dependencies_field_not_map",
            "The value of the 'dependencies' field is expected to be a map.",
            at(text, 2, 15),
            9,
        );
        assert!(
            pubspec_issues("name: test\ndependencies: {}\n", &temp_root("deps-flow")).is_empty()
        );
        // invalid_dependency /1 — publishable (a version, no
        // `publish_to: none`) with a git dependency reports at the key;
        // /2 — no version, not publishable, silent.
        let text = "name: test\nversion: 1.0.0\ndependencies:\n  foo:\n    git:\n      url: https://github.com/example/foo\n";
        let issues = pubspec_issues(text, &temp_root("invalid-dep"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "invalid_dependency",
            "Publishable packages can't have 'git' dependencies.",
            at(text, 5, 5),
            3,
        );
        let text = "name: test\ndependencies:\n  foo:\n    git:\n      url: https://github.com/example/foo\n";
        assert!(pubspec_issues(text, &temp_root("invalid-dep-neg")).is_empty());
        // unnecessary_dev_dependency /1 — duplicated in both sections; /2
        // dev-only stays silent.
        let text = "name: test\ndependencies:\n  aaa: any\ndev_dependencies:\n  aaa: any\n";
        let issues = pubspec_issues(text, &temp_root("dev-dep"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "unnecessary_dev_dependency",
            "The dev dependency on aaa is unnecessary because there is also a normal \
             dependency on that package.",
            at(text, 5, 3),
            3,
        );
        let text = "name: test\ndev_dependencies:\n  aaa: any\n";
        assert!(pubspec_issues(text, &temp_root("dev-dep-neg")).is_empty());
    }

    #[test]
    fn the_field_and_name_validators_pin_their_three_codes() {
        // deprecated_field /1 — `author` reports at its key; /2 a
        // non-deprecated field stays silent.
        let text = "name: test\nauthor: someone\n";
        let issues = pubspec_issues(text, &temp_root("deprecated"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "deprecated_field",
            "The 'author' field is no longer used and can be removed.",
            at(text, 2, 1),
            6,
        );
        assert!(
            pubspec_issues(
                "name: test\nmaintainer: someone\n",
                &temp_root("deprecated-neg")
            )
            .is_empty()
        );
        // missing_name /1 — an empty document reports at the origin; /2 a
        // named document stays silent.
        let issues = pubspec_issues("", &temp_root("missing-name"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "missing_name",
            "The 'name' field is required but missing.",
            0,
            0,
        );
        assert!(pubspec_issues("name: test\n", &temp_root("missing-name-neg")).is_empty());
        // name_not_string /1 — an int scalar is not a string; a quoted
        // spelling would be.
        let text = "name: 42\n";
        let issues = pubspec_issues(text, &temp_root("name-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "name_not_string",
            "The value of the 'name' field is required to be a string.",
            at(text, 1, 7),
            2,
        );
    }

    #[test]
    fn the_flutter_validator_pins_its_eight_codes() {
        // flutter_field_not_map /1 — a non-empty scalar section; /2 the
        // empty section stays silent.
        let text = "name: test\nflutter: not_a_map\n";
        let issues = pubspec_issues(text, &temp_root("flutter-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "flutter_field_not_map",
            "The value of the 'flutter' field is expected to be a map.",
            at(text, 2, 10),
            9,
        );
        assert!(pubspec_issues("name: test\nflutter:\n", &temp_root("flutter-neg")).is_empty());
        // asset_field_not_list /1 — a scalar assets value; /2 an empty
        // list stays silent.
        let text = "name: test\nflutter:\n  assets: not_a_list\n";
        let issues = pubspec_issues(text, &temp_root("assets-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_field_not_list",
            "The value of the 'assets' field is expected to be a list of relative file paths.",
            at(text, 3, 11),
            10,
        );
        assert!(
            pubspec_issues(
                "name: test\nflutter:\n  assets: []\n",
                &temp_root("assets-neg")
            )
            .is_empty()
        );
        // asset_directory_does_not_exist /1 — the trailing-slash entry
        // with no folder; /2 the folder present.
        let text = "name: test\nflutter:\n  assets:\n    - missing_dir/\n";
        let issues = pubspec_issues(text, &temp_root("asset-dir"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_directory_does_not_exist",
            "The asset directory 'missing_dir/' doesn't exist.",
            at(text, 4, 7),
            12,
        );
        let root = temp_root("asset-dir-neg");
        write(&root, "assets/keep.txt", "x");
        assert!(
            pubspec_issues("name: test\nflutter:\n  assets:\n    - assets/\n", &root).is_empty()
        );
        // asset_does_not_exist /1 — the file entry missing; /2 present.
        let text = "name: test\nflutter:\n  assets:\n    - missing.png\n";
        let issues = pubspec_issues(text, &temp_root("asset-file"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_does_not_exist",
            "The asset file 'missing.png' doesn't exist.",
            at(text, 4, 7),
            11,
        );
        let root = temp_root("asset-file-neg");
        write(&root, "logo.png", "x");
        assert!(
            pubspec_issues("name: test\nflutter:\n  assets:\n    - logo.png\n", &root).is_empty()
        );
        // asset_missing_path /1 — the map entry without `path:` reports at
        // the map node's span; /2 the map with an existing path.
        let text = "name: test\nflutter:\n  assets:\n    - source: images/x.png\n";
        let issues = pubspec_issues(text, &temp_root("asset-map"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_missing_path",
            "Asset map entry must contain a 'path' field.",
            at(text, 4, 7),
            20,
        );
        let root = temp_root("asset-map-neg");
        write(&root, "images/x.png", "x");
        assert!(
            pubspec_issues(
                "name: test\nflutter:\n  assets:\n    - path: images/x.png\n",
                &root
            )
            .is_empty()
        );
        // asset_not_string /1 — an int `path:`; /2 the string form exists.
        let text = "name: test\nflutter:\n  assets:\n    - path: 42\n";
        let issues = pubspec_issues(text, &temp_root("asset-path-kind"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_not_string",
            "Assets are required to be file paths (strings).",
            at(text, 4, 13),
            2,
        );
        // asset_path_not_string /1 — a list `path:` reports at the flow
        // list's span; /2 the scalar string form exists.
        let text = "name: test\nflutter:\n  assets:\n    - path: [a]\n";
        let issues = pubspec_issues(text, &temp_root("asset-path-flow"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_path_not_string",
            "Asset paths are required to be file paths (strings).",
            at(text, 4, 13),
            3,
        );
        // asset_not_string_or_map /1 — an int entry; /2 a string entry.
        let text = "name: test\nflutter:\n  assets:\n    - 42\n";
        let issues = pubspec_issues(text, &temp_root("asset-kind"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "asset_not_string_or_map",
            "An asset value is required to be a file path (string) or map.",
            at(text, 4, 7),
            2,
        );
        let root = temp_root("asset-kind-neg");
        write(&root, "logo.png", "x");
        assert!(
            pubspec_issues("name: test\nflutter:\n  assets:\n    - logo.png\n", &root).is_empty()
        );
    }

    #[test]
    fn the_platforms_and_workspace_validators_pin_their_six_codes() {
        // invalid_platforms_field /1 — a scalar section; /2 the map form.
        let text = "name: test\nplatforms: not_a_map\n";
        let issues = pubspec_issues(text, &temp_root("platforms-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "invalid_platforms_field",
            "The 'platforms' field must be a map with platforms as keys.",
            at(text, 2, 12),
            9,
        );
        assert!(
            pubspec_issues(
                "name: test\nplatforms:\n  android:\n",
                &temp_root("platforms-neg")
            )
            .is_empty()
        );
        // unknown_platform /1 — an unrecognized key; /2 a known one.
        let text = "name: test\nplatforms:\n  brower:\n";
        let issues = pubspec_issues(text, &temp_root("platform-unknown"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "unknown_platform",
            "The platform 'brower' is not a recognized platform.",
            at(text, 3, 3),
            6,
        );
        assert!(
            pubspec_issues(
                "name: test\nplatforms:\n  linux:\n",
                &temp_root("platform-unknown-neg")
            )
            .is_empty()
        );
        // platform_value_disallowed /1 — a flow-map value; /2 the null
        // value stays silent.
        let text = "name: test\nplatforms:\n  android: {}\n";
        let issues = pubspec_issues(text, &temp_root("platform-value"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "platform_value_disallowed",
            "Keys in the `platforms` field can't have values.",
            at(text, 3, 12),
            2,
        );
        assert!(
            pubspec_issues(
                "name: test\nplatforms:\n  android:\n",
                &temp_root("platform-value-neg")
            )
            .is_empty()
        );
        // workspace_field_not_list /1 — a scalar section; /2 the list
        // form over an existing subdirectory.
        let text = "name: test\nworkspace: not_a_list\n";
        let issues = pubspec_issues(text, &temp_root("workspace-shape"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "workspace_field_not_list",
            "The value of the 'workspace' field is required to be a list of relative file paths.",
            at(text, 2, 12),
            10,
        );
        let root = temp_root("workspace-list");
        std::fs::create_dir_all(root.join("pkg")).expect("pkg dir");
        assert!(pubspec_issues("name: test\nworkspace:\n  - pkg\n", &root).is_empty());
        // workspace_value_not_string /1 — an int entry; /2 the string
        // form over an existing subdirectory.
        let text = "name: test\nworkspace:\n  - 42\n";
        let issues = pubspec_issues(text, &temp_root("workspace-kind"));
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "workspace_value_not_string",
            "Workspace entries are required to be directory paths (strings).",
            at(text, 3, 5),
            2,
        );
        // workspace_value_not_subdirectory /1 — the escaping path reports
        // at the raw span (quotes included) with the package root as the
        // argument; /2 the in-root form stays silent.
        let root = temp_root("workspace-escape");
        std::fs::create_dir_all(root.join("pkg")).expect("pkg dir");
        let text = "name: test\nworkspace:\n  - '../escape'\n";
        let issues = pubspec_issues(text, &root);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "workspace_value_not_subdirectory",
            &format!(
                "Workspace values must be a relative path of a subdirectory of '{}'.",
                root.display()
            ),
            at(text, 3, 5),
            11,
        );
        assert!(pubspec_issues("name: test\nworkspace:\n  - pkg\n", &root).is_empty());
    }

    #[test]
    fn the_screenshots_walk_reuses_the_path_row() {
        let text = "name: test\nscreenshots:\n  - path: missing.png\n";
        let issues = pubspec_issues(text, &temp_root("screenshots"));
        assert_eq!(codes(&issues), ["path_does_not_exist"]);
        assert_row(
            &issues[0],
            "path_does_not_exist",
            "The path 'missing.png' doesn't exist.",
            at(text, 3, 11),
            11,
        );
        let root = temp_root("screenshots-neg");
        write(&root, "shot.png", "x");
        assert!(pubspec_issues("name: test\nscreenshots:\n  - path: shot.png\n", &root).is_empty());
    }

    #[test]
    fn the_manifest_validator_pins_its_seven_codes() {
        let touchscreen = "    <uses-feature android:name=\"android.hardware.touchscreen\" android:required=\"false\" />\n";
        // camera_permissions_incompatible /1 — CAMERA without the camera
        // feature pair; /2 both features declared optional.
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-permission android:name=\"android.permission.CAMERA\" />\n</manifest>\n"
        );
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "camera_permissions_incompatible",
            "Camera permissions make app incompatible for Chrome OS, consider adding optional \
             features \"android.hardware.camera\" and \"android.hardware.camera.autofocus\".",
            at(&text, 3, 22),
            39,
        );
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-feature android:name=\"android.hardware.camera\" android:required=\"false\" />\n    \
             <uses-feature android:name=\"android.hardware.camera.autofocus\" \
             android:required=\"false\" />\n    \
             <uses-permission android:name=\"android.permission.CAMERA\" />\n</manifest>\n"
        );
        assert!(manifest_issues(&text, true).is_empty());
        // no_touchscreen_feature /1 — the feature absent, the row on the
        // manifest element's own span; /2 the feature optional.
        let text = "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n</manifest>\n";
        let issues = manifest_issues(text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "no_touchscreen_feature",
            "The default \"android.hardware.touchscreen\" needs to be optional for Chrome OS.",
            0,
            81,
        );
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}\
             </manifest>\n"
        );
        assert!(manifest_issues(&text, true).is_empty());
        // unsupported_chrome_os_hardware /1 — nfc without `required`; /2
        // with `required="false"`.
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-feature android:name=\"android.hardware.nfc\" />\n</manifest>\n"
        );
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "unsupported_chrome_os_hardware",
            "The feature android.hardware.nfc isn't supported on Chrome OS, consider making \
             it optional.",
            at(&text, 3, 19),
            34,
        );
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-feature android:name=\"android.hardware.nfc\" android:required=\"false\" />\n\
             </manifest>\n"
        );
        assert!(manifest_issues(&text, true).is_empty());
        // unsupported_chrome_os_feature /1 — telephony `required="true"`;
        // /2 optional.
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-feature android:name=\"android.hardware.telephony\" \
             android:required=\"true\" />\n</manifest>\n"
        );
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "unsupported_chrome_os_feature",
            "The feature android.hardware.telephony isn't supported on Chrome OS, consider \
             making it optional.",
            at(&text, 3, 19),
            40,
        );
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-feature android:name=\"android.hardware.telephony\" \
             android:required=\"false\" />\n</manifest>\n"
        );
        assert!(manifest_issues(&text, true).is_empty());
        // The activity rows share one fixture shape.
        let activity = |attribute: &str| {
            format!(
                "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n\
                 {touchscreen}    <application>\n        <activity \
                 android:name=\"com.example.MainActivity\" {attribute} />\n    </application>\n\
                 </manifest>\n"
            )
        };
        // setting_orientation_on_activity /1 — a locked orientation;
        // /2 `unspecified`.
        let text = activity("android:screenOrientation=\"portrait\"");
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "setting_orientation_on_activity",
            "The `<activity>` element should not be locked to any orientation so that users \
             can take advantage of the multi-window environments and larger screens on \
             Chrome OS",
            at(&text, 4, 59),
            35,
        );
        assert!(
            manifest_issues(&activity("android:screenOrientation=\"unspecified\""), true)
                .is_empty()
        );
        // non_resizable_activity /1 — `resizeableActivity="false"`; /2
        // `"true"`.
        let text = activity("android:resizeableActivity=\"false\"");
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "non_resizable_activity",
            "The `<activity>` element should be allowed to be resized to allow users to take \
             advantage of the multi-window environment on Chrome OS",
            at(&text, 4, 59),
            33,
        );
        assert!(manifest_issues(&activity("android:resizeableActivity=\"true\""), true).is_empty());
        // permission_implies_unsupported_hardware /1 — CALL_PHONE implies
        // telephony; /2 INTERNET implies nothing.
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-permission android:name=\"android.permission.CALL_PHONE\" />\n</manifest>\n"
        );
        let issues = manifest_issues(&text, true);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "permission_implies_unsupported_hardware",
            "Permission makes app incompatible for Chrome OS, consider adding optional \
             android.hardware.telephony feature tag,",
            at(&text, 3, 22),
            43,
        );
        let text = format!(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n{touchscreen}    \
             <uses-permission android:name=\"android.permission.INTERNET\" />\n</manifest>\n"
        );
        assert!(manifest_issues(&text, true).is_empty());
        // The gate: the check off leaves the whole face silent.
        assert!(manifest_issues(
            "<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n</manifest>\n",
            false
        )
        .is_empty());
    }

    #[test]
    fn the_multiple_plugins_face_pins_its_conflict_row() {
        // multiple_plugins /1 — the second different name reports at its
        // own node (alongside the section's deprecation row); /2 a
        // repeated same-name entry is no conflict.
        let text = "analyzer:\n  plugins:\n    - aaa\n    - bbb\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let issues = local_issues(map.as_mapping().expect("mapping"), text);
        let row = issues
            .iter()
            .find(|issue| issue.code.name == "multiple_plugins")
            .expect("multiple_plugins row");
        assert!(codes(&issues).contains(&"analysis_options_deprecated_plugins"));
        assert_eq!(
            issues
                .iter()
                .filter(|issue| issue.code.name == "multiple_plugins")
                .count(),
            1
        );
        assert_row(
            row,
            "multiple_plugins",
            "Multiple plugins can't be enabled.",
            at(text, 4, 7),
            3,
        );
        let text = "analyzer:\n  plugins:\n    - aaa\n    - aaa\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let issues = local_issues(map.as_mapping().expect("mapping"), text);
        assert!(!codes(&issues).contains(&"multiple_plugins"));
    }
}

#[cfg(test)]
mod options_verifier_pins {
    use super::*;

    /// The UTF-16 offset of one 1-based line/column position (the
    /// expected.tsv face; every pin fixture is ASCII, so bytes and code
    /// units agree).
    fn at(text: &str, line: usize, col: usize) -> u32 {
        let mut offset = 0_u32;
        for (index, line_text) in text.split('\n').enumerate() {
            if index + 1 == line {
                return offset + col as u32 - 1;
            }
            offset += line_text.len() as u32 + 1;
        }
        unreachable!("line {line} past the fixture");
    }

    /// One row's full pin: code, severity family, message, span.
    fn assert_row(issue: &OptionsIssue, code: &str, message: &str, offset: u32, length: u32) {
        assert_eq!(issue.code.name, code, "code of {message}");
        assert_eq!(issue.code.dtype, DiagnosticType::StaticWarning);
        assert_eq!(issue.message, message);
        assert_eq!(issue.offset, offset, "offset of {code}");
        assert_eq!(issue.length, length, "length of {code}");
    }

    fn codes(issues: &[OptionsIssue]) -> Vec<&str> {
        issues.iter().map(|issue| issue.code.name).collect()
    }

    /// The initial file's local shape rows for one options text.
    fn local(text: &str) -> Vec<OptionsIssue> {
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        local_issues(map.as_mapping().expect("mapping"), text)
    }

    #[test]
    fn the_section_rows_anchor_the_sections_value_node() {
        // invalid_section_format /1 — the oracle anchors the analyzer
        // section's block-list value (`- not_a_map`, 2:3|11), never the
        // key (the 2026-10-29 probe's position diff, fixed here).
        let text = "analyzer:\n  - not_a_map\n";
        let issues = local(text);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "invalid_section_format",
            "Invalid format for the 'analyzer' section.",
            at(text, 2, 3),
            11,
        );
        // /2 — a well-formed analyzer.errors section stays silent.
        assert!(local("analyzer:\n  errors:\n    dead_code: ignore\n").is_empty());
        // A same-line scalar value anchors the scalar's own node.
        let text = "analyzer: not_a_map\n";
        let issues = local(text);
        assert_row(
            &issues[0],
            "invalid_section_format",
            "Invalid format for the 'analyzer' section.",
            at(text, 1, 11),
            9,
        );
        // A nested whole-section arm anchors that section's value node.
        let text = "analyzer:\n  errors:\n    - not_a_map\n";
        let issues = local(text);
        assert_eq!(codes(&issues), ["invalid_section_format"]);
        assert_eq!(issues[0].offset, at(text, 3, 5));
        assert_eq!(issues[0].length, 11);
    }

    #[test]
    fn the_registry_rows_render_their_oracle_spans() {
        // undefined_lint /1 — the row the CLI face renders for an
        // unregistered name, anchored at the name's own occurrence
        // (3:7|26). The registry gate itself rides dartq-lints at the CLI
        // face (an engine dependency would cycle); /2's `prefer_final_locals`
        // is registered there and stays silent.
        let text = "linter:\n  rules:\n    - definitely_not_a_real_rule\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_rule_names(&map["linter"]["rules"], text);
        let row = OptionsIssue::undefined_lint(&declared[0]);
        assert_row(
            &row,
            "undefined_lint",
            "'definitely_not_a_real_rule' isn't a recognized lint rule.",
            at(text, 3, 7),
            26,
        );
        assert_eq!(
            row.correction.as_deref(),
            Some("Try using the name of a recognized lint rule.")
        );
        // /2 — a registered name extracts with its own span; the gate at
        // the CLI face drops it before any row renders.
        let text = "linter:\n  rules:\n    - prefer_final_locals\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_rule_names(&map["linter"]["rules"], text);
        assert_eq!(declared[0].name, "prefer_final_locals");
        assert_eq!(declared[0].offset, at(text, 3, 7));
        assert_eq!(declared[0].length, 19);

        // duplicate_rule /1 — the second enabled occurrence reports at
        // its own span (4:7|19); /2's single declaration has no earlier
        // occurrence to repeat.
        let text = "linter:\n  rules:\n    - prefer_final_locals\n    - prefer_final_locals\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_rule_names(&map["linter"]["rules"], text);
        let row = OptionsIssue::duplicate_rule(&declared[1]);
        assert_row(
            &row,
            "duplicate_rule",
            "The rule 'prefer_final_locals' is already enabled and doesn't need to be \
             enabled again.",
            at(text, 4, 7),
            19,
        );
        assert_eq!(declared.len(), 2);
        assert!(OptionsIssue::duplicate_rule(&declared[0]).offset == at(text, 3, 7));

        // incompatible_lint /1 — the later declaration conflicts with the
        // earlier enabled one; the message nests the quoted join inside
        // the template's own quotes (4:7|23).
        let text =
            "linter:\n  rules:\n    - always_use_package_imports\n    - prefer_relative_imports\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_rule_names(&map["linter"]["rules"], text);
        let row = OptionsIssue::incompatible_lint(&declared[1], &["always_use_package_imports"]);
        assert_row(
            &row,
            "incompatible_lint",
            "The rule 'prefer_relative_imports' is incompatible with \
             ''always_use_package_imports''.",
            at(text, 4, 7),
            23,
        );

        // unrecognized_error_code /1 — an unknown analyzer.errors key at
        // its own span (3:5|17); /2's `dead_code` is a known diagnostic
        // code — the engine-side gate leg that keeps the row silent.
        let text = "analyzer:\n  errors:\n    not_an_error_code: ignore\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_error_codes(map.as_mapping().expect("mapping"), text);
        assert!(!is_known_diagnostic_code(&declared[0].code));
        let row = OptionsIssue::unrecognized_error_code(&declared[0]);
        assert_row(
            &row,
            "unrecognized_error_code",
            "'not_an_error_code' isn't a recognized diagnostic code.",
            at(text, 3, 5),
            17,
        );
        let text = "analyzer:\n  errors:\n    dead_code: ignore\n";
        let map: serde_yaml::Value = serde_yaml::from_str(text).expect("yaml");
        let declared = declared_error_codes(map.as_mapping().expect("mapping"), text);
        assert!(is_known_diagnostic_code(&declared[0].code));
    }

    #[test]
    fn the_inner_plugins_face_pins_its_row() {
        // plugins_in_inner_options /1 — the row anchors the top-level
        // plugins section's whole value node (`custom_lint:` through
        // `path: .`, 2:3|24), the same node the row fires on when the
        // file's directory is not its context root (the emitting caller
        // owns that condition — the CLI face's loaded-options walk).
        let text = "plugins:\n  custom_lint:\n    path: .\n";
        let anchor = section_value_anchor(text, "plugins").expect("plugins value node");
        let row = OptionsIssue::plugins_in_inner_options("/", anchor);
        assert_row(
            &row,
            "plugins_in_inner_options",
            "Plugins can only be specified in the root of a pub workspace or the root of a \
             package that isn't in a workspace.",
            at(text, 2, 3),
            24,
        );
        assert_eq!(
            row.correction.as_deref(),
            Some("Try specifying plugins in an analysis options file at '/'.")
        );
    }

    #[test]
    fn the_unicode_text_face_splits_comment_from_literal() {
        let bidi = '\u{202E}';
        // text_direction_code_point_in_comment /1 — the flipped comment.
        let text =
            format!("// The text direction is silently flipped here {bidi}\nvoid f() {{\n}}\n");
        let issues = text_direction_issues(&text);
        assert_eq!(issues.len(), 1);
        assert_row(
            &issues[0],
            "text_direction_code_point_in_comment",
            "The Unicode code point 'U+202E' changes the appearance of text from how it's \
             interpreted by the compiler.",
            at(&text, 1, 48),
            1,
        );
        assert_eq!(
            issues[0].correction.as_deref(),
            Some("Try removing the code point or using the Unicode escape sequence '\u{5c}u202E'.")
        );
        // /2 — an unchanged comment stays silent.
        assert!(
            text_direction_issues("// The text direction is unchanged here.\nvoid f() {}\n")
                .is_empty()
        );
        // text_direction_code_point_in_literal /1 — the flipped literal.
        let text = format!("var s = 'a{bidi}b';\n\nvoid f() {{\n  print(s);\n}}\n");
        let issues = text_direction_issues(&text);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code.name, "text_direction_code_point_in_literal");
        assert_eq!(issues[0].offset, at(&text, 1, 11));
        assert_eq!(issues[0].length, 1);
        // /2 — a bidi code point in a comment beside a clean literal
        // reports the in_comment arm at the comment's own code point.
        let text = format!(
            "// The flagged code point only appears in a comment: {bidi}\nvar s = 'ab';\n\nvoid f() {{\n  print(s);\n}}\n"
        );
        let issues = text_direction_issues(&text);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code.name, "text_direction_code_point_in_comment");
        assert_eq!(issues[0].offset, at(&text, 1, 54));
        // Every other bidi code point flags too, raw strings carry theirs
        // as literals, escapes never fire, and multiple code points report
        // in source order.
        let text = "\u{2066}a\u{202A}b";
        let issues = text_direction_issues(text);
        assert_eq!(issues.len(), 2);
        assert_eq!(
            issues[0].message,
            "The Unicode code point 'U+2066' changes the appearance of text from how it's interpreted by the compiler."
        );
        let text = format!("var a = r'{bidi}';\nvar b = '\\u{{202E}}';\n");
        let issues = text_direction_issues(&text);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code.name, "text_direction_code_point_in_literal");
        // A code point inside an interpolation's expression re-enters
        // code — the covering node is no string literal, so the in_comment
        // arm fires (the verifier's covering-node assumption).
        let text = format!("var s = '${{{bidi}}}';");
        let issues = text_direction_issues(&text);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code.name, "text_direction_code_point_in_comment");
        assert_eq!(issues[0].offset, at(&text, 1, 12));
    }
}
