//! The `dartq query` JSON face (§15.2) — today the `query symbol` answer,
//! one single-line object (`dartq_commands::query::to_json`). Future
//! query kinds (`references`/`implementations`/`dependencies`/`callers`)
//! extend this envelope with additive optional arrays under the same
//! version — see the crate docs, versioning discipline.
//!
//! Two of those arrays are declared now, on the frozen v1 envelope, as the
//! freeze-discipline extension point: `implementations` (the
//! subclass/implementation declaration rows §15.2's `query implementations`
//! will serve, mixin applications included — the writer waits on the #281
//! mixin/override semantics) and `dependencies` (the reverse-import-graph
//! rows, typed on `dartq-ai::explain`'s `DependencyRow` — the importing
//! library's stable `path`, the shape the explain `dependencies` section
//! already serves). No writer emits either today; both are optional and
//! absent-tolerant at every tier, so the first emitter lands without
//! unfreezing v1.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
    pinned_version, rows, rows_optional, session_fields, string, string_or_null,
};

/// The query answer's contract version. The writer stamps
/// `dartq_commands::SCHEMA_VERSION`; the writer-side test in dartq-commands
/// pins the two equal, the fixtures in `tests/output_faces.rs` pin the
/// shape.
pub const SCHEMA_VERSION_QUERY: u32 = 1;

/// The published query answer schema (JSON Schema draft-07 subset).
#[must_use]
pub fn query_schema() -> &'static Value {
    &QUERY_SCHEMA
}

/// The declaration-row schema — one contract shape shared by `declarations`
/// and the reserved `implementations` array: an implementation row IS a
/// declaration hit (the subclass/implementor's declaration site), so the
/// two arrays cannot drift apart.
static DECLARATION_ITEM: LazyLock<Value> = LazyLock::new(|| {
    json!({
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
    })
});

static QUERY_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/query-answer.json",
        "title": "dartq query answer (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "name", "declarations", "references"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_QUERY },
            "name": { "type": "string" },
            "declarations": { "type": "array", "items": DECLARATION_ITEM.clone() },
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
                    // Additive optional on frozen v1 (#361): the use-site
                    // file of the row, present when the producer knows it.
                    // Absent-tolerant — v1 producers never emit it.
                    "file": { "type": "string" },
                },
            }},
            // Reserved additive kind arrays (crate docs above): declared on
            // frozen v1, emitted by no writer yet, absent-tolerant. Both are
            // non-locating detail sections — the minimal tier drops them
            // (`validate_query_at_tier`).
            "implementations": { "type": "array", "items": DECLARATION_ITEM.clone() },
            "dependencies": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    // The importing library's stable path
                    // (`dartq-ai::explain::DependencyRow`).
                    "path": { "type": "string" },
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
// The reserved additive kind arrays (crate docs): declared, no writer yet.
const QUERY_ADDITIVE: &[&str] = &["implementations", "dependencies"];
const DECLARATION: &[&str] = &["file", "kind", "name", "line", "column", "length"];
const DECLARATION_OPTIONAL: &[&str] = &["container"];
const REFERENCE: &[&str] = &["target_file", "kind", "offset", "length"];
const REFERENCE_OPTIONAL: &[&str] = &["qualified", "file"];
const DEPENDENCY: &[&str] = &["path"];

/// The declaration-row checks — the `declarations` contract, shared verbatim
/// by the reserved `implementations` rows (same [`DECLARATION_ITEM`] shape).
fn validate_declaration_row(
    obj: &serde_json::Map<String, Value>,
    p: &str,
) -> Result<(), SchemaViolation> {
    exact_fields(obj, DECLARATION, DECLARATION_OPTIONAL, p)?;
    string(field(obj, "file", p)?, &join(p, "file"))?;
    enum_spelling(field(obj, "kind", p)?, &join(p, "kind"), DECL_KINDS)?;
    string(field(obj, "name", p)?, &join(p, "name"))?;
    let line = join(p, "line");
    one_based(field(obj, "line", p)?, &line)?;
    let column = join(p, "column");
    one_based(field(obj, "column", p)?, &column)?;
    let length = join(p, "length");
    non_negative(field(obj, "length", p)?, &length)?;
    if let Some(container) = obj.get("container") {
        string_or_null(container, &join(p, "container"))?;
    }
    Ok(())
}

pub(crate) fn validate_query(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    // The additive arrays ride the optional slot next to the session
    // fields: declared, absent-tolerant, unknown-key rejection intact.
    let optional: Vec<&str> = QUERY_SESSION
        .iter()
        .chain(QUERY_ADDITIVE.iter())
        .copied()
        .collect();
    exact_fields(root, QUERY_TOP, &optional, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_QUERY)?;
    string(field(root, "name", "")?, "name")?;
    session_fields(root)?;
    for (i, row) in rows(root, "declarations")? {
        let p = format!("declarations[{i}]");
        validate_declaration_row(object(row, &p)?, &p)?;
    }
    for (i, row) in rows(root, "references")? {
        let p = format!("references[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, REFERENCE, REFERENCE_OPTIONAL, &p)?;
        string(field(obj, "target_file", &p)?, &join(&p, "target_file"))?;
        enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), REF_KINDS)?;
        let offset = join(&p, "offset");
        non_negative(field(obj, "offset", &p)?, &offset)?;
        let length = join(&p, "length");
        non_negative(field(obj, "length", &p)?, &length)?;
        if let Some(qualified) = obj.get("qualified") {
            string(qualified, &join(&p, "qualified"))?;
        }
        if let Some(file) = obj.get("file") {
            string(file, &join(&p, "file"))?;
        }
    }
    // The reserved arrays validate their rows when present; no writer emits
    // them yet (crate docs), so absence is the everyday shape.
    for (i, row) in rows_optional(root, "implementations")? {
        let p = format!("implementations[{i}]");
        validate_declaration_row(object(row, &p)?, &p)?;
    }
    for (i, row) in rows_optional(root, "dependencies")? {
        let p = format!("dependencies[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, DEPENDENCY, &[], &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): minimal keeps the
/// row locators; the convenience spellings (`container`, `qualified`) are
/// the compact tier's contribution, and the reserved kind arrays
/// (`implementations`, `dependencies`) are dropped entirely — non-locating
/// detail sections (crate docs, query table). The additive use-file column
/// (`references[].file`) and the session fields ride absent-tolerant at
/// every tier (crate docs, "Token economics").
pub(crate) fn validate_query_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_query(value)?;
    if tier != Verbosity::Minimal {
        return Ok(());
    }
    let root = object(value, "")?;
    absent(root, "implementations", "", tier)?;
    absent(root, "dependencies", "", tier)?;
    for (i, row) in rows(root, "declarations")? {
        let p = format!("declarations[{i}]");
        absent(object(row, &p)?, "container", &p, tier)?;
    }
    for (i, row) in rows(root, "references")? {
        let p = format!("references[{i}]");
        absent(object(row, &p)?, "qualified", &p, tier)?;
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
            .chain(QUERY_ADDITIVE.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(query), top);
        // One declaration-row shape serves both declaration-bearing arrays.
        let declarations = &query["properties"]["declarations"]["items"];
        assert_eq!(
            declarations,
            &query["properties"]["implementations"]["items"]
        );
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

    /// Tier exactness: a minimal answer omits the convenience spellings;
    /// the same answer is exact at compact and full.
    #[test]
    fn minimal_tier_rejects_convenience_spellings() {
        let doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION_QUERY,
            "name": "fb",
            "declarations": [{
                "file": "/w/lib/a.dart",
                "container": "fb",
                "kind": "class",
                "name": "fb",
                "line": 3,
                "column": 7,
                "length": 2,
            }],
            "references": [{
                "target_file": "/w/lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "offset": 42,
                "length": 2,
            }],
        });
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the container");
        assert_eq!(violation.path, "declarations[0].container");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_query_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Full).is_ok());
    }

    /// The reserved additive kind arrays: present, their rows validate under
    /// the frozen v1 (implementations carry the declaration-row contract,
    /// dependencies the `path` row); a minimal answer drops them; and the
    /// undeclared-field rejection still holds inside them.
    #[test]
    fn reserved_kind_arrays_validate_and_minimal_drops_them() {
        let mut doc = serde_json::json!({
            "schema_version": SCHEMA_VERSION_QUERY,
            "name": "fb",
            "declarations": [{
                "file": "/w/lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "column": 7,
                "length": 2,
            }],
            "references": [],
            "implementations": [{
                "file": "/w/lib/sub.dart",
                "container": "Sub",
                "kind": "class",
                "name": "Sub",
                "line": 4,
                "column": 7,
                "length": 3,
            }],
            "dependencies": [{ "path": "/w/lib/app.dart" }],
        });
        assert!(validate_query(&doc).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Full).is_ok());
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the kind arrays");
        assert_eq!(violation.path, "implementations");
        assert_eq!(violation.expected, "omitted at the minimal tier");

        // Absent is the everyday shape — the no-writer-yet posture. The
        // minimal floor trims the convenience spelling too (crate docs,
        // query table), so the minimal variant drops it alongside the
        // arrays.
        let mut minimal = doc.clone();
        minimal
            .as_object_mut()
            .expect("object")
            .remove("implementations");
        minimal
            .as_object_mut()
            .expect("object")
            .remove("dependencies");
        minimal["declarations"][0]
            .as_object_mut()
            .expect("object")
            .remove("container");
        assert!(validate_query_at_tier(&minimal, Verbosity::Minimal).is_ok());

        // The additive rows are contract-bound too: an undeclared field in
        // an implementations row is rejected like any other.
        doc["implementations"] = serde_json::json!([{
            "file": "/w/lib/sub.dart",
            "kind": "class",
            "name": "Sub",
            "line": 4,
            "column": 7,
            "length": 3,
            "override_depth": 2,
        }]);
        let violation = validate_query(&doc).expect_err("undeclared field");
        assert_eq!(violation.path, "implementations[0].override_depth");
    }
}
