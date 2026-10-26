//! The `explain --diagnostic` query: the AI's one-stop answer for a single
//! diagnostic hit — the echoed row, the registry facts behind its code
//! (`why`), the code's mechanical fix rows (`fixes`), the same-code
//! neighbours in its file (`related`), and the published docs URL
//! (`documentation`).
//!
//! # The query shape
//!
//! Like [`explain`](crate::explain), the assembler is a pure function over
//! inputs the caller owns — it never sees the engine or the storage. The
//! caller (dartq-commands' `explain_diagnostic`) resolves the selected row
//! and every registry fact; the faces are plain borrowed data:
//!
//! | Section         | Input face                                                  |
//! |-----------------|-------------------------------------------------------------|
//! | `diagnostic`    | the selected row's echo (`DiagnosticEcho`)                  |
//! | `why`           | `code_family` + `trigger_kind`, plus `rule` for lint codes  |
//! | `fixes`         | the code's [`FixRow`]s, read straight off the fix registry  |
//! | `related`       | `total` (the run's same-code count) + `lines` (full tier)   |
//! | `documentation` | the code's published docs URL, `None` when unpublished      |
//!
//! `why.trigger_kind` is a coarse family tag over the high-frequency
//! codes; every code outside the named families spells `unclassified` —
//! an explicit contract, not a placeholder for a finer taxonomy the
//! report does not carry. There is deliberately no `confidence` field:
//! #2391's recon confirmed no such concept exists on the produced rows.
//!
//! # Provenance and ordering
//!
//! `related.lines` carries the file's other same-code 1-based lines in
//! ascending order (the emitted order already sorts by file/offset, so the
//! list is the natural read-off order). `fixes` is the registry table's
//! row order, which the table sorts by code — a code's row list is the
//! singleton the registry declares for it, or empty.
//!
//! # Verbosity
//!
//! * `minimal` — `diagnostic` alone (the red line).
//! * `compact` (default) — + `why`, `documentation` (when published),
//!   `related.total`.
//! * `full` — + `fixes` and `related.lines`.

use serde::{Deserialize, Serialize};

/// The report contract version: breaking output changes bump the major.
pub const DIAGNOSTIC_EXPLAIN_SCHEMA_VERSION: u32 = 1;

/// How much detail the report carries. Default [`Verbosity::Compact`].
///
/// Mirrors `dartq-commands`' `Verbosity` (this crate cannot see the
/// commands layer; the wiring maps one to the other — same three tiers,
/// same default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// `diagnostic` alone.
    Minimal,
    /// + `why`, `documentation`, `related.total` (the default).
    #[default]
    Compact,
    /// + `fixes`, `related.lines`.
    Full,
}

/// The selected row's echo: identity, locating span, message, and the
/// emitted severity — the diagnostics face's own field spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticEcho {
    /// The diagnostic code (`analyzerCode` spelling).
    pub code: String,
    /// The machine severity (`INFO`/`WARNING`/`ERROR`).
    pub severity: String,
    /// The emitted problem message.
    pub message: String,
    /// The canonical file path (the diagnostics envelope's own spelling).
    pub file: String,
    /// The `[startOffset, endOffset)` UTF-16 pair — split into `off`+`len`
    /// on the wire view.
    pub range: [u32; 2],
}

/// The lint-registry line a lint code resolves to — `name`/`group` always,
/// the removed/deprecated markers when the registry records them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleFact {
    /// The rule's machine name (the canonical lowercase spelling).
    pub name: String,
    /// The `package:lints` membership (`core`/`recommended`/`diagnostic`/
    /// `removed`/`standalone`).
    pub group: String,
    /// The SDK version that removed the rule, when it did.
    pub removed_since: Option<String>,
    /// The SDK version that deprecated the rule, when it did.
    pub deprecated_since: Option<String>,
}

/// One mechanical fix row of the code's registry entry — read-only
/// registry data: the row id, the provenance tag, and the one-sentence
/// action summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixRow {
    /// The fix family's stable id (the action-kind spelling).
    pub id: String,
    /// The pipeline that reports the code (`builtin-lint`/`builtin-semantic`/
    /// `builtin-parser`). Wire: `src`.
    #[serde(rename = "src")]
    pub source: String,
    /// One sentence naming the transform the family runs.
    pub summary: String,
}

/// The registry facts behind the code: the diagnostic type family, the
/// coarse trigger tag, and (lint codes only) the rule-registry line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyFace {
    /// The diagnostic type (`SYNTACTIC_ERROR`/`COMPILE_TIME_ERROR`/
    /// `STATIC_WARNING`/`LINT`/`HINT`/`TODO`/`TOOL`).
    pub code_family: String,
    /// The lint-registry line, `None` for non-lint codes.
    pub rule: Option<RuleFact>,
}

/// The report, JSON contract `v: 1` (the short-vocabulary envelope).
///
/// `why`/`related`/`documentation` are `Some` at [`Verbosity::Compact`]
/// and [`Verbosity::Full`], `None` at [`Verbosity::Minimal`];
/// `fixes`/`related.lines` join at `full` only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticExplainReport {
    /// The output contract version ([`DIAGNOSTIC_EXPLAIN_SCHEMA_VERSION`]).
    /// Wire: `v`.
    #[serde(rename = "v")]
    pub schema_version: u32,
    /// The selected diagnostic row, echoed.
    pub diagnostic: DiagnosticEchoView,
    /// The registry facts behind the code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<WhySection>,
    /// The published docs URL; `None` when the code publishes no page.
    /// Wire: `doc`.
    #[serde(rename = "doc", skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// The run's same-code neighbours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<RelatedSection>,
    /// The code's mechanical fix rows (`full` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixes: Option<Vec<FixRow>>,
}

/// The serialized `diagnostic` object (the echo's JSON face).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticEchoView {
    /// The diagnostic code.
    pub code: String,
    /// The machine severity spelling. Wire: `sev`.
    #[serde(rename = "sev")]
    pub severity: String,
    /// The emitted message. Wire: `msg`.
    #[serde(rename = "msg")]
    pub message: String,
    /// The canonical file path.
    pub file: String,
    /// The span's start offset, UTF-16 code units.
    pub off: u32,
    /// The span length.
    pub len: u32,
}

/// The serialized `why` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhySection {
    /// The diagnostic type (`code_family`).
    pub code_family: String,
    /// The coarse trigger family (`trigger_kind`).
    pub trigger_kind: String,
    /// The lint-registry line, lint codes only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleFact>,
}

/// The serialized `related` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedSection {
    /// The run's same-code row total (all files, this hit included).
    pub total: u32,
    /// The file's other same-code lines, 1-based, ascending (`full` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<u32>>,
}

/// The coarse trigger family a code belongs to — the static `code → kind`
/// map covering the high-frequency families (parser recovery, the
/// `type-mismatch` semantic family, the `unused-*` family, options-config
/// rows, flow reachability). Everything outside the named families is the
/// explicit `unclassified` floor — the report's contract, never a
/// placeholder.
fn trigger_kind(code: &str) -> &'static str {
    match code {
        // Scanner/parser recovery rows: the token stream could not yield a
        // legal unit, so the parser recovered and reported.
        "expected_token" | "missing_identifier" | "expected_executable"
        | "expected_string_literal" | "missing_const_final_var_or_type"
        | "expected_type_name" | "unexpected_token" | "unterminated_string_literal"
        | "unterminated_multi_line_comment" => "parse-recovery",
        // The type/resolution miss family: a name or type could not be
        // resolved, or a typed slot received an incompatible value.
        "undefined_identifier" | "undefined_method" | "undefined_getter"
        | "undefined_setter" | "undefined_function" | "undefined_named_parameter"
        | "undefined_class" | "uri_does_not_exist" | "import_of_non_library"
        | "argument_type_not_assignable" | "invalid_assignment"
        | "return_of_invalid_type" | "non_type_as_type_argument" => "type-mismatch",
        // The unused-family rows: a declared or imported entity nothing
        // reads.
        "unused_import" | "unused_local_variable" | "unused_field"
        | "unused_element" | "unused_shown_name" | "unused_catch_clause"
        | "unused_catch_stack" | "unused_label" | "unused_import_prefix"
        | "unused_result" => "unused-family",
        // The analysis-options configuration rows (the options registry
        // faces the run emits, not a code-smell in a source file).
        "undefined_lint" | "unsupported_lint_value" | "incompatible_lint"
        | "incompatible_lint_files" | "incompatible_included_file"
        | "removed_lint_use" | "deprecated_lint_use" | "duplicate_rule"
        | "unrecognized_error_code" | "plugins_in_inner_options"
        | "options_file_not_found" => "options-config",
        // The reachability family: code the flow analysis proves never
        // executes or never returns into.
        "dead_code" | "unreachable_switch_case" | "dead_null_aware_expression"
        | "invalid_null_aware_operator" => "flow-reach",
        _ => "unclassified",
    }
}

/// Assembles the diagnostic explain report over the caller-resolved faces:
/// `echo` is the selected row, `why` the registry facts, `fixes` the
/// code's registry rows, `total`/`lines` the same-code neighbours (lines
/// only consulted at `full`), `documentation` the published docs URL.
///
/// The sections are born at their tiers: `minimal` reports `diagnostic`
/// alone, `compact` adds `why`/`documentation`/`related.total`, `full`
/// adds `fixes` and `related.lines`.
#[must_use]
pub fn explain_diagnostic(
    echo: DiagnosticEcho,
    why: WhyFace,
    fixes: Vec<FixRow>,
    total: u32,
    lines: Vec<u32>,
    documentation: Option<String>,
    verbosity: Verbosity,
) -> DiagnosticExplainReport {
    let sectioned = verbosity != Verbosity::Minimal;
    let trigger_kind = trigger_kind(&echo.code).to_owned();
    DiagnosticExplainReport {
        schema_version: DIAGNOSTIC_EXPLAIN_SCHEMA_VERSION,
        diagnostic: DiagnosticEchoView {
            code: echo.code,
            severity: echo.severity,
            message: echo.message,
            file: echo.file,
            off: echo.range[0],
            len: echo.range[1].saturating_sub(echo.range[0]),
        },
        why: sectioned.then(|| WhySection {
            code_family: why.code_family,
            trigger_kind,
            rule: why.rule,
        }),
        documentation: sectioned.then_some(documentation).flatten(),
        related: sectioned.then(|| RelatedSection {
            total,
            lines: (verbosity == Verbosity::Full).then_some(lines),
        }),
        fixes: (verbosity == Verbosity::Full).then_some(fixes),
    }
}
