//! The `dartq index` run summary — the row totals the analyzed
//! workspace's maintained symbol index reports, serialized by
//! `dartq_commands::index::IndexRun::to_json` as one flat object:
//! `schema_version` plus four non-negative integer totals (`files`,
//! `libraries`, `symbols`, `references`). Totals-only, by writer shape: the
//! face tallies the engine's maintained index state (capture rows and
//! reference-surface rows) and prints the sums; it emits no per-file rows
//! and no locators.
//!
//! # Writer alignment
//!
//! The writer stamps `dartq_commands::SCHEMA_VERSION` — the shared v1
//! constant — because this document did not exist when it landed; its field
//! set and this contract agree exactly (the writer's own unit test and the
//! fixture here pin the same totals). Mirroring `SCHEMA_VERSION_INDEX` in
//! the writer, as `dartq-ai` does for impact/explain/compat, is the
//! recorded writer-side follow-up: values are equal at v1, so the payloads
//! are already conformant.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{exact_fields, field, non_negative, object, pinned_version};

/// The index run summary's contract version. The writer stamps the shared
/// `dartq_commands::SCHEMA_VERSION` (see "Writer alignment" above); the
/// fixtures pin the two equal.
pub const SCHEMA_VERSION_INDEX: u32 = 1;

/// The published index run summary schema (JSON Schema draft-07 subset).
#[must_use]
pub fn index_schema() -> &'static Value {
    &INDEX_SCHEMA
}

static INDEX_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/index-run.json",
        "title": "dartq index run summary (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version",
            "files",
            "libraries",
            "symbols",
            "references",
        ],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_INDEX },
            // Member files captured into the index (IndexRun::files).
            "files": { "type": "integer", "minimum": 0 },
            // Libraries whose reference surfaces were built
            // (IndexRun::libraries).
            "libraries": { "type": "integer", "minimum": 0 },
            // Declaration rows indexed (IndexRun::symbols).
            "symbols": { "type": "integer", "minimum": 0 },
            // Resolved reference rows indexed (IndexRun::references).
            "references": { "type": "integer", "minimum": 0 },
        },
    })
});

const INDEX_TOP: &[&str] = &[
    "schema_version",
    "files",
    "libraries",
    "symbols",
    "references",
];

pub(crate) fn validate_index(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, INDEX_TOP, &[], "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_INDEX)?;
    for key in ["files", "libraries", "symbols", "references"] {
        non_negative(field(root, key, "")?, key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift pin: the published document and the validator must name the
    /// same fields.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let index = index_schema();
        assert_eq!(
            crate::support::schema_keys(index),
            crate::support::const_names(INDEX_TOP)
        );
    }

    /// Shape pin: the totals the writer emits are exactly the contract's
    /// totals — the hand-traced fixture totals from
    /// `dartq_commands::index`'s own unit test (3 files, 3 libraries, 5
    /// declaration rows, 3 resolved references) validate, and every field
    /// is required.
    #[test]
    fn the_writer_totals_are_the_published_shape() {
        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION_INDEX,
            "files": 3,
            "libraries": 3,
            "symbols": 5,
            "references": 3,
        });
        assert!(validate_index(&doc).is_ok());
        let empty = serde_json::json!({ "schema_version": SCHEMA_VERSION_INDEX });
        let violation = validate_index(&empty).expect_err("totals are required");
        assert_eq!(violation.path, "files");
        assert_eq!(violation.expected, "present");
    }

    /// The red line: an undeclared field is rejected, and a count slot
    /// carries an integer, never prose.
    #[test]
    fn unknown_fields_and_non_counts_are_named() {
        let extra = serde_json::json!({
            "schema_version": SCHEMA_VERSION_INDEX,
            "files": 0,
            "libraries": 0,
            "symbols": 0,
            "references": 0,
            "per_file": [],
        });
        let violation = validate_index(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "per_file");
        let wrong_type = serde_json::json!({
            "schema_version": SCHEMA_VERSION_INDEX,
            "files": 0,
            "libraries": 0,
            "symbols": "5",
            "references": 0,
        });
        let violation = validate_index(&wrong_type).expect_err("a count is an integer");
        assert_eq!(violation.path, "symbols");
        assert_eq!(violation.expected, "an integer");
    }
}
