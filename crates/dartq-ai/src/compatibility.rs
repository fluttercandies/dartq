//! The `compatibility` query: the same base code analyzed under
//! several SDK contexts, aggregated into a per-axis ✓/✗ matrix.
//!
//! # The query shape
//!
//! Like [`impact`](crate::impact), the query is a pure function over inputs
//! the caller owns — it never sees the engine or the filesystem. One
//! **axis** is one SDK context: the caller runs the base code's analysis
//! under an [`SdkSnapshot`](../dartq_sdk/struct.SdkSnapshot.html)-identified
//! SDK and hands the query that axis's ERROR-severity diagnostics, or the
//! load error when the SDK itself was unusable. Axis identity *is* the
//! snapshot identity: Dart and Flutter SDKs are fixed pairs, never
//! independently combined, so an axis carries one label —
//! not a (dart, flutter) tuple.
//!
//! # The verdict rule
//!
//! An axis is compatible when its analysis produced **zero ERROR-severity
//! diagnostics**; warnings and lints never flip an axis. The `first_failure`
//! row is the lowest-position ERROR — ordered by file path (byte order,
//! the engine's canonical diagnostic order), then (line, column).
//!
//! # SDK load failures
//!
//! An SDK that fails to load never aborts the query (axis failure
//! isolation): the axis lands in the matrix as incompatible with a typed
//! `error` row ([`AxisError::SDK_UNAVAILABLE`]) instead of a fabricated
//! diagnostic — a load failure has no source position, so the
//! `first_failure` shape would be a lie. The remaining axes are unaffected.

use serde::{Deserialize, Serialize};

/// The report contract version: breaking output changes bump the major.
pub const COMPAT_SCHEMA_VERSION: u32 = 1;

/// One ERROR-severity diagnostic of an axis's analysis — both the query's
/// input row and the `first_failure` output row (`code` is the stable
/// diagnostic name, `file`/`line`/`col` the 1-based position).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// The diagnostic's stable code name (e.g. `uri_does_not_exist`).
    pub code: String,
    /// The diagnostic's file path, as resolved by the caller.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
    /// The diagnostic message text.
    pub message: String,
}

/// What one axis's run produced, before aggregation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxisAnalysis {
    /// The engine ran: the axis's ERROR-severity diagnostics (empty ⇒
    /// compatible).
    Analyzed(Vec<Failure>),
    /// The SDK failed to load; the axis records the load error's message
    /// and is incompatible.
    SdkUnavailable(String),
}

/// One matrix axis: its identity label and what its run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisInput {
    /// The axis label — the caller's SDK identity display (the Dart SDK's
    /// version text, or the root path when the SDK failed to load).
    pub sdk: String,
    /// The axis's run outcome.
    pub analysis: AxisAnalysis,
}

/// One row of the matrix: the axis verdict plus the evidence row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisCompat {
    /// The axis label ([`AxisInput::sdk`]).
    pub sdk: String,
    /// Zero ERROR-severity diagnostics in the axis's analysis.
    pub compatible: bool,
    /// The lowest-position ERROR, present exactly when the axis analyzed
    /// with errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_failure: Option<Failure>,
    /// The typed load-failure row, present exactly when the SDK failed to
    /// load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AxisError>,
}

/// The typed error row of an axis whose SDK failed to load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisError {
    /// The stable error kind marker ([`AxisError::SDK_UNAVAILABLE`]).
    pub kind: String,
    /// The load error's message (names the offending SDK file or root).
    pub message: String,
}

impl AxisError {
    /// An SDK root was missing, malformed, or otherwise unusable.
    pub const SDK_UNAVAILABLE: &str = "sdk_unavailable";
}

/// The compatibility report, JSON contract `schema_version: 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatReport {
    /// The output contract version ([`COMPAT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The queried file — the subject of the query, as given by the caller.
    pub file: String,
    /// One row per axis, in the caller's axis order.
    pub matrix: Vec<AxisCompat>,
}

/// Answers the compatibility query over `axes`.
///
/// `file` is the queried path (the report's subject label). The matrix
/// preserves `axes` order — axis order is the caller's contract (the CLI
/// passes its `--sdk-root` order), so the output is deterministic.
#[must_use]
pub fn compatibility(file: &str, axes: &[AxisInput]) -> CompatReport {
    let matrix = axes
        .iter()
        .map(|axis| match &axis.analysis {
            AxisAnalysis::Analyzed(errors) => AxisCompat {
                sdk: axis.sdk.clone(),
                compatible: errors.is_empty(),
                first_failure: lowest_position(errors),
                error: None,
            },
            AxisAnalysis::SdkUnavailable(message) => AxisCompat {
                sdk: axis.sdk.clone(),
                compatible: false,
                first_failure: None,
                error: Some(AxisError {
                    kind: AxisError::SDK_UNAVAILABLE.to_owned(),
                    message: message.clone(),
                }),
            },
        })
        .collect();
    CompatReport {
        schema_version: COMPAT_SCHEMA_VERSION,
        file: file.to_owned(),
        matrix,
    }
}

/// The lowest-position failure: file path byte order first (the engine's
/// canonical diagnostic order), then line, then column.
fn lowest_position(errors: &[Failure]) -> Option<Failure> {
    errors
        .iter()
        .min_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.col.cmp(&b.col))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(file: &str, line: u32, col: u32) -> Failure {
        Failure {
            code: "uri_does_not_exist".to_owned(),
            file: file.to_owned(),
            line,
            col,
            message: format!("Target of URI doesn't exist: {file}:{line}:{col}"),
        }
    }

    /// A zero-error axis is compatible with no evidence rows; matrix rows
    /// preserve the caller's axis order.
    #[test]
    fn clean_axis_is_compatible_without_evidence_rows() {
        let report = compatibility(
            "/w/lib/a.dart",
            &[
                AxisInput {
                    sdk: "3.13.1".to_owned(),
                    analysis: AxisAnalysis::Analyzed(vec![failure("/w/lib/a.dart", 1, 8)]),
                },
                AxisInput {
                    sdk: "3.5.0".to_owned(),
                    analysis: AxisAnalysis::Analyzed(Vec::new()),
                },
            ],
        );
        assert_eq!(report.schema_version, COMPAT_SCHEMA_VERSION);
        assert_eq!(report.file, "/w/lib/a.dart");
        assert_eq!(report.matrix.len(), 2);
        assert_eq!(report.matrix[0].sdk, "3.13.1");
        assert!(!report.matrix[0].compatible);
        assert!(report.matrix[0].error.is_none());
        assert_eq!(report.matrix[1].sdk, "3.5.0");
        assert!(report.matrix[1].compatible);
        assert_eq!(report.matrix[1].first_failure, None);
        assert_eq!(report.matrix[1].error, None);
    }

    /// The first failure is the lowest position: file path first (the
    /// canonical diagnostic order), then line, then column.
    #[test]
    fn error_axis_picks_the_lowest_position_failure() {
        let errors = vec![
            failure("/w/lib/z.dart", 1, 1),
            failure("/w/lib/a.dart", 5, 1),
            failure("/w/lib/a.dart", 3, 9),
        ];
        let report = compatibility(
            "/w/lib/a.dart",
            &[AxisInput {
                sdk: "3.13.1".to_owned(),
                analysis: AxisAnalysis::Analyzed(errors),
            }],
        );
        assert!(!report.matrix[0].compatible);
        assert_eq!(
            report.matrix[0].first_failure,
            Some(failure("/w/lib/a.dart", 3, 9)),
            "same file orders by line; the other file never wins on line alone"
        );
    }

    /// An SDK that failed to load is one incompatible matrix row with the
    /// typed `sdk_unavailable` marker — the other axes are unaffected.
    #[test]
    fn sdk_unavailable_axis_is_isolated_and_incompatible() {
        let report = compatibility(
            "/w/lib/a.dart",
            &[
                AxisInput {
                    sdk: "/sdk/broken".to_owned(),
                    analysis: AxisAnalysis::SdkUnavailable(
                        "required SDK file missing: /sdk/broken/version".to_owned(),
                    ),
                },
                AxisInput {
                    sdk: "3.13.1".to_owned(),
                    analysis: AxisAnalysis::Analyzed(Vec::new()),
                },
            ],
        );
        let broken = &report.matrix[0];
        assert!(!broken.compatible);
        assert_eq!(broken.first_failure, None, "no source position to report");
        let error = broken.error.as_ref().expect("typed error row");
        assert_eq!(error.kind, AxisError::SDK_UNAVAILABLE);
        assert!(error.message.contains("/sdk/broken/version"));
        assert!(
            report.matrix[1].compatible,
            "isolation: the load failure of one axis never touches the others"
        );
    }

    /// The JSON contract: `schema_version: 1`, a compatible row carries no
    /// `first_failure`/`error` keys, a failing row carries `first_failure`
    /// with the five pinned fields, and the report round-trips byte-stable.
    #[test]
    fn json_round_trip_with_schema_version() {
        let report = compatibility(
            "/w/lib/a.dart",
            &[
                AxisInput {
                    sdk: "3.5.0".to_owned(),
                    analysis: AxisAnalysis::Analyzed(Vec::new()),
                },
                AxisInput {
                    sdk: "3.13.1".to_owned(),
                    analysis: AxisAnalysis::Analyzed(vec![failure("/w/lib/a.dart", 1, 8)]),
                },
                AxisInput {
                    sdk: "/sdk/broken".to_owned(),
                    analysis: AxisAnalysis::SdkUnavailable("no version file".to_owned()),
                },
            ],
        );
        let text = serde_json::to_string(&report).expect("serialize");
        let back: CompatReport = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, report, "byte-stable round trip");

        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["file"], "/w/lib/a.dart");
        let clean = &value["matrix"][0];
        assert_eq!(clean["compatible"], true);
        assert!(
            clean.get("first_failure").is_none(),
            "clean row omits the key"
        );
        assert!(clean.get("error").is_none());
        let failing = &value["matrix"][1];
        assert_eq!(failing["compatible"], false);
        assert_eq!(failing["first_failure"]["code"], "uri_does_not_exist");
        assert_eq!(failing["first_failure"]["file"], "/w/lib/a.dart");
        assert_eq!(failing["first_failure"]["line"], 1);
        assert_eq!(failing["first_failure"]["col"], 8);
        assert!(failing.get("error").is_none());
        let broken = &value["matrix"][2];
        assert_eq!(broken["error"]["kind"], "sdk_unavailable");
        assert!(broken.get("first_failure").is_none());
    }
}
