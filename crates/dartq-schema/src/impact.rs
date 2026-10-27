//! The impact report — `dartq-ai`'s `impact` query output. The row
//! shapes are tier-invariant (crate docs, "Verbosity tiers"): every field is
//! a locator or a verdict, so `minimal` cannot shrink a row. The session
//! fields (`symbols`/`since`) are the face's token levers.
//!
//! The path columns spell through the one file locator (root-relative
//! inside the analysis root, absolute outside it — the `sym` label half
//! included), and the envelope's optional `root` (the absolute analysis
//! root) rides exactly when a file-bearing row does (`sym`/`files`/
//! `tests`).

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{
    boolean, enum_spelling, exact_fields, field, join, object, pinned_version, rows,
    session_fields, string, symbol_ref,
};

/// The impact report's output contract version. Mirrors the
/// authoritative `IMPACT_SCHEMA_VERSION` in `dartq-ai`; equality is pinned
/// by an alignment test (`tests/alignment.rs`). v2: `signature_changed`
/// consults the member-level signature digests instead of the v1
/// name-presence proxy — the field keeps its shape and name, the meaning
/// underneath changed (the freeze discipline's reuse-the-field major).
/// v3: `affected[].reference_lines` joins the row — the 1-based source
/// lines of that file's resolved references to the change's propagation
/// set (sorted, deduplicated; empty under a transitive-only hit).
pub const SCHEMA_VERSION_IMPACT: u32 = 3;

/// The published impact report schema (JSON Schema draft-07 subset).
#[must_use]
pub fn impact_schema() -> &'static Value {
    &IMPACT_SCHEMA
}

static IMPACT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v3/impact-report.json",
        "title": "dartq impact report (schema_version 3)",
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "sym", "files", "tests", "envs"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_IMPACT },
            // The analysis root the path columns locate against
            // (absolute): rides exactly when a file-bearing row does
            // (`sym`/`files`/`tests`), omitted on the all-empty report.
            "root": { "type": "string" },
            "sym": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["sym", "sig_changed"],
                "properties": {
                    "sym": { "type": "string" },
                    "sig_changed": { "type": "boolean" },
                },
            }},
            "files": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "impact", "lines"],
                "properties": {
                    "path": { "type": "string" },
                    "impact": { "enum": ["content-only", "semantic"] },
                    "lines": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                    },
                },
            }},
            "tests": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                },
            }},
            "envs": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["label", "compatible"],
                "properties": {
                    "label": { "type": "string" },
                    "compatible": { "type": "boolean" },
                },
            }},
            // Session token economics, declared before any emitter
            // exists so v1 does not unfreeze when they land (crate docs,
            // "Token economics").
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

const IMPACT_TOP: &[&str] = &[
    "v",
    "sym",
    "files",
    "tests",
    "envs",
];
/// The locator envelope's key — the analysis root the path columns locate
/// against, riding exactly when a file-bearing row does.
const IMPACT_LOCATOR: &[&str] = &["root"];
const IMPACT_SESSION: &[&str] = &["symbols", "since"];
const CHANGED_SYMBOL: &[&str] = &["sym", "sig_changed"];
const AFFECTED_TARGET: &[&str] = &["path", "impact", "lines"];
const AFFECTED_TEST: &[&str] = &["path"];
const ENVIRONMENT_IMPACT: &[&str] = &["label", "compatible"];
const IMPACT_LEVELS: &[&str] = &["content-only", "semantic"];

pub(crate) fn validate_impact(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    // The locator envelope and the session fields share the optional slot:
    // declared, absent-tolerant, unknown-key rejection intact.
    let optional: Vec<&str> = IMPACT_LOCATOR
        .iter()
        .chain(IMPACT_SESSION.iter())
        .copied()
        .collect();
    exact_fields(root, IMPACT_TOP, &optional, "")?;
    pinned_version(root, "v", SCHEMA_VERSION_IMPACT)?;
    if let Some(locator_root) = root.get("root") {
        string(locator_root, "root")?;
    }
    session_fields(root)?;
    for (i, row) in rows(root, "sym")? {
        let p = format!("sym[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, CHANGED_SYMBOL, &[], &p)?;
        symbol_ref(field(obj, "sym", &p)?, &join(&p, "sym"))?;
        boolean(
            field(obj, "sig_changed", &p)?,
            &join(&p, "sig_changed"),
        )?;
    }
    for (i, row) in rows(root, "files")? {
        let p = format!("files[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, AFFECTED_TARGET, &[], &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
        enum_spelling(
            field(obj, "impact", &p)?,
            &join(&p, "impact"),
            IMPACT_LEVELS,
        )?;
        let lines = field(obj, "lines", &join(&p, "lines"))?;
        let Some(items) = lines.as_array() else {
            return Err(SchemaViolation {
                path: join(&p, "lines"),
                expected: "an array of 1-based line integers".to_owned(),
                found: format!("{lines}"),
            });
        };
        for (j, line) in items.iter().enumerate() {
            let lp = join(&p, &format!("lines[{j}]"));
            let Some(n) = line.as_u64() else {
                return Err(SchemaViolation {
                    path: lp,
                    expected: "an integer >= 1".to_owned(),
                    found: format!("{line}"),
                });
            };
            if n == 0 {
                return Err(SchemaViolation {
                    path: lp,
                    expected: "an integer >= 1".to_owned(),
                    found: format!("{line}"),
                });
            }
        }
    }
    for (i, row) in rows(root, "tests")? {
        let p = format!("tests[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, AFFECTED_TEST, &[], &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
    }
    for (i, row) in rows(root, "envs")? {
        let p = format!("envs[{i}]");
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
            "v": SCHEMA_VERSION_IMPACT,
            "root": "/w",
            "sym": [{"sym": "a.dart|fb", "sig_changed": true}],
            "files": [{"path": "b.dart", "impact": "semantic", "lines": [3, 7]}],
            "tests": [],
            "envs": [],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level, or CI and the published contract diverge.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let impact = impact_schema();
        let mut top: Vec<String> = IMPACT_TOP
            .iter()
            .chain(IMPACT_LOCATOR.iter())
            .chain(IMPACT_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort();
        assert_eq!(schema_keys(impact), top);
        assert_eq!(
            schema_keys(&impact["properties"]["sym"]["items"]),
            const_names(CHANGED_SYMBOL)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["files"]["items"]),
            const_names(AFFECTED_TARGET)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["tests"]["items"]),
            const_names(AFFECTED_TEST)
        );
        assert_eq!(
            schema_keys(&impact["properties"]["envs"]["items"]),
            const_names(ENVIRONMENT_IMPACT)
        );
    }

    #[test]
    fn version_constant_is_pinned() {
        assert!(validate_impact(&report()).is_ok());
        let mut bumped = report();
        bumped["v"] = json!(SCHEMA_VERSION_IMPACT + 1);
        let violation = validate_impact(&bumped).expect_err("bumped version must violate");
        assert_eq!(violation.path, "v");
        assert_eq!(violation.expected, "the integer 3");
    }

    #[test]
    fn missing_section_is_named() {
        let mut payload = report();
        payload
            .as_object_mut()
            .expect("object")
            .remove("tests");
        let violation = validate_impact(&payload).expect_err("missing section must violate");
        assert_eq!(violation.path, "tests");
        assert_eq!(violation.found, "missing");
    }

    /// The locator envelope: `root` rides absent-tolerant (the all-empty
    /// report carries no file field), and a non-string is named.
    #[test]
    fn the_locator_root_validates_and_a_non_string_is_named() {
        let mut payload = report();
        payload
            .as_object_mut()
            .expect("object")
            .remove("root");
        assert!(validate_impact(&payload).is_ok());

        let mut payload = report();
        payload["root"] = json!(true);
        let violation = validate_impact(&payload).expect_err("non-string root");
        assert_eq!(violation.path, "root");
        assert_eq!(violation.expected, "a string");
    }
}
