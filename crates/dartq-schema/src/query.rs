//! The `dartq query` JSON face — today the `query symbol` answer,
//! one single-line object (`dartq_commands::query::to_json`). Future
//! query kinds (`references`/`implementations`/`dependencies`/`callers`)
//! extend this envelope with additive optional arrays under the same
//! version — see the crate docs, versioning discipline.
//!
//! The file columns spell through the one file locator: root-relative
//! inside the analysis root, absolute outside it, never a `package:` URI.
//! The envelope's optional `root` (the absolute analysis root) rides
//! exactly when the answer carries a file-bearing row, making the
//! relative tails unambiguous.
//!
//! Two of those arrays are declared now, on the frozen v1 envelope, as the
//! freeze-discipline extension point: `implementations` (the
//! subclass/implementation declaration rows the `query implementations`
//! will serve, mixin applications included — the writer waits on the #281
//! mixin/override semantics) and `dependencies` (the declaring library's
//! own import edges and consumed symbols — the forward direction — typed
//! on `dartq-ai::explain`'s `DependencyRow`: the joined library's stable
//! `path` plus the optional `name` column a per-symbol row carries; the
//! reverse question, who imports the declaring library, is the impact
//! face's and the explain `dependencies` section's answer). Both arrays
//! are optional and absent-tolerant at every tier, so a producer lands
//! without unfreezing v1.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::Verbosity;
use crate::support::{
    absent, array, enum_spelling, exact_fields, field, join, non_negative, object, one_based,
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
/// two arrays cannot drift apart. The additive `signature`/`annotations`
/// columns carry the element face's join (the rendered member signature,
/// the container's annotations); shapes with no matching face keep the
/// keys omitted.
static DECLARATION_ITEM: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file", "kind", "name", "line", "col", "len"],
        "properties": {
            "file": { "type": "string" },
            // The member container's name; top-level rows omit the key
            // (the omission rule — a convenience spelling, not a
            // locator). The union type stays: earlier producers emitted
            // an explicit null and the validator stays tolerant of it.
            "container": { "type": ["string", "null"] },
            "kind": { "enum": DECL_KINDS },
            "name": { "type": "string" },
            "line": { "type": "integer", "minimum": 1 },
            "col": { "type": "integer", "minimum": 1 },
            "len": { "type": "integer", "minimum": 0 },
            // Additive optional on frozen v1: the rendered signature face
            // (members and top-level executables; never a lossy one).
            "sig": { "type": "string" },
            // Additive optional on frozen v1: the container face's
            // annotations, `@name(args)` spellings in written order.
            "annotations": { "type": "array", "items": { "type": "string" } },
        },
    })
});

static QUERY_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/query-answer.json",
        "title": "dartq query answer (v 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "sym", "decls", "refs"],
        "properties": {
            "v": { "const": SCHEMA_VERSION_QUERY },
            // The analysis root the file columns locate against (absolute):
            // root-relative inside it, absolute outside (the SDK, the pub
            // cache, another package's checkout). Rides exactly once when
            // the answer carries a file-bearing row; omitted when it does
            // not (the unknown-symbol floor).
            "root": { "type": "string" },
            "sym": { "type": "string" },
            "decls": { "type": "array", "items": DECLARATION_ITEM.clone() },
            "refs": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["target", "kind", "off", "len"],
                "properties": {
                    "target": { "type": "string" },
                    "kind": { "enum": REF_KINDS },
                    "qualified": { "type": "string" },
                    "off": { "type": "integer", "minimum": 0 },
                    "len": { "type": "integer", "minimum": 0 },
                    // Additive optional on frozen v1 (#361): the use-site
                    // file of the row, present when the producer knows it.
                    // The query producers emit it (wired with the freeze);
                    // absent-tolerant for other producers.
                    "file": { "type": "string" },
                },
            }},
            // Reserved additive kind arrays (crate docs above): declared on
            // frozen v1, emitted by no writer yet, absent-tolerant. Both are
            // non-locating detail sections — the minimal tier drops them
            // (`validate_query_at_tier`).
            "implementations": { "type": "array", "items": DECLARATION_ITEM.clone() },
            "deps": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    // The joined library's stable path
                    // (`dartq-ai::explain::DependencyRow`).
                    "path": { "type": "string" },
                    // The dependency-library symbol the declaring
                    // library's resolved uses consume — the forward
                    // direction's per-symbol rows; a bare import edge
                    // keeps the key absent.
                    "name": { "type": "string" },
                },
            }},
            // Session token economics (crate docs, "Token economics").
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

/// The stable machine spellings of the reference relations — exhaustive
/// over the stored capture kinds plus the engine query face's
/// `override` widening (the overriding-declaration rows the references
/// answer appends for a queried member; the stored rows themselves map
/// 1:1 onto the first five).
pub const REF_KINDS: &[&str] = &[
    "reference",
    "read",
    "invoked",
    "written",
    "read-written",
    "override",
];

const QUERY_TOP: &[&str] = &["v", "sym", "decls", "refs"];
/// The locator envelope's key — the analysis root the file columns locate
/// against, riding exactly when a file-bearing row does.
const QUERY_LOCATOR: &[&str] = &["root"];
const QUERY_SESSION: &[&str] = &["symbols", "since"];
// The reserved additive kind arrays (crate docs): declared, no writer yet.
const QUERY_ADDITIVE: &[&str] = &["implementations", "deps"];
const DECLARATION: &[&str] = &["file", "kind", "name", "line", "col", "len"];
const DECLARATION_OPTIONAL: &[&str] = &["container", "sig", "annotations"];
const REFERENCE: &[&str] = &["target", "kind", "off", "len"];
const REFERENCE_OPTIONAL: &[&str] = &["qualified", "file"];
const DEPENDENCY: &[&str] = &["path"];
const DEPENDENCY_OPTIONAL: &[&str] = &["name"];

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
    let column = join(p, "col");
    one_based(field(obj, "col", p)?, &column)?;
    let length = join(p, "len");
    non_negative(field(obj, "len", p)?, &length)?;
    if let Some(container) = obj.get("container") {
        string_or_null(container, &join(p, "container"))?;
    }
    if let Some(signature) = obj.get("sig") {
        string(signature, &join(p, "sig"))?;
    }
    if let Some(annotations) = obj.get("annotations") {
        for (i, annotation) in array(annotations, &join(p, "annotations"))?
            .iter()
            .enumerate()
        {
            string(annotation, &format!("{}.annotations[{i}]", p))?;
        }
    }
    Ok(())
}

pub(crate) fn validate_query(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    // The additive arrays and the locator envelope ride the optional slot
    // next to the session fields: declared, absent-tolerant, unknown-key
    // rejection intact.
    let optional: Vec<&str> = QUERY_LOCATOR
        .iter()
        .chain(QUERY_SESSION.iter())
        .chain(QUERY_ADDITIVE.iter())
        .copied()
        .collect();
    exact_fields(root, QUERY_TOP, &optional, "")?;
    pinned_version(root, "v", SCHEMA_VERSION_QUERY)?;
    string(field(root, "sym", "")?, "sym")?;
    if let Some(locator_root) = root.get("root") {
        string(locator_root, "root")?;
    }
    session_fields(root)?;
    for (i, row) in rows(root, "decls")? {
        let p = format!("decls[{i}]");
        validate_declaration_row(object(row, &p)?, &p)?;
    }
    for (i, row) in rows(root, "refs")? {
        let p = format!("refs[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, REFERENCE, REFERENCE_OPTIONAL, &p)?;
        string(field(obj, "target", &p)?, &join(&p, "target"))?;
        enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), REF_KINDS)?;
        let offset = join(&p, "off");
        non_negative(field(obj, "off", &p)?, &offset)?;
        let length = join(&p, "len");
        non_negative(field(obj, "len", &p)?, &length)?;
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
    for (i, row) in rows_optional(root, "deps")? {
        let p = format!("deps[{i}]");
        let obj = object(row, &p)?;
        exact_fields(obj, DEPENDENCY, DEPENDENCY_OPTIONAL, &p)?;
        string(field(obj, "path", &p)?, &join(&p, "path"))?;
        if let Some(name) = obj.get("name") {
            string(name, &join(&p, "name"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): minimal keeps the
/// row locators; the convenience spellings (`container`, `qualified`) and
/// the element-face detail columns (`signature`, `annotations`) are the
/// compact tier's contribution, and the reserved kind arrays
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
    absent(root, "deps", "", tier)?;
    for (i, row) in rows(root, "decls")? {
        let p = format!("decls[{i}]");
        let obj = object(row, &p)?;
        absent(obj, "container", &p, tier)?;
        absent(obj, "sig", &p, tier)?;
        absent(obj, "annotations", &p, tier)?;
    }
    for (i, row) in rows(root, "refs")? {
        let p = format!("refs[{i}]");
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
            .chain(QUERY_LOCATOR.iter())
            .chain(QUERY_SESSION.iter())
            .chain(QUERY_ADDITIVE.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort();
        assert_eq!(schema_keys(query), top);
        // One declaration-row shape serves both declaration-bearing arrays.
        let declarations = &query["properties"]["decls"]["items"];
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
        let references = &query["properties"]["refs"]["items"];
        let mut ref_fields: Vec<String> = REFERENCE
            .iter()
            .chain(REFERENCE_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        ref_fields.sort_unstable();
        assert_eq!(schema_keys(references), ref_fields);
        let dependencies = &query["properties"]["deps"]["items"];
        let mut dep_fields: Vec<String> = DEPENDENCY
            .iter()
            .chain(DEPENDENCY_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        dep_fields.sort_unstable();
        assert_eq!(schema_keys(dependencies), dep_fields);
    }

    #[test]
    fn enum_spellings_match_the_declared_lists() {
        let query = query_schema();
        let decl_enum = &query["properties"]["decls"]["items"]["properties"]["kind"];
        let ref_enum = &query["properties"]["refs"]["items"]["properties"]["kind"];
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

    /// The locator envelope: `root` rides absent-tolerant (the
    /// unknown-symbol floor carries no file field, so no root), a present
    /// `root` is a string, and a non-string is named — the relative tails
    /// it disambiguates are the file columns' only ambiguity.
    #[test]
    fn the_locator_root_validates_and_a_non_string_is_named() {
        let doc = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "root": "/w/pkg",
            "sym": "fb",
            "decls": [{
                "file": "lib/a.dart",
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            }],
            "refs": [],
        });
        assert!(validate_query(&doc).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Minimal).is_ok());

        let floor = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "sym": "Nope",
            "decls": [],
            "refs": [],
        });
        assert!(validate_query(&floor).is_ok());

        let mut bad = doc.clone();
        bad["root"] = serde_json::json!(7);
        let violation = validate_query(&bad).expect_err("non-string root");
        assert_eq!(violation.path, "root");
        assert_eq!(violation.expected, "a string");
    }

    /// The `override` spelling validates on a reference row — the
    /// overriding-declaration rows the engine's references answer appends
    /// for a queried member (a kind widened at the query boundary, the
    /// stored capture kinds untouched). An unknown spelling stays rejected.
    #[test]
    fn override_reference_kind_validates() {
        let doc = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "sym": "m",
            "decls": [],
            "refs": [{
                "target": "/w/lib/base.dart",
                "kind": "override",
                "file": "/w/lib/sub.dart",
                "off": 44,
                "len": 1,
            }],
        });
        assert!(validate_query(&doc).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Minimal).is_ok());

        let mut bad = doc.clone();
        bad["refs"][0]["kind"] = serde_json::json!("overridden");
        let violation = validate_query(&bad).expect_err("unknown spelling");
        assert_eq!(violation.path, "refs[0].kind");
    }

    /// Tier exactness: a minimal answer omits the convenience spellings;
    /// the same answer is exact at compact and full.
    #[test]
    fn minimal_tier_rejects_convenience_spellings() {
        let doc = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "sym": "fb",
            "decls": [{
                "file": "/w/lib/a.dart",
                "container": "fb",
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            }],
            "refs": [{
                "target": "/w/lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "off": 42,
                "len": 2,
            }],
        });
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the container");
        assert_eq!(violation.path, "decls[0].container");
        assert_eq!(violation.expected, "omitted at the minimal tier");
        assert!(validate_query_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Full).is_ok());
    }

    /// The additive element-face columns (`signature`, `annotations`):
    /// present they are type-checked, absent they are the everyday shape
    /// (rows without a matching face), and the minimal tier drops them
    /// with the other convenience spellings.
    #[test]
    fn element_face_columns_validate_and_minimal_drops_them() {
        let mut doc = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "sym": "limit",
            "decls": [{
                "file": "/w/lib/a.dart",
                "container": "Widget",
                "kind": "field",
                "name": "limit",
                "line": 3,
                "col": 21,
                "len": 5,
                "sig": "static final int limit",
            }],
            "refs": [],
        });
        assert!(validate_query(&doc).is_ok());

        doc["decls"][0]["annotations"] = serde_json::json!(["@Deprecated('use Other')"]);
        assert!(validate_query(&doc).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Compact).is_ok());
        assert!(validate_query_at_tier(&doc, Verbosity::Full).is_ok());

        // The minimal tier strips the element-face columns with the other
        // convenience spellings, one violation at a time.
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the container");
        assert_eq!(violation.path, "decls[0].container");
        doc["decls"][0]
            .as_object_mut()
            .expect("object")
            .remove("container");
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the signature");
        assert_eq!(violation.path, "decls[0].sig");
        doc["decls"][0]
            .as_object_mut()
            .expect("object")
            .remove("sig");
        let violation = validate_query_at_tier(&doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the annotations");
        assert_eq!(violation.path, "decls[0].annotations");

        // Absent is the everyday shape: rows without a matching face.
        doc["decls"][0]
            .as_object_mut()
            .expect("object")
            .remove("annotations");
        let bare = doc.clone();
        assert!(validate_query(&bare).is_ok());
        assert!(validate_query_at_tier(&bare, Verbosity::Minimal).is_ok());

        // Wrong types are named.
        doc["decls"][0]["sig"] = serde_json::json!(42);
        let violation = validate_query(&doc).expect_err("non-string signature");
        assert_eq!(violation.path, "decls[0].sig");
        doc["decls"][0]["sig"] = serde_json::json!("static final int limit");
        doc["decls"][0]["annotations"] = serde_json::json!("@Deprecated");
        let violation = validate_query(&doc).expect_err("non-array annotations");
        assert_eq!(violation.path, "decls[0].annotations");
        doc["decls"][0]["annotations"] = serde_json::json!([42]);
        let violation = validate_query(&doc).expect_err("non-string annotation entry");
        assert_eq!(violation.path, "decls[0].annotations[0]");
    }

    /// The reserved additive kind arrays: present, their rows validate under
    /// the frozen v1 (implementations carry the declaration-row contract,
    /// dependencies the `path` row); a minimal answer drops them; and the
    /// undeclared-field rejection still holds inside them.
    #[test]
    fn reserved_kind_arrays_validate_and_minimal_drops_them() {
        let mut doc = serde_json::json!({
            "v": SCHEMA_VERSION_QUERY,
            "sym": "fb",
            "decls": [{
                "file": "/w/lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            }],
            "refs": [],
            "implementations": [{
                "file": "/w/lib/sub.dart",
                "container": "Sub",
                "kind": "class",
                "name": "Sub",
                "line": 4,
                "col": 7,
                "len": 3,
            }],
            "deps": [{ "path": "/w/lib/app.dart" }],
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
        minimal.as_object_mut().expect("object").remove("deps");
        minimal["decls"][0]
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
            "col": 7,
            "len": 3,
            "override_depth": 2,
        }]);
        let violation = validate_query(&doc).expect_err("undeclared field");
        assert_eq!(violation.path, "implementations[0].override_depth");
    }
}
