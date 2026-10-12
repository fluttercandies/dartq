//! The daemon's `dartq/check` answer — the machine-row check face behind
//! the RPC `schema_version` stamp. Each row is `check_row`
//! (`crates/dartq-daemon/src/commands.rs:957-983`): compact mirrors the CLI
//! machine line's field set (severity / type / code / file / position /
//! length / message, code in dartq's stable `snake_case` spelling) as JSON;
//! the minimal tier keeps the red line (`code` + the file locator). The
//! envelope is the shared `session_answer` shape (commands.rs:1008, 1045):
//! `schema_version` (the daemon stamps `dartq_commands::SCHEMA_VERSION`,
//! commands.rs:125) plus the `diagnostics` rows, the full-tier short-id
//! table (`symbols` — its values are the file paths substituted out of the
//! rows), and the delta cursor echo (`since`).

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::diagnostics::{DIAGNOSTIC_TYPES, SEVERITIES};
use crate::support::{
    absent, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
    path_session_fields, pinned_version, rows, string,
};

/// The check answer's contract version. The writer stamps
/// `dartq_commands::SCHEMA_VERSION` (via `commands.rs:125`); the value is
/// pinned by the fixtures in `tests/output_faces.rs`.
pub const SCHEMA_VERSION_CHECK: u32 = 1;

/// The published check answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn check_schema() -> &'static Value {
    &CHECK_SCHEMA
}

static CHECK_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/check-answer.json",
        "title": "dartq check answer (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "diagnostics"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_CHECK },
            "diagnostics": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["file", "code", "line", "column", "length"],
                "properties": {
                    "file": { "type": "string" },
                    "severity": { "enum": SEVERITIES },
                    "type": { "enum": DIAGNOSTIC_TYPES },
                    "code": { "type": "string" },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "length": { "type": "integer", "minimum": 0 },
                    "message": { "type": "string" },
                },
            }},
            // §15.8 session token economics (crate docs, "Token
            // economics"): the full-tier short-id table over the rows'
            // file paths, and the delta cursor echo.
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

const CHECK_TOP: &[&str] = &["schema_version", "diagnostics"];
// Tier-independent (crate docs, "Token economics").
const CHECK_SESSION: &[&str] = &["symbols", "since"];
// The red line (§15.8): the code plus the file locator; the classification
// and message fields are the compact tier's contribution (check_row,
// commands.rs:964-982 — full equals compact today).
const CHECK_ROW: &[&str] = &["file", "code", "line", "column", "length"];
const CHECK_ROW_COMPACT: &[&str] = &["severity", "type", "message"];

pub(crate) fn validate_check(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, CHECK_TOP, CHECK_SESSION, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_CHECK)?;
    path_session_fields(root)?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, CHECK_ROW, CHECK_ROW_COMPACT, &p)?;
        string(field(obj, "file", &p)?, &join(&p, "file"))?;
        if let Some(severity) = obj.get("severity") {
            enum_spelling(severity, &join(&p, "severity"), SEVERITIES)?;
        }
        if let Some(dtype) = obj.get("type") {
            enum_spelling(dtype, &join(&p, "type"), DIAGNOSTIC_TYPES)?;
        }
        string(field(obj, "code", &p)?, &join(&p, "code"))?;
        let line = join(&p, "line");
        one_based(field(obj, "line", &p)?, &line)?;
        let column = join(&p, "column");
        one_based(field(obj, "column", &p)?, &column)?;
        let length = join(&p, "length");
        non_negative(field(obj, "length", &p)?, &length)?;
        if let Some(message) = obj.get("message") {
            string(message, &join(&p, "message"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): at `minimal` a row
/// carries exactly the red line — `code` + the file locator — plus the
/// tier-independent session fields; the classification and message fields
/// are the compact tier's contribution (crate docs, check table).
pub(crate) fn validate_check_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_check(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        for key in ["severity", "type", "message"] {
            absent(obj, key, &p, tier)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::schema_keys;

    fn answer() -> serde_json::Value {
        json!({
            "schema_version": SCHEMA_VERSION_CHECK,
            "diagnostics": [{
                "file": "/w/lib/a.dart",
                "severity": "ERROR",
                "type": "COMPILE_TIME_ERROR",
                "code": "undefined_identifier",
                "line": 3,
                "column": 7,
                "length": 2,
                "message": "Undefined name 'fa'.",
            }],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let check = check_schema();
        let mut top: Vec<String> = CHECK_TOP
            .iter()
            .chain(CHECK_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(check), top);
        let row = &check["properties"]["diagnostics"]["items"];
        let mut row_fields: Vec<String> = CHECK_ROW
            .iter()
            .chain(CHECK_ROW_COMPACT.iter())
            .map(|s| (*s).to_owned())
            .collect();
        row_fields.sort_unstable();
        assert_eq!(schema_keys(row), row_fields);
    }

    /// The severity and type spellings are the diagnostics envelope's own —
    /// one contract, the same writers (`as_machine_str` / `as_str`).
    #[test]
    fn enums_match_the_diagnostics_domain() {
        let check = check_schema();
        assert_eq!(
            check["properties"]["diagnostics"]["items"]["properties"]["severity"]["enum"],
            json!(SEVERITIES)
        );
        assert_eq!(
            check["properties"]["diagnostics"]["items"]["properties"]["type"]["enum"],
            json!(DIAGNOSTIC_TYPES)
        );
    }

    /// Tier exactness: a minimal answer carries only the red line; the
    /// compact row (the writer's baseline, commands.rs:972-981) is exact at
    /// compact and full.
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let violation = validate_check_at_tier(&answer(), Verbosity::Minimal)
            .expect_err("minimal must not carry the severity");
        assert_eq!(violation.path, "diagnostics[0].severity");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_check_at_tier(&answer(), Verbosity::Compact).is_ok());
        assert!(validate_check_at_tier(&answer(), Verbosity::Full).is_ok());

        let minimal = json!({
            "schema_version": SCHEMA_VERSION_CHECK,
            "diagnostics": [{
                "file": "/w/lib/a.dart",
                "code": "undefined_identifier",
                "line": 3,
                "column": 7,
                "length": 2,
            }],
            "since": 42,
        });
        assert!(validate_check_at_tier(&minimal, Verbosity::Minimal).is_ok());
    }

    /// The red line: an undeclared field is named, the locator slots carry
    /// their types, and the enum spellings are exact.
    #[test]
    fn unknown_fields_and_wrong_types_are_named() {
        let extra = json!({
            "schema_version": SCHEMA_VERSION_CHECK,
            "diagnostics": [{
                "file": "/w/lib/a.dart",
                "code": "undefined_identifier",
                "line": 3,
                "column": 7,
                "length": 2,
                "correctionMessage": "use fb",
            }],
        });
        let violation = validate_check(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "diagnostics[0].correctionMessage");

        let wrong_severity = json!({
            "schema_version": SCHEMA_VERSION_CHECK,
            "diagnostics": [{
                "file": "/w/lib/a.dart",
                "severity": "Fatal",
                "type": "COMPILE_TIME_ERROR",
                "code": "undefined_identifier",
                "line": 3,
                "column": 7,
                "length": 2,
                "message": "Undefined name 'fa'.",
            }],
        });
        let violation = validate_check(&wrong_severity).expect_err("severity spelling is exact");
        assert_eq!(violation.path, "diagnostics[0].severity");

        let wrong_line = json!({
            "schema_version": SCHEMA_VERSION_CHECK,
            "diagnostics": [{
                "file": "/w/lib/a.dart",
                "code": "undefined_identifier",
                "line": 0,
                "column": 7,
                "length": 2,
            }],
        });
        let violation = validate_check(&wrong_line).expect_err("line is 1-based");
        assert_eq!(violation.path, "diagnostics[0].line");
    }
}
