//! The fix patch document — `dartq fix --format=machine`, one
//! single-line object per patched file.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, boolean, describe, edit_floor, enum_spelling, exact_fields, field, join, object,
    pinned_version, rows, string, violation,
};

/// The patch document's contract version. Mirrors the fix face's
/// `SCHEMA_VERSION` (`crates/dartq-commands/src/fix.rs:87`, the one
/// implementation the CLI and daemon faces share), which has no pub
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
        "$id": "dartq/schema/v1/fix-patch.json",
        "title": "dartq fix patch document (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["file", "v", "edits"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_FIX_PATCH },
            "file": { "type": "string" },
            "edits": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["off", "len", "text", "src", "role"],
                "properties": {
                    "off": { "type": "integer", "minimum": 0 },
                    "len": { "type": "integer", "minimum": 0 },
                    "text": { "type": "string" },
                    // The provenance tag (`edit_floor`,
                    // crates/dartq-commands/src/fix.rs:744-751): `builtin-lint`
                    // for the built-in rule rows, the plugin's manifest name
                    // for plugin rows. Never dropped — every tier emits it
                    //.
                    "src": { "type": "string" },
                    // The candidate's role (the fix face's `FixRole`,
                    // tagged at the producer): `primary` for a code change
                    // that resolves the diagnostic (plugin rows included),
                    // the three suppression spellings for the ignore trio.
                    // Rides every tier like `src`.
                    "role": {
                        "type": "string",
                        "enum": ["primary", "ignore_line", "ignore_file", "suppress"],
                    },
                    // The drift-correction locator: compact/full emit
                    // it, minimal omits it (crate docs, "Verbosity tiers";
                    // fix.rs:766-768, 778-780).
                    "anchor": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["line", "col"],
                        "properties": {
                            "line": { "type": "integer", "minimum": 1 },
                            "col": { "type": "integer", "minimum": 1 },
                            // Empty at line boundaries; the omission convention.
                            "before": { "type": "string" },
                            "after": { "type": "string" },
                        },
                    },
                    // The full tier's human-name rows (fix.rs:769-772 for
                    // the built-in rows, 781-784 for the plugin rows).
                    "rule": { "type": "string" },
                    "msg": { "type": "string" },
                    // Plugin rows only ("lower sorts first" — may be
                    // negative, fix.rs:139-141); structural validator, so
                    // the row-kind split is the emitter's obligation.
                    "priority": { "type": "integer" },
                    // Reserved patch surface, not yet emitted by the fix
                    // face (stale edits are skipped rather than marked).
                    // Declared absent-tolerant so enabling it is additive,
                    // not an unfreeze.
                    "stale": { "type": "boolean" },
                },
            }},
        },
    })
});

const PATCH_TOP: &[&str] = &["file", "v", "edits"];
const EDIT: &[&str] = &["off", "len", "text", "src", "role"];
const EDIT_OPTIONAL: &[&str] = &["anchor", "rule", "msg", "priority", "stale"];
const ROLE_SPELLINGS: &[&str] = &["primary", "ignore_line", "ignore_file", "suppress"];

pub(crate) fn validate_fix_patch(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, PATCH_TOP, &[], "")?;
    pinned_version(root, "v", SCHEMA_VERSION_FIX_PATCH)?;
    string(field(root, "file", "")?, "file")?;
    for (i, row) in rows(root, "edits")? {
        let p = format!("edits[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, EDIT, EDIT_OPTIONAL, &p)?;
        // The red-line trio, the provenance tag, the candidate's role,
        // and the optional anchor — the shared edit floor
        // (support::edit_floor).
        edit_floor(obj, &p)?;
        enum_spelling(field(obj, "role", &p)?, &join(&p, "role"), ROLE_SPELLINGS)?;
        if let Some(rule) = obj.get("rule") {
            string(rule, &join(&p, "rule"))?;
        }
        if let Some(message) = obj.get("msg") {
            string(message, &join(&p, "msg"))?;
        }
        if let Some(priority) = obj.get("priority") {
            // Signed: "lower sorts first" (fix.rs:139-141), so negatives are
            // legal plugin priorities — the shared `integer` helper is
            // non-negative only.
            priority
                .as_i64()
                .ok_or_else(|| violation(join(&p, "priority"), "an integer", describe(priority)))?;
        }
        if let Some(stale) = obj.get("stale") {
            boolean(stale, &join(&p, "stale"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): the anchor is the
/// compact tier's drift-correction locator and the human-name rows
/// (`rule`/`message`/`priority`) are full-tier only; the red-line trio,
/// the provenance tag, the candidate's `role`, and `stale` ride every
/// tier (crate docs, fix table).
pub(crate) fn validate_fix_patch_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_fix_patch(value)?;
    let root = object(value, "")?;
    for (i, row) in rows(root, "edits")? {
        let p = format!("edits[{i}]");
        let obj = object(row, &p)?;
        if tier == Verbosity::Minimal {
            absent(obj, "anchor", &p, tier)?;
        }
        if tier != Verbosity::Full {
            absent(obj, "rule", &p, tier)?;
            absent(obj, "msg", &p, tier)?;
            absent(obj, "priority", &p, tier)?;
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
        assert_eq!(
            crate::support::schema_keys(patch),
            crate::support::const_names(PATCH_TOP)
        );
        let edit = &patch["properties"]["edits"]["items"];
        let mut edit_fields: Vec<String> = EDIT
            .iter()
            .chain(EDIT_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        edit_fields.sort_unstable();
        assert_eq!(crate::support::schema_keys(edit), edit_fields);
        let anchor = &edit["properties"]["anchor"];
        let mut anchor_fields: Vec<String> = crate::support::ANCHOR
            .iter()
            .chain(crate::support::ANCHOR_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        anchor_fields.sort_unstable();
        assert_eq!(crate::support::schema_keys(anchor), anchor_fields);
    }

    /// Tier exactness: the anchor is compact-tier content, the human-name
    /// rows (`rule`/`message`/`priority`) full-tier only; the provenance tag
    /// and `stale` ride any tier.
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let doc = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "builtin-lint",
                "role": "primary",
                "anchor": {"line": 10, "col": 7},
                "rule": "unnecessary_new",
                "msg": "Unnecessary 'new' keyword.",
                "priority": 1,
                "stale": true,
            }],
        });
        let violation = validate_fix_patch_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the anchor");
        assert_eq!(violation.path, "edits[0].anchor");
        assert_eq!(violation.expected, "omitted at the minimal tier");

        let violation = validate_fix_patch_at_tier(&doc, Verbosity::Compact)
            .expect_err("compact must not carry the rule");
        assert_eq!(violation.path, "edits[0].rule");
        assert_eq!(violation.expected, "omitted at the compact tier");

        assert!(validate_fix_patch_at_tier(&doc, Verbosity::Full).is_ok());

        // `message` and `priority` are full-tier too: without the rule the
        // compact tier names them in turn.
        let no_rule = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "fixer",
                "role": "primary",
                "anchor": {"line": 10, "col": 7},
                "msg": "insert const",
                "priority": 1,
            }],
        });
        let violation = validate_fix_patch_at_tier(&no_rule, Verbosity::Compact)
            .expect_err("compact must not carry the message");
        assert_eq!(violation.path, "edits[0].msg");
        assert_eq!(violation.expected, "omitted at the compact tier");
        let no_message = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "fixer",
                "role": "ignore_line",
                "anchor": {"line": 10, "col": 7},
                "priority": -1,
            }],
        });
        let violation = validate_fix_patch_at_tier(&no_message, Verbosity::Compact)
            .expect_err("compact must not carry the priority");
        assert_eq!(violation.path, "edits[0].priority");
        assert_eq!(violation.expected, "omitted at the compact tier");

        // The minimal floor: the red-line trio plus the provenance tag
        // and the role (`edit_floor` never drops them, fix.rs:744-751)
        // — the anchor and the human names are the trims.
        let minimal = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "builtin-lint",
                "role": "suppress",
                "stale": true,
            }],
        });
        assert!(validate_fix_patch_at_tier(&minimal, Verbosity::Minimal).is_ok());

        // The provenance tag is required and a string; the priority is a
        // signed integer.
        let no_source = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{"off": 123, "len": 8, "text": "Bar", "role": "primary"}],
        });
        let violation = validate_fix_patch(&no_source).expect_err("provenance is required");
        assert_eq!(violation.path, "edits[0].src");
        assert_eq!(violation.found, "missing");

        // The role key is required and one of the four spellings — no
        // aliases.
        let no_role = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{"off": 123, "len": 8, "text": "Bar", "src": "builtin-lint"}],
        });
        let violation = validate_fix_patch(&no_role).expect_err("role is required");
        assert_eq!(violation.path, "edits[0].role");
        assert_eq!(violation.found, "missing");

        let bad_role = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "builtin-lint",
                "role": "ignore",
            }],
        });
        let violation = validate_fix_patch(&bad_role).expect_err("role is one of the spellings");
        assert_eq!(violation.path, "edits[0].role");
        assert_eq!(
            violation.expected,
            "\"primary\" or \"ignore_line\" or \"ignore_file\" or \"suppress\""
        );

        let wrong_types = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{"off": 123, "len": 8, "text": "Bar", "src": 7, "role": "primary"}],
        });
        let violation = validate_fix_patch(&wrong_types).expect_err("source is a string");
        assert_eq!(violation.path, "edits[0].src");
        assert_eq!(violation.expected, "a string");

        let wrong_types = serde_json::json!({
            "file": "lib/foo.dart",
            "v": SCHEMA_VERSION_FIX_PATCH,
            "edits": [{
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "fixer",
                "role": "primary",
                "priority": "high",
            }],
        });
        let violation = validate_fix_patch(&wrong_types).expect_err("priority is an integer");
        assert_eq!(violation.path, "edits[0].priority");
        assert_eq!(violation.expected, "an integer");
    }
}
