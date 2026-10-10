//! Hand-derived fixtures for every output face, each citing the writer it
//! mirrors, validated against the published schemas. The compact fixtures
//! are byte-shape transcripts of today's writers; the minimal fixtures
//! exercise the §15.8 verbosity floor; the negatives pin the red line and
//! the undeclared-field rejection. Real-producer alignment (dartq-ai) lives
//! in `tests/alignment.rs`.

use dartq_schema::{
    Domain, SCHEMA_VERSION_COMPAT, SCHEMA_VERSION_FIX_PATCH, SCHEMA_VERSION_QUERY,
    SCHEMA_VERSION_RUN_METRICS, validate,
};

/// The v1 freeze: every dartq-native face ships at contract version 1.
/// Bumping any of these is a major version event (crate docs, versioning
/// discipline).
#[test]
fn dartq_native_faces_are_frozen_at_v1() {
    assert_eq!(SCHEMA_VERSION_FIX_PATCH, 1);
    assert_eq!(SCHEMA_VERSION_QUERY, 1);
    assert_eq!(SCHEMA_VERSION_COMPAT, 1);
    assert_eq!(SCHEMA_VERSION_RUN_METRICS, 1);
}

// ---------------------------------------------------------------------------
// Fix patch document (§15.6)
// ---------------------------------------------------------------------------

/// Transcript of `patch_json` (`crates/dartq-cli/src/fix.rs:354-385`):
/// `file` + `schema_version` (the constant pinned at fix.rs:71) + `edits`
/// with `anchor` objects; the second edit shows the empty-context omission
/// at line boundaries (fix.rs:363-371, §15.8.5).
#[test]
fn compact_fix_patch_passes_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [
            {
                "offset": 123,
                "length": 8,
                "replacement": "Bar",
                "anchor": {
                    "line": 10,
                    "column": 7,
                    "context_before": "final Fo",
                    "context_after": "() = ",
                },
            },
            {
                "offset": 0,
                "length": 4,
                "replacement": "new",
                "anchor": {"line": 1, "column": 1},
            },
        ],
    });
    validate_ok(&doc.to_string(), Domain::FixPatch);
}

/// The minimal tier (crate docs, "Verbosity tiers"): the anchor is the
/// drift-correction extra; the directly-applicable trio is the red line.
#[test]
fn minimal_fix_patch_passes_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"offset": 123, "length": 8, "replacement": "Bar"}],
    });
    validate_ok(&doc.to_string(), Domain::FixPatch);
}

/// Full-tier extras declared absent-tolerant (crate docs): `rule` is the
/// producing rule id (carried by the human face's `RuleFix`, fix.rs:121-126,
/// not yet emitted on the machine document), `stale` the §15.6 marking.
#[test]
fn full_tier_fix_extras_pass_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "offset": 123,
            "length": 8,
            "replacement": "Bar",
            "anchor": {"line": 10, "column": 7, "context_before": "final Fo"},
            "rule": "unnecessary_new",
            "stale": false,
        }],
    });
    validate_ok(&doc.to_string(), Domain::FixPatch);
}

/// Red line: an edit without its replacement is never valid.
#[test]
fn fix_edit_without_replacement_is_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"offset": 123, "length": 8}],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].replacement");
    assert_eq!(violation.found, "missing");
}

/// Internal ids never cross the boundary (§15.7): the edit's verified
/// original slice (`Edit.source`, fix.rs:109-111, "deliberately not part of
/// the §15.6 document") is rejected as undeclared.
#[test]
fn fix_internal_source_slice_is_undeclared() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "offset": 123,
            "length": 8,
            "replacement": "Bar",
            "source": "new     ",
        }],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].source");
    assert_eq!(violation.found, "an undeclared field");
}

#[test]
fn fix_anchor_without_line_is_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "offset": 123,
            "length": 8,
            "replacement": "Bar",
            "anchor": {"column": 7},
        }],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].anchor.line");
    assert_eq!(violation.found, "missing");
}

#[test]
fn fix_offset_type_error_is_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"offset": "123", "length": 8, "replacement": "Bar"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].offset");
    assert_eq!(violation.expected, "an integer");
}

#[test]
fn fix_missing_file_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_FIX_PATCH,
        "edits": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "file");
    assert_eq!(violation.found, "missing");
}

// ---------------------------------------------------------------------------
// Query answer (§15.2 `query symbol`)
// ---------------------------------------------------------------------------

/// Transcript of `write_query_json`
/// (`crates/dartq-cli/src/main.rs:954-997`): `name` + `declarations` +
/// `references`, snake_case fields. `container` is a total column — null for
/// the top-level row (main.rs:967), spelled `-` on the machine face
/// (main.rs:931); `qualified` is a plain string on every row (main.rs:982).
/// `schema_version` is the §15.7 v1 addition (maintainer ruling: every
/// dartq-native face carries it); the writer at main.rs:991 does not emit
/// it yet — a tracked CLI-lane gap, not a schema liberty.
#[test]
fn compact_query_answer_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [
            {
                "file": "/w/lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "column": 7,
                "length": 2,
            },
            {
                "file": "/w/lib/a.dart",
                "container": "fb",
                "kind": "method",
                "name": "fb=",
                "line": 8,
                "column": 3,
                "length": 3,
            },
        ],
        "references": [
            {
                "target_file": "/w/lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "offset": 42,
                "length": 2,
            },
        ],
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// `qualified` has no null spelling — §15.8.5 omission, not null.
#[test]
fn query_qualified_null_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [],
        "references": [
            {
                "target_file": "/w/lib/b.dart",
                "kind": "read",
                "qualified": null,
                "offset": 7,
                "length": 2,
            },
        ],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "references[0].qualified");
    assert_eq!(violation.expected, "a string");
}

/// The references row's `qualified` is a compact-tier convenience spelling;
/// the minimal tier omits it (crate docs, query table).
#[test]
fn query_answer_rows_across_tiers_pass_validation() {
    let compact = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [
            {
                "file": "/w/lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "column": 7,
                "length": 2,
            },
        ],
        "references": [
            {
                "target_file": "/w/lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "offset": 42,
                "length": 2,
            },
        ],
    });
    validate_ok(&compact.to_string(), Domain::Query);

    let minimal = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [
            {
                "file": "/w/lib/a.dart",
                "kind": "class",
                "name": "fb",
                "line": 3,
                "column": 7,
                "length": 2,
            },
        ],
        "references": [
            {"target_file": "/w/lib/b.dart", "kind": "invoked", "offset": 42, "length": 2},
        ],
    });
    validate_ok(&minimal.to_string(), Domain::Query);
}

/// §15.8 session token economics on the query face: symbol table + delta
/// cursor echo, absent-tolerant (crate docs, "Token economics").
#[test]
fn query_answer_with_session_fields_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [],
        "references": [],
        "symbols": {"s1": "package:foo/a.dart|fb"},
        "since": 1_700_000_000,
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// Red line: a reference row without its offset is never valid.
#[test]
fn query_reference_without_offset_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [],
        "references": [{"target_file": "/w/lib/b.dart", "kind": "read"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "references[0].offset");
    assert_eq!(violation.found, "missing");
}

/// The reference row's optional `file` (the #361 use-file column, additive
/// on frozen v1): present on rows whose producer knows the use file, absent
/// on rows that do not — both valid, so v1 stays frozen.
#[test]
fn query_reference_use_file_is_optional() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [],
        "references": [
            {
                "target_file": "/w/lib/a.dart",
                "kind": "invoked",
                "file": "/w/lib/p.dart",
                "offset": 9,
                "length": 4,
            },
            {"target_file": "/w/lib/a.dart", "kind": "read", "offset": 21, "length": 2},
        ],
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// Red line: a present `file` is a string — internal ids never cross the
/// boundary (§15.7).
#[test]
fn query_reference_file_type_error_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [],
        "references": [
            {"target_file": "/w/lib/a.dart", "kind": "read", "file": 17, "offset": 9, "length": 4},
        ],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "references[0].file");
    assert_eq!(violation.expected, "a string");
}

/// Enum spellings are pinned to the writer's exhaustive tables
/// (`decl_kind_name`, main.rs:1001-1017).
#[test]
fn query_unknown_declaration_kind_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [{
            "file": "/w/lib/a.dart",
            "container": null,
            "kind": "struct",
            "name": "fb",
            "line": 3,
            "column": 7,
            "length": 2,
        }],
        "references": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "declarations[0].kind");
    assert!(violation.expected.contains("\"extension-type\""));
}

/// Internal ids never cross the boundary (§15.7).
#[test]
fn query_internal_file_id_is_undeclared() {
    let doc = serde_json::json!({
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
            "file_id": 17,
        }],
        "references": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "declarations[0].file_id");
    assert_eq!(violation.found, "an undeclared field");
}

#[test]
fn query_container_type_error_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_QUERY,
        "name": "fb",
        "declarations": [{
            "file": "/w/lib/a.dart",
            "container": 3,
            "kind": "class",
            "name": "fb",
            "line": 3,
            "column": 7,
            "length": 2,
        }],
        "references": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "declarations[0].container");
    assert_eq!(violation.expected, "a string");
}

// ---------------------------------------------------------------------------
// Compatibility report (§15.5)
// ---------------------------------------------------------------------------

/// Transcript of the CLI write (`crates/dartq-cli/src/compat.rs:151-159`,
/// `serde_json::to_string` of `dartq-ai`'s `CompatReport`,
/// compatibility.rs:106-114): one row per axis in CLI order, `first_failure`
/// and `error` omitted when absent (`skip_serializing_if`,
/// compatibility.rs:83-88).
#[test]
fn compact_compat_report_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "/w/lib/a.dart",
        "matrix": [
            {"sdk": "3.13.1", "compatible": true},
            {
                "sdk": "3.5.0",
                "compatible": false,
                "first_failure": {
                    "code": "uri_does_not_exist",
                    "file": "/w/lib/a.dart",
                    "line": 1,
                    "col": 8,
                    "message": "Target of URI doesn't exist: dart:async",
                },
            },
            {
                "sdk": "/w/sdks/broken",
                "compatible": false,
                "error": {
                    "kind": "sdk_unavailable",
                    "message": "required SDK file missing: /w/sdks/broken/version",
                },
            },
        ],
    });
    validate_ok(&doc.to_string(), Domain::Compat);
}

/// The minimal tier keeps the verdict + axis identity; evidence is one
/// `check` away (crate docs, compat table).
#[test]
fn minimal_compat_report_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "/w/lib/a.dart",
        "matrix": [
            {"sdk": "3.13.1", "compatible": true},
            {"sdk": "3.5.0", "compatible": false},
        ],
    });
    validate_ok(&doc.to_string(), Domain::Compat);
}

/// Red line: a verdict row without `compatible` is never valid.
#[test]
fn compat_row_without_verdict_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "/w/lib/a.dart",
        "matrix": [{"sdk": "3.5.0"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::Compat);
    assert_eq!(violation.path, "matrix[0].compatible");
    assert_eq!(violation.found, "missing");
}

#[test]
fn compat_unknown_error_kind_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "/w/lib/a.dart",
        "matrix": [{
            "sdk": "/w/sdks/broken",
            "compatible": false,
            "error": {"kind": "sdk_missing", "message": "no version file"},
        }],
    });
    let violation = violation_of(&doc.to_string(), Domain::Compat);
    assert_eq!(violation.path, "matrix[0].error.kind");
    assert_eq!(violation.expected, "\"sdk_unavailable\"");
}

#[test]
fn compat_failure_without_message_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "/w/lib/a.dart",
        "matrix": [{
            "sdk": "3.5.0",
            "compatible": false,
            "first_failure": {
                "code": "uri_does_not_exist",
                "file": "/w/lib/a.dart",
                "line": 1,
                "col": 8,
            },
        }],
    });
    let violation = violation_of(&doc.to_string(), Domain::Compat);
    assert_eq!(violation.path, "matrix[0].first_failure.message");
    assert_eq!(violation.found, "missing");
}

// ---------------------------------------------------------------------------
// Run metrics (check/analyze `--metrics-json`)
// ---------------------------------------------------------------------------

/// Transcript of `RunMetrics::json`
/// (`crates/dartq-cli/src/main.rs:477-489`). `schema_version` is the §15.7
/// v1 addition (maintainer ruling: the check/analyze machine faces carry
/// it); the writer does not emit it yet — a tracked CLI-lane gap. The null
/// `max_rss_bytes` spelling is the getrusage-failure shape (main.rs:473,
/// `Option<u64>`).
#[test]
fn compact_run_metrics_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "analyze_ms": 31,
        "total_ms": 35,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": 84_233_216,
    });
    validate_ok(&doc.to_string(), Domain::RunMetrics);
}

#[test]
fn run_metrics_with_null_rss_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "analyze_ms": 31,
        "total_ms": 35,
        "libraries_analyzed": 0,
        "libraries_from_cache": 0,
        "max_rss_bytes": null,
    });
    validate_ok(&doc.to_string(), Domain::RunMetrics);
}

#[test]
fn run_metrics_missing_total_ms_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "analyze_ms": 31,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": null,
    });
    let violation = violation_of(&doc.to_string(), Domain::RunMetrics);
    assert_eq!(violation.path, "total_ms");
    assert_eq!(violation.found, "missing");
}

#[test]
fn run_metrics_bad_files_type_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": "12",
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "analyze_ms": 31,
        "total_ms": 35,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": null,
    });
    let violation = violation_of(&doc.to_string(), Domain::RunMetrics);
    assert_eq!(violation.path, "files");
    assert_eq!(violation.expected, "an integer");
}

fn validate_ok(payload: &str, domain: Domain) {
    if let Err(violation) = validate(payload, domain) {
        panic!("{domain:?} payload rejected: {violation}");
    }
}

fn violation_of(payload: &str, domain: Domain) -> dartq_schema::SchemaViolation {
    validate(payload, domain).expect_err("mutation must violate")
}
