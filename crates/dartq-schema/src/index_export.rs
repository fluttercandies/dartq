//! The `dartq index --full` export stream — the workspace's maintained
//! symbol index serialized as JSONL: one header line carrying the stream's
//! row totals, then one document per analyzed file (the SCIP document
//! model — upstream Sourcegraph SCIP — re-expressed as JSON objects; the
//! symbol table is separated from the rows that reference it, and local
//! rows cross-reference it by short id).
//!
//! The writer is `dartq_commands::index::run_export`, which walks the
//! engine's maintained index state (per-file [`FileIndex`] declaration
//! surfaces plus per-library [`LibraryIndex`] reference surfaces) and
//! validates every line at birth through [`crate::validate_at_tier`].
//!
//! # Line shapes
//!
//! Header (always line 1):
//!
//! ```json
//! {"schema_version":1,"documents":3,"declarations":5}
//! ```
//!
//! `documents` counts the document lines that follow; `declarations` sums
//! every document's `symbol_count`. At the `full` tier the header also
//! carries `occurrences` and `supertypes` — the emitted row totals (a tier
//! that emits no such rows must not carry the keys, so a consumer can tell
//! "zero exported" from "not exported at this tier").
//!
//! Document (one per analyzed file, ascending by canonical path):
//!
//! ```json
//! {"schema_version":1,"path":"package:t/a.dart","symbol_count":2,
//!  "symbols":{"s1":"package:t/a.dart|Foo","s2":"package:t/a.dart|top"},
//!  "declarations":[{"symbol":"s1","kind":"class","name":"Foo","range":[0,3]}],
//!  "occurrences":[{"symbol":"s2","kind":"invoked","range":[40,42]}],
//!  "supertypes":[{"symbol":"s1","clause":"extends","supertype":"package:t/b.dart|Base"}]}
//! ```
//!
//! * `path` is the stable package-form file reference (the shared
//!   `pkg_label` grammar: `package:<name>/<lib-relative tail>`, canonical
//!   path as the fallback). Document order keys on the canonical path
//!   bytes, not the projected label.
//! * `symbol_count` is the document's declaration-row count — the minimal
//!   tier's whole content alongside `path`.
//! * `symbols` is the document's short-id table: `s1`, `s2`, … assigned in
//!   declaration order (the `(name, offset)` order [`FileIndex`] stores),
//!   each mapping to the stable `label|name` entity reference. The
//!   assignment is document-scoped: an id is meaningful only alongside
//!   this line's table.
//! * `declarations[].symbol` names the row's own short id; `container` is
//!   the enclosing container row's short id, omitted for top-level rows
//!   and unnamed-extension members (the omission convention). `range` is
//!   the `[startOffset, endOffset)` pair of the name token, in UTF-16 code
//!   units — the engine's external convention (`SourceRange`).
//! * `occurrences[].symbol` resolves the reference target: `s<k>` when the
//!   target declares in this document (an entry in this line's table), the
//!   full `label|name` otherwise. `kind` spells the stored relation set
//!   (`EXPORT_REF_KINDS` — no `override`; that spelling is a query-face
//!   widening, not a stored kind). `qualifier` is the import prefix that
//!   armed the use (`p` in `p.fa`), omitted for bare uses.
//! * `supertypes[].symbol` is the subtype container's short id; `clause`
//!   spells the header clause (`extends`/`with`/`on`/`implements`);
//!   `supertype` resolves like an occurrence symbol — `s<k>` in-document,
//!   `label|name` external, or a bare name when the supertype lies outside
//!   the analyzed set (`TargetRef::Name`: `dart:` SDK, unanalyzed
//!   dependency — the official string-keyed fallback).
//! * The unresolved-names recovery table is deliberately not exported: it
//!   is resolution residue (the `undefined_*` surface), not index
//!   content, and carries no use-site file to key a document on.
//!
//! # Tiers
//!
//! | document field   | minimal | compact | full |
//! |------------------|---------|---------|------|
//! | `schema_version` | ●       | ●       | ●    |
//! | `path`           | ●       | ●       | ●    |
//! | `symbol_count`   | ●       | ●       | ●    |
//! | `symbols`        |         | ●       | ●    |
//! | `declarations`   |         | ●       | ●    |
//! | `occurrences`    |         |         | ●    |
//! | `supertypes`     |         |         | ●    |
//!
//! Header fields: `schema_version`, `documents`, `declarations` ride every
//! tier; `occurrences`, `supertypes` join only at `full`.
//!
//! # Determinism
//!
//! Same input, same bytes: documents ascend by canonical path bytes;
//! `symbols` assigns ids in the stored `(name, offset)` declaration order;
//! occurrences sort by `(start, end, kind, target symbol)`; supertypes
//! keep the library table's `(subtype file, name, clause)` order filtered
//! to the document's file — every order is a stored order or a stated
//! sort, never a hash iteration.
//!
//! [`FileIndex`]: dartq_index::FileIndex
//! [`LibraryIndex`]: dartq_index::LibraryIndex

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::support::{
    array, enum_spelling, exact_fields, field, join, non_negative, object, pinned_version,
    range_pair, string, violation,
};
use crate::{SchemaViolation, Verbosity};

/// The export stream's contract version. The writer stamps the shared
/// `dartq_commands::SCHEMA_VERSION`; the fixtures pin the two equal.
pub const SCHEMA_VERSION_INDEX_EXPORT: u32 = 1;

/// The published export document schema (JSON Schema draft-07 subset) —
/// the per-line document shape. The header line's shape is documented in
/// the module docs and pinned by the validator.
#[must_use]
pub fn index_export_schema() -> &'static Value {
    &INDEX_EXPORT_SCHEMA
}

/// The stored relation spellings: the capture's five `RefKind`s — no
/// `override` (a query-face widening, never a stored row kind).
pub const EXPORT_REF_KINDS: &[&str] =
    &["reference", "read", "invoked", "written", "read-written"];

/// The supertype clause spellings, `SuperClause` declaration order.
pub const SUPER_CLAUSES: &[&str] = &["extends", "with", "on", "implements"];

static INDEX_EXPORT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/index-export.json",
        "title": "dartq index export document (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "path", "symbol_count"],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_INDEX_EXPORT },
            // The stable package-form file reference (pkg_label).
            "path": { "type": "string" },
            // The document's declaration-row count (the minimal tier's
            // content alongside `path`).
            "symbol_count": { "type": "integer", "minimum": 0 },
            // Short-id → `label|name` entity reference; document-scoped
            // assignment in declaration order.
            "symbols": { "type": "object",
                "additionalProperties": { "type": "string" } },
            "declarations": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["symbol", "kind", "name", "range"],
                "properties": {
                    "symbol": { "type": "string" },
                    "kind": { "enum": crate::query::DECL_KINDS },
                    "name": { "type": "string" },
                    "range": { "$ref": "#/definitions/range_pair" },
                    "container": { "type": "string" },
                },
            }},
            "occurrences": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["symbol", "kind", "range"],
                "properties": {
                    "symbol": { "type": "string" },
                    "kind": { "enum": EXPORT_REF_KINDS },
                    "range": { "$ref": "#/definitions/range_pair" },
                    "qualifier": { "type": "string" },
                },
            }},
            "supertypes": { "type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["symbol", "clause", "supertype"],
                "properties": {
                    "symbol": { "type": "string" },
                    "clause": { "enum": SUPER_CLAUSES },
                    "supertype": { "type": "string" },
                },
            }},
        },
        "definitions": {
            // A `[startOffset, endOffset)` pair, UTF-16 code units.
            "range_pair": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "minItems": 2,
                "maxItems": 2,
            },
        },
    })
});

const HEADER: &[&str] = &["schema_version", "documents", "declarations"];
const HEADER_FULL: &[&str] = &["occurrences", "supertypes"];

const DOCUMENT: &[&str] = &["schema_version", "path", "symbol_count"];
const DOCUMENT_OPTIONAL: &[&str] =
    &["symbols", "declarations", "occurrences", "supertypes"];
const DECLARATION: &[&str] = &["symbol", "kind", "name", "range"];
const DECLARATION_OPTIONAL: &[&str] = &["container"];
const OCCURRENCE: &[&str] = &["symbol", "kind", "range"];
const OCCURRENCE_OPTIONAL: &[&str] = &["qualifier"];
const SUPERTYPE: &[&str] = &["symbol", "clause", "supertype"];

/// Validates one export line: the header shape when the payload carries
/// `documents`, the document shape when it carries `path`. Union-valid
/// ([`crate::validate`]) — a stream validates line by line.
pub(crate) fn validate_index_export(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    if root.contains_key("documents") {
        return validate_header(root);
    }
    if root.contains_key("path") {
        return validate_document(root);
    }
    Err(violation(
        "",
        "an export header (`documents`) or document (`path`) line",
        "an object with neither",
    ))
}

fn validate_header(root: &serde_json::Map<String, Value>) -> Result<(), SchemaViolation> {
    exact_fields(root, HEADER, HEADER_FULL, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_INDEX_EXPORT)?;
    for key in ["documents", "declarations", "occurrences", "supertypes"] {
        if let Some(count) = root.get(key) {
            non_negative(count, key)?;
        }
    }
    Ok(())
}

fn validate_document(root: &serde_json::Map<String, Value>) -> Result<(), SchemaViolation> {
    exact_fields(root, DOCUMENT, DOCUMENT_OPTIONAL, "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_INDEX_EXPORT)?;
    string(field(root, "path", "")?, "path")?;
    non_negative(field(root, "symbol_count", "")?, "symbol_count")?;
    if let Some(symbols) = root.get("symbols") {
        let table = object(symbols, "symbols")?;
        for (id, entry) in table {
            crate::support::symbol_ref(entry, &join("symbols", id))?;
        }
    }
    if let Some(decls) = root.get("declarations") {
        for (i, row) in array(decls, "declarations")?.iter().enumerate() {
            let p = format!("declarations[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, DECLARATION, DECLARATION_OPTIONAL, &p)?;
            string(field(obj, "symbol", &p)?, &join(&p, "symbol"))?;
            enum_spelling(
                field(obj, "kind", &p)?,
                &join(&p, "kind"),
                crate::query::DECL_KINDS,
            )?;
            string(field(obj, "name", &p)?, &join(&p, "name"))?;
            range_pair(field(obj, "range", &p)?, &join(&p, "range"))?;
            if let Some(container) = obj.get("container") {
                string(container, &join(&p, "container"))?;
            }
        }
    }
    if let Some(occs) = root.get("occurrences") {
        for (i, row) in array(occs, "occurrences")?.iter().enumerate() {
            let p = format!("occurrences[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, OCCURRENCE, OCCURRENCE_OPTIONAL, &p)?;
            string(field(obj, "symbol", &p)?, &join(&p, "symbol"))?;
            enum_spelling(field(obj, "kind", &p)?, &join(&p, "kind"), EXPORT_REF_KINDS)?;
            range_pair(field(obj, "range", &p)?, &join(&p, "range"))?;
            if let Some(qualifier) = obj.get("qualifier") {
                string(qualifier, &join(&p, "qualifier"))?;
            }
        }
    }
    if let Some(supers) = root.get("supertypes") {
        for (i, row) in array(supers, "supertypes")?.iter().enumerate() {
            let p = format!("supertypes[{i}]");
            let obj = object(row, &p)?;
            exact_fields(obj, SUPERTYPE, &[], &p)?;
            string(field(obj, "symbol", &p)?, &join(&p, "symbol"))?;
            enum_spelling(field(obj, "clause", &p)?, &join(&p, "clause"), SUPER_CLAUSES)?;
            string(field(obj, "supertype", &p)?, &join(&p, "supertype"))?;
        }
    }
    Ok(())
}

/// The tier-exact check ([`crate::validate_at_tier`]): the union shape
/// plus the tier's omission rules — `minimal` documents carry only the
/// red line (`path` + `symbol_count`), `compact` adds the symbol table and
/// declaration rows, `full` adds the occurrence and supertype rows. The
/// header carries its count columns by the same tiers: `occurrences` and
/// `supertypes` join it only at `full`.
pub(crate) fn validate_index_export_at_tier(
    value: &Value,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    validate_index_export(value)?;
    let root = object(value, "")?;
    if root.contains_key("documents") {
        if tier != Verbosity::Full {
            for key in HEADER_FULL {
                crate::support::absent(root, key, "", tier)?;
            }
        }
        return Ok(());
    }
    match tier {
        Verbosity::Minimal => {
            for key in DOCUMENT_OPTIONAL {
                crate::support::absent(root, key, "", tier)?;
            }
        }
        Verbosity::Compact => {
            for key in ["occurrences", "supertypes"] {
                crate::support::absent(root, key, "", tier)?;
            }
        }
        Verbosity::Full => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::schema_keys;

    /// Drift pin: the published document and the validator name the same
    /// fields at every level.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let schema = index_export_schema();
        let mut top: Vec<String> = DOCUMENT
            .iter()
            .chain(DOCUMENT_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        top.sort_unstable();
        assert_eq!(schema_keys(schema), top);
        let decls = &schema["properties"]["declarations"]["items"];
        let mut decl_fields: Vec<String> = DECLARATION
            .iter()
            .chain(DECLARATION_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        decl_fields.sort_unstable();
        assert_eq!(schema_keys(decls), decl_fields);
        let occs = &schema["properties"]["occurrences"]["items"];
        let mut occ_fields: Vec<String> = OCCURRENCE
            .iter()
            .chain(OCCURRENCE_OPTIONAL.iter())
            .map(|s| (*s).to_owned())
            .collect();
        occ_fields.sort_unstable();
        assert_eq!(schema_keys(occs), occ_fields);
        let supers = &schema["properties"]["supertypes"]["items"];
        assert_eq!(
            schema_keys(supers),
            crate::support::const_names(SUPERTYPE)
        );
    }

    /// Both line shapes validate: the header (minimal/compact counts and
    /// the full-tier extended counts) and a document at each tier.
    #[test]
    fn header_and_document_shapes_validate() {
        let header = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "documents": 2,
            "declarations": 5,
        });
        assert!(validate_index_export_at_tier(&header, Verbosity::Minimal).is_ok());
        assert!(validate_index_export_at_tier(&header, Verbosity::Compact).is_ok());
        assert!(validate_index_export_at_tier(&header, Verbosity::Full).is_ok());

        let full_header = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "documents": 2,
            "declarations": 5,
            "occurrences": 7,
            "supertypes": 1,
        });
        assert!(validate_index_export_at_tier(&full_header, Verbosity::Full).is_ok());
        // Below full the extended counts must not ride along.
        let violation = validate_index_export_at_tier(&full_header, Verbosity::Compact)
            .expect_err("occurrences is full-tier only");
        assert_eq!(violation.path, "occurrences");
        assert_eq!(violation.expected, "omitted at the compact tier");

        let minimal_doc = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/a.dart",
            "symbol_count": 2,
        });
        for tier in [Verbosity::Minimal, Verbosity::Compact, Verbosity::Full] {
            assert!(
                validate_index_export_at_tier(&minimal_doc, tier).is_ok(),
                "the red line validates at {tier}"
            );
        }

        let compact_doc = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/a.dart",
            "symbol_count": 1,
            "symbols": {"s1": "package:t/a.dart|Foo"},
            "declarations": [{
                "symbol": "s1",
                "kind": "class",
                "name": "Foo",
                "range": [0, 3],
            }],
        });
        assert!(validate_index_export_at_tier(&compact_doc, Verbosity::Compact).is_ok());
        assert!(validate_index_export_at_tier(&compact_doc, Verbosity::Full).is_ok());
        let violation = validate_index_export_at_tier(&compact_doc, Verbosity::Minimal)
            .expect_err("minimal must not carry the table");
        assert_eq!(violation.path, "symbols");

        let full_doc = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/b.dart",
            "symbol_count": 1,
            "symbols": {"s1": "package:t/b.dart|B"},
            "declarations": [{
                "symbol": "s1",
                "kind": "class",
                "name": "B",
                "range": [0, 1],
                "container": "s1",
            }],
            "occurrences": [{
                "symbol": "package:t/a.dart|Foo",
                "kind": "invoked",
                "range": [40, 43],
                "qualifier": "p",
            }],
            "supertypes": [{
                "symbol": "s1",
                "clause": "extends",
                "supertype": "package:t/a.dart|Foo",
            }],
        });
        assert!(validate_index_export_at_tier(&full_doc, Verbosity::Full).is_ok());
        let violation = validate_index_export_at_tier(&full_doc, Verbosity::Compact)
            .expect_err("compact must not carry occurrences");
        assert_eq!(violation.path, "occurrences");
    }

    /// The red line holds inside the rows: undeclared fields, wrong types,
    /// unknown enum spellings, and a line that is neither shape are named.
    #[test]
    fn violations_name_the_first_bad_field() {
        let bad_kind = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/b.dart",
            "symbol_count": 0,
            "occurrences": [{
                "symbol": "package:t/a.dart|Foo",
                "kind": "override",
                "range": [1, 2],
            }],
        });
        let violation = validate_index_export(&bad_kind)
            .expect_err("override is a query widening, not a stored kind");
        assert_eq!(violation.path, "occurrences[0].kind");

        let bad_range = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/b.dart",
            "symbol_count": 1,
            "declarations": [{
                "symbol": "s1", "kind": "class", "name": "B", "range": [1],
            }],
        });
        let violation = validate_index_export(&bad_range).expect_err("a pair needs two");
        assert_eq!(violation.path, "declarations[0].range");

        let extra = json!({
            "schema_version": SCHEMA_VERSION_INDEX_EXPORT,
            "path": "package:t/b.dart",
            "symbol_count": 0,
            "language": "dart",
        });
        let violation = validate_index_export(&extra).expect_err("undeclared field");
        assert_eq!(violation.path, "language");

        let neither = json!({"schema_version": SCHEMA_VERSION_INDEX_EXPORT});
        assert!(validate_index_export(&neither).is_err());
    }
}
