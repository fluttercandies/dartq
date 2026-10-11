//! The `dart analyze --format=json` diagnostics envelope (`dartq-diagnostics`'
//! oracle-exact writers). The envelope keeps the oracle's `version` key — see
//! the crate docs, "The `version` alias".

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
    pinned_version, rows, string,
};

/// The diagnostics JSON envelope's contract version — the oracle's
/// `version` key. Deliberate §15.7 naming alias: the envelope predates the
/// generic `schema_version` naming and stays byte-compatible with the
/// oracle, so no second key is ever added (crate docs, "The `version`
/// alias").
pub const SCHEMA_VERSION_DIAGNOSTICS: u32 = 1;

/// The published diagnostics JSON envelope schema (JSON Schema draft-07
/// subset). Models the union of the verbosity tiers (crate docs,
/// "Verbosity tiers"): the compact tier is the envelope as emitted, the
/// minimal tier keeps the red-line locating fields per row.
#[must_use]
pub fn diagnostics_schema() -> &'static Value {
    &DIAGNOSTICS_SCHEMA
}

static DIAGNOSTICS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let position = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["offset", "line", "column"],
        "properties": {
            "offset": { "type": "integer", "minimum": 0 },
            "line": { "type": "integer", "minimum": 1 },
            "column": { "type": "integer", "minimum": 1 },
        },
    });
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/diagnostics-json.json",
        "title": "dartq diagnostics JSON envelope (version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["version", "diagnostics"],
        "properties": {
            "version": { "const": SCHEMA_VERSION_DIAGNOSTICS },
            "diagnostics": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "location"],
                "properties": {
                    "code": { "type": "string" },
                    "severity": { "enum": ["INFO", "WARNING", "ERROR"] },
                    "type": { "enum": ["SYNTACTIC_ERROR", "COMPILE_TIME_ERROR", "STATIC_WARNING", "LINT", "TOOL"] },
                    "location": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["file", "range"],
                        "properties": {
                            "file": { "type": "string" },
                            "range": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["start", "end"],
                                "properties": {
                                    "start": position,
                                    "end": position,
                                },
                            },
                        },
                    },
                    "problemMessage": { "type": "string" },
                    "correctionMessage": { "type": "string" },
                    "documentation": { "type": "string" },
                },
            }},
        },
    })
});

const DIAGNOSTICS_TOP: &[&str] = &["version", "diagnostics"];
// Red line (§15.8): every tier keeps `code` + `location`; the message and
// classification fields are the compact/full tiers' contribution.
const DIAGNOSTIC: &[&str] = &["code", "location"];
const DIAGNOSTIC_OPTIONAL: &[&str] = &[
    "severity",
    "type",
    "problemMessage",
    "correctionMessage",
    "documentation",
];
const LOCATION: &[&str] = &["file", "range"];
const RANGE: &[&str] = &["start", "end"];
const POSITION: &[&str] = &["offset", "line", "column"];
const SEVERITIES: &[&str] = &["INFO", "WARNING", "ERROR"];
const DIAGNOSTIC_TYPES: &[&str] = &[
    "SYNTACTIC_ERROR",
    "COMPILE_TIME_ERROR",
    "STATIC_WARNING",
    "LINT",
    "TOOL",
];

pub(crate) fn validate_diagnostics(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, DIAGNOSTICS_TOP, &[], "")?;
    pinned_version(root, "version", SCHEMA_VERSION_DIAGNOSTICS)?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, DIAGNOSTIC, DIAGNOSTIC_OPTIONAL, &p)?;
        string(field(obj, "code", &p)?, &join(&p, "code"))?;
        if let Some(severity) = obj.get("severity") {
            enum_spelling(severity, &join(&p, "severity"), SEVERITIES)?;
        }
        if let Some(dtype) = obj.get("type") {
            enum_spelling(dtype, &join(&p, "type"), DIAGNOSTIC_TYPES)?;
        }
        let lp = join(&p, "location");
        let location = object(field(obj, "location", &p)?, &lp)?;
        exact_fields(location, LOCATION, &[], &lp)?;
        string(field(location, "file", &lp)?, &join(&lp, "file"))?;
        let rp = join(&lp, "range");
        let range = object(field(location, "range", &lp)?, &rp)?;
        exact_fields(range, RANGE, &[], &rp)?;
        position(field(range, "start", &rp)?, &join(&rp, "start"))?;
        position(field(range, "end", &rp)?, &join(&rp, "end"))?;
        if let Some(m) = obj.get("problemMessage") {
            string(m, &join(&p, "problemMessage"))?;
        }
        if let Some(m) = obj.get("correctionMessage") {
            string(m, &join(&p, "correctionMessage"))?;
        }
        if let Some(d) = obj.get("documentation") {
            string(d, &join(&p, "documentation"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): at `minimal` a row
/// carries exactly the red line — `code` + `location` — plus the
/// tier-independent optionals; the message and classification fields are
/// the compact tier's contribution (crate docs, diagnostics table).
pub(crate) fn validate_diagnostics_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_diagnostics(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        for key in ["severity", "type", "problemMessage"] {
            absent(obj, key, &p, tier)?;
        }
    }
    Ok(())
}

fn position(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let obj = object(value, path)?;
    exact_fields(obj, POSITION, &[], path)?;
    let offset = join(path, "offset");
    non_negative(field(obj, "offset", path)?, &offset)?;
    for key in ["line", "column"] {
        let p = join(path, key);
        one_based(field(obj, key, path)?, &p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    fn envelope() -> serde_json::Value {
        json!({
            "version": SCHEMA_VERSION_DIAGNOSTICS,
            "diagnostics": [{
                "code": "undefined_identifier",
                "severity": "ERROR",
                "type": "COMPILE_TIME_ERROR",
                "location": {
                    "file": "lib/main.dart",
                    "range": {
                        "start": {"offset": 0, "line": 1, "column": 1},
                        "end": {"offset": 8, "line": 1, "column": 9},
                    },
                },
                "problemMessage": "Undefined name 'fa'.",
            }],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let diagnostics = diagnostics_schema();
        assert_eq!(schema_keys(diagnostics), const_names(DIAGNOSTICS_TOP));
        let item = &diagnostics["properties"]["diagnostics"]["items"];
        let mut item_fields: Vec<String> = DIAGNOSTIC
            .iter()
            .chain(DIAGNOSTIC_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        item_fields.sort_unstable();
        assert_eq!(schema_keys(item), item_fields);
        assert_eq!(
            schema_keys(&item["properties"]["location"]),
            const_names(LOCATION)
        );
        let range = &item["properties"]["location"]["properties"]["range"];
        assert_eq!(
            schema_keys(&range["properties"]["start"]),
            const_names(POSITION)
        );
        assert_eq!(
            schema_keys(&range["properties"]["end"]),
            const_names(POSITION)
        );
    }

    #[test]
    fn compact_envelope_passes() {
        assert!(validate_diagnostics(&envelope()).is_ok());
    }

    /// The minimal tier keeps exactly `code` + `location` per row.
    #[test]
    fn minimal_row_passes() {
        let mut value = envelope();
        value["diagnostics"][0] = json!({
            "code": "undefined_identifier",
            "location": {
                "file": "lib/main.dart",
                "range": {
                    "start": {"offset": 0, "line": 1, "column": 1},
                    "end": {"offset": 8, "line": 1, "column": 9},
                },
            },
        });
        assert!(validate_diagnostics(&value).is_ok());
    }

    /// Red line (§15.8): a minimal row without its location is never valid.
    #[test]
    fn minimal_row_without_location_is_named() {
        let mut value = envelope();
        value["diagnostics"][0] = json!({"code": "undefined_identifier"});
        let violation = validate_diagnostics(&value).expect_err("red-line omission");
        assert_eq!(violation.path, "diagnostics[0].location");
        assert_eq!(violation.found, "missing");
    }

    /// Tier exactness: a minimal answer may not carry the compact tier's
    /// message and classification fields; the same envelope is exact at
    /// compact and full.
    #[test]
    fn minimal_tier_rejects_prose_fields() {
        let mut value = envelope();
        let violation = validate_diagnostics_at_tier(&value, Verbosity::Minimal)
            .expect_err("minimal must not carry severity");
        assert_eq!(violation.path, "diagnostics[0].severity");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_diagnostics_at_tier(&value, Verbosity::Compact).is_ok());
        assert!(validate_diagnostics_at_tier(&value, Verbosity::Full).is_ok());

        value["diagnostics"][0] = json!({
            "code": "undefined_identifier",
            "location": {
                "file": "lib/main.dart",
                "range": {
                    "start": {"offset": 0, "line": 1, "column": 1},
                    "end": {"offset": 8, "line": 1, "column": 9},
                },
            },
            "problemMessage": "Undefined name 'fa'.",
        });
        let violation = validate_diagnostics_at_tier(&value, Verbosity::Minimal)
            .expect_err("minimal must not carry the message");
        assert_eq!(violation.path, "diagnostics[0].problemMessage");
    }
}
