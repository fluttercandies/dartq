//! The daemon's `dartq/analyze` answer — the LSP-native pull face behind
//! the RPC `schema_version` stamp. Each entry is a workspace diagnostic
//! flattened next to its document URI (`lsp::AnalyzeEntry`,
//! `crates/dartq-daemon/src/lsp.rs:105-110`, over the LSP §Diagnostic
//! shape of lsp.rs:83-91 — the writer's field set is closed on purpose);
//! the answer writer is `analyze_answer`
//! (`crates/dartq-daemon/src/commands.rs:924-946`). Positions are LSP:
//! zero-based lines and UTF-16 columns (lsp.rs:15-26), so this domain's
//! range contract is `non_negative`, unlike the CLI faces' 1-based rows.
//! The envelope carries the full-tier short-id table (`symbols` — its
//! values are the entry URIs substituted out of the rows,
//! commands.rs:944); no writer emits `since` on this face (the pull face
//! has no delta parameter), so none is declared.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, exact_fields, field, integer, join, non_negative, object, path_session_fields,
    pinned_version, rows, string, violation,
};

/// The analyze answer's contract version. The writer stamps
/// `dartq_commands::SCHEMA_VERSION` (via `commands.rs:125`); the value is
/// pinned by the fixtures in `tests/output_faces.rs`.
pub const SCHEMA_VERSION_ANALYZE: u32 = 1;

/// The published analyze answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn analyze_schema() -> &'static Value {
    &ANALYZE_SCHEMA
}

/// The LSP §Position — zero-based line and UTF-16 code-unit column
/// (lsp.rs:15-26).
static POSITION: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["line", "character"],
        "properties": {
            "line": { "type": "integer", "minimum": 0 },
            "character": { "type": "integer", "minimum": 0 },
        },
    })
});

static ANALYZE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/analyze-answer.json",
        "title": "dartq analyze answer (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "diagnostics"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_ANALYZE },
            "diagnostics": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["uri", "code", "range"],
                "properties": {
                    "uri": { "type": "string" },
                    // The LSP §DiagnosticSeverity values dartq produces
                    // (lsp.rs:85): 1 Error · 2 Warning · 3 Information.
                    "severity": { "enum": [1, 2, 3] },
                    // The stable dartq code name (snake_case, PRD §13.2).
                    "code": { "type": "string" },
                    "source": { "type": "string" },
                    "message": { "type": "string" },
                    "range": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["start", "end"],
                        "properties": {
                            "start": POSITION.clone(),
                            "end": POSITION.clone(),
                        },
                    },
                },
            }},
            // §15.8 session token economics (crate docs, "Token
            // economics"): the full-tier short-id table over the entry
            // URIs. `since` is not declared — this face has no delta
            // parameter, so no writer emits the cursor.
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
        },
    })
});

const ANALYZE_TOP: &[&str] = &["schema_version", "diagnostics"];
// Tier-independent (crate docs, "Token economics").
const ANALYZE_SESSION: &[&str] = &["symbols"];
// The red line (§15.8): the code plus the locator (`uri` + `range`);
// severity, source, and message are the compact tier's contribution
// (analyze_answer, commands.rs:928-937 — full equals compact plus the
// short-id table).
const ANALYZE_ENTRY: &[&str] = &["uri", "code", "range"];
const ANALYZE_ENTRY_COMPACT: &[&str] = &["severity", "source", "message"];
const RANGE: &[&str] = &["start", "end"];

pub(crate) fn validate_analyze(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, ANALYZE_TOP, ANALYZE_SESSION, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_ANALYZE)?;
    path_session_fields(root)?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, ANALYZE_ENTRY, ANALYZE_ENTRY_COMPACT, &p)?;
        string(field(obj, "uri", &p)?, &join(&p, "uri"))?;
        string(field(obj, "code", &p)?, &join(&p, "code"))?;
        if let Some(severity) = obj.get("severity") {
            let sp = join(&p, "severity");
            let value = integer(severity, &sp)?;
            if !matches!(value, 1..=3) {
                return Err(violation(sp, "the integer 1, 2, or 3", value.to_string()));
            }
        }
        if let Some(source) = obj.get("source") {
            string(source, &join(&p, "source"))?;
        }
        if let Some(message) = obj.get("message") {
            string(message, &join(&p, "message"))?;
        }
        let rp = join(&p, "range");
        let range = object(field(obj, "range", &p)?, &rp)?;
        exact_fields(range, RANGE, &[], &rp)?;
        position(field(range, "start", &rp)?, &join(&rp, "start"))?;
        position(field(range, "end", &rp)?, &join(&rp, "end"))?;
    }
    Ok(())
}

fn position(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let obj = object(value, path)?;
    exact_fields(obj, &["line", "character"], &[], path)?;
    non_negative(field(obj, "line", path)?, &join(path, "line"))?;
    non_negative(field(obj, "character", path)?, &join(path, "character"))?;
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): at `minimal` an
/// entry carries exactly the red line — `code` + `uri` + `range` — plus
/// the tier-independent session fields; severity, source, and message are
/// the compact tier's contribution (crate docs, analyze table).
pub(crate) fn validate_analyze_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_analyze(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        for key in ["severity", "source", "message"] {
            absent(obj, key, &p, tier)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    fn answer() -> serde_json::Value {
        json!({
            "schema_version": SCHEMA_VERSION_ANALYZE,
            "diagnostics": [{
                "uri": "file:///w/lib/a.dart",
                "range": {
                    "start": {"line": 2, "character": 6},
                    "end": {"line": 2, "character": 8},
                },
                "severity": 1,
                "code": "undefined_identifier",
                "source": "dartq",
                "message": "Undefined name 'fa'.",
            }],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let analyze = analyze_schema();
        let mut top: Vec<String> = ANALYZE_TOP
            .iter()
            .chain(ANALYZE_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(analyze), top);
        let entry = &analyze["properties"]["diagnostics"]["items"];
        let mut entry_fields: Vec<String> = ANALYZE_ENTRY
            .iter()
            .chain(ANALYZE_ENTRY_COMPACT.iter())
            .map(|s| (*s).to_owned())
            .collect();
        entry_fields.sort_unstable();
        assert_eq!(schema_keys(entry), entry_fields);
        assert_eq!(
            schema_keys(&entry["properties"]["range"]["properties"]["start"]),
            const_names(&["line", "character"])
        );
    }

    /// Tier exactness: a minimal entry carries only `uri` + `code` +
    /// `range` (the writer's minimal arm, commands.rs:929-933); the compact
    /// entry is exact at compact and full.
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let violation = validate_analyze_at_tier(&answer(), Verbosity::Minimal)
            .expect_err("minimal must not carry the severity");
        assert_eq!(violation.path, "diagnostics[0].severity");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_analyze_at_tier(&answer(), Verbosity::Compact).is_ok());
        assert!(validate_analyze_at_tier(&answer(), Verbosity::Full).is_ok());

        let minimal = json!({
            "schema_version": SCHEMA_VERSION_ANALYZE,
            "diagnostics": [{
                "uri": "file:///w/lib/a.dart",
                "code": "undefined_identifier",
                "range": {
                    "start": {"line": 2, "character": 6},
                    "end": {"line": 2, "character": 8},
                },
            }],
        });
        assert!(validate_analyze_at_tier(&minimal, Verbosity::Minimal).is_ok());
    }

    /// The red line: an undeclared field is named, the LSP severity is one
    /// of the produced values, and positions are zero-based (LSP, unlike
    /// the CLI faces' 1-based rows).
    #[test]
    fn unknown_fields_and_wrong_values_are_named() {
        let extra = json!({
            "schema_version": SCHEMA_VERSION_ANALYZE,
            "diagnostics": [{
                "uri": "file:///w/lib/a.dart",
                "code": "undefined_identifier",
                "range": {
                    "start": {"line": 2, "character": 6},
                    "end": {"line": 2, "character": 8},
                },
                "tags": [1],
            }],
        });
        let violation = validate_analyze(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "diagnostics[0].tags");

        let wrong_severity = json!({
            "schema_version": SCHEMA_VERSION_ANALYZE,
            "diagnostics": [{
                "uri": "file:///w/lib/a.dart",
                "range": {
                    "start": {"line": 2, "character": 6},
                    "end": {"line": 2, "character": 8},
                },
                "severity": 4,
                "code": "undefined_identifier",
                "source": "dartq",
                "message": "Undefined name 'fa'.",
            }],
        });
        let violation = validate_analyze(&wrong_severity).expect_err("severity is one of 1|2|3");
        assert_eq!(violation.path, "diagnostics[0].severity");
    }
}
