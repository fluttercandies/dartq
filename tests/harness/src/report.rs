//! Differential report output: JSON (full diff entries, for regression analysis) + Markdown (summary, <= 100 lines).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::compare::{Comparison, Distribution};

pub(crate) struct RunMeta {
    pub(crate) corpus_dir: String,
    pub(crate) oracle_cmd: String,
    pub(crate) dartq_cmd: String,
    /// Full `dart --version` banner line captured once per run (PRD §19.1:
    /// every report records the oracle reference version)
    pub(crate) oracle_version: String,
    pub(crate) threshold: f64,
    pub(crate) severity_filter: String,
    pub(crate) type_filter: String,
    /// Raw path as passed to --codes-file; empty string when not provided
    pub(crate) codes_file: String,
}

/// Gate verdict for one run. `Skipped` = the ERROR tier — the gate metric's tier — is not
/// in scope under the severity filter (pure-report mode): nothing was judged, and every
/// verdict face must say so instead of masquerading as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    Skipped,
}

impl Verdict {
    /// Summary/markdown token; the skipped variant carries its reason inline.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skipped => "SKIPPED (ERROR gate not in scope)",
        }
    }

    /// JSON `passed` face: `null` when the gate was not judged.
    pub(crate) fn passed(self) -> Option<bool> {
        match self {
            Self::Pass => Some(true),
            Self::Fail => Some(false),
            Self::Skipped => None,
        }
    }

    /// Process exit: only a judged FAIL exits 1 (skipped = pure-report mode, exit 0).
    pub(crate) fn exit_success(self) -> bool {
        !matches!(self, Self::Fail)
    }
}

/// Maximum entries shown per side in the Markdown diff table; the rest is counted in a trailing line pointing to the full JSON.
const MD_DIFF_CAP: usize = 20;

#[derive(Serialize)]
struct JsonReport<'a> {
    meta: JsonMeta<'a>,
    /// `null` = the ERROR gate was not in scope (verdict [`Verdict::Skipped`]):
    /// a skipped gate must be distinguishable from a judged pass.
    passed: Option<bool>,
    error_jaccard: f64,
    oracle_total: usize,
    dartq_total: usize,
    filters: JsonFilterAccounting,
    file_count: usize,
    severities: Vec<JsonSeverityStats>,
    oracle_distribution: JsonDistribution,
    oracle_only: Vec<JsonDiffEntry>,
    dartq_only: Vec<JsonDiffEntry>,
}

/// Per-side filter accounting (see `compare::Side::dropped_by_filter`):
/// a filter silently narrowing the gate (e.g. a truncated codes file) shows
/// up here instead of passing unnoticed.
#[derive(Serialize)]
struct JsonFilterAccounting {
    oracle: JsonSideFilterAccounting,
    dartq: JsonSideFilterAccounting,
}

#[derive(Serialize)]
struct JsonSideFilterAccounting {
    /// Diagnostic instances before any filter
    pre_filter_total: usize,
    /// Instances removed by the severity/type/codes filters
    dropped_by_filter: usize,
    /// Every (severity tier, code, file) combination that survived the filters, sorted,
    /// with the tier's own instance count — the machine-side data plane for per-diff
    /// gate attribution, populated even when both diff lists are empty (e.g. a
    /// passing e2e run).
    entries: Vec<JsonSideEntry>,
}

#[derive(Serialize)]
struct JsonSideEntry {
    code: String,
    file: String,
    severity: &'static str,
    count: usize,
}

#[derive(Serialize)]
struct JsonMeta<'a> {
    corpus_dir: &'a str,
    oracle_cmd: &'a str,
    dartq_cmd: &'a str,
    oracle_version: &'a str,
    threshold: f64,
    severity_filter: &'a str,
    type_filter: &'a str,
    codes_file: &'a str,
}

/// Oracle-side distribution (baseline analysis data): all values are diagnostic instance counts; codes use the lowercase entry-key form.
#[derive(Serialize)]
struct JsonDistribution {
    by_type: BTreeMap<String, usize>,
    code_by_severity: BTreeMap<&'static str, BTreeMap<String, usize>>,
    files_by_code: BTreeMap<String, usize>,
    files_by_type: BTreeMap<String, BTreeMap<String, usize>>,
}

impl JsonDistribution {
    fn new(d: &Distribution) -> Self {
        Self {
            by_type: d.by_type.clone(),
            code_by_severity: d
                .code_by_severity
                .iter()
                .map(|(sev, codes)| (sev.as_str(), codes.clone()))
                .collect(),
            files_by_code: d.files_by_code.clone(),
            files_by_type: d.files_by_type.clone(),
        }
    }
}

#[derive(Serialize)]
struct JsonSeverityStats {
    severity: &'static str,
    jaccard: f64,
    intersection: usize,
    union: usize,
    oracle_only: usize,
    dartq_only: usize,
}

#[derive(Serialize)]
struct JsonDiffEntry {
    code: String,
    file: String,
    count: usize,
    dtype: String,
    sample_line: u32,
    sample_col: u32,
    sample_length: u32,
    sample_message: String,
}

/// Shared meta face for both report shapes (standard run + panic attribution).
fn json_meta(meta: &RunMeta) -> JsonMeta<'_> {
    JsonMeta {
        corpus_dir: &meta.corpus_dir,
        oracle_cmd: &meta.oracle_cmd,
        dartq_cmd: &meta.dartq_cmd,
        oracle_version: &meta.oracle_version,
        threshold: meta.threshold,
        severity_filter: &meta.severity_filter,
        type_filter: &meta.type_filter,
        codes_file: &meta.codes_file,
    }
}

/// Attribution report shape (dartq crash tier; see [`crate::bisect`]). The
/// gate never produced a verdict on this path, so there is no comparison
/// payload — only the run meta and the attributed panicking files.
#[derive(Serialize)]
struct PanicReport<'a> {
    meta: JsonMeta<'a>,
    /// Why the gate produced no verdict.
    gate: &'static str,
    /// Attributed panicking files, labeled with the corpus name on
    /// multi-corpus runs (same prefix convention as the merged entry keys).
    dartq_panic_files: Vec<String>,
    /// `true` = the rerun budget ran out (or reruns failed off-signature):
    /// the list may be incomplete.
    attribution_partial: bool,
    /// dartq rerun invocations spent bisecting.
    probe_reruns: usize,
    notes: &'a [String],
}

pub(crate) fn write_reports(
    cmp: &Comparison,
    meta: &RunMeta,
    verdict: Verdict,
    out_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("creating {}: {e}", out_dir.display()))?;

    let json = JsonReport {
        meta: json_meta(meta),
        passed: verdict.passed(),
        error_jaccard: cmp.error_jaccard(),
        oracle_total: cmp.oracle_total,
        dartq_total: cmp.dartq_total,
        filters: JsonFilterAccounting {
            oracle: JsonSideFilterAccounting {
                pre_filter_total: cmp.oracle_pre_filter,
                dropped_by_filter: cmp.oracle_dropped,
                entries: side_entries_json(&cmp.oracle_entries),
            },
            dartq: JsonSideFilterAccounting {
                pre_filter_total: cmp.dartq_pre_filter,
                dropped_by_filter: cmp.dartq_dropped,
                entries: side_entries_json(&cmp.dartq_entries),
            },
        },
        file_count: cmp.file_count,
        oracle_distribution: JsonDistribution::new(&cmp.oracle_distribution),
        severities: cmp
            .severities
            .iter()
            .map(|s| JsonSeverityStats {
                severity: s.severity.as_str(),
                jaccard: s.jaccard,
                intersection: s.intersection,
                union: s.union,
                oracle_only: s.oracle_only,
                dartq_only: s.dartq_only,
            })
            .collect(),
        oracle_only: cmp
            .oracle_only
            .iter()
            .map(|e| JsonDiffEntry {
                code: e.code.clone(),
                file: e.file.clone(),
                count: e.count,
                dtype: e.dtype.clone(),
                sample_line: e.line,
                sample_col: e.col,
                sample_length: e.length,
                sample_message: e.sample_message.clone(),
            })
            .collect(),
        dartq_only: cmp
            .dartq_only
            .iter()
            .map(|e| JsonDiffEntry {
                code: e.code.clone(),
                file: e.file.clone(),
                count: e.count,
                dtype: e.dtype.clone(),
                sample_line: e.line,
                sample_col: e.col,
                sample_length: e.length,
                sample_message: e.sample_message.clone(),
            })
            .collect(),
    };
    let json_text =
        serde_json::to_string_pretty(&json).map_err(|e| format!("serializing report: {e}"))?;

    let json_path = out_dir.join("report.json");
    let md_path = out_dir.join("report.md");
    fs::write(&json_path, json_text)
        .map_err(|e| format!("writing {}: {e}", json_path.display()))?;
    fs::write(&md_path, markdown(cmp, meta, verdict))
        .map_err(|e| format!("writing {}: {e}", md_path.display()))?;
    Ok((json_path, md_path))
}

fn side_entries_json(entries: &[crate::compare::SideEntry]) -> Vec<JsonSideEntry> {
    entries
        .iter()
        .map(|e| JsonSideEntry {
            code: e.code.clone(),
            file: e.file.clone(),
            severity: e.severity.as_str(),
            count: e.count,
        })
        .collect()
}

/// Attribution report for a crashed dartq side: `report.json` carrying the
/// attributed panicking files (no markdown twin — the console error line and
/// the JSON are the two faces this path needs).
pub(crate) fn write_panic_report(
    meta: &RunMeta,
    attribution: &crate::bisect::PanicAttribution,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("creating {}: {e}", out_dir.display()))?;
    let report = PanicReport {
        meta: json_meta(meta),
        gate: "not judged: dartq side crashed (exit 4, empty stdout)",
        dartq_panic_files: attribution
            .entries
            .iter()
            .map(|(label, file)| match label {
                Some(label) => format!("{label}/{file}"),
                None => file.clone(),
            })
            .collect(),
        attribution_partial: attribution.partial,
        probe_reruns: attribution.probes,
        notes: &attribution.notes,
    };
    let json_text =
        serde_json::to_string_pretty(&report).map_err(|e| format!("serializing report: {e}"))?;
    let json_path = out_dir.join("report.json");
    fs::write(&json_path, json_text)
        .map_err(|e| format!("writing {}: {e}", json_path.display()))?;
    Ok(json_path)
}

fn markdown(cmp: &Comparison, meta: &RunMeta, verdict: Verdict) -> String {
    let mut md = String::new();
    let _ = write!(md, "# dartq differential report (M0)\n\n");
    let _ = writeln!(
        md,
        "- corpus: {} (files with diagnostics: {})",
        meta.corpus_dir, cmp.file_count
    );
    let _ = writeln!(md, "- oracle version: {}", meta.oracle_version);
    let _ = writeln!(
        md,
        "- oracle: `{}` -> {} diags",
        meta.oracle_cmd, cmp.oracle_total
    );
    let _ = writeln!(
        md,
        "- dartq: `{}` -> {} diags",
        meta.dartq_cmd, cmp.dartq_total
    );
    let _ = write!(
        md,
        "- severity filter: {}; threshold (ERROR): {}\n\
         - type filter: {}; codes file: {}\n\
         (post-filter: oracle {} diags, dartq {} diags)\n\
         - dropped by filters: oracle {} of {} pre-filter, dartq {} of {} pre-filter\n\n",
        meta.severity_filter,
        meta.threshold,
        meta.type_filter,
        if meta.codes_file.is_empty() {
            "(none)"
        } else {
            &meta.codes_file
        },
        cmp.oracle_total,
        cmp.dartq_total,
        cmp.oracle_dropped,
        cmp.oracle_pre_filter,
        cmp.dartq_dropped,
        cmp.dartq_pre_filter
    );
    md.push_str("## Jaccard per severity\n\n");
    md.push_str("| severity | jaccard | intersect | union | oracle-only | dartq-only |\n");
    md.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for s in &cmp.severities {
        let _ = writeln!(
            md,
            "| {} | {:.3} | {} | {} | {} | {}",
            s.severity.as_str(),
            s.jaccard,
            s.intersection,
            s.union,
            s.oracle_only,
            s.dartq_only
        );
    }
    md.push_str("\n## oracle-only entries\n\n");
    push_diff_table(&mut md, &cmp.oracle_only);
    md.push_str("\n## dartq-only entries\n\n");
    push_diff_table(&mut md, &cmp.dartq_only);
    let _ = write!(
        md,
        "\n## Verdict\n\n- ERROR jaccard = {:.3} vs threshold {} -> {}\n",
        cmp.error_jaccard(),
        meta.threshold,
        verdict.token()
    );
    md
}

fn push_diff_table(md: &mut String, entries: &[crate::compare::DiffEntry]) {
    if entries.is_empty() {
        md.push_str("(none)\n");
        return;
    }
    md.push_str("| code | file | count | message sample |\n| --- | --- | --- | --- |\n");
    for e in entries.iter().take(MD_DIFF_CAP) {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} |",
            e.code,
            e.file,
            e.count,
            escape_md_cell(&e.sample_message)
        );
    }
    let rest = entries.len() - entries.len().min(MD_DIFF_CAP);
    if rest > 0 {
        let _ = writeln!(md, "\n…{rest} more entries, see report.json");
    }
}

fn escape_md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{build_side_labeled, compare};
    use crate::machine::{MachineDiag, Severity};
    use std::path::PathBuf;

    #[test]
    fn writes_json_and_md_within_line_budget() {
        let oracle: Vec<MachineDiag> = (0..30)
            .map(|i| MachineDiag {
                severity: Severity::Error,
                dtype: "SYNTACTIC_ERROR".into(),
                code: format!("E_{i:02}"),
                file: PathBuf::from(format!("/corpus/f{i:02}.dart")),
                line: 1,
                col: 1,
                length: 1,
                message: format!("msg | with pipe {i}\nsecond line"),
            })
            .collect();
        let dartq: Vec<MachineDiag> = oracle[..5].to_vec();
        let cmp = compare(
            &build_side_labeled(&oracle, Path::new("/corpus"), None, |_| true),
            &build_side_labeled(&dartq, Path::new("/corpus"), None, |_| true),
        );
        let meta = RunMeta {
            corpus_dir: "/corpus".into(),
            oracle_cmd: "dart analyze".into(),
            dartq_cmd: "dartq check".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "all".into(),
            type_filter: "syntactic_error".into(),
            codes_file: "tests/corpus/syntax-codes.txt".into(),
        };

        let out = std::env::temp_dir().join(format!("dq_report_{}", std::process::id()));
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Fail, &out).expect("reports");

        let json_text = fs::read_to_string(&json_path).expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json_text).expect("valid json");
        // oracle has 30 keys, dartq reuses the first 5: oracle-only = 25, dartq-only = 0
        assert_eq!(
            parsed["oracle_only"].as_array().unwrap().len(),
            25,
            "JSON keeps ALL diff entries"
        );
        assert_eq!(parsed["dartq_only"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["error_jaccard"], 1.0 / 6.0);
        assert_eq!(parsed["passed"], false);
        assert_eq!(parsed["meta"]["type_filter"], "syntactic_error");
        assert_eq!(
            parsed["meta"]["codes_file"],
            "tests/corpus/syntax-codes.txt"
        );
        assert_eq!(
            parsed["meta"]["oracle_version"], "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"",
            "the oracle reference version rides the report meta"
        );
        let dist = &parsed["oracle_distribution"];
        assert_eq!(
            dist["by_type"]["SYNTACTIC_ERROR"], 30,
            "30 instances, all syntactic"
        );
        assert_eq!(dist["code_by_severity"]["ERROR"]["e_00"], 1);
        assert_eq!(dist["files_by_code"]["e_00"], 1);
        assert_eq!(dist["files_by_type"]["SYNTACTIC_ERROR"]["f00.dart"], 1);

        // Full per-side (code, file) entry lists under filters.*: sorted,
        // severity-tagged, populated even though this run FAILs via the diff lists
        let filters = &parsed["filters"];
        assert_eq!(
            filters["oracle"]["entries"].as_array().unwrap().len(),
            30,
            "every post-filter key listed per side"
        );
        assert_eq!(filters["dartq"]["entries"].as_array().unwrap().len(), 5);
        let first = &filters["oracle"]["entries"][0];
        assert_eq!(first["code"], "e_00", "lexicographic order");
        assert_eq!(first["file"], "f00.dart");
        assert_eq!(first["severity"], "ERROR");
        assert_eq!(first["count"], 1);
        assert_eq!(
            filters["oracle"]["pre_filter_total"], 30,
            "existing accounting fields unchanged"
        );

        let md = fs::read_to_string(&md_path).expect("md");
        assert!(
            md.lines().count() <= 100,
            "markdown must stay within 100 lines, got {}",
            md.lines().count()
        );
        assert!(md.contains("FAIL"));
        assert!(
            md.contains("type filter: syntactic_error; codes file: tests/corpus/syntax-codes.txt"),
            "md records type filter and codes file"
        );
        assert!(
            md.contains("- oracle version: Dart SDK version: 3.9.4"),
            "md header records the oracle reference version"
        );
        // md is capped to the first 20 entries, the rest points to report.json
        assert!(md.contains("5 more entries"), "cap notice present");

        fs::remove_dir_all(&out).ok();
    }

    /// Filter-drop accounting reaches both reports: the truncated-codes-file
    /// failure mode (a filter silently narrowing the gate) leaves a trace in
    /// report.json and the markdown summary, not a silently passing gate.
    #[test]
    fn reports_carry_filter_drop_accounting() {
        let mk = |code: &str| MachineDiag {
            severity: Severity::Error,
            dtype: "COMPILE_TIME_ERROR".into(),
            code: code.into(),
            file: PathBuf::from("/corpus/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        // Simulates a truncated codes file: only expected_token survives
        let oracle = vec![mk("EXPECTED_TOKEN"), mk("UNDEFINED_CLASS")];
        let dartq = vec![mk("expected_token")];
        let in_set = |d: &MachineDiag| d.code.to_lowercase() == "expected_token";
        let cmp = compare(
            &build_side_labeled(&oracle, Path::new("/corpus"), None, in_set),
            &build_side_labeled(&dartq, Path::new("/corpus"), None, in_set),
        );
        let meta = RunMeta {
            corpus_dir: "/corpus".into(),
            oracle_cmd: "dart analyze".into(),
            dartq_cmd: "dartq check".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "all".into(),
            type_filter: "all".into(),
            codes_file: "tests/corpus/syntax-codes.txt".into(),
        };

        let out = std::env::temp_dir().join(format!("dq_report_drop_{}", std::process::id()));
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Pass, &out).expect("reports");

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).expect("json")).expect("valid");
        assert_eq!(parsed["passed"], true);
        assert_eq!(parsed["filters"]["oracle"]["pre_filter_total"], 2);
        assert_eq!(parsed["filters"]["oracle"]["dropped_by_filter"], 1);
        assert_eq!(parsed["filters"]["dartq"]["pre_filter_total"], 1);
        assert_eq!(parsed["filters"]["dartq"]["dropped_by_filter"], 0);
        assert_eq!(parsed["oracle_total"], 1, "post-filter totals unchanged");

        let md = fs::read_to_string(&md_path).expect("md");
        assert!(
            md.contains("dropped by filters: oracle 1 of 2 pre-filter"),
            "md shows the accounting: {md}"
        );

        fs::remove_dir_all(&out).ok();
    }

    /// Skipped gate (pure-report mode: the severity filter drops the ERROR tier):
    /// nothing was judged, so every verdict face must distinguish itself from a
    /// judged pass — JSON `passed` is null, the md verdict reads SKIPPED.
    #[test]
    fn skipped_gate_does_not_masquerade_as_pass() {
        let mk = |code: &str| MachineDiag {
            severity: Severity::Error,
            dtype: "COMPILE_TIME_ERROR".into(),
            code: code.into(),
            file: PathBuf::from("/corpus/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        let cmp = compare(
            &build_side_labeled(&[mk("EXPECTED_TOKEN")], Path::new("/corpus"), None, |_| {
                true
            }),
            &build_side_labeled(&[mk("EXPECTED_TOKEN")], Path::new("/corpus"), None, |_| {
                true
            }),
        );
        let meta = RunMeta {
            corpus_dir: "/corpus".into(),
            oracle_cmd: "dart analyze".into(),
            dartq_cmd: "dartq check".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "WARNING".into(),
            type_filter: "all".into(),
            codes_file: String::new(),
        };

        let out = std::env::temp_dir().join(format!("dq_report_skip_{}", std::process::id()));
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Skipped, &out).expect("reports");

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).expect("json")).expect("valid");
        assert!(
            parsed["passed"].is_null(),
            "a skipped gate must not claim passed: {}",
            parsed["passed"]
        );
        let md = fs::read_to_string(&md_path).expect("md");
        assert!(md.contains("-> SKIPPED (ERROR gate not in scope)"), "{md}");
        assert!(
            !md.contains("PASS"),
            "no judged-pass token may appear: {md}"
        );

        fs::remove_dir_all(&out).ok();
    }

    /// Attribution report (dartq crash tier): `dartq_panic_files` with corpus
    /// labels, the not-judged gate marker, partial flag and probe accounting.
    #[test]
    fn panic_report_carries_attribution_fields() {
        let meta = RunMeta {
            corpus_dir: "/corpus".into(),
            oracle_cmd: "dart analyze machine".into(),
            dartq_cmd: "dartq check --format=machine".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "all".into(),
            type_filter: "all".into(),
            codes_file: String::new(),
        };
        let attribution = crate::bisect::PanicAttribution {
            entries: vec![
                (Some("m0".into()), "lib/poison.dart".into()),
                (None, "lib/other.dart".into()),
            ],
            partial: true,
            probes: 12,
            notes: vec!["rerun budget exhausted".into()],
        };

        let out = std::env::temp_dir().join(format!("dq_report_panic_{}", std::process::id()));
        let json_path =
            write_panic_report(&meta, &attribution, &out).expect("panic report written");

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).expect("json")).expect("valid");
        assert_eq!(
            parsed["dartq_panic_files"],
            serde_json::json!(["m0/lib/poison.dart", "lib/other.dart"]),
            "labels prefix entries on multi-corpus runs"
        );
        assert_eq!(parsed["attribution_partial"], true);
        assert_eq!(parsed["probe_reruns"], 12);
        assert!(
            parsed["gate"]
                .as_str()
                .expect("gate marker")
                .contains("not judged"),
            "the panic path never claims a verdict"
        );
        assert_eq!(parsed["meta"]["dartq_cmd"], "dartq check --format=machine");

        fs::remove_dir_all(&out).ok();
    }
}
