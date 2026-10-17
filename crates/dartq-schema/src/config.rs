//! The workspace config — `dartq.yaml`, the all-optional declaration
//! of context labels, the generated-file policy, and the memory/cache
//! defaults. The schema travels with the output faces (the config schema
//! is `dartq-schema`'s, under the same versioning discipline).
//!
//! Unlike the output faces, this module publishes the schema document only
//! — no [`crate::validate`] entry. The document's enforcement point is the
//! reader, `dartq_commands::config`, which must stay *lenient* (unknown
//! keys ignored, forward-compatible config, the pub-tooling convention its
//! docs pin); a second, strict structural checker here would duplicate that
//! logic and contradict the reader's documented posture. This document is
//! the published v1 shape — what a `dartq.yaml` may declare today — for
//! editor tooling and for the drift pins below: a field that moves here
//! first, then in the reader, in the same release.

use std::sync::LazyLock;

use serde_json::{Value, json};

/// The workspace config's contract version. Mirrors the reader contract in
/// `dartq_commands::config` (an absent `version` means 1; any other value
/// is a structured usage error there).
pub const SCHEMA_VERSION_CONFIG: u32 = 1;

/// The published `dartq.yaml` schema (JSON Schema draft-07 subset). No
/// field is required — the all-optional, zero-config default is the
/// contract, so `{}` is a valid v1 config.
#[must_use]
pub fn config_schema() -> &'static Value {
    &CONFIG_SCHEMA
}

static CONFIG_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartq/schema/v1/workspace-config.json",
        "title": "dartq workspace config (dartq.yaml, version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": [],
        "properties": {
            "version": { "const": SCHEMA_VERSION_CONFIG },
            "contexts": {
                // Label → context, file order (the matrix axis order).
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sdk"],
                    "properties": {
                        // `fvm:<version>` | `path:<dir>` | `system`.
                        "sdk": { "type": "string" },
                        "target": { "enum": TARGET_KINDS },
                        "define": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                        },
                    },
                },
            },
            "generated": {
                "type": "object",
                "additionalProperties": false,
                "required": [],
                "properties": {
                    "include": { "type": "boolean" },
                    "editable": { "type": "boolean" },
                    "patterns": { "type": "array", "items": { "type": "string" } },
                },
            },
            "memory": {
                "type": "object",
                "additionalProperties": false,
                "required": ["limit_mib"],
                "properties": {
                    "limit_mib": { "type": "integer", "minimum": 1 },
                },
            },
            "cache": {
                "type": "object",
                "additionalProperties": false,
                "required": ["size_limit_gib"],
                "properties": {
                    "size_limit_gib": { "type": "integer", "minimum": 1 },
                },
            },
        },
    })
});

/// The context target spellings (lowercase, `io` is the reader's
/// default when absent).
pub const TARGET_KINDS: &[&str] = &["io", "web", "wasm"];

// Test-only: the drift pins name the document's fields. The output faces'
// consts are shared with their validators; this input document has none
// (the reader in `dartq-commands` enforces), so the lists live under cfg.
#[cfg(test)]
const CONFIG_TOP: &[&str] = &["version", "contexts", "generated", "memory", "cache"];
#[cfg(test)]
const CONTEXT: &[&str] = &["sdk", "target", "define"];
#[cfg(test)]
const GENERATED: &[&str] = &["include", "editable", "patterns"];
#[cfg(test)]
const MEMORY: &[&str] = &["limit_mib"];
#[cfg(test)]
const CACHE: &[&str] = &["size_limit_gib"];

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift pin: the published document and the field consts name the same
    /// fields at every level, and the top level requires nothing — the
    /// zero-config default is the schema's own contract.
    #[test]
    fn document_declares_the_v1_fields_and_requires_none() {
        let config = config_schema();
        assert!(config["required"].as_array().expect("required").is_empty());
        assert_eq!(
            crate::support::schema_keys(config),
            crate::support::const_names(CONFIG_TOP)
        );
        let context = &config["properties"]["contexts"]["additionalProperties"];
        assert_eq!(
            crate::support::schema_keys(context),
            crate::support::const_names(CONTEXT)
        );
        assert_eq!(context["required"], json!(["sdk"]));
        assert_eq!(
            crate::support::schema_keys(&config["properties"]["generated"]),
            crate::support::const_names(GENERATED)
        );
        assert_eq!(
            crate::support::schema_keys(&config["properties"]["memory"]),
            crate::support::const_names(MEMORY)
        );
        assert_eq!(
            crate::support::schema_keys(&config["properties"]["cache"]),
            crate::support::const_names(CACHE)
        );
    }

    /// The example shape — two contexts, every section — is exactly
    /// what the document describes: the pin that keeps the config draft
    /// and the schema from drifting apart.
    #[test]
    fn the_prd_example_shape_is_the_published_shape() {
        let config = config_schema();
        let context = &config["properties"]["contexts"]["additionalProperties"]["properties"];
        assert_eq!(context["sdk"], json!({ "type": "string" }));
        assert_eq!(context["target"], json!({ "enum": TARGET_KINDS }));
        assert_eq!(
            config["properties"]["version"],
            json!({ "const": SCHEMA_VERSION_CONFIG }),
        );
        assert_eq!(TARGET_KINDS, ["io", "web", "wasm"]);
    }
}
