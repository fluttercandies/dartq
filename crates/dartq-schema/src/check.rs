//! The daemon's `dartq/check` answer — the machine-row check face behind
//! the RPC `v` stamp. Each row is `check_row`
//! (`crates/dartq-daemon/src/commands.rs`, the check section): the
//! flattened short-vocabulary row `{code, sev, file, line, col, len,
//! msg}` (compact/full; `sev` the lowercase word, `line`/`col` 1-based,
//! code in dartq's stable `snake_case` spelling); the minimal tier keeps
//! the red line (`code` + the file locator + `line`/`col`/`len`). The
//! envelope is the shared `session_answer` shape: `v` (the daemon stamps
//! `dartq_commands::SCHEMA_VERSION`) plus the `diags` rows, the full-tier
//! short-id table (`symbols` — its values are the file paths substituted
//! out of the rows), and the delta cursor echo (`since`).

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
    path_session_fields, pinned_version, rows, string,
};

/// The check answer's contract version. The writer stamps
/// `dartq_commands::SCHEMA_VERSION`; the value is pinned by the fixtures
/// in `tests/output_faces.rs`.
pub const SCHEMA_VERSION_CHECK: u32 = 1;

/// The lowercase severity words the flattened rows' `sev` carries (the
/// wire vocabulary's spelling — not the diagnostics envelope's oracle
/// UPPERCASE).
pub const SEVERITY_WORDS: &[&str] = &["error", "warning", "info"];

/// The published check answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn check_schema() -> &'static Value {
    &CHECK_SCHEMA
}

static CHECK_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/check-answer.json",
        "title": "dartq check answer (v 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "diags"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_CHECK },
            "diags": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "file", "line", "col", "len"],
                "properties": {
                    "code": { "type": "string" },
                    "file": { "type": "string" },
                    "line": { "type": "integer", "minimum": 1 },
                    "col": { "type": "integer", "minimum": 1 },
                    "len": { "type": "integer", "minimum": 0 },
                    "sev": { "enum": SEVERITY_WORDS },
                    "msg": { "type": "string" },
                },
            }},
            // Session token economics (crate docs, "Token
            // economics"): the full-tier short-id table over the rows'
            // file paths, and the delta cursor echo.
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

const CHECK_TOP: &[&str] = &["v", "diags"];
// Tier-independent (crate docs, "Token economics").
const CHECK_SESSION: &[&str] = &["symbols", "since"];
// The red line: the code plus the file locator and the 1-based position;
// the severity word and message are the compact tier's contribution (full
// equals compact today).
const CHECK_ROW: &[&str] = &["code", "file", "line", "col", "len"];
const CHECK_ROW_COMPACT: &[&str] = &["sev", "msg"];

pub(crate) fn validate_check(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, CHECK_TOP, CHECK_SESSION, "")?;
    pinned_version(root, "v", SCHEMA_VERSION_CHECK)?;
    path_session_fields(root)?;
    for (i, row) in rows(root, "diags")? {
        let p = format!("diags[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, CHECK_ROW, CHECK_ROW_COMPACT, &p)?;
        string(field(obj, "code", &p)?, &join(&p, "code"))?;
        string(field(obj, "file", &p)?, &join(&p, "file"))?;
        let line = join(&p, "line");
        one_based(field(obj, "line", &p)?, &line)?;
        let col = join(&p, "col");
        one_based(field(obj, "col", &p)?, &col)?;
        let len = join(&p, "len");
        non_negative(field(obj, "len", &p)?, &len)?;
        if let Some(sev) = obj.get("sev") {
            enum_spelling(sev, &join(&p, "sev"), SEVERITY_WORDS)?;
        }
        if let Some(msg) = obj.get("msg") {
            string(msg, &join(&p, "msg"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): at `minimal` a row
/// carries exactly the red line — `code` + the locator — plus the
/// tier-independent session fields; the severity word and message are the
/// compact tier's contribution (crate docs, check table).
pub(crate) fn validate_check_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_check(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "diags")? {
        let p = format!("diags[{i}]");
        let obj = object(row, &p)?;
        for key in ["sev", "msg"] {
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
            "v": SCHEMA_VERSION_CHECK,
            "diags": [{
                "code": "undefined_identifier",
                "sev": "error",
                "file": "/w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "msg": "Undefined name 'fa'.",
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
        let row = &check["properties"]["diags"]["items"];
        let mut row_fields: Vec<String> = CHECK_ROW
            .iter()
            .chain(CHECK_ROW_COMPACT.iter())
            .map(|s| (*s).to_owned())
            .collect();
        row_fields.sort_unstable();
        assert_eq!(schema_keys(row), row_fields);
    }

    /// The `sev` spelling is the lowercase wire word — the vocabulary's
    /// own, not the diagnostics envelope's oracle UPPERCASE.
    #[test]
    fn severity_words_are_the_lowercase_spellings() {
        let check = check_schema();
        assert_eq!(
            check["properties"]["diags"]["items"]["properties"]["sev"]["enum"],
            json!(SEVERITY_WORDS)
        );
    }

    /// Tier exactness: a minimal answer carries only the red line; the
    /// compact row is exact at compact and full.
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let violation = validate_check_at_tier(&answer(), Verbosity::Minimal)
            .expect_err("minimal must not carry the severity");
        assert_eq!(violation.path, "diags[0].sev");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_check_at_tier(&answer(), Verbosity::Compact).is_ok());
        assert!(validate_check_at_tier(&answer(), Verbosity::Full).is_ok());

        let minimal = json!({
            "v": SCHEMA_VERSION_CHECK,
            "diags": [{
                "code": "undefined_identifier",
                "file": "/w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
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
            "v": SCHEMA_VERSION_CHECK,
            "diags": [{
                "code": "undefined_identifier",
                "file": "/w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "correctionMessage": "use fb",
            }],
        });
        let violation = validate_check(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "diags[0].correctionMessage");

        let wrong_severity = json!({
            "v": SCHEMA_VERSION_CHECK,
            "diags": [{
                "code": "undefined_identifier",
                "sev": "Fatal",
                "file": "/w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "msg": "Undefined name 'fa'.",
            }],
        });
        let violation = validate_check(&wrong_severity).expect_err("sev spelling is exact");
        assert_eq!(violation.path, "diags[0].sev");

        let wrong_line = json!({
            "v": SCHEMA_VERSION_CHECK,
            "diags": [{
                "code": "undefined_identifier",
                "file": "/w/lib/a.dart",
                "line": 0,
                "col": 7,
                "len": 2,
            }],
        });
        let violation = validate_check(&wrong_line).expect_err("line is 1-based");
        assert_eq!(violation.path, "diags[0].line");
    }
}
