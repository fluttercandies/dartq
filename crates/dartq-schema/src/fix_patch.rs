//! The §15.6 fix patch document — `dartq fix --format=machine`, one
//! single-line object per patched file.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{
    boolean, exact_fields, field, join, non_negative, object, one_based, pinned_version, rows,
    string,
};

/// The patch document's contract version. Mirrors the CLI fix face's
/// `SCHEMA_VERSION` (`crates/dartq-cli/src/fix.rs:71`), which has no pub
/// constant to align against — the value is pinned by the fixtures in
/// `tests/output_faces.rs` instead.
pub const SCHEMA_VERSION_FIX_PATCH: u32 = 1;

/// The published fix patch schema (JSON Schema draft-07 subset).
#[must_use]
pub fn fix_patch_schema() -> &'static Value {
    &FIX_PATCH_SCHEMA
}

static FIX_PATCH_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartiq/schema/v1/fix-patch.json",
        "title": "dartiq fix patch document (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["file", "schema_version", "edits"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_FIX_PATCH },
            "file": { "type": "string" },
            "edits": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["offset", "length", "replacement"],
                "properties": {
                    "offset": { "type": "integer", "minimum": 0 },
                    "length": { "type": "integer", "minimum": 0 },
                    "replacement": { "type": "string" },
                    // The §15.6 drift-correction locator: compact/full emit
                    // it, minimal omits it (crate docs, "Verbosity tiers").
                    "anchor": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["line", "column"],
                        "properties": {
                            "line": { "type": "integer", "minimum": 1 },
                            "column": { "type": "integer", "minimum": 1 },
                            // Empty at line boundaries; §15.8.5 omission.
                            "context_before": { "type": "string" },
                            "context_after": { "type": "string" },
                        },
                    },
                    // Reserved §15.6 surface, not yet emitted by the fix
                    // face (the human row carries the rule; stale edits are
                    // skipped rather than marked). Declared absent-tolerant
                    // so enabling them is additive, not an unfreeze.
                    "rule": { "type": "string" },
                    "stale": { "type": "boolean" },
                },
            }},
        },
    })
});

const PATCH_TOP: &[&str] = &["file", "schema_version", "edits"];
const EDIT: &[&str] = &["offset", "length", "replacement"];
const EDIT_OPTIONAL: &[&str] = &["anchor", "rule", "stale"];
const ANCHOR: &[&str] = &["line", "column"];
const ANCHOR_OPTIONAL: &[&str] = &["context_before", "context_after"];

pub(crate) fn validate_fix_patch(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, PATCH_TOP, &[], "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_FIX_PATCH)?;
    string(field(root, "file", "")?, "file")?;
    for (i, row) in rows(root, "edits")? {
        let p = format!("edits[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, EDIT, EDIT_OPTIONAL, &p)?;
        let offset = join(&p, "offset");
        non_negative(field(obj, "offset", &p)?, &offset)?;
        let length = join(&p, "length");
        non_negative(field(obj, "length", &p)?, &length)?;
        string(
            field(obj, "replacement", &p)?,
            &join(&p, "replacement"),
        )?;
        if let Some(anchor) = obj.get("anchor") {
            let ap = join(&p, "anchor");
            let anchor = object(anchor, &ap)?;
            exact_fields(anchor, ANCHOR, ANCHOR_OPTIONAL, &ap)?;
            let line = join(&ap, "line");
            one_based(field(anchor, "line", &ap)?, &line)?;
            let column = join(&ap, "column");
            one_based(field(anchor, "column", &ap)?, &column)?;
            if let Some(before) = anchor.get("context_before") {
                string(before, &join(&ap, "context_before"))?;
            }
            if let Some(after) = anchor.get("context_after") {
                string(after, &join(&ap, "context_after"))?;
            }
        }
        if let Some(rule) = obj.get("rule") {
            string(rule, &join(&p, "rule"))?;
        }
        if let Some(stale) = obj.get("stale") {
            boolean(stale, &join(&p, "stale"))?;
        }
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
        let patch = fix_patch_schema();
        assert_eq!(crate::support::schema_keys(patch), crate::support::const_names(PATCH_TOP));
        let edit = &patch["properties"]["edits"]["items"];
        let mut edit_fields: Vec<String> = EDIT
            .iter()
            .chain(EDIT_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        edit_fields.sort_unstable();
        assert_eq!(crate::support::schema_keys(edit), edit_fields);
        let anchor = &edit["properties"]["anchor"];
        let mut anchor_fields: Vec<String> = ANCHOR
            .iter()
            .chain(ANCHOR_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        anchor_fields.sort_unstable();
        assert_eq!(crate::support::schema_keys(anchor), anchor_fields);
    }
}
