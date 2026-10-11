//! The §15.3 explain report — `dartq-ai`'s `explain` query output, the
//! AI's one-stop symbol report. The §15.3 sections map onto the substrate
//! the assembler can reach; two sections (`interfaces`, `constructors`)
//! are documented empty seams until the capture grows the edges/rows, and
//! they serialize as empty arrays, never placeholders.
//!
//! # Verbosity tiers (crate docs, "Verbosity tiers")
//!
//! `minimal` keeps the red line — `symbol` plus the `declaration` locators
//! — and replaces every section array with the `counts` object (a zero is
//! a value, not an absent section, so the AI sees which sections exist and
//! are empty). `compact` (the default) carries every section as an array,
//! empty sections included. `full` equals `compact` today: every field the
//! substrate reaches is a locator or a verdict, nothing prose-like to
//! inline. Per-row trims are marked in the table.
//!
//! | field                     | minimal | compact | full |
//! |---------------------------|---------|---------|------|
//! | `symbol`                  | ●       | ●       | ●    |
//! | `declaration`             | ●       | ●       | ●    |
//! | `declaration.container`   |         | ●       | ●    |
//! | section arrays (7)        | counts  | ●       | ●    |
//! | `references[].qualified`  | (count) | ●       | ●    |
//! | `counts`                  | ●       |         |      |
//!
//! Ranges are the §15.8.5 `[startOffset, endOffset)` pair form (this face
//! is new — no legacy spelling to keep). The §15.8 session fields
//! (`symbols`/`since`) are declared absent-tolerant like every daemon face.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::query::{DECL_KINDS, REF_KINDS};
use crate::support::{
    absent, array, boolean, enum_spelling, exact_fields, field, join, non_negative, object,
    pinned_version, range_pair, session_fields, string,
};

/// The explain report's output contract version (PRD §15.7). Mirrors the
/// authoritative `EXPLAIN_SCHEMA_VERSION` in `dartq-ai`; equality is pinned
/// by an alignment test (`tests/alignment.rs`).
pub const SCHEMA_VERSION_EXPLAIN: u32 = 1;

/// The published explain report schema (JSON Schema draft-07 subset).
#[must_use]
pub fn explain_schema() -> &'static Value {
    &EXPLAIN_SCHEMA
}

static EXPLAIN_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/explain.json",
        "title": "dartq explain report (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "symbol"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_EXPLAIN },
            "symbol": { "type": "string" },
            "declaration": { "type": "object",
                "additionalProperties": false,
                "required": ["file", "name", "kind", "range"],
                "properties": {
                    "file": { "type": "string" },
                    "name": { "type": "string" },
                    "kind": { "enum": DECL_KINDS },
                    // Compact/full only (minimal trims the convenience
                    // field); absent for a top-level declaration.
                    "container": { "type": "string" },
                    "range": range_pair_schema(),
                },
            },
            "inheritance": supertype_rows(),
            // The documented empty seam until the capture records
            // implements/with edges (dartq-ai `explain.rs`).
            "interfaces": supertype_rows(),
            // The documented empty seam until the capture mints
            // constructor rows (dartq-ai `explain.rs`).
            "constructors": member_rows(),
            "methods": member_rows(),
            "references": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["file", "kind", "range"],
                "properties": {
                    "file": { "type": "string" },
                    "kind": { "enum": REF_KINDS },
                    "range": range_pair_schema(),
                    "qualified": { "type": "boolean" },
                },
            }},
            "dependencies": path_rows(),
            "affected_tests": path_rows(),
            // The `minimal`-tier replacement for the arrays: one entry per
            // section, all seven always present.
            "counts": { "type": "object",
                "additionalProperties": false,
                "required": EXPLAIN_SECTIONS,
                "properties": {
                    "inheritance": { "type": "integer", "minimum": 0 },
                    "interfaces": { "type": "integer", "minimum": 0 },
                    "constructors": { "type": "integer", "minimum": 0 },
                    "methods": { "type": "integer", "minimum": 0 },
                    "references": { "type": "integer", "minimum": 0 },
                    "dependencies": { "type": "integer", "minimum": 0 },
                    "affected_tests": { "type": "integer", "minimum": 0 },
                },
            },
            // §15.8 session token economics (crate docs, "Token economics").
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

/// The shared range-pair schema fragment (§15.8.5).
fn range_pair_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "integer", "minimum": 0 },
        "minItems": 2,
        "maxItems": 2,
    })
}

fn supertype_rows() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name"],
            "properties": {
                "name": { "type": "string" },
                // Absent = a bare name outside the analyzed set.
                "file": { "type": "string" },
            },
        },
    })
}

fn member_rows() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "kind", "range"],
            "properties": {
                "name": { "type": "string" },
                "kind": { "enum": MEMBER_KINDS },
                "range": range_pair_schema(),
            },
        },
    })
}

fn path_rows() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
            },
        },
    })
}

/// The member families that ride the `methods` array.
pub const MEMBER_KINDS: &[&str] = &["method", "getter", "setter", "field"];

/// The explain report's array sections, in `counts` order.
pub const EXPLAIN_SECTIONS: &[&str] = &[
    "inheritance",
    "interfaces",
    "constructors",
    "methods",
    "references",
    "dependencies",
    "affected_tests",
];

const EXPLAIN_TOP: &[&str] = &["schema_version", "symbol"];
/// Everything beyond the red line is optional: the section arrays drop at
/// `minimal`, `counts` exists only there, and the §15.8 session fields
/// (`symbols`/`since`) ride absent-tolerant.
const EXPLAIN_OPTIONAL: &[&str] = &[
    "declaration",
    "inheritance",
    "interfaces",
    "constructors",
    "methods",
    "references",
    "dependencies",
    "affected_tests",
    "counts",
    "symbols",
    "since",
];
const DECLARATION_ROW: &[&str] = &["file", "name", "kind", "range"];
const DECLARATION_ROW_OPTIONAL: &[&str] = &["container"];
const SUPERTYPE_ROW: &[&str] = &["name"];
const SUPERTYPE_ROW_OPTIONAL: &[&str] = &["file"];
const MEMBER_ROW: &[&str] = &["name", "kind", "range"];
const REFERENCE_ROW: &[&str] = &["file", "kind", "range"];
const REFERENCE_ROW_OPTIONAL: &[&str] = &["qualified"];
const PATH_ROW: &[&str] = &["path"];

pub(crate) fn validate_explain(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, EXPLAIN_TOP, EXPLAIN_OPTIONAL, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_EXPLAIN)?;
    string(field(root, "symbol", "")?, "symbol")?;
    session_fields(root)?;
    if let Some(declaration) = root.get("declaration") {
        let p = "declaration";
        let obj = object(declaration, p)?;
        exact_fields(obj, DECLARATION_ROW, DECLARATION_ROW_OPTIONAL, p)?;
        string(field(obj, "file", p)?, &join(p, "file"))?;
        string(field(obj, "name", p)?, &join(p, "name"))?;
        enum_spelling(field(obj, "kind", p)?, &join(p, "kind"), DECL_KINDS)?;
        range_pair(field(obj, "range", p)?, &join(p, "range"))?;
        if let Some(container) = obj.get("container") {
            string(container, &join(p, "container"))?;
        }
    }
    // Section arrays are optional (the `minimal` tier drops them); a
    // present section's rows validate whole.
    for section in ["inheritance", "interfaces"] {
        let Some(list) = root.get(section) else {
            continue;
        };
        for (i, row) in array(list, section)?.iter().enumerate() {
            let p = format!("{section}[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, SUPERTYPE_ROW, SUPERTYPE_ROW_OPTIONAL, &p)?;
            string(field(obj, "name", &p)?, &join(&p, "name"))?;
            if let Some(file) = obj.get("file") {
                string(file, &join(&p, "file"))?;
            }
        }
    }
    for section in ["constructors", "methods"] {
        let Some(list) = root.get(section) else {
            continue;
        };
        for (i, row) in array(list, section)?.iter().enumerate() {
            let p = format!("{section}[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, MEMBER_ROW, &[], &p)?;
            string(field(obj, "name", &p)?, &join(&p, "name"))?;
            enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), MEMBER_KINDS)?;
            range_pair(field(obj, "range", &p)?, &join(&p, "range"))?;
        }
    }
    if let Some(list) = root.get("references") {
        for (i, row) in array(list, "references")?.iter().enumerate() {
            let p = format!("references[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, REFERENCE_ROW, REFERENCE_ROW_OPTIONAL, &p)?;
            string(field(obj, "file", &p)?, &join(&p, "file"))?;
            enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), REF_KINDS)?;
            range_pair(field(obj, "range", &p)?, &join(&p, "range"))?;
            if let Some(qualified) = obj.get("qualified") {
                boolean(qualified, &join(&p, "qualified"))?;
            }
        }
    }
    for section in ["dependencies", "affected_tests"] {
        let Some(list) = root.get(section) else {
            continue;
        };
        for (i, row) in array(list, section)?.iter().enumerate() {
            let p = format!("{section}[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, PATH_ROW, &[], &p)?;
            string(field(obj, "path", &p)?, &join(&p, "path"))?;
        }
    }
    if let Some(counts) = root.get("counts") {
        let obj = object(counts, "counts")?;
        exact_fields(obj, EXPLAIN_SECTIONS, &[], "counts")?;
        for section in EXPLAIN_SECTIONS {
            let value = field(obj, section, "counts")?;
            non_negative(value, &join("counts", section))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): minimal replaces the
/// section arrays with `counts` and drops the declaration's container (the
/// `references[].qualified` row goes with the array it rides); compact and
/// full carry the arrays, so the minimal-only `counts` is forbidden there
/// (crate docs and the table above).
pub(crate) fn validate_explain_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_explain(value)?;
    let root = object(value, "")?;
    if tier == Verbosity::Minimal {
        if let Some(declaration) = root.get("declaration") {
            absent(
                object(declaration, "declaration")?,
                "container",
                "declaration",
                tier,
            )?;
        }
        for section in EXPLAIN_SECTIONS {
            absent(root, section, "", tier)?;
        }
    } else {
        absent(root, "counts", "", tier)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::support::{const_names, schema_keys};

    /// A compact-tier report with every section filled (the fixture the
    /// dartq-ai tests assemble).
    fn full_report() -> Value {
        json!({
            "schema_version": SCHEMA_VERSION_EXPLAIN,
            "symbol": "Foo",
            "declaration": {
                "file": "a.dart",
                "name": "Foo",
                "kind": "class",
                "range": [40, 43],
            },
            "inheritance": [{"name": "Bar", "file": "a.dart"}],
            "interfaces": [],
            "constructors": [],
            "methods": [
                {"name": "area", "kind": "method", "range": [90, 93]},
                {"name": "label", "kind": "field", "range": [70, 73]},
            ],
            "references": [
                {"file": "b.dart", "kind": "reference", "range": [10, 13], "qualified": false},
            ],
            "dependencies": [{"path": "package:t/b.dart"}],
            "affected_tests": [{"path": "package:t/a_test.dart"}],
        })
    }

    /// A minimal-tier report: declaration + counts, no arrays.
    fn minimal_report() -> Value {
        json!({
            "schema_version": SCHEMA_VERSION_EXPLAIN,
            "symbol": "Foo",
            "declaration": {
                "file": "a.dart",
                "name": "Foo",
                "kind": "class",
                "range": [40, 43],
            },
            "counts": {
                "inheritance": 1,
                "interfaces": 0,
                "constructors": 0,
                "methods": 2,
                "references": 1,
                "dependencies": 1,
                "affected_tests": 1,
            },
        })
    }

    /// Drift pin: the published document and the validator must name the
    /// same fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let explain = explain_schema();
        let mut top: Vec<String> = EXPLAIN_TOP
            .iter()
            .chain(EXPLAIN_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(explain), top);
        let declaration = &explain["properties"]["declaration"];
        let mut decl_fields: Vec<String> = DECLARATION_ROW
            .iter()
            .chain(DECLARATION_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        decl_fields.sort_unstable();
        assert_eq!(schema_keys(declaration), decl_fields);
        let supertype = &explain["properties"]["inheritance"]["items"];
        let mut super_fields: Vec<String> = SUPERTYPE_ROW
            .iter()
            .chain(SUPERTYPE_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        super_fields.sort_unstable();
        assert_eq!(schema_keys(supertype), super_fields);
        let member = &explain["properties"]["methods"]["items"];
        assert_eq!(schema_keys(member), const_names(MEMBER_ROW));
        let reference = &explain["properties"]["references"]["items"];
        let mut ref_fields: Vec<String> = REFERENCE_ROW
            .iter()
            .chain(REFERENCE_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        ref_fields.sort_unstable();
        assert_eq!(schema_keys(reference), ref_fields);
        let path_row = &explain["properties"]["dependencies"]["items"];
        assert_eq!(schema_keys(path_row), const_names(PATH_ROW));
        let counts = &explain["properties"]["counts"];
        assert_eq!(schema_keys(counts), const_names(EXPLAIN_SECTIONS));
    }

    fn spelling(property: &serde_json::Value) -> Vec<&str> {
        property["enum"]
            .as_array()
            .expect("enum list")
            .iter()
            .filter_map(|v| v.as_str())
            .collect()
    }

    /// The section enums pin the shared query-face spellings plus the
    /// member subset.
    #[test]
    fn kind_enums_match_the_declared_lists() {
        let explain = explain_schema();
        assert_eq!(
            spelling(&explain["properties"]["declaration"]["properties"]["kind"]),
            DECL_KINDS
        );
        assert_eq!(
            spelling(&explain["properties"]["methods"]["items"]["properties"]["kind"]),
            MEMBER_KINDS
        );
        assert_eq!(
            spelling(&explain["properties"]["references"]["items"]["properties"]["kind"]),
            REF_KINDS
        );
    }

    #[test]
    fn compact_report_validates() {
        assert!(validate_explain(&full_report()).is_ok());
        let empty = json!({
            "schema_version": SCHEMA_VERSION_EXPLAIN,
            "symbol": "Nope",
            "inheritance": [],
            "interfaces": [],
            "constructors": [],
            "methods": [],
            "references": [],
            "dependencies": [],
            "affected_tests": [],
        });
        assert!(validate_explain(&empty).is_ok());
    }

    #[test]
    fn minimal_report_validates() {
        assert!(validate_explain(&minimal_report()).is_ok());
        // The unknown-symbol floor: symbol and version only.
        let floor = json!({
            "schema_version": SCHEMA_VERSION_EXPLAIN,
            "symbol": "Nope",
        });
        assert!(validate_explain(&floor).is_ok());
    }

    #[test]
    fn version_constant_is_pinned() {
        let mut bumped = full_report();
        bumped["schema_version"] = json!(SCHEMA_VERSION_EXPLAIN + 1);
        let violation = validate_explain(&bumped).expect_err("bumped version must violate");
        assert_eq!(violation.path, "schema_version");
        assert_eq!(violation.expected, "the integer 1");
    }

    #[test]
    fn missing_symbol_is_named() {
        let mut payload = full_report();
        payload.as_object_mut().expect("object").remove("symbol");
        let violation = validate_explain(&payload).expect_err("missing symbol must violate");
        assert_eq!(violation.path, "symbol");
        assert_eq!(violation.found, "missing");
    }

    /// Type red lines on the new face's shapes: a range that is not a
    /// pair, a kind spelling outside its enum, a boolean spelled as a
    /// string, a counts entry spelled as a string, and an undeclared
    /// field — each named with its path.
    #[test]
    fn type_errors_are_named() {
        let mut payload = full_report();
        payload["declaration"]["range"] = json!([40]);
        let violation = validate_explain(&payload).expect_err("one-element range");
        assert_eq!(violation.path, "declaration.range");

        let mut payload = full_report();
        payload["declaration"]["range"] = json!("40-43");
        let violation = validate_explain(&payload).expect_err("string range");
        assert_eq!(violation.path, "declaration.range");

        let mut payload = full_report();
        payload["declaration"]["kind"] = json!("banana");
        let violation = validate_explain(&payload).expect_err("unknown kind spelling");
        assert_eq!(violation.path, "declaration.kind");

        let mut payload = full_report();
        payload["methods"][0]["kind"] = json!("constructor");
        let violation =
            validate_explain(&payload).expect_err("constructor is not a member kind yet");
        assert_eq!(violation.path, "methods[0].kind");

        let mut payload = full_report();
        payload["references"][0]["qualified"] = json!("true");
        let violation = validate_explain(&payload).expect_err("string qualified");
        assert_eq!(violation.path, "references[0].qualified");

        let mut payload = minimal_report();
        payload["counts"]["methods"] = json!("two");
        let violation = validate_explain(&payload).expect_err("string count");
        assert_eq!(violation.path, "counts.methods");

        let mut payload = full_report();
        payload["dependencies"][0]["file"] = json!("b.dart");
        let violation = validate_explain(&payload).expect_err("undeclared row field");
        assert_eq!(violation.path, "dependencies[0].file");
    }

    /// A counts object missing one section is a violation: the minimal
    /// tier always answers the full seven-entry map.
    #[test]
    fn partial_counts_are_rejected() {
        let mut payload = minimal_report();
        payload["counts"]
            .as_object_mut()
            .expect("object")
            .remove("references");
        let violation = validate_explain(&payload).expect_err("missing count entry");
        assert_eq!(violation.path, "counts.references");
        assert_eq!(violation.found, "missing");
    }

    /// Tier exactness: the arrays and `counts` are mutually exclusive —
    /// minimal answers counts only, compact/full answers arrays only.
    #[test]
    fn tiers_keep_arrays_and_counts_exclusive() {
        let violation = validate_explain_at_tier(&minimal_report(), Verbosity::Compact)
            .expect_err("compact must not carry the minimal counts");
        assert_eq!(violation.path, "counts");
        assert_eq!(violation.expected, "omitted at the compact tier");

        let violation = validate_explain_at_tier(&full_report(), Verbosity::Minimal)
            .expect_err("minimal must not carry the section arrays");
        assert_eq!(violation.path, "inheritance");
        assert_eq!(violation.expected, "omitted at the minimal tier");

        assert!(validate_explain_at_tier(&full_report(), Verbosity::Compact).is_ok());
        assert!(validate_explain_at_tier(&full_report(), Verbosity::Full).is_ok());
        assert!(validate_explain_at_tier(&minimal_report(), Verbosity::Minimal).is_ok());
    }

    /// The minimal tier also drops the declaration's container convenience
    /// field.
    #[test]
    fn minimal_tier_rejects_the_declaration_container() {
        let mut payload = minimal_report();
        payload["declaration"]["container"] = json!("Foo");
        let violation = validate_explain_at_tier(&payload, Verbosity::Minimal)
            .expect_err("minimal must not carry the container");
        assert_eq!(violation.path, "declaration.container");
    }
}
