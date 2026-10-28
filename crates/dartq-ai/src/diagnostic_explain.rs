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
//! `why.trigger_kind` is a coarse family tag derived from the generated
//! code registry (`dartq-diagnostics`' upstream class table): one total
//! `CodeFamily → TriggerKind` mapping buckets every registry code — a new
//! upstream family without a deliberate bucket is a compile error — with
//! per-code carve-outs where a curated semantic family (the resolution
//! misses, the unused rows, the flow rows) cuts across the class data.
//! Codes outside the named families spell `unclassified` — an explicit
//! contract, not a placeholder for a finer taxonomy the report does not
//! carry. There is deliberately no `confidence` field: #2391's recon
//! confirmed no such concept exists on the produced rows.
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

use dartq_diagnostics::CodeFamily;
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
    /// The SDK version that removed the rule, when it did (absent on the
    /// wire otherwise — the schema's string-or-absent column, never a
    /// null).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_since: Option<String>,
    /// The SDK version that deprecated the rule, when it did (absent on
    /// the wire, like `removed_since`).
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// The coarse trigger family a code belongs to — one value per
/// `why.trigger_kind` spelling of the published schema (dartq-schema's
/// `TRIGGER_KINDS`; the spellings are pinned by test).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerKind {
    /// Parser/scanner recovery rows.
    ParseRecovery,
    /// The type/resolution miss family.
    TypeMismatch,
    /// The unused declaration/import family.
    UnusedFamily,
    /// The configuration-input registries (options, pubspec, manifest).
    OptionsConfig,
    /// The flow-analysis reachability family.
    FlowReach,
    /// The explicit floor for codes outside the named families.
    Unclassified,
}

impl TriggerKind {
    /// The wire spelling — the schema enum's exact string.
    fn as_str(self) -> &'static str {
        match self {
            TriggerKind::ParseRecovery => "parse-recovery",
            TriggerKind::TypeMismatch => "type-mismatch",
            TriggerKind::UnusedFamily => "unused-family",
            TriggerKind::OptionsConfig => "options-config",
            TriggerKind::FlowReach => "flow-reach",
            TriggerKind::Unclassified => "unclassified",
        }
    }
}

/// The total upstream-class → bucket mapping: every registry family has a
/// deliberate bucket, and a new family without one is a compile error
/// (the match is exhaustive over [`CodeFamily`]).
fn family_trigger(family: CodeFamily) -> TriggerKind {
    match family {
        // Parser/scanner recovery rows: the token stream could not yield a
        // legal unit, so the parser recovered and reported. The shared
        // front-end class is the same population — upstream packaging, not
        // behavior.
        CodeFamily::ParserErrorCode
        | CodeFamily::ScannerErrorCode
        | CodeFamily::FeAnalyzerShared => TriggerKind::ParseRecovery,
        // Configuration-input rows: the analysis options registries plus
        // the pubspec and build-manifest warnings — registries the run
        // emits over its inputs, not a code smell in a source file.
        CodeFamily::AnalysisOptionsErrorCode
        | CodeFamily::AnalysisOptionsWarningCode
        | CodeFamily::PubspecWarningCode
        | CodeFamily::ManifestWarningCode => TriggerKind::OptionsConfig,
        // The analyzer's semantic registries carry no single trigger
        // story: `CompileTimeErrorCode` spans declaration, type, and
        // resolution errors, `WarningCode`/`HintCode` span style, flow,
        // and advisory rows — so the class floor is the explicit
        // `unclassified`, and the high-frequency semantic families are
        // carved out per code in [`CODE_OVERRIDES`].
        CodeFamily::CompileTimeErrorCode
        | CodeFamily::StaticWarningCode
        | CodeFamily::WarningCode
        | CodeFamily::HintCode
        | CodeFamily::TodoCode
        | CodeFamily::FfiCode
        | CodeFamily::JsInteropCode => TriggerKind::Unclassified,
    }
}

/// Per-code buckets the upstream class cannot express — every entry is a
/// code the pre-registry name match carried, so each one preserves its
/// historical bucket (the byte-stability pin extracts the same table).
/// Each entry is load-bearing: the no-redundant-override pin asserts the
/// class mapping alone would give a different bucket (or the code is not
/// in the upstream registry at all).
const CODE_OVERRIDES: &[(&str, TriggerKind)] = &[
    // The type/resolution miss family: a name or type could not be
    // resolved, or a typed slot received an incompatible value. All
    // `CompileTimeErrorCode` — a class that spans 400+ declaration and
    // constraint errors — so the carve-out is per code.
    ("undefined_identifier", TriggerKind::TypeMismatch),
    ("undefined_method", TriggerKind::TypeMismatch),
    ("undefined_getter", TriggerKind::TypeMismatch),
    ("undefined_setter", TriggerKind::TypeMismatch),
    ("undefined_function", TriggerKind::TypeMismatch),
    ("undefined_named_parameter", TriggerKind::TypeMismatch),
    ("undefined_class", TriggerKind::TypeMismatch),
    ("uri_does_not_exist", TriggerKind::TypeMismatch),
    ("import_of_non_library", TriggerKind::TypeMismatch),
    ("argument_type_not_assignable", TriggerKind::TypeMismatch),
    ("invalid_assignment", TriggerKind::TypeMismatch),
    ("return_of_invalid_type", TriggerKind::TypeMismatch),
    ("non_type_as_type_argument", TriggerKind::TypeMismatch),
    // The unused-family rows: a declared or imported entity nothing reads.
    // `WarningCode`-classed (the class also carries style and flow rows);
    // `unused_import_prefix` is not in the upstream registry at all.
    ("unused_import", TriggerKind::UnusedFamily),
    ("unused_local_variable", TriggerKind::UnusedFamily),
    ("unused_field", TriggerKind::UnusedFamily),
    ("unused_element", TriggerKind::UnusedFamily),
    ("unused_shown_name", TriggerKind::UnusedFamily),
    ("unused_catch_clause", TriggerKind::UnusedFamily),
    ("unused_catch_stack", TriggerKind::UnusedFamily),
    ("unused_label", TriggerKind::UnusedFamily),
    ("unused_import_prefix", TriggerKind::UnusedFamily),
    ("unused_result", TriggerKind::UnusedFamily),
    // The reachability family: code the flow analysis proves never
    // executes or never returns into. `WarningCode`- and
    // `StaticWarningCode`-classed.
    ("dead_code", TriggerKind::FlowReach),
    ("unreachable_switch_case", TriggerKind::FlowReach),
    ("dead_null_aware_expression", TriggerKind::FlowReach),
    ("invalid_null_aware_operator", TriggerKind::FlowReach),
    // Options-registry rows the class mapping cannot reach:
    // `removed_lint_use` is `WarningCode`-classed upstream but is an
    // options-registry row; the rest are dartq options-registry
    // spellings absent from the upstream analyzer registry.
    ("removed_lint_use", TriggerKind::OptionsConfig),
    ("unsupported_lint_value", TriggerKind::OptionsConfig),
    ("incompatible_lint_files", TriggerKind::OptionsConfig),
    ("incompatible_included_file", TriggerKind::OptionsConfig),
    ("deprecated_lint_use", TriggerKind::OptionsConfig),
    ("plugins_in_inner_options", TriggerKind::OptionsConfig),
    ("options_file_not_found", TriggerKind::OptionsConfig),
];

/// Derives the coarse trigger family of a code from the generated
/// registry data: the per-code carve-out table first, then the lint fact
/// (lint codes never enter the analyzer registry — their bucket is the
/// explicit floor), then the total class → bucket mapping over the code's
/// upstream registry class.
///
/// A code outside the analyzer registry that is neither a carve-out nor a
/// lint — the dartq-own `dartq/` namespace and plugin-registry rows,
/// whose names are runtime data — carries the explicit
/// [`TriggerKind::Unclassified`] floor: the report's contract for codes
/// outside the named families, never a silent fallback. Registry codes
/// cannot land there by construction — the class table covers every
/// `OFFICIAL_CODES` name, pinned in `dartq-diagnostics`.
fn trigger_kind(code: &str, is_lint: bool) -> TriggerKind {
    if let Some((_, kind)) = CODE_OVERRIDES.iter().find(|(name, _)| *name == code) {
        return *kind;
    }
    if is_lint {
        return TriggerKind::Unclassified;
    }
    dartq_diagnostics::family(code).map_or(TriggerKind::Unclassified, family_trigger)
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
    let trigger_kind = trigger_kind(&echo.code, why.rule.is_some())
        .as_str()
        .to_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-registry name match's arms, verbatim — the byte-stability
    /// pin: every code the old hand-written match named keeps its exact
    /// bucket under the registry-derived derivation (the parse-recovery
    /// and options-config arms come from the class mapping, the rest from
    /// [`CODE_OVERRIDES`]).
    #[test]
    fn old_name_match_buckets_are_byte_stable() {
        let old_arms: &[(&str, &str)] = &[
            ("expected_token", "parse-recovery"),
            ("missing_identifier", "parse-recovery"),
            ("expected_executable", "parse-recovery"),
            ("expected_string_literal", "parse-recovery"),
            ("missing_const_final_var_or_type", "parse-recovery"),
            ("expected_type_name", "parse-recovery"),
            ("unexpected_token", "parse-recovery"),
            ("unterminated_string_literal", "parse-recovery"),
            ("unterminated_multi_line_comment", "parse-recovery"),
            ("undefined_identifier", "type-mismatch"),
            ("undefined_method", "type-mismatch"),
            ("undefined_getter", "type-mismatch"),
            ("undefined_setter", "type-mismatch"),
            ("undefined_function", "type-mismatch"),
            ("undefined_named_parameter", "type-mismatch"),
            ("undefined_class", "type-mismatch"),
            ("uri_does_not_exist", "type-mismatch"),
            ("import_of_non_library", "type-mismatch"),
            ("argument_type_not_assignable", "type-mismatch"),
            ("invalid_assignment", "type-mismatch"),
            ("return_of_invalid_type", "type-mismatch"),
            ("non_type_as_type_argument", "type-mismatch"),
            ("unused_import", "unused-family"),
            ("unused_local_variable", "unused-family"),
            ("unused_field", "unused-family"),
            ("unused_element", "unused-family"),
            ("unused_shown_name", "unused-family"),
            ("unused_catch_clause", "unused-family"),
            ("unused_catch_stack", "unused-family"),
            ("unused_label", "unused-family"),
            ("unused_import_prefix", "unused-family"),
            ("unused_result", "unused-family"),
            ("undefined_lint", "options-config"),
            ("unsupported_lint_value", "options-config"),
            ("incompatible_lint", "options-config"),
            ("incompatible_lint_files", "options-config"),
            ("incompatible_included_file", "options-config"),
            ("removed_lint_use", "options-config"),
            ("deprecated_lint_use", "options-config"),
            ("duplicate_rule", "options-config"),
            ("unrecognized_error_code", "options-config"),
            ("plugins_in_inner_options", "options-config"),
            ("options_file_not_found", "options-config"),
            ("dead_code", "flow-reach"),
            ("unreachable_switch_case", "flow-reach"),
            ("dead_null_aware_expression", "flow-reach"),
            ("invalid_null_aware_operator", "flow-reach"),
        ];
        for (code, bucket) in old_arms {
            assert_eq!(trigger_kind(code, false).as_str(), *bucket, "{code}");
        }
    }

    /// No redundant carve-out: every [`CODE_OVERRIDES`] entry either names
    /// a code the upstream registry does not carry, or diverges from its
    /// class's bucket — removing an entry would change the code's trigger
    /// kind, so the table stays minimal.
    #[test]
    fn overrides_are_load_bearing() {
        for (code, kind) in CODE_OVERRIDES {
            if let Some(family) = dartq_diagnostics::family(code) {
                assert_ne!(
                    family_trigger(family),
                    *kind,
                    "{code}: the class mapping already gives this bucket — \
                     the override is redundant"
                );
            }
        }
    }

    /// The six spellings are exactly the published schema's
    /// `TRIGGER_KINDS` — the schema enum is unchanged by the rework.
    #[test]
    fn trigger_kind_spellings_are_the_schema_enum() {
        assert_eq!(
            [
                TriggerKind::ParseRecovery,
                TriggerKind::TypeMismatch,
                TriggerKind::UnusedFamily,
                TriggerKind::OptionsConfig,
                TriggerKind::FlowReach,
                TriggerKind::Unclassified,
            ]
            .map(TriggerKind::as_str),
            [
                "parse-recovery",
                "type-mismatch",
                "unused-family",
                "options-config",
                "flow-reach",
                "unclassified",
            ]
        );
    }

    /// Registry codes the old match never named now land in their family's
    /// deliberate bucket instead of the silent generic floor — the
    /// extension seam this derivation closed (a new registry code can no
    /// longer land anywhere but its class's bucket).
    #[test]
    fn registry_codes_map_through_their_family() {
        // Parser/scanner rows outside the old match's name list.
        assert_eq!(
            trigger_kind("expected_body", false).as_str(),
            "parse-recovery"
        );
        assert_eq!(trigger_kind("encoding", false).as_str(), "parse-recovery");
        // A pubspec configuration row.
        assert_eq!(
            trigger_kind("asset_does_not_exist", false).as_str(),
            "options-config"
        );
        // A compile-time registry code the semantic carve-outs do not
        // name keeps the explicit floor.
        assert_eq!(
            trigger_kind("illegal_language_version_override", false).as_str(),
            "unclassified"
        );
    }

    /// The explicit floor for codes outside the analyzer registry: lint
    /// codes (their bucket is decided by the lint fact, not the analyzer
    /// class table), the dartq-own namespace, and plugin rows whose code
    /// names are runtime data.
    #[test]
    fn non_registry_codes_carry_the_explicit_floor() {
        assert_eq!(
            trigger_kind("prefer_single_quotes", true).as_str(),
            "unclassified"
        );
        assert_eq!(
            trigger_kind("dartq/internal_error", false).as_str(),
            "unclassified"
        );
        assert_eq!(
            trigger_kind("some_plugin_code", false).as_str(),
            "unclassified"
        );
    }

    /// The face wires the lint fact through: a lint-registry code whose
    /// rule line resolved lands `unclassified` on the wire, and a
    /// parse-recovery code keeps its bucket end to end.
    #[test]
    fn explain_diagnostic_carries_the_derived_trigger_kind() {
        let lint = explain_diagnostic(
            DiagnosticEcho {
                code: "prefer_single_quotes".to_owned(),
                severity: "INFO".to_owned(),
                message: "Unnecessary use of double quotes.".to_owned(),
                file: "lib/a.dart".to_owned(),
                range: [0, 1],
            },
            WhyFace {
                code_family: "LINT".to_owned(),
                rule: Some(RuleFact {
                    name: "prefer_single_quotes".to_owned(),
                    group: "core".to_owned(),
                    removed_since: None,
                    deprecated_since: None,
                }),
            },
            Vec::new(),
            1,
            Vec::new(),
            None,
            Verbosity::Compact,
        );
        assert_eq!(
            lint.why.as_ref().expect("compact tier").trigger_kind,
            "unclassified"
        );

        let parse = explain_diagnostic(
            DiagnosticEcho {
                code: "expected_token".to_owned(),
                severity: "ERROR".to_owned(),
                message: "Expected to find ';'.".to_owned(),
                file: "lib/a.dart".to_owned(),
                range: [4, 5],
            },
            WhyFace {
                code_family: "SYNTACTIC_ERROR".to_owned(),
                rule: None,
            },
            Vec::new(),
            1,
            Vec::new(),
            None,
            Verbosity::Compact,
        );
        assert_eq!(
            parse.why.as_ref().expect("compact tier").trigger_kind,
            "parse-recovery"
        );
    }
}
