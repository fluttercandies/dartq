//! The §15.4 impact report — `dartq-ai`'s `impact` query output. The row
//! shapes are tier-invariant (crate docs, "Verbosity tiers"): every field is
//! a locator or a verdict, so `minimal` cannot shrink a row. The §15.8
//! session fields (`symbols`/`since`) are the face's token levers.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{
    boolean, enum_spelling, exact_fields, field, join, object, pinned_version, rows,
    session_fields, string, symbol_ref,
};

/// The impact report's output contract version (PRD §15.7). Mirrors the
/// authoritative `IMPACT_SCHEMA_VERSION` in `dartq-ai`; equality is pinned
/// by an alignment test (`tests/alignment.rs`).
pub const SCHEMA_VERSION_IMPACT: u32 = 1;

/// The published impact report schema (JSON Schema draft-07 subset).
#[must_use]
pub fn impact_schema() -> &'static Value {
    &IMPACT_SCHEMA
}

static IMPACT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartiq/schema/v1/impact-report.json",
        "title": "dartiq impact report (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "changed", "affected", "affected_tests", "affected_environments"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_IMPACT },
            "changed": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["symbol", "signature_changed"],
                "properties": {
                    "symbol": { "type": "string" },
                    "signature_changed": { "type": "boolean" },
                },
            }},
            "affected": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "impact"],
                "properties": {
                    "path": { "type": "string" },
                    "impact": { "enum": ["content-only", "semantic"] },
                },
            }},
            "affected_tests": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                },
            }},
            "affected_environments": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["label", "compatible"],
                "properties": {
                    "label": { "type": "string" },
                    "compatible": { "type": "boolean" },
                },
            }},
            // §15.8 session token economics, declared before any emitter
            // exists so v1 does not unfreeze when they land (crate docs,
            // "Token economics").
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

const IMPACT_TOP: &[&str] = &[
    "schema_version",
    "changed",
    "affected",
    "affected_tests",
    "affected_environments",
];
const IMPACT_SESSION: &[&str] = &["symbols", "since"];
const CHANGED_SYMBOL: &[&str] = &["symbol", "signature_changed"];
const AFFECTED_TARGET: &[&str] = &["path", "impact"];
const AFFECTED_TEST: &[&str] = &["path"];
const ENVIRONMENT_IMPACT: &[&str] = &["label", "compatible"];
const IMPACT_LEVELS: &[&str] = &["content-only", "semantic"];

pub(crate) fn validate_impact(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, IMPACT_TOP, IMPACT_SESSION, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_IMPACT)?;
    session_fields(root)?;
    for (i, row) in rows(root, "changed")? {
        let p = format!("changed[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, CHANGED_SYMBOL, &[], &p)?;
        symbol_ref(field(obj, "symbol", &p)?, &join(&p, "symbol"))?;
        boolean(
            field(obj, "signature_changed", &p)?,
            &join(&p, "signature_changed"),
        )?;
    }
    for (i, row) in rows(root, "affected")? {
        let p = format!("affected[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, AFFECTED_TARGET, &[], &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
        enum_spelling(
            field(obj, "impact", &p)?,
            &join(&p, "impact"),
            IMPACT_LEVELS,
        )?;
    }
    for (i, row) in rows(root, "affected_tests")? {
        let p = format!("affected_tests[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, AFFECTED_TEST, &[], &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
    }
    for (i, row) in rows(root, "affected_environments")? {
        let p = format!("affected_environments[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, ENVIRONMENT_IMPACT, &[], &p)?;
        string(field(obj, "label", &p)?, &join(&p, "label"))?;
        boolean(field(obj, "compatible", &p)?, &join(&p, "compatible"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    fn report() -> serde_json::Value {
        json!({
            "schema_version": SCHEMA_VERSION_IMPACT,
            "changed": [{"symbol": "a.dart|fb", "signature_changed": true}],
            "affected": [{"path": "b.dart", "impact": "semantic"}],
            "affected_tests": [],
            "affected_environments": [],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level, or CI and the published contract diverge.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let impact = impact_schema();
        let mut top: Vec<String> = IMPACT_TOP
            .iter()
            .chain(IMPACT_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(impact), top);
        assert_eq!(
            schema_keys(&impact["properties"]["changed"]["items"]),
            const_names(CHANGED_SYMBOL)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["affected"]["items"]),
            const_names(AFFECTED_TARGET)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["affected_tests"]["items"]),
            const_names(AFFECTED_TEST)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["affected_environments"]["items"]),
            const_names(ENVIRONMENT_IMPACT)
        );
    }

    #[test]
    fn version_constant_is_pinned() {
        assert!(validate_impact(&report()).is_ok());
        let mut bumped = report();
        bumped["schema_version"] = json!(SCHEMA_VERSION_IMPACT + 1);
        let violation = validate_impact(&bumped).expect_err("bumped version must violate");
        assert_eq!(violation.path, "schema_version");
        assert_eq!(violation.expected, "the integer 1");
    }

    #[test]
    fn missing_section_is_named() {
        let mut payload = report();
        payload
            .as_object_mut()
            .expect("object")
            .remove("affected_tests");
        let violation = validate_impact(&payload).expect_err("missing section must violate");
        assert_eq!(violation.path, "affected_tests");
        assert_eq!(violation.found, "missing");
    }
}
