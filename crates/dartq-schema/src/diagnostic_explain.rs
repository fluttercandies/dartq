//! The `dartq explain --diagnostic` report — the AI's one-stop answer for a
//! single diagnostic hit: the echoed diagnostic, the registry facts behind
//! it (`why`), the mechanical fix rows the code carries (`fixes`), the
//! same-code neighbours in its file (`related`), and the published docs
//! URL (`documentation`).
//!
//! The report answers from the engine's full analyze run (engine rows +
//! the lint stage + the options/provenance rows), so every selectable
//! diagnostic is one a `dart analyze`-equivalent run already emitted.
//!
//! # Verbosity tiers (crate docs, "Verbosity tiers")
//!
//! `minimal` is the red line — the `diagnostic` echo alone. `compact` adds
//! the registry facts (`why`), the docs pointer (`documentation`), and the
//! run-wide `related` count. `full` adds the `fixes` rows and `related`'s
//! line list.
//!
//! | field                 | minimal | compact | full |
//! |-----------------------|---------|---------|------|
//! | `diagnostic`          | ●       | ●       | ●    |
//! | `why`                 |         | ●       | ●    |
//! | `documentation`       |         | opt     | opt  |
//! | `related`             |         | ●       | ●    |
//! | `related.lines`       |         |         | ●    |
//! | `fixes`               |         |         | ●    |
//!
//! Ranges are the `[startOffset, endOffset)` UTF-16 pair form (the face is
//! new — no legacy spelling to keep). The `diagnostic.severity` spelling
//! reuses the diagnostics envelope's machine form (`INFO`/`WARNING`/
//! `ERROR`), `why.code_family` the `type` column's (`SYNTACTIC_ERROR`, …).
//!
//! `why.trigger_kind` is a coarse family tag over the high-frequency codes
//! (`parse` recovery, `type-mismatch`, `unused-*`, `options-config`,
//! `flow-reach`, …); codes outside the named families carry the explicit
//! `unclassified` spelling — a contract, not a placeholder. The tag set is
//! deliberately small: richer per-code narrative is the docs page's job,
//! not this report's.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::diagnostics::{DIAGNOSTIC_TYPES, SEVERITIES};
use crate::support::{
    absent, array, enum_spelling, exact_fields, field, join, non_negative, object,
    pinned_version, range_pair, string,
};
use crate::{SchemaViolation, Verbosity};

/// The diagnostic-explain report's contract version. Mirrors the
/// authoritative `DIAGNOSTIC_EXPLAIN_SCHEMA_VERSION` in `dartq-ai`.
pub const SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN: u32 = 1;

/// The published diagnostic-explain report schema (JSON Schema draft-07
/// subset).
#[must_use]
pub fn diagnostic_explain_schema() -> &'static Value {
    &DIAGNOSTIC_EXPLAIN_SCHEMA
}

/// The `why.trigger_kind` spellings — the coarse family tags covering the
/// high-frequency codes plus the explicit `unclassified` floor.
pub const TRIGGER_KINDS: &[&str] = &[
    "parse-recovery",
    "type-mismatch",
    "unused-family",
    "options-config",
    "flow-reach",
    "unclassified",
];

/// The `fixes[].source` provenance spellings — `dartq-commands` fix.rs's
/// `BUILTIN_*_SOURCE` constants, re-spelled here so the schema is the
/// contract of record.
pub const FIX_SOURCES: &[&str] = &["builtin-lint", "builtin-semantic", "builtin-parser"];

static DIAGNOSTIC_EXPLAIN_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/diagnostic-explain.json",
        "title": "dartq diagnostic explain report (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "diagnostic"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN },
            // The selected row, echoed: identity, locating range, and the
            // emitted message/severity.
            "diagnostic": { "type": "object",
                "additionalProperties": false,
                "required": ["code", "severity", "message", "file", "range"],
                "properties": {
                    "code": { "type": "string" },
                    "severity": { "enum": SEVERITIES },
                    "message": { "type": "string" },
                    // The canonical file path — the diagnostics envelope's
                    // own spelling convention.
                    "file": { "type": "string" },
                    "range": { "$ref": "#/definitions/range_pair" },
                },
            },
            // The registry facts behind the code.
            "why": { "type": "object",
                "additionalProperties": false,
                "required": ["code_family", "trigger_kind"],
                "properties": {
                    // The diagnostic type column (`LINT`,
                    // `COMPILE_TIME_ERROR`, …).
                    "code_family": { "enum": DIAGNOSTIC_TYPES },
                    // The coarse trigger family (`TRIGGER_KINDS`).
                    "trigger_kind": { "enum": TRIGGER_KINDS },
                    // Lint-registry detail; lint codes only.
                    "rule": { "type": "object",
                        "additionalProperties": false,
                        "required": ["name", "group"],
                        "properties": {
                            "name": { "type": "string" },
                            // The `package:lints` membership:
                            // core/recommended/diagnostic/removed/
                            // standalone.
                            "group": { "enum": RULE_GROUPS },
                            // Removed/deprecated markers, upstream versions.
                            "removed_since": { "type": "string" },
                            "deprecated_since": { "type": "string" },
                        },
                    },
                },
            },
            // The published docs page (`has_published_docs` only).
            "documentation": { "type": "string" },
            // The run's same-code rows: the total count and (full tier)
            // the other lines in this file.
            "related": { "type": "object",
                "additionalProperties": false,
                "required": ["total"],
                "properties": {
                    "total": { "type": "integer", "minimum": 0 },
                    "lines": { "type": "array",
                        "items": { "type": "integer", "minimum": 1 } },
                },
            },
            // The code's mechanical fix rows (read-only registry data).
            "fixes": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "source", "summary"],
                "properties": {
                    "id": { "type": "string" },
                    "source": { "enum": FIX_SOURCES },
                    "summary": { "type": "string" },
                },
            }},
        },
        "definitions": {
            // A `[startOffset, endOffset)` pair, UTF-16 code units.
            "range_pair": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "minItems": 2,
                "maxItems": 2,
            },
        },
    })
});

/// The lint-registry group spellings (`dartq-lints`' `Group` —
/// core/recommended member, the diagnostic mirror, the SDK-removed
/// registrations, and the options-driven standalone rest).
pub const RULE_GROUPS: &[&str] =
    &["core", "recommended", "diagnostic", "removed", "standalone"];

const TOP: &[&str] = &["schema_version", "diagnostic"];
const TOP_OPTIONAL: &[&str] = &["why", "documentation", "related", "fixes"];
const DIAGNOSTIC: &[&str] = &["code", "severity", "message", "file", "range"];
const WHY: &[&str] = &["code_family", "trigger_kind"];
const WHY_OPTIONAL: &[&str] = &["rule"];
const RULE: &[&str] = &["name", "group"];
const RULE_OPTIONAL: &[&str] = &["removed_since", "deprecated_since"];
const RELATED: &[&str] = &["total"];
const RELATED_OPTIONAL: &[&str] = &["lines"];
const FIX_ROW: &[&str] = &["id", "source", "summary"];

pub(crate) fn validate_diagnostic_explain(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, TOP, TOP_OPTIONAL, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN)?;
    let p = "diagnostic";
    let diag = object(field(root, "diagnostic", "")?, p)?;
    exact_fields(diag, DIAGNOSTIC, &[], p)?;
    string(field(diag, "code", p)?, &join(p, "code"))?;
    enum_spelling(field(diag, "severity", p)?, &join(p, "severity"), SEVERITIES)?;
    string(field(diag, "message", p)?, &join(p, "message"))?;
    string(field(diag, "file", p)?, &join(p, "file"))?;
    range_pair(field(diag, "range", p)?, &join(p, "range"))?;
    if let Some(why) = root.get("why") {
        let p = "why";
        let obj = object(why, p)?;
        exact_fields(obj, WHY, WHY_OPTIONAL, p)?;
        enum_spelling(
            field(obj, "code_family", p)?,
            &join(p, "code_family"),
            DIAGNOSTIC_TYPES,
        )?;
        enum_spelling(
            field(obj, "trigger_kind", p)?,
            &join(p, "trigger_kind"),
            TRIGGER_KINDS,
        )?;
        if let Some(rule) = obj.get("rule") {
            let rp = join(p, "rule");
            let obj = object(rule, &rp)?;
            exact_fields(obj, RULE, RULE_OPTIONAL, &rp)?;
            string(field(obj, "name", &rp)?, &join(&rp, "name"))?;
            enum_spelling(field(obj, "group", &rp)?, &join(&rp, "group"), RULE_GROUPS)?;
            if let Some(v) = obj.get("removed_since") {
                string(v, &join(&rp, "removed_since"))?;
            }
            if let Some(v) = obj.get("deprecated_since") {
                string(v, &join(&rp, "deprecated_since"))?;
            }
        }
    }
    if let Some(docs) = root.get("documentation") {
        string(docs, "documentation")?;
    }
    if let Some(related) = root.get("related") {
        let p = "related";
        let obj = object(related, p)?;
        exact_fields(obj, RELATED, RELATED_OPTIONAL, p)?;
        non_negative(field(obj, "total", p)?, &join(p, "total"))?;
        if let Some(lines) = obj.get("lines") {
            for (i, line) in array(lines, &join(p, "lines"))?.iter().enumerate() {
                let lp = format!("{}.lines[{i}]", p);
                crate::support::one_based(line, &lp)?;
            }
        }
    }
    if let Some(fixes) = root.get("fixes") {
        for (i, row) in array(fixes, "fixes")?.iter().enumerate() {
            let p = format!("fixes[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, FIX_ROW, &[], &p)?;
            string(field(obj, "id", &p)?, &join(&p, "id"))?;
            enum_spelling(field(obj, "source", &p)?, &join(&p, "source"), FIX_SOURCES)?;
            string(field(obj, "summary", &p)?, &join(&p, "summary"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): `minimal` carries
/// the diagnostic echo only; `compact` adds `why`/`documentation`/
/// `related.total` (never `related.lines`, never `fixes`); `full` adds
/// `related.lines` and `fixes`.
pub(crate) fn validate_diagnostic_explain_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_diagnostic_explain(value)?;
    let root = object(value, "")?;
    match tier {
        Verbosity::Minimal => {
            for key in TOP_OPTIONAL {
                absent(root, key, "", tier)?;
            }
        }
        Verbosity::Compact => {
            absent(root, "fixes", "", tier)?;
            if let Some(related) = root.get("related") {
                absent(object(related, "related")?, "lines", "related", tier)?;
            }
        }
        Verbosity::Full => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    /// A full-tier report over a lint hit (the section-complete shape).
    fn full_report() -> Value {
        json!({
            "schema_version": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN,
            "diagnostic": {
                "code": "unused_import",
                "severity": "WARNING",
                "message": "Unused import: 'dart:async'.",
                "file": "/w/lib/a.dart",
                "range": [10, 22],
            },
            "why": {
                "code_family": "STATIC_WARNING",
                "trigger_kind": "unused-family",
            },
            "documentation": "https://dart.dev/diagnostics/unused_import",
            "related": { "total": 3, "lines": [4, 9] },
            "fixes": [{
                "id": "remove-lexeme",
                "source": "builtin-semantic",
                "summary": "Remove the reported lexeme.",
            }],
        })
    }

    /// A minimal-tier report: the diagnostic echo alone.
    fn minimal_report() -> Value {
        json!({
            "schema_version": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN,
            "diagnostic": {
                "code": "unused_import",
                "severity": "WARNING",
                "message": "Unused import: 'dart:async'.",
                "file": "/w/lib/a.dart",
                "range": [10, 22],
            },
        })
    }

    /// Drift pin: the published document and the validator name the same
    /// fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let schema = diagnostic_explain_schema();
        let mut top: Vec<String> = TOP
            .iter()
            .chain(TOP_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(schema), top);
        assert_eq!(
            schema_keys(&schema["properties"]["diagnostic"]),
            const_names(DIAGNOSTIC)
        );
        let why = &schema["properties"]["why"];
        let mut why_fields: Vec<String> = WHY
            .iter()
            .chain(WHY_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        why_fields.sort_unstable();
        assert_eq!(schema_keys(why), why_fields);
        let rule = &why["properties"]["rule"];
        let mut rule_fields: Vec<String> = RULE
            .iter()
            .chain(RULE_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        rule_fields.sort_unstable();
        assert_eq!(schema_keys(rule), rule_fields);
        let related = &schema["properties"]["related"];
        let mut related_fields: Vec<String> = RELATED
            .iter()
            .chain(RELATED_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        related_fields.sort_unstable();
        assert_eq!(schema_keys(related), related_fields);
        let fix = &schema["properties"]["fixes"]["items"];
        assert_eq!(schema_keys(fix), const_names(FIX_ROW));
    }

    /// Every tier validates its own shape: minimal the echo alone, compact
    /// the registry+related-count answer, full the complete report.
    #[test]
    fn each_tier_validates_its_own_shape() {
        assert!(validate_diagnostic_explain_at_tier(&minimal_report(), Verbosity::Minimal).is_ok());
        let compact = json!({
            "schema_version": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN,
            "diagnostic": minimal_report()["diagnostic"],
            "why": { "code_family": "STATIC_WARNING", "trigger_kind": "unused-family" },
            "related": { "total": 3 },
        });
        assert!(validate_diagnostic_explain_at_tier(&compact, Verbosity::Compact).is_ok());
        assert!(validate_diagnostic_explain_at_tier(&full_report(), Verbosity::Full).is_ok());
    }

    /// Tier exactness: the minimal echo drops every section; compact must
    /// not carry `fixes` or `related.lines`; `full` carries them.
    #[test]
    fn tiers_enforce_the_field_floor() {
        let violation =
            validate_diagnostic_explain_at_tier(&full_report(), Verbosity::Minimal).expect_err(
                "minimal must not carry the registry/related/fix sections",
            );
        assert_eq!(violation.path, "why");
        assert_eq!(violation.expected, "omitted at the minimal tier");

        let compact_with_fixes = json!({
            "schema_version": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN,
            "diagnostic": minimal_report()["diagnostic"],
            "why": { "code_family": "LINT", "trigger_kind": "unclassified" },
            "fixes": [{ "id": "x", "source": "builtin-lint", "summary": "s" }],
        });
        let violation =
            validate_diagnostic_explain_at_tier(&compact_with_fixes, Verbosity::Compact)
                .expect_err("compact must not carry fixes");
        assert_eq!(violation.path, "fixes");

        let compact_with_lines = json!({
            "schema_version": SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN,
            "diagnostic": minimal_report()["diagnostic"],
            "related": { "total": 3, "lines": [4] },
        });
        let violation =
            validate_diagnostic_explain_at_tier(&compact_with_lines, Verbosity::Compact)
                .expect_err("compact must not carry related.lines");
        assert_eq!(violation.path, "related.lines");
    }

    /// The red lines inside the rows: enum spellings are checked, an
    /// undeclared field is named, `related.lines` entries are 1-based.
    #[test]
    fn violations_name_the_first_bad_field() {
        let mut bad = full_report();
        bad["diagnostic"]["severity"] = json!("BLOCKER");
        let violation =
            validate_diagnostic_explain(&bad).expect_err("unknown severity spelling");
        assert_eq!(violation.path, "diagnostic.severity");

        let mut bad = full_report();
        bad["why"]["trigger_kind"] = json!("magic");
        let violation =
            validate_diagnostic_explain(&bad).expect_err("unknown trigger kind");
        assert_eq!(violation.path, "why.trigger_kind");

        let mut bad = full_report();
        bad["related"]["lines"] = json!([4, 0]);
        let violation =
            validate_diagnostic_explain(&bad).expect_err("a zero line is not a position");
        assert_eq!(violation.path, "related.lines[1]");

        let mut bad = full_report();
        bad["fixes"][0]["note"] = json!("extra");
        let violation = validate_diagnostic_explain(&bad).expect_err("undeclared fix field");
        assert_eq!(violation.path, "fixes[0].note");
    }
}
