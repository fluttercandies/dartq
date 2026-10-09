//! Machine-readable JSON schemas for dartiq's versioned output faces, plus
//! the lightweight structural validator CI runs against real output (PRD
//! §15.7: the diagnostics/patch/impact report schemas ship with the version,
//! in-crate, so tooling can check outputs against their published shapes).
//!
//! # Versioning discipline (PRD §15.7 / §15.8)
//!
//! * Every versioned output carries its contract version — the impact
//!   report's `schema_version` and the diagnostics envelope's `version` (the
//!   oracle's own key; see below).
//! * A breaking output change bumps the major. Additive evolution still goes
//!   through a schema review: this validator rejects undeclared fields, so a
//!   new field forces a documented schema change.
//! * Optional fields are omitted rather than null (§15.8.5) — the
//!   diagnostics envelope's `correctionMessage`/`documentation` follow this;
//!   impact report sections are always present, possibly as empty arrays.
//! * Every entity reference is the stable `label|name` path form; internal
//!   ids never leave the engine. The validator pins the `label|name` form
//!   (by containment) for the impact report's symbol refs.
//! * Enum spellings are the shortest unambiguous form: kebab-case tags for
//!   impact levels, the oracle's UPPERCASE spellings for diagnostic
//!   severities and types.
//!
//! # Domains
//!
//! * [`Domain::Impact`] — the §15.4 impact report emitted by the `dartq-ai`
//!   crate's `impact` query. [`SCHEMA_VERSION_IMPACT`] mirrors the
//!   authoritative `IMPACT_SCHEMA_VERSION` there (pinned equal by test).
//! * [`Domain::Diagnostics`] — the `dart analyze --format=json` envelope the
//!   `dartq-diagnostics` crate emits: `{"version":1,"diagnostics":[…]}`.
//!   The key is `version`, not §15.7's generic `schema_version`, because the
//!   envelope is byte-compatible with the oracle; [`SCHEMA_VERSION_DIAGNOSTICS`]
//!   names that key's contract version. Recorded gap: the envelope predates
//!   the §15.7 naming; renaming it would break oracle byte-compatibility, so
//!   the schema models the shape as emitted.
//!
//! # What [`validate`] checks — and what it does not
//!
//! The documents ([`impact_schema`], [`diagnostics_schema`]) are hand-written
//! JSON Schema draft-07 subset documents — the published contract.
//! [`validate`] is a hand-rolled structural checker over the same shapes,
//! not a JSON Schema engine: it parses the payload, pins the version
//! constant, requires the declared fields with their declared JSON types,
//! enforces the exact enum spellings, and rejects undeclared fields. It
//! reports the first violation as a [`SchemaViolation`] (field path +
//! expected vs found). Anything richer (patterns, conditional schemas,
//! per-code registries) is out of scope; the differential harness covers
//! semantics, this covers shape.
//!
//! # CI consumption
//!
//! CI (and the harness) serializes a real output face — or captures it from
//! the CLI/daemon — and calls [`validate`] with the producing [`Domain`]; any
//! [`SchemaViolation`] fails the run via its `Display`. New faces (the
//! compatibility and fix reports) land as new `Domain` variants plus their
//! documents.

use std::fmt;
use std::sync::LazyLock;

use serde_json::{Map, Value, json};

/// The impact report's output contract version (PRD §15.7). Mirrors the
/// authoritative `IMPACT_SCHEMA_VERSION` in `dartq-ai`; equality is pinned
/// by an alignment test.
pub const SCHEMA_VERSION_IMPACT: u32 = 1;

/// The diagnostics JSON envelope's contract version — the oracle's
/// `version` key (PRD §15.7 gap: the envelope predates the generic
/// `schema_version` naming and stays byte-compatible with the oracle).
pub const SCHEMA_VERSION_DIAGNOSTICS: u32 = 1;

/// Which published schema a payload is validated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// The §15.4 impact report (`dartq-ai`'s `impact` query output).
    Impact,
    /// The `dart analyze --format=json` envelope (`dartq-diagnostics`).
    Diagnostics,
}

/// The first structural violation found in a payload, named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaViolation {
    /// Field path of the violation: `schema_version`,
    /// `affected[0].impact`, `diagnostics[0].location.range.start.line`.
    /// Empty for whole-payload failures (JSON syntax, root not an object).
    pub path: String,
    /// What the schema requires at that path.
    pub expected: String,
    /// What the payload actually had there.
    pub found: String,
}

impl fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "expected {}, found {}", self.expected, self.found)
        } else {
            write!(
                f,
                "at `{}`: expected {}, found {}",
                self.path, self.expected, self.found
            )
        }
    }
}

impl std::error::Error for SchemaViolation {}

/// Validates a serialized output payload against its domain's published
/// schema: JSON syntax, version constant, required fields and types, exact
/// enum spellings, `label|name` symbol refs, and no undeclared fields.
/// Reports the first violation.
///
/// # Errors
///
/// Returns a [`SchemaViolation`] naming the first check the payload fails;
/// `Ok(())` means the payload matches the published v1 shape.
pub fn validate(payload: &str, domain: Domain) -> Result<(), SchemaViolation> {
    let value = serde_json::from_str::<Value>(payload).map_err(|error| SchemaViolation {
        path: String::new(),
        expected: "a JSON document".to_owned(),
        found: error.to_string(),
    })?;
    match domain {
        Domain::Impact => validate_impact(&value),
        Domain::Diagnostics => validate_diagnostics(&value),
    }
}

/// The published impact report schema (JSON Schema draft-07 subset).
#[must_use]
pub fn impact_schema() -> &'static Value {
    &IMPACT_SCHEMA
}

/// The published diagnostics JSON envelope schema (JSON Schema draft-07
/// subset). Models the shape as emitted — the `version` key included.
#[must_use]
pub fn diagnostics_schema() -> &'static Value {
    &DIAGNOSTICS_SCHEMA
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
        },
    })
});

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
        "$id": "dartiq/schema/v1/diagnostics-json.json",
        "title": "dartiq diagnostics JSON envelope (version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["version", "diagnostics"],
        "properties": {
            "version": { "const": SCHEMA_VERSION_DIAGNOSTICS },
            "diagnostics": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "severity", "type", "location", "problemMessage"],
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

const IMPACT_TOP: &[&str] = &[
    "schema_version",
    "changed",
    "affected",
    "affected_tests",
    "affected_environments",
];
const CHANGED_SYMBOL: &[&str] = &["symbol", "signature_changed"];
const AFFECTED_TARGET: &[&str] = &["path", "impact"];
const AFFECTED_TEST: &[&str] = &["path"];
const ENVIRONMENT_IMPACT: &[&str] = &["label", "compatible"];
const IMPACT_LEVELS: &[&str] = &["content-only", "semantic"];

const DIAGNOSTICS_TOP: &[&str] = &["version", "diagnostics"];
const DIAGNOSTIC: &[&str] = &["code", "severity", "type", "location", "problemMessage"];
const DIAGNOSTIC_OPTIONAL: &[&str] = &["correctionMessage", "documentation"];
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

fn validate_impact(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, IMPACT_TOP, &[], "")?;
    let version = field(root, "schema_version", "")?;
    if version.as_u64() != Some(u64::from(SCHEMA_VERSION_IMPACT)) {
        return Err(violation(
            "schema_version",
            format!("the integer {SCHEMA_VERSION_IMPACT}"),
            describe(version),
        ));
    }
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

fn validate_diagnostics(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, DIAGNOSTICS_TOP, &[], "")?;
    let version = field(root, "version", "")?;
    if version.as_u64() != Some(u64::from(SCHEMA_VERSION_DIAGNOSTICS)) {
        return Err(violation(
            "version",
            format!("the integer {SCHEMA_VERSION_DIAGNOSTICS}"),
            describe(version),
        ));
    }
    for (i, row) in rows(root, "diagnostics")? {
        let p = format!("diagnostics[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, DIAGNOSTIC, DIAGNOSTIC_OPTIONAL, &p)?;
        string(field(obj, "code", &p)?, &join(&p, "code"))?;
        enum_spelling(
            field(obj, "severity", &p)?,
            &join(&p, "severity"),
            SEVERITIES,
        )?;
        enum_spelling(field(obj, "type", &p)?, &join(&p, "type"), DIAGNOSTIC_TYPES)?;
        let lp = join(&p, "location");
        let location = object(field(obj, "location", &p)?, &lp)?;
        exact_fields(location, LOCATION, &[], &lp)?;
        string(field(location, "file", &lp)?, &join(&lp, "file"))?;
        let rp = join(&lp, "range");
        let range = object(field(location, "range", &lp)?, &rp)?;
        exact_fields(range, RANGE, &[], &rp)?;
        position(field(range, "start", &rp)?, &join(&rp, "start"))?;
        position(field(range, "end", &rp)?, &join(&rp, "end"))?;
        string(
            field(obj, "problemMessage", &p)?,
            &join(&p, "problemMessage"),
        )?;
        if let Some(m) = obj.get("correctionMessage") {
            string(m, &join(&p, "correctionMessage"))?;
        }
        if let Some(d) = obj.get("documentation") {
            string(d, &join(&p, "documentation"))?;
        }
    }
    Ok(())
}

fn violation(
    path: impl Into<String>,
    expected: impl Into<String>,
    found: impl Into<String>,
) -> SchemaViolation {
    SchemaViolation {
        path: path.into(),
        expected: expected.into(),
        found: found.into(),
    }
}

/// `"" + "key"` is `"key"`, not `".key"`.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// Names containers by kind (an error message never needs their contents)
/// and renders scalars verbatim (`"semanticx"`, `2`, `1.0`, `null`).
fn describe(value: &Value) -> String {
    match value {
        Value::Object(_) => "an object".to_owned(),
        Value::Array(_) => "an array".to_owned(),
        other => other.to_string(),
    }
}

fn object<'v>(value: &'v Value, path: &str) -> Result<&'v Map<String, Value>, SchemaViolation> {
    value
        .as_object()
        .ok_or_else(|| violation(path, "an object", describe(value)))
}

fn array<'v>(value: &'v Value, path: &str) -> Result<&'v [Value], SchemaViolation> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| violation(path, "an array", describe(value)))
}

fn string<'v>(value: &'v Value, path: &str) -> Result<&'v str, SchemaViolation> {
    value
        .as_str()
        .ok_or_else(|| violation(path, "a string", describe(value)))
}

fn boolean(value: &Value, path: &str) -> Result<bool, SchemaViolation> {
    value
        .as_bool()
        .ok_or_else(|| violation(path, "a boolean", describe(value)))
}

fn integer(value: &Value, path: &str) -> Result<u64, SchemaViolation> {
    value
        .as_u64()
        .ok_or_else(|| violation(path, "an integer", describe(value)))
}

fn field<'v>(
    obj: &'v Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'v Value, SchemaViolation> {
    obj.get(key)
        .ok_or_else(|| violation(join(path, key), "present", "missing"))
}

fn rows<'v>(
    root: &'v Map<String, Value>,
    key: &str,
) -> Result<impl Iterator<Item = (usize, &'v Value)>, SchemaViolation> {
    Ok(array(field(root, key, "")?, key)?.iter().enumerate())
}

fn enum_spelling(value: &Value, path: &str, allowed: &[&str]) -> Result<(), SchemaViolation> {
    let spelling = string(value, path)?;
    let expected = allowed
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(" or ");
    if allowed.contains(&spelling) {
        Ok(())
    } else {
        Err(violation(path, expected, describe(value)))
    }
}

/// The §15.7 stable path form `label|name` — pinned by containment; the
/// label grammar is the producer's business.
fn symbol_ref(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let r#ref = string(value, path)?;
    if r#ref.contains('|') {
        Ok(())
    } else {
        Err(violation(
            path,
            "a `label|name` path reference",
            describe(value),
        ))
    }
}

fn position(value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let obj = object(value, path)?;
    exact_fields(obj, POSITION, &[], path)?;
    for (key, minimum) in [("offset", 0), ("line", 1), ("column", 1)] {
        let p = join(path, key);
        let n = integer(field(obj, key, path)?, &p)?;
        if n < minimum {
            return Err(violation(
                p,
                format!("an integer >= {minimum}"),
                n.to_string(),
            ));
        }
    }
    Ok(())
}

fn exact_fields(
    obj: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    path: &str,
) -> Result<(), SchemaViolation> {
    for key in obj.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            let declared = if optional.is_empty() {
                format!("one of: {}", required.join(", "))
            } else {
                format!(
                    "one of: {}; optional: {}",
                    required.join(", "),
                    optional.join(", ")
                )
            };
            return Err(violation(join(path, key), declared, "an undeclared field"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value["properties"]
            .as_object()
            .expect("properties object")
            .keys()
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    }

    fn names(consts: &[&str]) -> Vec<String> {
        let mut names = consts.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    /// Drift pin: the published documents and the validator must name the
    /// same fields at every level, or CI and the published contract diverge.
    #[test]
    fn documents_and_validator_name_the_same_fields() {
        let impact = impact_schema();
        assert_eq!(keys(impact), names(IMPACT_TOP));
        assert_eq!(
            keys(&impact["properties"]["changed"]["items"]),
            names(CHANGED_SYMBOL)
        );
        assert_eq!(
            keys(&impact["properties"]["affected"]["items"]),
            names(AFFECTED_TARGET)
        );
        assert_eq!(
            keys(&impact["properties"]["affected_tests"]["items"]),
            names(AFFECTED_TEST)
        );
        assert_eq!(
            keys(&impact["properties"]["affected_environments"]["items"]),
            names(ENVIRONMENT_IMPACT)
        );

        let diagnostics = diagnostics_schema();
        assert_eq!(keys(diagnostics), names(DIAGNOSTICS_TOP));
        let item = &diagnostics["properties"]["diagnostics"]["items"];
        let mut item_fields: Vec<String> = DIAGNOSTIC
            .iter()
            .chain(DIAGNOSTIC_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        item_fields.sort_unstable();
        assert_eq!(keys(item), item_fields);
        assert_eq!(keys(&item["properties"]["location"]), names(LOCATION));
        assert_eq!(
            keys(&item["properties"]["location"]["properties"]["range"]),
            names(RANGE)
        );
        let range = &item["properties"]["location"]["properties"]["range"];
        assert_eq!(keys(&range["properties"]["start"]), names(POSITION));
        assert_eq!(keys(&range["properties"]["end"]), names(POSITION));
    }
}
