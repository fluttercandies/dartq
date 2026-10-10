//! The `--metrics-json` run summary shared by `dartq check` and
//! `dartq analyze` (`crates/dartq-cli/src/main.rs:464-490`): one JSON object
//! at exit, the engine-api-spec §3 benchmark face. Tier-invariant (crate
//! docs, "Verbosity tiers") — a benchmark sidecar has no trimmable prose.

use std::sync::LazyLock;

use serde_json::{Value, json};

use crate::SchemaViolation;
use crate::support::{
    exact_fields, field, non_negative, non_negative_or_null, object, pinned_version,
};

/// The run-metrics envelope's contract version. The CLI writer does not
/// emit the field today (§15.7 gap tracked for the CLI lane); the value is
/// pinned by the fixtures in `tests/output_faces.rs`.
pub const SCHEMA_VERSION_RUN_METRICS: u32 = 1;

/// The published run-metrics schema (JSON Schema draft-07 subset).
#[must_use]
pub fn run_metrics_schema() -> &'static Value {
    &RUN_METRICS_SCHEMA
}

static RUN_METRICS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "dartiq/schema/v1/run-metrics.json",
        "title": "dartiq run metrics (schema_version 1)",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version",
            "files",
            "loc",
            "diagnostics",
            "discovery_ms",
            "analyze_ms",
            "total_ms",
            "libraries_analyzed",
            "libraries_from_cache",
            "max_rss_bytes",
        ],
        "properties": {
            "schema_version": { "const": SCHEMA_VERSION_RUN_METRICS },
            "files": { "type": "integer", "minimum": 0 },
            "loc": { "type": "integer", "minimum": 0 },
            "diagnostics": { "type": "integer", "minimum": 0 },
            "discovery_ms": { "type": "integer", "minimum": 0 },
            "analyze_ms": { "type": "integer", "minimum": 0 },
            "total_ms": { "type": "integer", "minimum": 0 },
            "libraries_analyzed": { "type": "integer", "minimum": 0 },
            "libraries_from_cache": { "type": "integer", "minimum": 0 },
            // getrusage failure spells null (main.rs:473 Option<u64>) — a
            // total column, not an omitted one (§15.8.5 note, crate docs).
            "max_rss_bytes": { "type": ["integer", "null"], "minimum": 0 },
        },
    })
});

const RUN_METRICS_TOP: &[&str] = &[
    "schema_version",
    "files",
    "loc",
    "diagnostics",
    "discovery_ms",
    "analyze_ms",
    "total_ms",
    "libraries_analyzed",
    "libraries_from_cache",
    "max_rss_bytes",
];

pub(crate) fn validate_run_metrics(value: &Value) -> Result<(), SchemaViolation> {
    let root = object(value, "")?;
    exact_fields(root, RUN_METRICS_TOP, &[], "")?;
    pinned_version(root, "schema_version", SCHEMA_VERSION_RUN_METRICS)?;
    for key in [
        "files",
        "loc",
        "diagnostics",
        "discovery_ms",
        "analyze_ms",
        "total_ms",
        "libraries_analyzed",
        "libraries_from_cache",
    ] {
        non_negative(field(root, key, "")?, key)?;
    }
    non_negative_or_null(field(root, "max_rss_bytes", "")?, "max_rss_bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift pin: the published document and the validator must name the
    /// same fields.
    #[test]
    fn document_and_validator_name_the_same_fields() {
        let metrics = run_metrics_schema();
        assert_eq!(
            crate::support::schema_keys(metrics),
            crate::support::const_names(RUN_METRICS_TOP)
        );
    }
}
