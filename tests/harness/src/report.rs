//! Differential report output: JSON (full diff entries, for regression analysis) + Markdown (summary, <= 100 lines)
//! + raw per-side machine outputs (`raw-outputs.json`, written on every gate run: both sides' complete
//! parsed diagnostics, all severities, unfiltered — the post-hoc attribution data plane).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::compare::{Comparison, Distribution, EntryKey};
use crate::machine::{EntryKeyBuilder, MachineDiag};

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

/// Filename of the raw per-side machine outputs, written next to `report.json` on every gate run.
const RAW_OUTPUTS_FILE: &str = "raw-outputs.json";

/// Maximum paired-instance rows listed in the `position_deltas` sample; the section's
/// counts (matched entries, nonzero entries/instances) always cover every matched entry.
const POSITION_DELTA_SAMPLE_CAP: usize = 50;

/// Raw per-side machine outputs of one gate run: the complete parsed diagnostics of both
/// sides exactly as captured (all severities, all TYPEs, before any severity/type/codes
/// filter), parallel to `corpora`. Persisted on every gate run so post-hoc attribution
/// (e.g. WARNING parity) has full per-instance data even when the gate itself filters
/// to ERROR-only.
pub(crate) struct RawSideOutputs<'a> {
    /// Corpus dirs with their merge labels; `oracle`/`dartq` are parallel arrays
    /// (one diagnostic list per corpus dir, in run order).
    pub(crate) corpora: &'a [(Option<String>, PathBuf)],
    pub(crate) oracle: &'a [Vec<MachineDiag>],
    pub(crate) dartq: &'a [Vec<MachineDiag>],
}

/// Additive `raw_outputs` pointer in `report.json`: where the complete per-side machine
/// outputs live and how many instances each side produced. The pre-existing report faces
/// keep their shape; this field is additive.
#[derive(Serialize)]
struct JsonRawOutputsPointer {
    file: &'static str,
    oracle_instances: usize,
    dartq_instances: usize,
}

#[derive(Serialize)]
struct JsonRawOutputs {
    oracle: JsonRawSide,
    dartq: JsonRawSide,
}

/// One side's raw diagnostics: one record per diagnostic instance, all severities
/// retained (the gate's severity/type/codes filters never apply here).
#[derive(Serialize)]
struct JsonRawSide {
    /// Total instance count across all corpora.
    instances: usize,
    /// Per-corpus sections in run order (parallel to the run's `--corpus-dir` list).
    corpora: Vec<JsonRawCorpus>,
}

#[derive(Serialize)]
struct JsonRawCorpus {
    /// The corpus dir as run (canonical), matching its `meta.corpus_dir` entry.
    dir: String,
    /// Merge label; `null` for a single-corpus run (same convention as the entry keys).
    label: Option<String>,
    diagnostics: Vec<JsonRawDiag>,
}

/// One raw diagnostic instance. `code`/`file` use the entry-key form (lowercase code,
/// corpus-relative label-prefixed file) so records join against `report.json`'s
/// `filters.*.entries` and diff lists; severity/message/location are verbatim.
#[derive(Serialize)]
struct JsonRawDiag {
    code: String,
    file: String,
    severity: &'static str,
    dtype: String,
    line: u32,
    column: u32,
    length: u32,
    message: String,
}

/// Build one side's raw JSON: per-corpus sections with every parsed instance in
/// emission order. Size is linear in the diagnostics actually produced (bounded by
/// corpus size); the per-corpus sections share the run's canonicalize memo via
/// [`EntryKeyBuilder`] per corpus.
fn build_raw_side(
    corpora: &[(Option<String>, PathBuf)],
    per_corpus: &[Vec<MachineDiag>],
) -> JsonRawSide {
    let mut instances = 0;
    let mut sections = Vec::with_capacity(per_corpus.len());
    for ((label, dir), diags) in corpora.iter().zip(per_corpus) {
        let mut keys = EntryKeyBuilder::new(dir);
        let diagnostics = diags
            .iter()
            .map(|d| {
                let (code, file) = keys.key(d);
                JsonRawDiag {
                    file: match label {
                        Some(label) => format!("{label}/{file}"),
                        None => file,
                    },
                    code,
                    severity: d.severity.as_str(),
                    dtype: d.dtype.clone(),
                    line: d.line,
                    column: d.col,
                    length: d.length,
                    message: d.message.clone(),
                }
            })
            .collect();
        instances += diags.len();
        sections.push(JsonRawCorpus {
            dir: dir.display().to_string(),
            label: label.clone(),
            diagnostics,
        });
    }
    JsonRawSide {
        instances,
        corpora: sections,
    }
}

/// One paired diagnostic instance of a matched (code, file) entry whose position
/// differs between the sides. `delta_*` = dartq − oracle; both sides report the
/// same 1-based line/column coordinates, so the deltas are directly comparable.
#[derive(Serialize)]
struct JsonPositionDeltaRow {
    code: String,
    file: String,
    oracle_line: u32,
    oracle_col: u32,
    dartq_line: u32,
    dartq_col: u32,
    delta_line: i64,
    delta_col: i64,
}

/// Position-delta channel over the matched (code, file) entries (PRD §19.1:
/// range/position differences never enter the Jaccard denominator — they are
/// reported here, separately). Purely informational: nothing in this section
/// feeds the gate verdict.
///
/// Provenance: matched entries come from the post-filter comparison; the paired
/// instance positions come from the raw per-side machine outputs (the same plane
/// as `raw-outputs.json`). A key spanning several severity tiers contributes all
/// of its raw instances, including any the gate's filters dropped.
///
/// Pairing rule: within an entry, each side's instances are sorted by (line, col)
/// ascending and paired by index; surplus instances on the longer side stay
/// unpaired. Sorting (not emission order) makes the pairing deterministic with no
/// fuzzy matching.
#[derive(Serialize)]
struct JsonPositionDeltas {
    /// Matched (code, file) entries examined (post-filter intersection)
    matched_entries: usize,
    /// Matched entries with at least one nonzero paired delta
    entries_with_nonzero_delta: usize,
    /// Total paired instances with a nonzero delta across all matched entries
    /// (the sample below is capped)
    nonzero_instances: usize,
    /// Nonzero-delta pairs in key-lexicographic order, then by oracle position,
    /// capped at [`POSITION_DELTA_SAMPLE_CAP`]. Zero-delta pairs stay silent.
    sample: Vec<JsonPositionDeltaRow>,
}

/// Entry keys matched by both sides in the post-filter comparison (the set the
/// Jaccard intersection sees, flattened across severity tiers).
fn matched_entry_keys(cmp: &Comparison) -> BTreeSet<EntryKey> {
    let dartq_keys: BTreeSet<EntryKey> = cmp
        .dartq_entries
        .iter()
        .map(|e| (e.code.clone(), e.file.clone()))
        .collect();
    cmp.oracle_entries
        .iter()
        .map(|e| (e.code.clone(), e.file.clone()))
        .filter(|key| dartq_keys.contains(key))
        .collect()
}

/// Per-key instance positions of one side, from the raw per-corpus machine outputs,
/// keyed in the entry-key form (lowercase code, label-prefixed corpus-relative file)
/// so keys join against the comparison entries.
fn side_positions(
    corpora: &[(Option<String>, PathBuf)],
    per_corpus: &[Vec<MachineDiag>],
) -> FxHashMap<EntryKey, Vec<(u32, u32)>> {
    let mut positions: FxHashMap<_, Vec<_>> = FxHashMap::default();
    for ((label, dir), diags) in corpora.iter().zip(per_corpus) {
        let mut keys = EntryKeyBuilder::new(dir);
        for d in diags {
            let (code, file) = keys.key(d);
            let file = match label {
                Some(label) => format!("{label}/{file}"),
                None => file,
            };
            positions
                .entry((code, file))
                .or_default()
                .push((d.line, d.col));
        }
    }
    positions
}

/// Position-delta channel over the matched entries: see [`JsonPositionDeltas`].
fn position_deltas(cmp: &Comparison, raw: &RawSideOutputs<'_>) -> JsonPositionDeltas {
    let matched = matched_entry_keys(cmp);
    let mut oracle = side_positions(raw.corpora, raw.oracle);
    let mut dartq = side_positions(raw.corpora, raw.dartq);
    for positions in oracle.values_mut() {
        positions.sort_unstable();
    }
    for positions in dartq.values_mut() {
        positions.sort_unstable();
    }
    let mut sample = Vec::new();
    let mut nonzero_entries = 0;
    let mut nonzero_instances = 0;
    for key in &matched {
        let (Some(o_pos), Some(d_pos)) = (oracle.get(key), dartq.get(key)) else {
            // Unreachable by construction (a matched key survived the filters on
            // both sides, so both raw sides hold its instances); skipping keeps
            // the channel informational either way.
            continue;
        };
        let mut entry_nonzero = 0;
        for (&(o_line, o_col), &(d_line, d_col)) in o_pos.iter().zip(d_pos.iter()) {
            let delta_line = i64::from(d_line) - i64::from(o_line);
            let delta_col = i64::from(d_col) - i64::from(o_col);
            if delta_line == 0 && delta_col == 0 {
                continue;
            }
            entry_nonzero += 1;
            if sample.len() < POSITION_DELTA_SAMPLE_CAP {
                sample.push(JsonPositionDeltaRow {
                    code: key.0.clone(),
                    file: key.1.clone(),
                    oracle_line: o_line,
                    oracle_col: o_col,
                    dartq_line: d_line,
                    dartq_col: d_col,
                    delta_line,
                    delta_col,
                });
            }
        }
        if entry_nonzero > 0 {
            nonzero_entries += 1;
            nonzero_instances += entry_nonzero;
        }
    }
    JsonPositionDeltas {
        matched_entries: matched.len(),
        entries_with_nonzero_delta: nonzero_entries,
        nonzero_instances,
        sample,
    }
}

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
    /// Additive pointer to the raw per-side machine outputs (all severities,
    /// unfiltered); the faces above keep their shape.
    raw_outputs: JsonRawOutputsPointer,
    /// Additive position-delta channel over matched entries (PRD §19.1:
    /// reported separately, never part of the Jaccard verdict).
    position_deltas: JsonPositionDeltas,
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
    raw: &RawSideOutputs<'_>,
    out_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("creating {}: {e}", out_dir.display()))?;

    // Raw per-side machine outputs first: the report.json pointer references this
    // file, so a failed raw write aborts the whole report (never a dangling pointer).
    // Serialized straight into a buffered file writer — the full JSON text is never
    // buffered in memory, and the explicit flush surfaces write errors while the
    // error path can still name the file.
    let raw_json = JsonRawOutputs {
        oracle: build_raw_side(raw.corpora, raw.oracle),
        dartq: build_raw_side(raw.corpora, raw.dartq),
    };
    let raw_path = out_dir.join(RAW_OUTPUTS_FILE);
    let mut raw_file = std::io::BufWriter::new(
        fs::File::create(&raw_path).map_err(|e| format!("creating {}: {e}", raw_path.display()))?,
    );
    serde_json::to_writer_pretty(&mut raw_file, &raw_json)
        .map_err(|e| format!("writing {}: {e}", raw_path.display()))?;
    raw_file
        .flush()
        .map_err(|e| format!("writing {}: {e}", raw_path.display()))?;

    let deltas = position_deltas(cmp, raw);
    let md_text = markdown(cmp, meta, verdict, &deltas);
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
        raw_outputs: JsonRawOutputsPointer {
            file: RAW_OUTPUTS_FILE,
            oracle_instances: raw_json.oracle.instances,
            dartq_instances: raw_json.dartq.instances,
        },
        position_deltas: deltas,
    };
    let json_text =
        serde_json::to_string_pretty(&json).map_err(|e| format!("serializing report: {e}"))?;

    let json_path = out_dir.join("report.json");
    let md_path = out_dir.join("report.md");
    fs::write(&json_path, json_text)
        .map_err(|e| format!("writing {}: {e}", json_path.display()))?;
    fs::write(&md_path, md_text).map_err(|e| format!("writing {}: {e}", md_path.display()))?;
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

fn markdown(
    cmp: &Comparison,
    meta: &RunMeta,
    verdict: Verdict,
    deltas: &JsonPositionDeltas,
) -> String {
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
    let _ = writeln!(
        md,
        "- position deltas (informational, not gated): {} matched entries, {} \
         with nonzero line/col ({} instances; sample in report.json)",
        deltas.matched_entries, deltas.entries_with_nonzero_delta, deltas.nonzero_instances
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
        let oracle_lists = [oracle];
        let dartq_lists = [dartq];
        let corpora = [(None, PathBuf::from("/corpus"))];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Fail, &raw, &out).expect("reports");

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
        // Additive raw_outputs pointer: file name + per-side instance counts
        assert_eq!(parsed["raw_outputs"]["file"], "raw-outputs.json");
        assert_eq!(parsed["raw_outputs"]["oracle_instances"], 30);
        assert_eq!(parsed["raw_outputs"]["dartq_instances"], 5);
        // The 5 matched entries share identical positions: the delta channel
        // counts them but stays silent
        assert_eq!(parsed["position_deltas"]["matched_entries"], 5);
        assert_eq!(parsed["position_deltas"]["entries_with_nonzero_delta"], 0);
        assert!(
            parsed["position_deltas"]["sample"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let raw_parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(out.join("raw-outputs.json")).expect("raw"))
                .expect("valid raw json");
        assert_eq!(raw_parsed["oracle"]["instances"], 30);
        assert_eq!(raw_parsed["dartq"]["instances"], 5);

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
        let oracle_lists = [oracle];
        let dartq_lists = [dartq];
        let corpora = [(None, PathBuf::from("/corpus"))];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Pass, &raw, &out).expect("reports");

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

    /// Raw per-side outputs, the core contract: every gate run persists both
    /// sides' complete parsed diagnostics (all severities, unfiltered) to
    /// `raw-outputs.json` — even when the gate itself filters to ERROR-only —
    /// and report.json carries the additive `raw_outputs` pointer. Records use
    /// the entry-key code/file form (joinable against the report faces) with
    /// verbatim severity, message and location.
    #[test]
    fn raw_outputs_keep_every_severity_unfiltered_on_error_only_gate() {
        let root = std::env::temp_dir().join(format!("dq_raw_{}", std::process::id()));
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).expect("mkdir");
        std::fs::write(lib.join("a.dart"), "var x = 1;").expect("a.dart");
        std::fs::write(lib.join("b.dart"), "var y = 2;").expect("b.dart");
        let root_canon = root.canonicalize().expect("canonicalize root");

        let diag = |severity: Severity, code: &str, file: &str, message: &str| MachineDiag {
            severity,
            dtype: match severity {
                Severity::Error => "SYNTACTIC_ERROR".into(),
                Severity::Warning => "STATIC_WARNING".into(),
                Severity::Info => "LINT".into(),
            },
            code: code.into(),
            file: lib.join(file),
            line: 1,
            col: 1,
            length: 1,
            message: message.into(),
        };
        // Oracle: one diagnostic per severity tier; dartq: two warnings, one
        // sharing the oracle's (code, file) key, one dartq-only.
        let oracle = vec![
            diag(Severity::Error, "EXPECTED_TOKEN", "a.dart", "Expected to find ';'."),
            diag(Severity::Warning, "UNUSED_LOCAL_VARIABLE", "a.dart", "The value of the local variable 'x' isn't used."),
            diag(Severity::Info, "PREFER_SINGLE_QUOTES", "b.dart", "Unnecessary use of double quotes."),
        ];
        let dartq = vec![
            diag(Severity::Warning, "unused_local_variable", "a.dart", "The value of the local variable 'x' isn't used."),
            diag(Severity::Warning, "DEAD_CODE", "b.dart", "Dead code."),
        ];
        // ERROR-only gate: the filters drop the WARNING/INFO tiers from the
        // comparison — the raw outputs must keep them anyway.
        let keep = |d: &MachineDiag| d.severity == Severity::Error;
        let cmp = compare(
            &build_side_labeled(&oracle, &root_canon, None, keep),
            &build_side_labeled(&dartq, &root_canon, None, keep),
        );
        let meta = RunMeta {
            corpus_dir: root_canon.display().to_string(),
            oracle_cmd: "dart analyze".into(),
            dartq_cmd: "dartq check".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "ERROR".into(),
            type_filter: "all".into(),
            codes_file: String::new(),
        };

        let out = std::env::temp_dir().join(format!("dq_raw_out_{}", std::process::id()));
        let oracle_lists = [oracle];
        let dartq_lists = [dartq];
        let corpora = [(None, root_canon.clone())];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        let (json_path, _) = write_reports(&cmp, &meta, Verdict::Pass, &raw, &out).expect("reports");

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).expect("json")).expect("valid");
        // Gate face: ERROR-only. Oracle keeps its one ERROR key; the dartq
        // side has no ERROR tier (both its entries are WARNING) -> 0.
        assert_eq!(parsed["oracle_total"], 1);
        assert_eq!(parsed["dartq_total"], 0);
        assert_eq!(parsed["passed"], true);

        // Pointer matches the persisted raw file
        assert_eq!(parsed["raw_outputs"]["file"], "raw-outputs.json");
        assert_eq!(parsed["raw_outputs"]["oracle_instances"], 3);
        assert_eq!(parsed["raw_outputs"]["dartq_instances"], 2);

        let raw: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(out.join("raw-outputs.json")).expect("raw file"),
        )
        .expect("valid raw json");

        // All severities retained on the oracle side despite the ERROR-only gate
        let oracle_diags = raw["oracle"]["corpora"][0]["diagnostics"]
            .as_array()
            .expect("oracle diagnostics");
        assert_eq!(raw["oracle"]["instances"], 3);
        assert_eq!(oracle_diags.len(), 3);
        let severities: Vec<&str> = oracle_diags
            .iter()
            .map(|d| d["severity"].as_str().expect("severity"))
            .collect();
        assert_eq!(severities, ["ERROR", "WARNING", "INFO"]);

        // Record fields: entry-key code/file, verbatim message and location
        let expected_token = &oracle_diags[0];
        assert_eq!(expected_token["code"], "expected_token");
        assert_eq!(
            expected_token["file"], "lib/a.dart",
            "corpus-relative file key"
        );
        assert_eq!(expected_token["dtype"], "SYNTACTIC_ERROR");
        assert_eq!(expected_token["message"], "Expected to find ';'.");
        assert_eq!(expected_token["line"], 1);
        assert_eq!(expected_token["column"], 1);
        assert_eq!(expected_token["length"], 1);
        assert_eq!(raw["dartq"]["instances"], 2);
        let dartq_codes: Vec<&str> = raw["dartq"]["corpora"][0]["diagnostics"]
            .as_array()
            .expect("dartq diagnostics")
            .iter()
            .map(|d| d["code"].as_str().expect("code"))
            .collect();
        assert_eq!(
            dartq_codes,
            ["unused_local_variable", "dead_code"],
            "dartq-only raw instances retained verbatim"
        );

        // Corpus section header: canonical dir, single-corpus label = null
        assert_eq!(raw["oracle"]["corpora"][0]["dir"], root_canon.display().to_string());
        assert!(raw["oracle"]["corpora"][0]["label"].is_null());

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&out).ok();
    }

    /// Multi-corpus raw outputs: per-corpus sections in run order, file keys
    /// label-prefixed exactly like the merged entry keys.
    #[test]
    fn raw_outputs_prefix_files_with_corpus_labels() {
        let mk = |code: &str| MachineDiag {
            severity: Severity::Error,
            dtype: "SYNTACTIC_ERROR".into(),
            code: code.into(),
            file: PathBuf::from("/c1/lib/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        let corpora = vec![
            (Some("m0".to_string()), PathBuf::from("/c1")),
            (Some("m1".to_string()), PathBuf::from("/c2")),
        ];
        let per_corpus = vec![vec![mk("EXPECTED_TOKEN")], vec![]];
        let side = build_raw_side(&corpora, &per_corpus);
        assert_eq!(side.instances, 1);
        assert_eq!(side.corpora.len(), 2);
        assert_eq!(side.corpora[0].dir, "/c1");
        assert_eq!(side.corpora[0].label.as_deref(), Some("m0"));
        assert_eq!(
            side.corpora[0].diagnostics[0].file, "m0/lib/a.dart",
            "label-prefixed file key, same convention as the merged entry keys"
        );
        assert_eq!(side.corpora[1].dir, "/c2");
        assert!(side.corpora[1].diagnostics.is_empty());
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
        let oracle_lists = [vec![mk("EXPECTED_TOKEN")]];
        let dartq_lists = [vec![mk("EXPECTED_TOKEN")]];
        let corpora = [(None, PathBuf::from("/corpus"))];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        let (json_path, md_path) =
            write_reports(&cmp, &meta, Verdict::Skipped, &raw, &out).expect("reports");

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

    fn pos_diag(code: &str, file: &str, line: u32, col: u32) -> MachineDiag {
        MachineDiag {
            severity: Severity::Error,
            dtype: "SYNTACTIC_ERROR".into(),
            code: code.into(),
            file: PathBuf::from(file),
            line,
            col,
            length: 1,
            message: "m".into(),
        }
    }

    /// Position-delta channel over a single-corpus fixture, exercised the same
    /// way write_reports drives it: compare the post-filter sides, feed the raw
    /// per-corpus lists as the position data plane.
    fn deltas_for(oracle: Vec<MachineDiag>, dartq: Vec<MachineDiag>) -> JsonPositionDeltas {
        let cmp = compare(
            &build_side_labeled(&oracle, Path::new("/corpus"), None, |_| true),
            &build_side_labeled(&dartq, Path::new("/corpus"), None, |_| true),
        );
        let oracle_lists = [oracle];
        let dartq_lists = [dartq];
        let corpora = [(None, PathBuf::from("/corpus"))];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        position_deltas(&cmp, &raw)
    }

    /// Multi-instance pairing: within a matched entry, both sides' instances are
    /// sorted by (line, col) and paired by index — deterministic regardless of
    /// emission order, no fuzzy matching. Deltas are dartq − oracle; surplus
    /// instances on the longer side stay unpaired.
    #[test]
    fn position_deltas_pair_multi_instances_by_sorted_index() {
        // Same key, three instances each, emitted out of order on both sides;
        // dartq is shifted one line down per instance.
        let oracle = vec![
            pos_diag("EXPECTED_TOKEN", "/corpus/a.dart", 10, 1),
            pos_diag("EXPECTED_TOKEN", "/corpus/a.dart", 5, 2),
            pos_diag("EXPECTED_TOKEN", "/corpus/a.dart", 7, 1),
        ];
        let dartq = vec![
            pos_diag("expected_token", "/corpus/a.dart", 6, 2),
            pos_diag("expected_token", "/corpus/a.dart", 8, 1),
            pos_diag("expected_token", "/corpus/a.dart", 11, 1),
        ];
        let deltas = deltas_for(oracle, dartq);
        assert_eq!(deltas.matched_entries, 1);
        assert_eq!(deltas.entries_with_nonzero_delta, 1);
        assert_eq!(deltas.nonzero_instances, 3);
        let rows: Vec<(u32, u32, u32, u32, i64, i64)> = deltas
            .sample
            .iter()
            .map(|r| {
                (
                    r.oracle_line,
                    r.oracle_col,
                    r.dartq_line,
                    r.dartq_col,
                    r.delta_line,
                    r.delta_col,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [(5, 2, 6, 2, 1, 0), (7, 1, 8, 1, 1, 0), (10, 1, 11, 1, 1, 0)],
            "sorted-index pairing: (5,2)->(6,2), (7,1)->(8,1), (10,1)->(11,1)"
        );

        // Count asymmetry: 2 oracle instances vs 1 dartq instance pairs one row,
        // the surplus oracle instance stays unpaired
        let deltas = deltas_for(
            vec![
                pos_diag("A", "/corpus/c.dart", 1, 1),
                pos_diag("A", "/corpus/c.dart", 4, 1),
            ],
            vec![pos_diag("a", "/corpus/c.dart", 2, 1)],
        );
        assert_eq!(deltas.matched_entries, 1);
        assert_eq!(deltas.nonzero_instances, 1);
        assert_eq!(deltas.sample.len(), 1);
        assert_eq!(
            (deltas.sample[0].oracle_line, deltas.sample[0].dartq_line),
            (1, 2),
            "pairs the first sorted index, surplus (4,1) unpaired"
        );
    }

    /// Zero-delta silence: a matched entry with identical positions counts toward
    /// `matched_entries` but never produces sample rows or nonzero counts; only
    /// the shifted sibling entry shows up.
    #[test]
    fn position_deltas_stay_silent_on_zero_delta_entries() {
        let oracle = vec![
            pos_diag("EXPECTED_TOKEN", "/corpus/same.dart", 3, 7),
            pos_diag("UNTERMINATED_STRING_LITERAL", "/corpus/b.dart", 1, 15),
        ];
        let dartq = vec![
            pos_diag("expected_token", "/corpus/same.dart", 3, 7),
            pos_diag("unterminated_string_literal", "/corpus/b.dart", 2, 15),
        ];
        let deltas = deltas_for(oracle, dartq);
        assert_eq!(deltas.matched_entries, 2);
        assert_eq!(deltas.entries_with_nonzero_delta, 1);
        assert_eq!(deltas.nonzero_instances, 1);
        assert_eq!(deltas.sample.len(), 1);
        assert_eq!(deltas.sample[0].code, "unterminated_string_literal");
        assert_eq!(deltas.sample[0].delta_line, 1);
        assert_eq!(deltas.sample[0].delta_col, 0);
    }

    /// Bounded sample: nonzero rows are counted in full but the listed sample is
    /// capped at [`POSITION_DELTA_SAMPLE_CAP`], truncating in key-lexicographic
    /// order.
    #[test]
    fn position_delta_sample_is_capped() {
        let oracle: Vec<MachineDiag> = (0..60)
            .map(|i| pos_diag(&format!("C_{i:02}"), "/corpus/f.dart", 1, 1))
            .collect();
        let dartq: Vec<MachineDiag> = (0..60)
            .map(|i| pos_diag(&format!("c_{i:02}"), "/corpus/f.dart", 2, 1))
            .collect();
        let deltas = deltas_for(oracle, dartq);
        assert_eq!(deltas.matched_entries, 60);
        assert_eq!(deltas.entries_with_nonzero_delta, 60);
        assert_eq!(deltas.nonzero_instances, 60);
        assert_eq!(deltas.sample.len(), POSITION_DELTA_SAMPLE_CAP);
        assert_eq!(deltas.sample[0].code, "c_00");
        assert_eq!(deltas.sample[49].code, "c_49", "truncation keeps the head");
    }

    /// Verdict neutrality: wildly different positions on identical entry sets
    /// leave the Jaccard untouched (bit-identical to the same fixture with
    /// identical positions), and the additive report.json section plus the md
    /// summary line carry the delta accounting.
    #[test]
    #[allow(clippy::float_cmp)] // both fixtures share entry sets; jaccard 1.0 is exact
    fn position_deltas_never_enter_the_verdict() {
        let mk_oracle = || {
            vec![
                pos_diag("EXPECTED_TOKEN", "/corpus/a.dart", 2, 11),
                pos_diag("UNDEFINED_CLASS", "/corpus/b.dart", 4, 3),
            ]
        };
        let shifted = vec![
            pos_diag("expected_token", "/corpus/a.dart", 900, 40),
            pos_diag("undefined_class", "/corpus/b.dart", 1000, 1),
        ];
        let identical = vec![
            pos_diag("expected_token", "/corpus/a.dart", 2, 11),
            pos_diag("undefined_class", "/corpus/b.dart", 4, 3),
        ];
        let cmp_shifted = compare(
            &build_side_labeled(&mk_oracle(), Path::new("/corpus"), None, |_| true),
            &build_side_labeled(&shifted, Path::new("/corpus"), None, |_| true),
        );
        let cmp_identical = compare(
            &build_side_labeled(&mk_oracle(), Path::new("/corpus"), None, |_| true),
            &build_side_labeled(&identical, Path::new("/corpus"), None, |_| true),
        );
        assert_eq!(
            cmp_shifted.error_jaccard(),
            cmp_identical.error_jaccard(),
            "position drift alone cannot move the gate metric"
        );
        assert_eq!(cmp_shifted.error_jaccard(), 1.0);

        let meta = RunMeta {
            corpus_dir: "/corpus".into(),
            oracle_cmd: "dart analyze".into(),
            dartq_cmd: "dartq check".into(),
            oracle_version: "Dart SDK version: 3.9.4 (stable) on \"macos_arm64\"".into(),
            threshold: 0.99,
            severity_filter: "all".into(),
            type_filter: "all".into(),
            codes_file: String::new(),
        };
        let out = std::env::temp_dir().join(format!("dq_report_pos_{}", std::process::id()));
        let oracle_lists = [mk_oracle()];
        let dartq_lists = [shifted];
        let corpora = [(None, PathBuf::from("/corpus"))];
        let raw = RawSideOutputs {
            corpora: &corpora,
            oracle: &oracle_lists,
            dartq: &dartq_lists,
        };
        let (json_path, md_path) =
            write_reports(&cmp_shifted, &meta, Verdict::Pass, &raw, &out).expect("reports");

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&json_path).expect("json")).expect("valid");
        assert_eq!(parsed["passed"], true, "nonzero deltas never fail the gate");
        assert_eq!(parsed["position_deltas"]["matched_entries"], 2);
        assert_eq!(parsed["position_deltas"]["entries_with_nonzero_delta"], 2);
        assert_eq!(parsed["position_deltas"]["nonzero_instances"], 2);
        assert_eq!(
            parsed["position_deltas"]["sample"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let row = &parsed["position_deltas"]["sample"][0];
        assert_eq!(row["code"], "expected_token");
        assert_eq!(row["oracle_line"], 2);
        assert_eq!(row["dartq_line"], 900);
        assert_eq!(row["delta_line"], 898);

        let md = fs::read_to_string(&md_path).expect("md");
        assert!(
            md.contains(
                "position deltas (informational, not gated): 2 matched entries, \
                 2 with nonzero line/col (2 instances"
            ),
            "md summary line carries the accounting: {md}"
        );

        fs::remove_dir_all(&out).ok();
    }
}
