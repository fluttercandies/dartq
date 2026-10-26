//! The explain report — `dartq-ai`'s `explain` query output, the
//! AI's one-stop symbol report. The sections map onto the substrate
//! the assembler can reach; `inheritance` carries the `extends` supertype
//! edges while `interfaces` carries the `with`/`on`/`implements` ones
//! (each row naming its clause), and `constructors` carries the element
//! face's constructor rows (no source span exists for them — the name
//! and the rendered parameter face are the facts).
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
//! Ranges are the `[startOffset, endOffset)` pair form (this face
//! is new — no legacy spelling to keep). The session fields
//! (`symbols`/`since`) are declared absent-tolerant like every daemon face;
//! the face's `symbols` table is the id → FILE path spelling — the
//! economics over this face's own path-valued rows — not query/impact's
//! `label|name` entity-reference table.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::query::{DECL_KINDS, REF_KINDS};
use crate::support::{
    absent, array, boolean, enum_spelling, exact_fields, field, join, non_negative, object,
    path_session_fields, pinned_version, string,
};

/// The explain report's output contract version. Mirrors the
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
        "required": ["v", "sym"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_EXPLAIN },
            "sym": { "type": "string" },
            "declaration": { "type": "object",
                "additionalProperties": false,
                "required": ["file", "name", "kind", "off", "len"],
                "properties": {
                    "file": { "type": "string" },
                    "name": { "type": "string" },
                    "kind": { "enum": DECL_KINDS },
                    // Compact/full only (minimal trims the convenience
                    // field); absent for a top-level declaration.
                    "container": { "type": "string" },
                    "off": { "type": "integer", "minimum": 0 },
                    "len": { "type": "integer", "minimum": 0 },
                },
            },
            "bases": supertype_rows(),
            // The `with`/`on`/`implements` supertype edges, each row
            // naming its clause (dartq-ai `explain.rs`, the clause split).
            "ifaces": supertype_rows(),
            // The element face's constructor rows: the spanless shape (the
            // element model carries no constructor ranges).
            "ctors": constructor_rows(),
            "methods": member_rows(),
            "refs": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["file", "kind", "off", "len"],
                "properties": {
                    "file": { "type": "string" },
                    "kind": { "enum": REF_KINDS },
                    "off": { "type": "integer", "minimum": 0 },
                    "len": { "type": "integer", "minimum": 0 },
                    "qualified": { "type": "boolean" },
                },
            }},
            "deps": path_rows(),
            "tests": path_rows(),
            // The `minimal`-tier replacement for the arrays: one entry per
            // section, all seven always present.
            "counts": { "type": "object",
                "additionalProperties": false,
                "required": EXPLAIN_SECTIONS,
                "properties": {
                    "bases": { "type": "integer", "minimum": 0 },
                    "ifaces": { "type": "integer", "minimum": 0 },
                    "ctors": { "type": "integer", "minimum": 0 },
                    "methods": { "type": "integer", "minimum": 0 },
                    "refs": { "type": "integer", "minimum": 0 },
                    "deps": { "type": "integer", "minimum": 0 },
                    "tests": { "type": "integer", "minimum": 0 },
                },
            },
            // Session token economics (crate docs, "Token economics").
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "since": { "type": "integer", "minimum": 0 },
        },
    })
});

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
                // Absent only for producers that cannot attribute the
                // clause; the explain producer always emits it.
                "clause": { "enum": CLAUSE_KINDS },
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
            "required": ["name", "kind", "off", "len"],
            "properties": {
                "name": { "type": "string" },
                "kind": { "enum": MEMBER_KINDS },
                "off": { "type": "integer", "minimum": 0 },
                    "len": { "type": "integer", "minimum": 0 },
            },
        },
    })
}

fn constructor_rows() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "kind"],
            "properties": {
                "name": { "type": "string" },
                "kind": { "enum": CONSTRUCTOR_KINDS },
                // The rendered parameter face; absent only for producers
                // that cannot render it (the explain producer always does).
                "sig": { "type": "string" },
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

/// The `constructors` section's kind — the member-kind grammar's
/// constructor spelling, exclusive to that section (the `methods` rows
/// come from the capture, whose kind set has no constructor).
pub const CONSTRUCTOR_KINDS: &[&str] = &["constructor"];

/// The supertype clauses the `inheritance`/`interfaces` rows name.
pub const CLAUSE_KINDS: &[&str] = &["extends", "with", "on", "implements"];

/// The explain report's array sections, in `counts` order.
pub const EXPLAIN_SECTIONS: &[&str] = &[
    "bases",
    "ifaces",
    "ctors",
    "methods",
    "refs",
    "deps",
    "tests",
];

const EXPLAIN_TOP: &[&str] = &["v", "sym"];
/// Everything beyond the red line is optional: the section arrays drop at
/// `minimal`, `counts` exists only there, and the session fields
/// (`symbols`/`since`) ride absent-tolerant.
const EXPLAIN_OPTIONAL: &[&str] = &[
    "declaration",
    "bases",
    "ifaces",
    "ctors",
    "methods",
    "refs",
    "deps",
    "tests",
    "counts",
    "symbols",
    "since",
];
const DECLARATION_ROW: &[&str] = &["file", "name", "kind", "off", "len"];
const DECLARATION_ROW_OPTIONAL: &[&str] = &["container"];
const SUPERTYPE_ROW: &[&str] = &["name"];
const SUPERTYPE_ROW_OPTIONAL: &[&str] = &["file", "clause"];
const MEMBER_ROW: &[&str] = &["name", "kind", "off", "len"];
const CONSTRUCTOR_ROW: &[&str] = &["name", "kind"];
const CONSTRUCTOR_ROW_OPTIONAL: &[&str] = &["sig"];
const REFERENCE_ROW: &[&str] = &["file", "kind", "off", "len"];
const REFERENCE_ROW_OPTIONAL: &[&str] = &["qualified"];
const PATH_ROW: &[&str] = &["path"];

/// The off+len locator pair every ranged explain row carries.
fn span_fields(obj: &serde_json::Map<String, Value>, p: &str) -> Result<(), SchemaViolation> {
    non_negative(field(obj, "off", p)?, &join(p, "off"))?;
    non_negative(field(obj, "len", p)?, &join(p, "len"))?;
    Ok(())
}

pub(crate) fn validate_explain(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, EXPLAIN_TOP, EXPLAIN_OPTIONAL, "")?;
    pinned_version(root, "v", SCHEMA_VERSION_EXPLAIN)?;
    string(field(root, "sym", "")?, "sym")?;
    path_session_fields(root)?;
    if let Some(declaration) = root.get("declaration") {
        let p = "declaration";
        let obj = object(declaration, p)?;
        exact_fields(obj, DECLARATION_ROW, DECLARATION_ROW_OPTIONAL, p)?;
        string(field(obj, "file", p)?, &join(p, "file"))?;
        string(field(obj, "name", p)?, &join(p, "name"))?;
        enum_spelling(field(obj, "kind", p)?, &join(p, "kind"), DECL_KINDS)?;
        span_fields(obj, p)?;
        if let Some(container) = obj.get("container") {
            string(container, &join(p, "container"))?;
        }
    }
    // Section arrays are optional (the `minimal` tier drops them); a
    // present section's rows validate whole.
    for section in ["bases", "ifaces"] {
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
            if let Some(clause) = obj.get("clause") {
                enum_spelling(clause, &join(&p, "clause"), CLAUSE_KINDS)?;
            }
        }
    }
    for section in ["ctors", "methods"] {
        let Some(list) = root.get(section) else {
            continue;
        };
        for (i, row) in array(list, section)?.iter().enumerate() {
            let p = format!("{section}[{i}]");
            let obj = object(row, &p)?;
            // The constructors rows are the spanless shape (the element
            // model carries no constructor ranges); methods keep the
            // captured declaration rows' locator.
            let (required, optional, kinds): (&[&str], &[&str], &[&str]) = match section {
                "ctors" => (CONSTRUCTOR_ROW, CONSTRUCTOR_ROW_OPTIONAL, CONSTRUCTOR_KINDS),
                _ => (MEMBER_ROW, &[], MEMBER_KINDS),
            };
            exact_fields(obj, required, optional, &p)?;
            string(field(obj, "name", &p)?, &join(&p, "name"))?;
            enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), kinds)?;
            if section == "methods" {
                span_fields(obj, &p)?;
            } else if let Some(signature) = obj.get("sig") {
                string(signature, &join(&p, "sig"))?;
            }
        }
    }
    if let Some(list) = root.get("refs") {
        for (i, row) in array(list, "refs")?.iter().enumerate() {
            let p = format!("refs[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, REFERENCE_ROW, REFERENCE_ROW_OPTIONAL, &p)?;
            string(field(obj, "file", &p)?, &join(&p, "file"))?;
            enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), REF_KINDS)?;
            span_fields(obj, &p)?;
            if let Some(qualified) = obj.get("qualified") {
                boolean(qualified, &join(&p, "qualified"))?;
            }
        }
    }
    for section in ["deps", "tests"] {
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
            "v": SCHEMA_VERSION_EXPLAIN,
            "sym": "Foo",
            "declaration": {
                "file": "a.dart",
                "name": "Foo",
                "kind": "class",
                "off": 40,
                "len": 3,
            },
            "bases": [{"name": "Bar", "file": "a.dart", "clause": "extends"}],
            "ifaces": [{"name": "Mix", "file": "b.dart", "clause": "with"}],
            "ctors": [
                {"name": "Foo", "kind": "constructor", "sig": "Foo()"},
            ],
            "methods": [
                {"name": "area", "kind": "method", "off": 90, "len": 3},
                {"name": "label", "kind": "field", "off": 70, "len": 3},
            ],
            "refs": [
                {"file": "b.dart", "kind": "reference", "off": 10, "len": 3, "qualified": false},
            ],
            "deps": [{"path": "package:t/b.dart"}],
            "tests": [{"path": "package:t/a_test.dart"}],
        })
    }

    /// A minimal-tier report: declaration + counts, no arrays.
    fn minimal_report() -> Value {
        json!({
            "v": SCHEMA_VERSION_EXPLAIN,
            "sym": "Foo",
            "declaration": {
                "file": "a.dart",
                "name": "Foo",
                "kind": "class",
                "off": 40,
                "len": 3,
            },
            "counts": {
                "bases": 1,
                "ifaces": 0,
                "ctors": 0,
                "methods": 2,
                "refs": 1,
                "deps": 1,
                "tests": 1,
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
        let supertype = &explain["properties"]["bases"]["items"];
        let mut super_fields: Vec<String> = SUPERTYPE_ROW
            .iter()
            .chain(SUPERTYPE_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        super_fields.sort_unstable();
        assert_eq!(schema_keys(supertype), super_fields);
        let constructor = &explain["properties"]["ctors"]["items"];
        let mut ctor_fields: Vec<String> = CONSTRUCTOR_ROW
            .iter()
            .chain(CONSTRUCTOR_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        ctor_fields.sort_unstable();
        assert_eq!(schema_keys(constructor), ctor_fields);
        let member = &explain["properties"]["methods"]["items"];
        assert_eq!(schema_keys(member), const_names(MEMBER_ROW));
        let reference = &explain["properties"]["refs"]["items"];
        let mut ref_fields: Vec<String> = REFERENCE_ROW
            .iter()
            .chain(REFERENCE_ROW_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        ref_fields.sort_unstable();
        assert_eq!(schema_keys(reference), ref_fields);
        let path_row = &explain["properties"]["deps"]["items"];
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
    /// member subset; the constructors section owns the constructor
    /// spelling (the methods rows come from the capture, whose kind set
    /// has no constructor).
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
            spelling(&explain["properties"]["ctors"]["items"]["properties"]["kind"]),
            CONSTRUCTOR_KINDS
        );
        assert_eq!(
            spelling(&explain["properties"]["ifaces"]["items"]["properties"]["clause"]),
            CLAUSE_KINDS
        );
        assert_eq!(
            spelling(&explain["properties"]["refs"]["items"]["properties"]["kind"]),
            REF_KINDS
        );
    }

    /// The additive supertype/constructor columns: the clause spelling is
    /// checked, an unknown clause is named, the constructor row's
    /// signature is optional but contract-bound, and a constructor row
    /// carrying a range (the methods row's locator) is rejected — the
    /// element model has no constructor spans.
    #[test]
    fn supertype_and_constructor_columns_are_contract_bound() {
        let mut payload = full_report();
        payload["ifaces"][0]["clause"] = json!("implements");
        assert!(validate_explain(&payload).is_ok(), "clause swap is legal");

        let mut payload = full_report();
        payload["bases"][0]
            .as_object_mut()
            .expect("object")
            .remove("clause");
        assert!(
            validate_explain(&payload).is_ok(),
            "the clause column is optional (absent-tolerant)"
        );

        let mut payload = full_report();
        payload["ifaces"][0]["clause"] = json!("banana");
        let violation = validate_explain(&payload).expect_err("unknown clause");
        assert_eq!(violation.path, "ifaces[0].clause");

        let mut payload = full_report();
        payload["ctors"][0]
            .as_object_mut()
            .expect("object")
            .remove("sig");
        assert!(
            validate_explain(&payload).is_ok(),
            "the signature column is optional (absent-tolerant)"
        );

        let mut payload = full_report();
        payload["ctors"][0]["sig"] = json!(42);
        let violation = validate_explain(&payload).expect_err("non-string signature");
        assert_eq!(violation.path, "ctors[0].sig");

        let mut payload = full_report();
        payload["ctors"][0]["off"] = json!(1);
        let violation = validate_explain(&payload).expect_err("constructor rows are spanless");
        assert_eq!(violation.path, "ctors[0].off");
    }

    #[test]
    fn compact_report_validates() {
        assert!(validate_explain(&full_report()).is_ok());
        let empty = json!({
            "v": SCHEMA_VERSION_EXPLAIN,
            "sym": "Nope",
            "bases": [],
            "ifaces": [],
            "ctors": [],
            "methods": [],
            "refs": [],
            "deps": [],
            "tests": [],
        });
        assert!(validate_explain(&empty).is_ok());
    }

    #[test]
    fn minimal_report_validates() {
        assert!(validate_explain(&minimal_report()).is_ok());
        // The unknown-symbol floor: symbol and version only.
        let floor = json!({
            "v": SCHEMA_VERSION_EXPLAIN,
            "sym": "Nope",
        });
        assert!(validate_explain(&floor).is_ok());
    }

    #[test]
    fn version_constant_is_pinned() {
        let mut bumped = full_report();
        bumped["v"] = json!(SCHEMA_VERSION_EXPLAIN + 1);
        let violation = validate_explain(&bumped).expect_err("bumped version must violate");
        assert_eq!(violation.path, "v");
        assert_eq!(violation.expected, "the integer 1");
    }

    #[test]
    fn missing_symbol_is_named() {
        let mut payload = full_report();
        payload.as_object_mut().expect("object").remove("sym");
        let violation = validate_explain(&payload).expect_err("missing symbol must violate");
        assert_eq!(violation.path, "sym");
        assert_eq!(violation.found, "missing");
    }

    /// Type red lines on the new face's shapes: a range that is not a
    /// pair, a kind spelling outside its enum, a boolean spelled as a
    /// string, a counts entry spelled as a string, and an undeclared
    /// field — each named with its path.
    #[test]
    fn type_errors_are_named() {
        let mut payload = full_report();
        payload["declaration"]["off"] = json!(-1);
        let violation = validate_explain(&payload).expect_err("negative offset");
        assert_eq!(violation.path, "declaration.off");

        let mut payload = full_report();
        payload["declaration"]["len"] = json!("3");
        let violation = validate_explain(&payload).expect_err("string length");
        assert_eq!(violation.path, "declaration.len");

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
        payload["refs"][0]["qualified"] = json!("true");
        let violation = validate_explain(&payload).expect_err("string qualified");
        assert_eq!(violation.path, "refs[0].qualified");

        let mut payload = minimal_report();
        payload["counts"]["methods"] = json!("two");
        let violation = validate_explain(&payload).expect_err("string count");
        assert_eq!(violation.path, "counts.methods");

        let mut payload = full_report();
        payload["deps"][0]["file"] = json!("b.dart");
        let violation = validate_explain(&payload).expect_err("undeclared row field");
        assert_eq!(violation.path, "deps[0].file");
    }

    /// A counts object missing one section is a violation: the minimal
    /// tier always answers the full seven-entry map.
    #[test]
    fn partial_counts_are_rejected() {
        let mut payload = minimal_report();
        payload["counts"]
            .as_object_mut()
            .expect("object")
            .remove("refs");
        let violation = validate_explain(&payload).expect_err("missing count entry");
        assert_eq!(violation.path, "counts.refs");
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
        assert_eq!(violation.path, "bases");
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
