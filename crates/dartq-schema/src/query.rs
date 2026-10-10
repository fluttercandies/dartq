//! The `dartq query` JSON face (§15.2) — today the `query symbol` answer,
//! one single-line object (`crates/dartq-cli/src/main.rs:954-997`). Future
//! query kinds (`references`/`implementations`/`dependencies`/`callers`)
//! extend this envelope with additive optional arrays under the same
//! version — see the crate docs, versioning discipline.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{
    enum_spelling, exact_fields, field, join, non_negative, object, one_based, pinned_version,
    rows, session_fields, string, string_or_null,
};

/// The query answer's contract version. No producer constant exists yet —
/// the CLI writer does not emit the field today (§15.7 gap tracked for the
/// CLI lane); the value is pinned by the fixtures in
/// `tests/output_faces.rs`.
pub const SCHEMA_VERSION_QUERY: u32 = 1;

/// The published query answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn query_schema() -> &'static Value {
    &QUERY_SCHEMA
}

static QUERY_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartiq/schema/v1/query-answer.json",
        "title": "dartiq query answer (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "name", "declarations", "references"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_QUERY },
            "name": { "type": "string" },
            "declarations": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["file", "kind", "name", "line", "column", "length"],
                "properties": {
                    "file": { "type": "string" },
                    // `-` on the machine face (main.rs:931); JSON null for
                    // top-level rows (main.rs:967) — a total column, not an
                    // omitted one.
                    "container": { "type": ["string", "null"] },
                    "kind": { "enum": DECL_KINDS },
                    "name": { "type": "string" },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "length": { "type": "integer", "minimum": 0 },
                },
            }},
            "references": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["target_file", "kind", "offset", "length"],
                "properties": {
                    "target_file": { "type": "string" },
                    "kind": { "enum": REF_KINDS },
                    "qualified": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 },
                    "length": { "type": "integer", "minimum": 0 },
                },
            }},
            // §15.8 session token economics (crate docs, "Token economics").
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

/// The stable machine spellings of the declaration families
/// (`main.rs:1001-1017`, exhaustive over `dartq-engine`'s `DeclKind`).
pub const DECL_KINDS: &[&str] = &[
    "class",
    "enum",
    "mixin",
    "extension",
    "extension-type",
    "type-alias",
    "function",
    "variable",
    "type-param",
    "method",
    "getter",
    "setter",
    "field",
];

/// The stable machine spellings of the reference relations
/// (`main.rs:1020-1028`, exhaustive over `RefKind`).
pub const REF_KINDS: &[&str] = &["reference", "read", "invoked", "written", "read-written"];

const QUERY_TOP: &[&str] = &["schema_version", "name", "declarations", "references"];
const QUERY_SESSION: &[&str] = &["symbols", "since"];
const DECLARATION: &[&str] = &["file", "kind", "name", "line", "column", "length"];
const DECLARATION_OPTIONAL: &[&str] = &["container"];
const REFERENCE: &[&str] = &["target_file", "kind", "offset", "length"];
const REFERENCE_OPTIONAL: &[&str] = &["qualified"];

pub(crate) fn validate_query(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, QUERY_TOP, QUERY_SESSION, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_QUERY)?;
    string(field(root, "name", "")?, "name")?;
    session_fields(root)?;
    for (i, row) in rows(root, "declarations")? {
        let p = format!("declarations[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, DECLARATION, DECLARATION_OPTIONAL, &p)?;
        string(field(obj, "file", &p)?, &join(&p, "file"))?;
        enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), DECL_KINDS)?;
        string(field(obj, "name", &p)?, &join(&p, "name"))?;
        let line = join(&p, "line");
        one_based(field(obj, "line", &p)?, &line)?;
        let column = join(&p, "column");
        one_based(field(obj, "column", &p)?, &column)?;
        let length = join(&p, "length");
        non_negative(field(obj, "length", &p)?, &length)?;
        if let Some(container) = obj.get("container") {
            string_or_null(container, &join(&p, "container"))?;
        }
    }
    for (i, row) in rows(root, "references")? {
        let p = format!("references[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, REFERENCE, REFERENCE_OPTIONAL, &p)?;
        string(
            field(obj, "target_file", &p)?,
            &join(&p, "target_file"),
        )?;
        enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), REF_KINDS)?;
        let offset = join(&p, "offset");
        non_negative(field(obj, "offset", &p)?, &offset)?;
        let length = join(&p, "length");
        non_negative(field(obj, "length", &p)?, &length)?;
        if let Some(qualified) = obj.get("qualified") {
            string(qualified, &join(&p, "qualified"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::schema_keys;

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let query = query_schema();
        let mut top: Vec<String> = QUERY_TOP
            .iter()
            .chain(QUERY_SESSION.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(query), top);
        let declarations = &query["properties"]["declarations"]["items"];
        let mut decl_fields: Vec<String> = DECLARATION
            .iter()
            .chain(DECLARATION_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        decl_fields.sort_unstable();
        assert_eq!(schema_keys(declarations), decl_fields);
        let references = &query["properties"]["references"]["items"];
        let mut ref_fields: Vec<String> = REFERENCE
            .iter()
            .chain(REFERENCE_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        ref_fields.sort_unstable();
        assert_eq!(schema_keys(references), ref_fields);
    }

    #[test]
    fn enum_spellings_match_the_declared_lists() {
        let query = query_schema();
        let decl_enum = &query["properties"]["declarations"]["items"]["properties"]["kind"];
        let ref_enum = &query["properties"]["references"]["items"]["properties"]["kind"];
        for (property, declared) in [(decl_enum, DECL_KINDS), (ref_enum, REF_KINDS)] {
            let spellings: Vec<&str> = property["enum"]
                .as_array()
                .expect("enum list")
                .iter()
                .map(|v| v.as_str().expect("spelling"))
                .collect();
            assert_eq!(spellings, declared);
        }
    }
}
