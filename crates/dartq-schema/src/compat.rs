//! The §15.5 compatibility report — `dartq compat`, the per-SDK-context
//! verdict matrix produced by `dartq-ai::compatibility::CompatReport` and
//! serialized by the CLI (`crates/dartq-cli/src/compat.rs:151-159`).

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, boolean, enum_spelling, exact_fields, field, join, object, one_based, pinned_version,
    rows, string,
};

/// The compatibility report's contract version. Mirrors the authoritative
/// `COMPAT_SCHEMA_VERSION` in `dartq-ai`; equality is pinned by an
/// alignment test (`tests/alignment.rs`).
pub const SCHEMA_VERSION_COMPAT: u32 = 1;

/// The published compatibility report schema (JSON Schema draft-07 subset).
#[must_use]
pub fn compat_schema() -> &'static Value {
    &COMPAT_SCHEMA
}

static COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let failure = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["code", "file", "line", "col", "message"],
        "properties": {
            "code": { "type": "string" },
            "file": { "type": "string" },
            "line": { "type": "integer", "minimum": 1 },
            "col": { "type": "integer", "minimum": 1 },
            "message": { "type": "string" },
        },
    });
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/compat-report.json",
        "title": "dartq compatibility report (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "file", "matrix"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_COMPAT },
            "file": { "type": "string" },
            "matrix": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["sdk", "compatible"],
                "properties": {
                    "sdk": { "type": "string" },
                    "compatible": { "type": "boolean" },
                    // Present exactly when the axis analyzed with errors
                    // (`skip_serializing_if`, compatibility.rs:83-84).
                    "first_failure": failure,
                    // Present exactly when the SDK failed to load.
                    "error": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["kind", "message"],
                        "properties": {
                            "kind": { "enum": ERROR_KINDS },
                            "message": { "type": "string" },
                        },
                    },
                },
            }},
        },
    })
});

/// The typed axis-load-failure markers (`AxisError::SDK_UNAVAILABLE` is the
/// only kind today, compatibility.rs:100-103; a new kind extends this list
/// additively).
pub const ERROR_KINDS: &[&str] = &["sdk_unavailable"];

const COMPAT_TOP: &[&str] = &["schema_version", "file", "matrix"];
const AXIS: &[&str] = &["sdk", "compatible"];
const AXIS_OPTIONAL: &[&str] = &["first_failure", "error"];
const FAILURE: &[&str] = &["code", "file", "line", "col", "message"];
const AXIS_ERROR: &[&str] = &["kind", "message"];

pub(crate) fn validate_compat(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, COMPAT_TOP, &[], "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_COMPAT)?;
    string(field(root, "file", "")?, "file")?;
    for (i, row) in rows(root, "matrix")? {
        let p = format!("matrix[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, AXIS, AXIS_OPTIONAL, &p)?;
        string(field(obj, "sdk", &p)?, &join(&p, "sdk"))?;
        boolean(field(obj, "compatible", &p)?, &join(&p, "compatible"))?;
        if let Some(failure) = obj.get("first_failure") {
            let fp = join(&p, "first_failure");
            let failure = object(failure, &fp)?;
            exact_fields(failure, FAILURE, &[], &fp)?;
            string(field(failure, "code", &fp)?, &join(&fp, "code"))?;
            string(field(failure, "file", &fp)?, &join(&fp, "file"))?;
            let line = join(&fp, "line");
            one_based(field(failure, "line", &fp)?, &line)?;
            let col = join(&fp, "col");
            one_based(field(failure, "col", &fp)?, &col)?;
            string(field(failure, "message", &fp)?, &join(&fp, "message"))?;
        }
        if let Some(error) = obj.get("error") {
            let ep = join(&p, "error");
            let error = object(error, &ep)?;
            exact_fields(error, AXIS_ERROR, &[], &ep)?;
            enum_spelling(field(error, "kind", &ep)?, &join(&ep, "kind"), ERROR_KINDS)?;
            string(field(error, "message", &ep)?, &join(&ep, "message"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): minimal keeps the
/// verdict + axis identity — the evidence columns are the compact tier's
/// content, one `check` away (crate docs, compat table). At compact and
/// full the columns appear exactly when the axis's content demands them,
/// so nothing is forbidden there.
pub(crate) fn validate_compat_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_compat(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "matrix")? {
        let p = format!("matrix[{i}]");
        let obj = object(row, &p)?;
        absent(obj, "first_failure", &p, tier)?;
        absent(obj, "error", &p, tier)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let compat = compat_schema();
        assert_eq!(
            crate::support::schema_keys(compat),
            crate::support::const_names(COMPAT_TOP)
        );
        let axis = &compat["properties"]["matrix"]["items"];
        let mut axis_fields: Vec<String> = AXIS
            .iter()
            .chain(AXIS_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        axis_fields.sort_unstable();
        assert_eq!(crate::support::schema_keys(axis), axis_fields);
        assert_eq!(
            crate::support::schema_keys(&axis["properties"]["first_failure"]),
            crate::support::const_names(FAILURE)
        );
        assert_eq!(
            crate::support::schema_keys(&axis["properties"]["error"]),
            crate::support::const_names(AXIS_ERROR)
        );
    }

    /// Tier exactness: a minimal answer carries the verdict + axis identity
    /// only; the same answer with evidence columns is exact at compact.
    #[test]
    fn minimal_tier_rejects_evidence_columns() {
        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION_COMPAT,
            "file": "/w/lib/a.dart",
            "matrix": [{
                "sdk": "3.5.0",
                "compatible": false,
                "first_failure": {
                    "code": "uri_does_not_exist",
                    "file": "/w/lib/a.dart",
                    "line": 1,
                    "col": 8,
                    "message": "Target of URI doesn't exist: dart:async",
                },
            }],
        });
        let violation = validate_compat_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry evidence");
        assert_eq!(violation.path, "matrix[0].first_failure");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_compat_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_compat_at_tier(&doc, Verbosity::Full).is_ok());
    }
}
