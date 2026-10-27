//! Hand-derived fixtures for every output face, each citing the writer it
//! mirrors, validated against the published schemas. The compact fixtures
//! are byte-shape transcripts of today's writers; the minimal fixtures
//! exercise the verbosity floor; the negatives pin the red line and
//! the undeclared-field rejection. Real-producer alignment (dartq-ai) lives
//! in `tests/alignment.rs`.

use dartq_schema::{
    Domain, SCHEMA_VERSION_ANALYZE, SCHEMA_VERSION_ASSISTS, SCHEMA_VERSION_CHECK,
    SCHEMA_VERSION_COMPAT, SCHEMA_VERSION_FIX_PATCH, SCHEMA_VERSION_INDEX, SCHEMA_VERSION_QUERY,
    SCHEMA_VERSION_RUN_METRICS, Verbosity, validate, validate_at_tier,
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
    assert_eq!(SCHEMA_VERSION_INDEX, 1);
    assert_eq!(SCHEMA_VERSION_CHECK, 1);
    assert_eq!(SCHEMA_VERSION_ANALYZE, 1);
    assert_eq!(SCHEMA_VERSION_ASSISTS, 1);
}

// ---------------------------------------------------------------------------
// Fix patch document
// ---------------------------------------------------------------------------

/// Transcript of `patch_json` (`crates/dartq-commands/src/fix.rs:761-793`):
/// `file` + `schema_version` (the constant pinned at fix.rs:87) + `edits`
/// with `anchor` objects; every row carries its provenance tag
/// (`edit_floor`, fix.rs:744-751 — `builtin-lint` for the built-in row,
/// fix.rs:91, the plugin's manifest name for the plugin row, fix.rs:777)
/// and its `role` key (the candidate's role, plugin rows `primary`); the
/// second edit shows the empty-context omission at line boundaries
///.
#[test]
fn compact_fix_patch_passes_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [
            {
                "off": 123,
                "len": 8,
                "text": "Bar",
                "src": "builtin-lint",
                "role": "primary",
                "anchor": {
                    "line": 10,
                    "col": 7,
                    "before": "final Fo",
                    "after": "() = ",
                },
            },
            {
                "off": 0,
                "len": 4,
                "text": "new",
                "src": "fixer",
                "role": "primary",
                "anchor": {"line": 1, "col": 1},
            },
        ],
    });
    validate_ok(&doc.to_string(), Domain::FixPatch);
}

/// The minimal tier (crate docs, "Verbosity tiers"): the anchor is the
/// drift-correction extra; the directly-applicable trio is the red line,
/// and the provenance tag and role ride it (`edit_floor` never drops
/// them, fix.rs:744-751 — the writer's own minimal pin, fix.rs:1044-1050).
#[test]
fn minimal_fix_patch_passes_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "off": 123,
            "len": 8,
            "text": "Bar",
            "src": "builtin-lint",
            "role": "suppress",
        }],
    });
    validate_ok(&doc.to_string(), Domain::FixPatch);
}

/// Full-tier extras (`patch_json`, fix.rs:769-772 for the built-in rows,
/// 781-784 for the plugin rows): `rule` names the producing rule,
/// `message` the human row, `priority` the plugin change's ordering
/// (signed — "lower sorts first", fix.rs:139-141); `stale` stays the
/// absent-tolerant patch marking with no writer yet.
#[test]
fn full_tier_fix_extras_pass_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "off": 123,
            "len": 8,
            "text": "Bar",
            "src": "builtin-lint",
            "role": "ignore_line",
            "anchor": {"line": 10, "col": 7, "before": "final Fo"},
            "rule": "unnecessary_new",
            "msg": "Unnecessary 'new' keyword.",
            "priority": 1,
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
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"off": 123, "len": 8}],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].text");
    assert_eq!(violation.found, "missing");
}

/// The provenance tag is required: an edit row without its `source` is
/// never valid (`edit_floor` emits it at every tier, fix.rs:749).
#[test]
fn fix_edit_without_provenance_is_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"off": 123, "len": 8, "text": "Bar"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].src");
    assert_eq!(violation.found, "missing");
}

#[test]
fn fix_anchor_without_line_is_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "off": 123,
            "len": 8,
            "text": "Bar",
            "src": "builtin-lint",
            "anchor": {"col": 7},
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
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{"off": "123", "len": 8, "text": "Bar"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "edits[0].off");
    assert_eq!(violation.expected, "an integer");
}

#[test]
fn fix_missing_file_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::FixPatch);
    assert_eq!(violation.path, "file");
    assert_eq!(violation.found, "missing");
}

// ---------------------------------------------------------------------------
// Query answer
// ---------------------------------------------------------------------------

/// Transcript of `to_json` (`crates/dartq-commands/src/query.rs`): `sym` +
/// `decls` + `refs`, `snake_case` fields. `container` is a total
/// column — null for the top-level row, spelled `-` on the machine face.
/// `qualified` is the optional qualified-name convenience spelling: the
/// writer omits it today because the index row carries only the
/// `isQualified` bit, not the qualifier spelling. Each reference row names
/// its use file. The file columns spell through the one file locator
/// (root-relative inside the analysis root, absolute outside), the envelope
/// carrying the `root` they locate against. `v` is the v1 addition
/// (maintainer ruling: every dartq-native face carries a version stamp);
/// the writer stamps `crate::SCHEMA_VERSION`.
#[test]
fn compact_query_answer_passes_validation() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "root": "/w",
        "sym": "fb",
        "decls": [
            {
                "file": "lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            },
            {
                "file": "lib/a.dart",
                "container": "fb",
                "kind": "method",
                "name": "fb=",
                "line": 8,
                "col": 3,
                "len": 3,
            },
        ],
        "refs": [
            {
                "target": "lib/b.dart",
                "kind": "invoked",
                "file": "lib/c.dart",
                "off": 42,
                "len": 2,
            },
        ],
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// `qualified` has no null spelling — omitted, not null.
#[test]
fn query_qualified_null_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [],
        "refs": [
            {
                "target": "lib/b.dart",
                "kind": "read",
                "qualified": null,
                "off": 7,
                "len": 2,
            },
        ],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "refs[0].qualified");
    assert_eq!(violation.expected, "a string");
}

/// The references row's `qualified` is a compact-tier convenience spelling;
/// the minimal tier omits it (crate docs, query table).
#[test]
fn query_answer_rows_across_tiers_pass_validation() {
    let compact = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [
            {
                "file": "lib/a.dart",
                "container": null,
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            },
        ],
        "refs": [
            {
                "target": "lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "off": 42,
                "len": 2,
            },
        ],
    });
    validate_ok(&compact.to_string(), Domain::Query);

    let minimal = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [
            {
                "file": "lib/a.dart",
                "kind": "class",
                "name": "fb",
                "line": 3,
                "col": 7,
                "len": 2,
            },
        ],
        "refs": [
            {"target": "lib/b.dart", "kind": "invoked", "off": 42, "len": 2},
        ],
    });
    validate_ok(&minimal.to_string(), Domain::Query);
}

/// Session token economics on the query face: symbol table + delta
/// cursor echo, absent-tolerant (crate docs, "Token economics").
#[test]
fn query_answer_with_session_fields_passes_validation() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [],
        "refs": [],
        "symbols": {"s1": "package:foo/a.dart|fb"},
        "since": 1_700_000_000,
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// Red line: a reference row without its offset is never valid.
#[test]
fn query_reference_without_offset_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [],
        "refs": [{"target": "lib/b.dart", "kind": "read"}],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "refs[0].off");
    assert_eq!(violation.found, "missing");
}

/// The reference row's optional `file` (the #361 use-file column, additive
/// on frozen v1): present on rows whose producer knows the use file, absent
/// on rows that do not — both valid, so v1 stays frozen.
#[test]
fn query_reference_use_file_is_optional() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [],
        "refs": [
            {
                "target": "lib/a.dart",
                "kind": "invoked",
                "file": "lib/p.dart",
                "off": 9,
                "len": 4,
            },
            {"target": "lib/a.dart", "kind": "read", "off": 21, "len": 2},
        ],
    });
    validate_ok(&doc.to_string(), Domain::Query);
}

/// Red line: a present `file` is a string — internal ids never cross the
/// boundary.
#[test]
fn query_reference_file_type_error_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [],
        "refs": [
            {"target": "lib/a.dart", "kind": "read", "file": 17, "off": 9, "len": 4},
        ],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "refs[0].file");
    assert_eq!(violation.expected, "a string");
}

/// Enum spellings are pinned to the writer's exhaustive tables
/// (`decl_kind_name`, main.rs:1001-1017).
#[test]
fn query_unknown_declaration_kind_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [{
            "file": "lib/a.dart",
            "container": null,
            "kind": "struct",
            "name": "fb",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
        "refs": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "decls[0].kind");
    assert!(violation.expected.contains("\"extension-type\""));
}

/// Internal ids never cross the boundary.
#[test]
fn query_internal_file_id_is_undeclared() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [{
            "file": "lib/a.dart",
            "container": null,
            "kind": "class",
            "name": "fb",
            "line": 3,
            "col": 7,
            "len": 2,
            "file_id": 17,
        }],
        "refs": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "decls[0].file_id");
    assert_eq!(violation.found, "an undeclared field");
}

#[test]
fn query_container_type_error_is_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [{
            "file": "lib/a.dart",
            "container": 3,
            "kind": "class",
            "name": "fb",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
        "refs": [],
    });
    let violation = violation_of(&doc.to_string(), Domain::Query);
    assert_eq!(violation.path, "decls[0].container");
    assert_eq!(violation.expected, "a string");
}

// ---------------------------------------------------------------------------
// Compatibility report
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
        "root": "/w",
        "file": "lib/a.dart",
        "matrix": [
            {"sdk": "3.13.1", "compatible": true},
            {
                "sdk": "3.5.0",
                "compatible": false,
                "first_failure": {
                    "code": "uri_does_not_exist",
                    "file": "lib/a.dart",
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
        "file": "lib/a.dart",
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
        "file": "lib/a.dart",
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
        "file": "lib/a.dart",
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
        "file": "lib/a.dart",
        "matrix": [{
            "sdk": "3.5.0",
            "compatible": false,
            "first_failure": {
                "code": "uri_does_not_exist",
                "file": "lib/a.dart",
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

/// Transcript of `RunMetrics::json` (`crates/dartq-cli`), which stamps
/// `dartq_commands::SCHEMA_VERSION` — the v1 addition (maintainer
/// ruling: the check/analyze machine faces carry it). The null
/// `max_rss_bytes` spelling is the getrusage-failure shape (`Option<u64>`).
#[test]
fn compact_run_metrics_passes_validation() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
        "total_ms": 46,
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
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
        "total_ms": 46,
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
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
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
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
        "total_ms": 46,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": null,
    });
    let violation = violation_of(&doc.to_string(), Domain::RunMetrics);
    assert_eq!(violation.path, "files");
    assert_eq!(violation.expected, "an integer");
}

/// The phase split (#2628): every phase key is individually required — a
/// document missing any one of them is rejected with the key named.
#[test]
fn run_metrics_missing_phase_key_is_named() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
        "total_ms": 46,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": null,
    });
    for key in ["sdk_ms", "assembly_ms", "lints_ms", "render_ms"] {
        let mut short = doc.clone();
        short.as_object_mut().expect("object").remove(key);
        let violation = violation_of(&short.to_string(), Domain::RunMetrics);
        assert_eq!(violation.path, key, "missing phase key is named");
        assert_eq!(violation.found, "missing");
    }
}

// ---------------------------------------------------------------------------
// Index run summary (`dartq index`)
// ---------------------------------------------------------------------------

/// Transcript of `IndexRun::to_json` (`crates/dartq-commands/src/index.rs`),
/// with the hand-traced fixture totals its own unit test pins (3 member
/// files over 3 libraries, 5 declaration rows, 3 resolved references). The
/// writer stamps the shared `dartq_commands::SCHEMA_VERSION` — equal to
/// `SCHEMA_VERSION_INDEX` at v1; mirroring the domain constant is the
/// recorded writer-side follow-up (schema module docs, "Writer alignment").
#[test]
fn compact_index_run_passes_validation() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_INDEX,
        "files": 3,
        "libraries": 3,
        "symbols": 5,
        "refs": 3,
    });
    validate_ok(&doc.to_string(), Domain::Index);
}

/// The empty-workspace run (the writer's zero totals) and the red line: an
/// undeclared column is rejected, a non-count slot is named.
#[test]
fn index_run_zero_totals_pass_and_extras_fail() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_INDEX,
        "files": 0,
        "libraries": 0,
        "symbols": 0,
        "refs": 0,
    });
    validate_ok(&doc.to_string(), Domain::Index);

    let extra = serde_json::json!({
        "v": SCHEMA_VERSION_INDEX,
        "files": 0,
        "libraries": 0,
        "symbols": 0,
        "refs": 0,
        "unresolved": 7,
    });
    let violation = violation_of(&extra.to_string(), Domain::Index);
    assert_eq!(violation.path, "unresolved");

    let mut wrong = doc;
    wrong["symbols"] = serde_json::json!("5");
    let violation = violation_of(&wrong.to_string(), Domain::Index);
    assert_eq!(violation.path, "symbols");
    assert_eq!(violation.expected, "an integer");
}

// ---------------------------------------------------------------------------
// Check answer (daemon `dartq/check`)
// ---------------------------------------------------------------------------

/// Transcript of `check_row` at its compact tier
/// (`crates/dartq-daemon/src/commands.rs`, the check section) inside the
/// `session_answer` envelope: the flattened short-vocabulary rows
/// (`code`/`sev`/`file`/`line`/`col`/`len`/`msg`, `sev` the lowercase
/// word). `symbols`/`since` ride absent-tolerant.
#[test]
fn compact_check_answer_passes_validation() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_CHECK,
        "diags": [
            {
                "code": "undefined_identifier",
                "sev": "error",
                "file": "/w/lib/a.dart",
                "line": 3,
                "col": 7,
                "len": 2,
                "msg": "Undefined name 'fa'.",
            },
            {
                "code": "dead_null_aware_expression",
                "sev": "warning",
                "file": "/w/lib/b.dart",
                "line": 12,
                "col": 3,
                "len": 5,
                "msg": "The value of the expression is dead.",
            },
        ],
        "since": 42,
    });
    validate_ok(&doc.to_string(), Domain::Check);
}

/// The minimal tier (`check_row`, commands.rs:965-971): the red line is the
/// code plus the file locator; an undeclared column is rejected and the
/// severity spelling is exact.
#[test]
fn minimal_check_rows_pass_and_negatives_are_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_CHECK,
        "diags": [{
            "code": "undefined_identifier",
            "file": "/w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
    });
    validate_ok(&doc.to_string(), Domain::Check);
    assert!(validate_at_tier(&doc.to_string(), Domain::Check, Verbosity::Minimal).is_ok());

    let extra = serde_json::json!({
        "v": SCHEMA_VERSION_CHECK,
        "diags": [{
            "code": "undefined_identifier",
            "file": "/w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
            "correctionMessage": "use fb",
        }],
    });
    let violation = violation_of(&extra.to_string(), Domain::Check);
    assert_eq!(violation.path, "diags[0].correctionMessage");

    let wrong = serde_json::json!({
        "v": SCHEMA_VERSION_CHECK,
        "diags": [{
            "code": "undefined_identifier",
            "sev": "Fatal",
            "file": "/w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
    });
    let violation = violation_of(&wrong.to_string(), Domain::Check);
    assert_eq!(violation.path, "diags[0].sev");
}

// ---------------------------------------------------------------------------
// Analyze answer (daemon `dartq/analyze`)
// ---------------------------------------------------------------------------

/// Transcript of the compact `analyze_answer` document
/// (`crates/dartq-daemon/src/commands.rs`, the analyze section, over the
/// shared `diags_document` builder): the diagnostic flattened onto the
/// short vocabulary — `{code, sev, file, line, col, len, msg}` with
/// `file` carrying the located spelling and `line`/`col` 1-based; the
/// optional `root` key carries the envelope's analysis root (#2657).
#[test]
fn compact_analyze_answer_passes_validation() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_ANALYZE,
        "root": "/w",
        "diags": [{
            "code": "undefined_identifier",
            "sev": "error",
            "file": "lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
            "msg": "Undefined name 'fa'.",
        }],
    });
    validate_ok(&doc.to_string(), Domain::Analyze);
}

/// The minimal tier (`analyze_answer`'s reduced row): `code` + the
/// locator; the severity word spelling is exact and undeclared row fields
/// are named.
#[test]
fn minimal_analyze_entries_pass_and_negatives_are_named() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_ANALYZE,
        "diags": [{
            "code": "undefined_identifier",
            "file": "file:///w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
    });
    validate_ok(&doc.to_string(), Domain::Analyze);
    assert!(validate_at_tier(&doc.to_string(), Domain::Analyze, Verbosity::Minimal).is_ok());

    let wrong = serde_json::json!({
        "v": SCHEMA_VERSION_ANALYZE,
        "diags": [{
            "code": "undefined_identifier",
            "sev": "Fatal",
            "file": "file:///w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
    });
    let violation = violation_of(&wrong.to_string(), Domain::Analyze);
    assert_eq!(violation.path, "diags[0].sev");

    let extra = serde_json::json!({
        "v": SCHEMA_VERSION_ANALYZE,
        "diags": [{
            "code": "undefined_identifier",
            "file": "file:///w/lib/a.dart",
            "line": 3,
            "col": 7,
            "len": 2,
            "tags": [1],
        }],
    });
    let violation = violation_of(&extra.to_string(), Domain::Analyze);
    assert_eq!(violation.path, "diags[0].tags");
}

// ---------------------------------------------------------------------------
// Assists document (`dartq assists` / daemon `dartq/assists`)
// ---------------------------------------------------------------------------

/// Transcript of `assists_json` at its compact tier
/// (`crates/dartq-commands/src/fix.rs:801-850`): plugin rows in priority
/// order, each carrying its provenance (`source`), title (`message`),
/// priority, and anchored edits on the red-line floor; the second edit
/// shows the empty-context omission at line boundaries, and the
/// unserviceable host answers in `errors` (fix.rs:837-848).
#[test]
fn compact_assists_document_passes_validation() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_ASSISTS,
        "assists": [
            {
                "msg": "Wrap in StatefulWidget",
                "src": "assistant",
                "priority": 1,
                "edits": [{
                    "off": 19,
                    "len": 4,
                    "text": "StatefulWidget",
                    "src": "assistant",
                    "anchor": {
                        "line": 2,
                        "col": 16,
                        "before": "class Foo ",
                        "after": "{}",
                    },
                }],
            },
            {
                "msg": "Import 'package:flutter/widgets.dart'",
                "src": "importer",
                "priority": -2,
                "edits": [{
                    "off": 0,
                    "len": 0,
                    "text": "import 'package:flutter/widgets.dart';\n",
                    "src": "importer",
                    "anchor": {"line": 1, "col": 1},
                }],
            },
        ],
        "errors": [{"plugin": "offline", "msg": "manifest not found"}],
    });
    validate_ok(&doc.to_string(), Domain::Assists);
}

/// The minimal tier (`assists_json`, fix.rs:802, 824-828): the row priority
/// and the edit anchors are the trims — the title and the provenance tag
/// ride every tier; the patch-only `rule` column is not an assists field.
#[test]
fn minimal_assists_pass_and_negatives_are_named() {
    let doc = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_ASSISTS,
        "assists": [{
            "msg": "Wrap in StatefulWidget",
            "src": "assistant",
            "edits": [{
                "off": 19,
                "len": 4,
                "text": "StatefulWidget",
                "src": "assistant",
            }],
        }],
    });
    validate_ok(&doc.to_string(), Domain::Assists);
    assert!(validate_at_tier(&doc.to_string(), Domain::Assists, Verbosity::Minimal).is_ok());

    let full = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_ASSISTS,
        "assists": [{
            "msg": "Wrap in StatefulWidget",
            "src": "assistant",
            "priority": 1,
            "edits": [{
                "off": 19,
                "len": 4,
                "text": "StatefulWidget",
                "src": "assistant",
                "anchor": {"line": 2, "col": 16},
            }],
        }],
    });
    let violation = validate_at_tier(&full.to_string(), Domain::Assists, Verbosity::Minimal)
        .expect_err("minimal must not carry the priority");
    assert_eq!(violation.path, "assists[0].priority");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    let patch_row = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_ASSISTS,
        "assists": [{
            "msg": "Wrap in StatefulWidget",
            "src": "assistant",
            "edits": [{
                "off": 19,
                "len": 4,
                "text": "StatefulWidget",
                "src": "assistant",
                "rule": "unnecessary_new",
            }],
        }],
    });
    let violation = violation_of(&patch_row.to_string(), Domain::Assists);
    assert_eq!(violation.path, "assists[0].edits[0].rule");
}

// ---------------------------------------------------------------------------
// Verbosity tiers: the at-tier validator over the public API
// ---------------------------------------------------------------------------

/// Tier exactness on the fix face: the anchor is compact-tier content and
/// the human-name rows (`rule`/`message`/`priority`) full-tier only, so a
/// tier violation names the field and its tier; the provenance tag and the
/// candidate's role ride every tier (`patch_json`, fix.rs:749).
#[test]
fn fix_patch_is_exact_at_its_tier() {
    let compact = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "off": 123,
            "len": 8,
            "text": "Bar",
            "src": "builtin-lint",
            "role": "primary",
            "anchor": {"line": 10, "col": 7},
        }],
    });
    assert!(validate_at_tier(&compact.to_string(), Domain::FixPatch, Verbosity::Compact).is_ok());
    let violation = validate_at_tier(&compact.to_string(), Domain::FixPatch, Verbosity::Minimal)
        .expect_err("minimal must not carry the anchor");
    assert_eq!(violation.path, "edits[0].anchor");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    // The full tier's human-name rows: exact at full, named at compact in
    // turn (`rule`, then `message`, then `priority` — fix.rs:769-784).
    let full = serde_json::json!({
        "file": "lib/foo.dart",
        "v": SCHEMA_VERSION_FIX_PATCH,
        "edits": [{
            "off": 123,
            "len": 8,
            "text": "Bar",
            "src": "fixer",
            "role": "primary",
            "anchor": {"line": 10, "col": 7},
            "rule": "unnecessary_new",
            "msg": "insert const",
            "priority": 1,
        }],
    });
    assert!(validate_at_tier(&full.to_string(), Domain::FixPatch, Verbosity::Full).is_ok());
    let violation = validate_at_tier(&full.to_string(), Domain::FixPatch, Verbosity::Compact)
        .expect_err("compact must not carry the rule");
    assert_eq!(violation.path, "edits[0].rule");
    assert_eq!(violation.expected, "omitted at the compact tier");

    let mut no_rule = full;
    no_rule["edits"][0]
        .as_object_mut()
        .expect("edit row")
        .remove("rule");
    let violation = validate_at_tier(&no_rule.to_string(), Domain::FixPatch, Verbosity::Compact)
        .expect_err("compact must not carry the message");
    assert_eq!(violation.path, "edits[0].msg");
    assert_eq!(violation.expected, "omitted at the compact tier");

    no_rule["edits"][0]
        .as_object_mut()
        .expect("edit row")
        .remove("msg");
    let violation = validate_at_tier(&no_rule.to_string(), Domain::FixPatch, Verbosity::Compact)
        .expect_err("compact must not carry the priority");
    assert_eq!(violation.path, "edits[0].priority");
    assert_eq!(violation.expected, "omitted at the compact tier");
}

/// Tier exactness on the query face: minimal omits `container` and
/// `qualified`; compact/full carry them. The at-tier walk reports the
/// first violation in face order (declarations before references).
#[test]
fn query_answer_is_exact_at_its_tier() {
    let compact = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [{
            "file": "lib/a.dart",
            "container": null,
            "kind": "class",
            "name": "fb",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
        "refs": [
            {
                "target": "lib/b.dart",
                "kind": "invoked",
                "qualified": "fb.fma()",
                "off": 42,
                "len": 2,
            },
        ],
    });
    assert!(validate_at_tier(&compact.to_string(), Domain::Query, Verbosity::Compact).is_ok());
    let violation = validate_at_tier(&compact.to_string(), Domain::Query, Verbosity::Minimal)
        .expect_err("minimal must not carry the convenience spellings");
    assert_eq!(violation.path, "decls[0].container");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    // With the container dropped, the reference row's `qualified` is named
    // on its own.
    let mut minimal = compact;
    minimal["decls"][0]
        .as_object_mut()
        .expect("object")
        .remove("container");
    let violation = validate_at_tier(&minimal.to_string(), Domain::Query, Verbosity::Minimal)
        .expect_err("minimal must not carry the qualified spelling");
    assert_eq!(violation.path, "refs[0].qualified");
    assert_eq!(violation.expected, "omitted at the minimal tier");
}

/// Tier exactness on the compat face: minimal keeps the verdict + axis
/// identity; the evidence columns are compact-tier content.
#[test]
fn compat_report_is_exact_at_its_tier() {
    let compact = serde_json::json!({
        "schema_version": SCHEMA_VERSION_COMPAT,
        "file": "lib/a.dart",
        "matrix": [
            {"sdk": "3.13.1", "compatible": true},
            {
                "sdk": "3.5.0",
                "compatible": false,
                "error": {
                    "kind": "sdk_unavailable",
                    "message": "required SDK file missing: /w/sdks/broken/version",
                },
            },
        ],
    });
    assert!(validate_at_tier(&compact.to_string(), Domain::Compat, Verbosity::Compact).is_ok());
    let violation = validate_at_tier(&compact.to_string(), Domain::Compat, Verbosity::Minimal)
        .expect_err("minimal must not carry the load error");
    assert_eq!(violation.path, "matrix[1].error");
    assert_eq!(violation.expected, "omitted at the minimal tier");
}

/// Tier-invariant face (crate docs, "Verbosity tiers"): the at-tier check
/// is the union check at every tier.
#[test]
fn run_metrics_are_tier_invariant() {
    let doc = serde_json::json!({
        "schema_version": SCHEMA_VERSION_RUN_METRICS,
        "files": 12,
        "loc": 3_456,
        "diagnostics": 2,
        "discovery_ms": 4,
        "sdk_ms": 2,
        "assembly_ms": 1,
        "analyze_ms": 31,
        "lints_ms": 5,
        "render_ms": 1,
        "total_ms": 46,
        "libraries_analyzed": 9,
        "libraries_from_cache": 3,
        "max_rss_bytes": null,
    });
    for tier in [Verbosity::Minimal, Verbosity::Compact, Verbosity::Full] {
        assert!(
            validate_at_tier(&doc.to_string(), Domain::RunMetrics, tier).is_ok(),
            "run metrics must validate at the {tier} tier"
        );
    }
}

/// Tier-invariant face like run metrics: the index summary is four totals —
/// no locator, no prose, nothing a tier trims (crate docs, "Verbosity
/// tiers").
#[test]
fn index_run_is_tier_invariant() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_INDEX,
        "files": 3,
        "libraries": 3,
        "symbols": 5,
        "refs": 3,
    });
    for tier in [Verbosity::Minimal, Verbosity::Compact, Verbosity::Full] {
        assert!(
            validate_at_tier(&doc.to_string(), Domain::Index, tier).is_ok(),
            "index run must validate at the {tier} tier"
        );
    }
}

/// The reserved query kind arrays (crate docs, query table): present, the
/// answer is exact at compact and full — implementations rows carry the
/// declaration-row contract, dependencies rows the explain `DependencyRow`
/// shape — and a minimal answer drops them. Without the arrays the minimal
/// shape is today's writer shape, unchanged.
#[test]
fn query_reserved_kind_arrays_are_exact_at_their_tier() {
    let doc = serde_json::json!({
        "v": SCHEMA_VERSION_QUERY,
        "sym": "fb",
        "decls": [{
            "file": "lib/a.dart",
            "container": null,
            "kind": "class",
            "name": "fb",
            "line": 3,
            "col": 7,
            "len": 2,
        }],
        "refs": [],
        "implementations": [{
            "file": "lib/sub.dart",
            "container": "Sub",
            "kind": "class",
            "name": "Sub",
            "line": 4,
            "col": 7,
            "len": 3,
        }],
        "deps": [{ "path": "lib/app.dart" }],
    });
    assert!(validate_at_tier(&doc.to_string(), Domain::Query, Verbosity::Compact).is_ok());
    assert!(validate_at_tier(&doc.to_string(), Domain::Query, Verbosity::Full).is_ok());
    let violation = validate_at_tier(&doc.to_string(), Domain::Query, Verbosity::Minimal)
        .expect_err("minimal must not carry the reserved arrays");
    assert_eq!(violation.path, "implementations");
    assert_eq!(violation.expected, "omitted at the minimal tier");

    // The arrays' declared-but-unwritten posture: absent from a minimal
    // answer — which also trims the `container` convenience spelling (crate
    // docs, query table) — the payload is today's frozen v1 shape.
    let mut minimal = doc;
    minimal
        .as_object_mut()
        .expect("object")
        .remove("implementations");
    minimal.as_object_mut().expect("object").remove("deps");
    minimal["decls"][0]
        .as_object_mut()
        .expect("object")
        .remove("container");
    assert!(validate_at_tier(&minimal.to_string(), Domain::Query, Verbosity::Minimal).is_ok());
}

fn validate_ok(payload: &str, domain: Domain) {
    if let Err(violation) = validate(payload, domain) {
        panic!("{domain:?} payload rejected: {violation}");
    }
}

fn violation_of(payload: &str, domain: Domain) -> dartq_schema::SchemaViolation {
    validate(payload, domain).expect_err("mutation must violate")
}
