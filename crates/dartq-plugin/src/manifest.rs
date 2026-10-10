//! Plugin manifest (`dartq_plugin.json`) parsing and validation.
//!
//! A manifest is how a plugin directory describes itself to the core before
//! any process starts: identity (`name`, `version`), how to launch it
//! (`entrypoint`), what it can do (`capabilities`), and its resource budget
//! ([`Timeouts`], `memory_budget_mb` — PRD §14.3 fields; the core enforces
//! them at runtime, the protocol only carries them).
//!
//! Unknown top-level fields are ignored, so a newer manifest still parses on
//! an older core. Unknown capability names are ignored likewise.

use std::path::Path;

use serde_json::{Map, Value};

use crate::capability::CapabilitySet;

/// The file name the core looks for inside a plugin directory.
pub const MANIFEST_FILE_NAME: &str = "dartq_plugin.json";

/// Default memory budget in MB (PRD §14.3: 512 MB RSS cap per plugin).
pub const DEFAULT_MEMORY_BUDGET_MB: u32 = 512;

/// Wall-clock and retry budgets of a plugin session (PRD §14.3: 5 s per
/// analyze/fix callback, 10 s per IPC request, 3 retries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Per-callback wall-clock timeout in milliseconds; must be positive —
    /// a zero budget fails at parse (it would insta-timeout every
    /// interaction and kill the child on first use).
    pub callback_ms: u64,
    /// Per-request IPC timeout in milliseconds; must be positive — a zero
    /// budget fails at parse (it would insta-timeout every interaction and
    /// kill the child on first use).
    pub request_ms: u64,
    /// Total process-restart budget over the runtime's whole lifetime, never
    /// reset across generations: every crash/restart consumes one
    /// (`stats.restarts`), and a spent budget turns the next crash terminal.
    pub request_retries: u32,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            callback_ms: 5_000,
            request_ms: 10_000,
            request_retries: 3,
        }
    }
}

/// A parsed plugin manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Unique plugin name (used to namespace its contributions).
    pub name: String,
    /// Plugin version string.
    pub version: String,
    /// Entry point the core launches (a Dart script path, relative to the
    /// plugin directory).
    pub entrypoint: String,
    /// Capabilities the plugin advertises; unknown names were dropped.
    pub capabilities: CapabilitySet,
    /// Resource budgets, defaults from PRD §14.3.
    pub timeouts: Timeouts,
    /// Memory budget in MB, default [`DEFAULT_MEMORY_BUDGET_MB`].
    pub memory_budget_mb: u32,
}

/// Manifest failures. Field-level errors always name the field.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// The manifest file could not be read.
    #[error("cannot read manifest `{path}`: {reason}")]
    Io {
        /// The path attempted.
        path: String,
        /// The underlying I/O error message.
        reason: String,
    },
    /// The manifest text was not valid JSON.
    #[error("manifest is not valid JSON: {0}")]
    Json(String),
    /// The manifest was valid JSON but not an object.
    #[error("manifest must be a JSON object")]
    NotAnObject,
    /// A required field was absent (or JSON null).
    #[error("manifest field `{0}` is required")]
    MissingField(&'static str),
    /// A field was present but malformed.
    #[error("manifest field `{0}`: {1}")]
    InvalidField(&'static str, String),
}

impl Manifest {
    /// Reads and parses the manifest file at `path` (typically
    /// [`MANIFEST_FILE_NAME`] inside a plugin directory).
    ///
    /// # Errors
    ///
    /// [`ManifestError::Io`] when the file cannot be read, plus everything
    /// from [`Manifest::parse`].
    pub fn from_path(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::parse(&text)
    }

    /// Parses manifest text.
    ///
    /// # Errors
    ///
    /// Typed [`ManifestError`] naming the offending field for every schema
    /// violation: required `name`/`version`/`entrypoint` must be non-empty
    /// strings, `capabilities` (optional) must be an array of wire names,
    /// `timeouts` (optional) must hold positive `callback_ms`/`request_ms`
    /// and a non-negative `request_retries`, and `memory_budget_mb`
    /// (optional) must be a positive integer.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| ManifestError::Json(e.to_string()))?;
        let object = value.as_object().ok_or(ManifestError::NotAnObject)?;

        let name = required_string(object, "name")?;
        let version = required_string(object, "version")?;
        let entrypoint = required_string(object, "entrypoint")?;

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert_wire_names(object.get("capabilities"));

        let timeouts = match object.get("timeouts") {
            None | Some(Value::Null) => Timeouts::default(),
            Some(value) => parse_timeouts(value)?,
        };

        let memory_budget_mb = match object.get("memory_budget_mb") {
            None | Some(Value::Null) => DEFAULT_MEMORY_BUDGET_MB,
            Some(value) => {
                let mb = value.as_u64().filter(|mb| *mb > 0).ok_or_else(|| {
                    ManifestError::InvalidField(
                        "memory_budget_mb",
                        "must be a positive integer".to_owned(),
                    )
                })?;
                u32::try_from(mb).map_err(|_| {
                    ManifestError::InvalidField(
                        "memory_budget_mb",
                        "must be a positive integer".to_owned(),
                    )
                })?
            }
        };

        Ok(Self {
            name,
            version,
            entrypoint,
            capabilities,
            timeouts,
            memory_budget_mb,
        })
    }
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, ManifestError> {
    match object.get(field) {
        None | Some(Value::Null) => Err(ManifestError::MissingField(field)),
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(_) => Err(ManifestError::InvalidField(
            field,
            "must be a non-empty string".to_owned(),
        )),
    }
}

fn parse_timeouts(value: &Value) -> Result<Timeouts, ManifestError> {
    let object = value.as_object().ok_or_else(|| {
        ManifestError::InvalidField("timeouts", "must be a JSON object".to_owned())
    })?;
    let mut timeouts = Timeouts::default();
    if let Some(v) = object.get("callback_ms") {
        timeouts.callback_ms = as_positive(v, "timeouts.callback_ms")?;
    }
    if let Some(v) = object.get("request_ms") {
        timeouts.request_ms = as_positive(v, "timeouts.request_ms")?;
    }
    if let Some(v) = object.get("request_retries") {
        timeouts.request_retries = u32::try_from(as_non_negative(v, "timeouts.request_retries")?)
            .map_err(|_| {
            ManifestError::InvalidField(
                "timeouts.request_retries",
                "must fit in 32 bits".to_owned(),
            )
        })?;
    }
    Ok(timeouts)
}

fn as_non_negative(value: &Value, field: &'static str) -> Result<u64, ManifestError> {
    value.as_u64().ok_or_else(|| {
        ManifestError::InvalidField(field, "must be a non-negative integer".to_owned())
    })
}

/// A wall-clock budget: a non-negative integer that must not be zero — every
/// interaction would insta-timeout and kill the child.
fn as_positive(value: &Value, field: &'static str) -> Result<u64, ManifestError> {
    value
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| ManifestError::InvalidField(field, "must be a positive integer".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    fn full_manifest() -> &'static str {
        r#"{
            "name": "my_plugin",
            "version": "0.1.0",
            "entrypoint": "bin/plugin.dart",
            "capabilities": ["lint", "fix", "mystery_future_cap"],
            "timeouts": {"callback_ms": 8000, "request_ms": 20000, "request_retries": 5},
            "memory_budget_mb": 256
        }"#
    }

    #[test]
    fn valid_manifest_parses_with_all_fields() {
        let manifest = Manifest::parse(full_manifest()).expect("parse");
        assert_eq!(manifest.name, "my_plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.entrypoint, "bin/plugin.dart");
        assert!(manifest.capabilities.contains(Capability::Lint));
        assert!(manifest.capabilities.contains(Capability::Fix));
        assert!(!manifest.capabilities.contains(Capability::Assist));
        assert_eq!(manifest.timeouts.callback_ms, 8_000);
        assert_eq!(manifest.timeouts.request_ms, 20_000);
        assert_eq!(manifest.timeouts.request_retries, 5);
        assert_eq!(manifest.memory_budget_mb, 256);
    }

    #[test]
    fn minimal_manifest_falls_back_to_default_budgets() {
        let manifest =
            Manifest::parse(r#"{"name": "n", "version": "1.0.0", "entrypoint": "bin/p.dart"}"#)
                .expect("parse");
        assert_eq!(
            manifest.timeouts,
            Timeouts {
                callback_ms: 5_000,
                request_ms: 10_000,
                request_retries: 3
            }
        );
        assert_eq!(manifest.memory_budget_mb, DEFAULT_MEMORY_BUDGET_MB);
        assert!(manifest.capabilities.is_empty());
    }

    #[test]
    fn missing_required_fields_are_named_in_typed_errors() {
        for (text, field) in [
            (
                r#"{"version": "1.0.0", "entrypoint": "bin/p.dart"}"#,
                "name",
            ),
            (r#"{"name": "n", "entrypoint": "bin/p.dart"}"#, "version"),
            (r#"{"name": "n", "version": "1.0.0"}"#, "entrypoint"),
        ] {
            assert_eq!(
                Manifest::parse(text).unwrap_err(),
                ManifestError::MissingField(field)
            );
        }
    }

    #[test]
    fn malformed_fields_are_typed_errors_with_field_names() {
        assert_eq!(
            Manifest::parse(r#"{"name": "", "version": "1.0.0", "entrypoint": "x"}"#).unwrap_err(),
            ManifestError::InvalidField("name", "must be a non-empty string".to_owned())
        );
        assert!(matches!(
            Manifest::parse(r#"{"name": 42, "version": "1.0.0", "entrypoint": "x"}"#).unwrap_err(),
            ManifestError::InvalidField("name", _)
        ));
        assert!(matches!(
            Manifest::parse(
                r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "memory_budget_mb": 0}"#
            )
            .unwrap_err(),
            ManifestError::InvalidField("memory_budget_mb", _)
        ));
        assert!(matches!(
            Manifest::parse(r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "timeouts": {"callback_ms": -1}}"#)
                .unwrap_err(),
            ManifestError::InvalidField("timeouts.callback_ms", _)
        ));
    }

    #[test]
    fn zero_wall_clock_budgets_are_rejected_zero_retries_is_legal() {
        // A zero callback/request budget insta-times-out every interaction
        // and kills the child on first use — rejected at parse, not at
        // runtime. Zero retries is a legitimate configuration: a runtime
        // that never restarts.
        for (text, field) in [
            (
                r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "timeouts": {"callback_ms": 0}}"#,
                "timeouts.callback_ms",
            ),
            (
                r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "timeouts": {"request_ms": 0}}"#,
                "timeouts.request_ms",
            ),
        ] {
            assert_eq!(
                Manifest::parse(text).unwrap_err(),
                ManifestError::InvalidField(field, "must be a positive integer".to_owned())
            );
        }
        let manifest = Manifest::parse(
            r#"{"name": "n", "version": "1.0.0", "entrypoint": "x", "timeouts": {"request_retries": 0}}"#,
        )
        .expect("parse");
        assert_eq!(manifest.timeouts.request_retries, 0);
    }

    #[test]
    fn broken_documents_are_rejected() {
        assert!(matches!(
            Manifest::parse("{not json").unwrap_err(),
            ManifestError::Json(_)
        ));
        assert_eq!(
            Manifest::parse("[1]").unwrap_err(),
            ManifestError::NotAnObject
        );
    }
}
