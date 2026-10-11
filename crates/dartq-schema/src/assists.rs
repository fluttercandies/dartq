//! The assists document — `dartq assists` and the daemon's `dartq/assists`
//! answer, one object per file: `assists_json`
//! (`crates/dartq-commands/src/fix.rs:801-850`, the one implementation
//! both faces share). The plugin changes answer in priority order, each
//! row carrying its provenance (`source` — the plugin's manifest name),
//! title (`message`), priority (signed — "lower sorts first",
//! fix.rs:139-141), and its anchored edits on the §15.6 red-line floor
//! (the shared `edit_floor` shape); the hosts that could not serve the
//! request answer in `errors` (omitted when empty, §15.8.5). The writer
//! stamps `fix::SCHEMA_VERSION` (fix.rs:87). Mirrors the fix patch's
//! provenance surface over the plugin rows alone — note the tier split is
//! the writer's, not the patch's: the row title (`message`) rides every
//! tier here, and the built-in rows (`builtin-lint`) never appear (the
//! built-in layers contribute no assists, fix.rs:646-647).

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, describe, edit_floor, exact_fields, field, join, object, pinned_version, rows,
    rows_optional, string, violation,
};

/// The assists document's contract version. Mirrors the shared fix face's
/// `SCHEMA_VERSION` (`crates/dartq-commands/src/fix.rs:87`), which has no
/// pub constant to align against — the value is pinned by the fixtures in
/// `tests/output_faces.rs`.
pub const SCHEMA_VERSION_ASSISTS: u32 = 1;

/// The published assists document schema (JSON Schema draft-07 subset).
#[must_use]
pub fn assists_schema() -> &'static Value {
    &ASSISTS_SCHEMA
}

/// One edit row's schema — the §15.6 floor plus the optional compact-tier
/// anchor (the shared `edit_floor` contract), factored out of the document
/// like the query domain's declaration row.
static EDIT_ITEM: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["offset", "length", "replacement", "source"],
        "properties": {
            "offset": { "type": "integer", "minimum": 0 },
            "length": { "type": "integer", "minimum": 0 },
            "replacement": { "type": "string" },
            "source": { "type": "string" },
            // The compact tier's drift-correction locator (fix.rs:802,
            // 812-814); minimal omits it.
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
        },
    })
});

static ASSISTS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/assists.json",
        "title": "dartq assists document (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["file", "schema_version", "assists"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_ASSISTS },
            "file": { "type": "string" },
            "assists": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["message", "source", "edits"],
                "properties": {
                    // The row title rides every tier (assists_json removes
                    // only the priority at minimal, fix.rs:824-828).
                    "message": { "type": "string" },
                    // The provenance tag: the plugin's manifest name.
                    "source": { "type": "string" },
                    // Signed ("lower sorts first", fix.rs:139-141);
                    // structural validator, so sign legality is the
                    // emitter's obligation.
                    "priority": { "type": "integer" },
                    "edits": { "type": "array", "items": EDIT_ITEM.clone() },
                },
            }},
            // The hosts that could not serve the request, omitted when
            // empty (fix.rs:837-848) — any tier.
            "errors": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["plugin", "message"],
                "properties": {
                    "plugin": { "type": "string" },
                    "message": { "type": "string" },
                },
            }},
        },
    })
});

const ASSISTS_TOP: &[&str] = &["file", "schema_version", "assists"];
const ASSISTS_ERROR: &[&str] = &["errors"];
const ASSIST: &[&str] = &["message", "source", "edits"];
const ASSIST_OPTIONAL: &[&str] = &["priority"];
const EDIT: &[&str] = &["offset", "length", "replacement", "source"];
const EDIT_OPTIONAL: &[&str] = &["anchor"];

pub(crate) fn validate_assists(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, ASSISTS_TOP, ASSISTS_ERROR, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_ASSISTS)?;
    string(field(root, "file", "")?, "file")?;
    for (i, row) in rows(root, "assists")? {
        let p = format!("assists[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, ASSIST, ASSIST_OPTIONAL, &p)?;
        string(field(obj, "message", &p)?, &join(&p, "message"))?;
        string(field(obj, "source", &p)?, &join(&p, "source"))?;
        if let Some(priority) = obj.get("priority") {
            // Signed: "lower sorts first" (fix.rs:139-141), so negatives
            // are legal — the shared `integer` helper is non-negative only.
            priority
                .as_i64()
                .ok_or_else(|| violation(join(&p, "priority"), "an integer", describe(priority)))?;
        }
        for (j, edit) in rows(obj, "edits")? {
            let ep = format!("assists[{i}].edits[{j}]");
            let edit = object(edit, &ep)?;
            exact_fields(edit, EDIT, EDIT_OPTIONAL, &ep)?;
            // The red-line trio, the provenance tag, and the optional
            // anchor — the shared §15.6 floor (support::edit_floor).
            edit_floor(edit, &ep)?;
        }
    }
    // Absent is the everyday shape: hosts that served the request leave no
    // `errors` (fix.rs:837-848).
    for (i, error) in rows_optional(root, "errors")? {
        let p = format!("errors[{i}]");
        let error = object(error, &p)?;
        exact_fields(error, &["plugin", "message"], &[], &p)?;
        string(field(error, "plugin", &p)?, &join(&p, "plugin"))?;
        string(field(error, "message", &p)?, &join(&p, "message"))?;
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): at `minimal` the
/// row's priority and the edits' anchor are the trims — the title, the
/// provenance tag, and the red-line floor ride every tier, and `errors`
/// answers at any tier (crate docs, assists table).
pub(crate) fn validate_assists_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_assists(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    for (i, row) in rows(root, "assists")? {
        let p = format!("assists[{i}]");
        let obj = object(row, &p)?;
        absent(obj, "priority", &p, tier)?;
        for (j, edit) in rows(obj, "edits")? {
            let ep = format!("assists[{i}].edits[{j}]");
            absent(object(edit, &ep)?, "anchor", &ep, tier)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    fn document() -> serde_json::Value {
        json!({
            "file": "lib/foo.dart",
            "schema_version": SCHEMA_VERSION_ASSISTS,
            "assists": [{
                "message": "Wrap in StatefulWidget",
                "source": "assistant",
                "priority": 1,
                "edits": [{
                    "offset": 19,
                    "length": 4,
                    "replacement": "StatefulWidget",
                    "source": "assistant",
                    "anchor": {"line": 2, "column": 16},
                }],
            }],
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let assists = assists_schema();
        let mut top: Vec<String> = ASSISTS_TOP
            .iter()
            .chain(ASSISTS_ERROR.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(assists), top);
        let row = &assists["properties"]["assists"]["items"];
        let mut row_fields: Vec<String> = ASSIST
            .iter()
            .chain(ASSIST_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        row_fields.sort_unstable();
        assert_eq!(schema_keys(row), row_fields);
        let edit = &row["properties"]["edits"]["items"];
        assert_eq!(schema_keys(edit), const_names_from(EDIT, EDIT_OPTIONAL));
        assert_eq!(
            schema_keys(&edit["properties"]["anchor"]),
            const_names(&["column", "context_after", "context_before", "line"])
        );
        assert_eq!(
            schema_keys(&assists["properties"]["errors"]["items"]),
            const_names(&["message", "plugin"])
        );
    }

    fn const_names_from(required: &[&str], optional: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = required
            .iter()
            .chain(optional.iter())
            .map(|s| (*s).to_owned())
            .collect();
        names.sort_unstable();
        names
    }

    /// Tier exactness: the minimal tier drops the row priority and the
    /// edit anchors — the title, the provenance tag, and the floor ride
    /// every tier (the writer's own minimal pin, fix.rs:1402-1418).
    #[test]
    fn tiers_reject_higher_tier_fields() {
        let violation = validate_assists_at_tier(&document(), Verbosity::Minimal)
            .expect_err("minimal must not carry the priority");
        assert_eq!(violation.path, "assists[0].priority");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_assists_at_tier(&document(), Verbosity::Compact).is_ok());
        assert!(validate_assists_at_tier(&document(), Verbosity::Full).is_ok());

        let minimal = json!({
            "file": "lib/foo.dart",
            "schema_version": SCHEMA_VERSION_ASSISTS,
            "assists": [{
                "message": "Wrap in StatefulWidget",
                "source": "assistant",
                "edits": [{
                    "offset": 19,
                    "length": 4,
                    "replacement": "StatefulWidget",
                    "source": "assistant",
                }],
            }],
            "errors": [{"plugin": "offline", "message": "manifest not found"}],
        });
        assert!(validate_assists_at_tier(&minimal, Verbosity::Minimal).is_ok());
    }

    /// The red line: an undeclared field is named, the priority is a
    /// (signed) integer, and the floor trio with its provenance tag is
    /// required.
    #[test]
    fn unknown_fields_and_wrong_types_are_named() {
        let violation = validate_assists(&json!({
            "file": "lib/foo.dart",
            "schema_version": SCHEMA_VERSION_ASSISTS,
            "assists": [{
                "message": "Wrap in StatefulWidget",
                "source": "assistant",
                "edits": [{
                    "offset": 19,
                    "length": 4,
                    "replacement": "StatefulWidget",
                    "source": "assistant",
                    "rule": "unnecessary_new",
                }],
            }],
        }))
        .expect_err("the patch-only rule column is not an assists field");
        assert_eq!(violation.path, "assists[0].edits[0].rule");

        let wrong = json!({
            "file": "lib/foo.dart",
            "schema_version": SCHEMA_VERSION_ASSISTS,
            "assists": [{
                "message": "Wrap in StatefulWidget",
                "source": "assistant",
                "priority": "high",
                "edits": [],
            }],
        });
        let violation = validate_assists(&wrong).expect_err("priority is an integer");
        assert_eq!(violation.path, "assists[0].priority");
        assert_eq!(violation.expected, "an integer");
    }
}
