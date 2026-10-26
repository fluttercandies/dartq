//! The daemon's `dartq/analyze` answer — the pull face behind the RPC `v`
//! stamp. Each row is the entry flattened onto the short vocabulary
//! (`wire::row`, `crates/dartq-daemon/src/wire.rs`):
//! `{code, sev, file, line, col, len, msg}` — `file` carrying the entry's
//! document URI, `line`/`col` 1-based (the push face's zero-based LSP
//! positions plus one), `len` the span's UTF-16 length; the minimal tier
//! builds the reduced row at the call site (`analyze_answer`): `code` +
//! the locator. The envelope carries the full-tier short-id table
//! (`symbols` — its values are the entry URIs substituted out of the
//! rows); no writer emits `since` on this face (the pull face has no
//! delta parameter), so none is declared.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::check::SEVERITY_WORDS;
use crate::support::{
    absent, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
    path_session_fields, pinned_version, rows, string,
};

/// The analyze answer's contract version. The writer stamps
/// `dartq_commands::SCHEMA_VERSION`; the value is pinned by the fixtures
/// in `tests/output_faces.rs`.
pub const SCHEMA_VERSION_ANALYZE: u32 = 1;

/// The published analyze answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn analyze_schema() -> &'static Value {
    &ANALYZE_SCHEMA
}

static ANALYZE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/analyze-answer.json",
        "title": "dartq analyze answer (v 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "diags"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_ANALYZE },
            "diags": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "file", "line", "col", "len"],
                "properties": {
                    "code": { "type": "string" },
                    // The entry's document URI.
                    "file": { "type": "string" },
                    "line": { "type": "integer", "minimum": 1 },
                    "col": { "type": "integer", "minimum": 1 },
                    "len": { "type": "integer", "minimum": 0 },
                    "sev": { "enum": SEVERITY_WORDS },
                    "msg": { "type": "string" },
                },
            }},
            // Session token economics (crate docs, "Token
            // economics"): the full-tier short-id table over the entry
            // URIs. `since` is not declared — this face has no delta
            // parameter, so no writer emits the cursor.
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
        },
    })
});

const ANALYZE_TOP: &[&str] = &["v", "diags"];
// Tier-independent (crate docs, "Token economics").
const ANALYZE_SESSION: &[&str] = &["symbols"];
// The red line: the code plus the locator; the severity word and message
// are the compact tier's contribution (analyze_answer — full equals
// compact plus the short-id table).
const ANALYZE_ROW: &[&str] = &["code", "file", "line", "col", "len"];
const ANALYZE_ROW_COMPACT: &[&str] = &["sev", "msg"];

pub(crate) fn validate_analyze(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, ANALYZE_TOP, ANALYZE_SESSION, "")?;
    pinned_version(root, "v", SCHEMA_VERSION_ANALYZE)?;
    path_session_fields(root)?;
    for (i, row) in rows(root, "diags")? {
        let p = format!("diags[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, ANALYZE_ROW, ANALYZE_ROW_COMPACT, &p)?;
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
/// compact tier's contribution (crate docs, analyze table).
pub(crate) fn validate_analyze_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_analyze(value)?;
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
    use crate::support::{const_names, schema_keys};

    fn answer() -> serde_json::Value {
        json!({
            "v": SCHEMA_VERSION_ANALYZE,
            "diags": [{
                "code": "undefined_identifier",
                "sev": "error",
                "file": "file:///w/lib/a.dart",
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
        let analyze = analyze_schema();
        let mut top: Vec<String> = ANALYZE_TOP
            .iter()
            .chain(ANALYZE_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(analyze), top);
        let row = &analyze["properties"]["diags"]["items"];
        let mut row_fields: Vec<String> = ANALYZE_ROW
            .iter()
            .chain(ANALYZE_ROW_COMPACT.iter())
            .map(|s| (*s).to_owned())
            .collect();
        row_fields.sort_unstable();
        assert_eq!(schema_keys(row), row_fields);
        assert_eq!(
            row["properties"]["sev"]["enum"],
            json!(SEVERITY_WORDS)
        );
        assert_eq!(
            schema_keys(row),
            {
                let mut names = const_names(&["code", "file", "line", "col", "len", "sev", "msg"]);
                names.sort_unstable();
                names
            }
        );
    }

    /// Tier exactness: a minimal row carries the red line only; the
    /// compact row is exact at compact and full.
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let violation = validate_analyze_at_tier(&answer(), Verbosity::Minimal)
            .expect_err("minimal must not carry the severity");
        assert_eq!(violation.path, "diags[0].sev");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_analyze_at_tier(&answer(), Verbosity::Compact).is_ok());
        assert!(validate_analyze_at_tier(&answer(), Verbosity::Full).is_ok());

        let minimal = json!({
            "v": SCHEMA_VERSION_ANALYZE,
            "diags": [{
                "code": "undefined_identifier",
                "file": "file:///w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
            }],
        });
        assert!(validate_analyze_at_tier(&minimal, Verbosity::Minimal).is_ok());
    }

    /// The red line: an undeclared field is named and the severity word
    /// spelling is exact.
    #[test]
    fn unknown_fields_and_wrong_values_are_named() {
        let extra = json!({
            "v": SCHEMA_VERSION_ANALYZE,
            "diags": [{
                "code": "undefined_identifier",
                "file": "file:///w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "tags": [1],
            }],
        });
        let violation = validate_analyze(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "diags[0].tags");

        let wrong_severity = json!({
            "v": SCHEMA_VERSION_ANALYZE,
            "diags": [{
                "code": "undefined_identifier",
                "sev": "Fatal",
                "file": "file:///w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "msg": "Undefined name 'fa'.",
            }],
        });
        let violation = validate_analyze(&wrong_severity).expect_err("sev spelling is exact");
        assert_eq!(violation.path, "diags[0].sev");
    }
}
