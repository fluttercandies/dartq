//! §19.3 benchmark metric substrate (M3 gate arbiter tooling): three
//! process-level collectors as the `harness bench` subcommand — no criterion,
//! the metrics are end-to-end process measurements, not micro-benchmarks.
//!
//! Metrics (PRD §1.5):
//! - `rss-100k`: peak tree-RSS (KiB) of one full dartq analysis over the
//!   corpus, divided by `LOC/100_000`. Sampling polls `/bin/ps -axo pgid=,rss=`
//!   (the child runs in its own process group, so the pgid filter covers the
//!   whole tree — `cargo run` wrappers and grandchildren included) at a
//!   configurable interval (default 50 ms). A watchdog kills the group the
//!   moment a sample exceeds 8 GiB — the same policy the serial phase runs
//!   under, built into the collector so a runaway bench can never melt the
//!   host. LOC = non-blank lines (at least one non-whitespace character)
//!   across the corpus `.dart` files, `.dart_tool` excluded; the verbatim
//!   definition rides every report.
//! - `incremental-p95`: per-step wall time of the dartq driver over a
//!   materialized fixture package driven through an edit manifest (the
//!   hoisted shape of the in-test tables in
//!   `crates/dartq-engine/tests/incremental_oracle.rs`: per step, a batch of
//!   `{path -> content | remove}` edits). Reports P50/P95/P99 (nearest-rank).
//!   The oracle comparison column (`oracle_duration_ms`, one full
//!   `dart analyze` per step) is captured only with `--with-oracle`.
//! - `token-cost`: `dartq check --format=machine` stdout vs
//!   `dart analyze --format=machine` stdout on the same corpus, tokenized
//!   with a pluggable [`Tokenizer`] (v1: the `chars-per-4` heuristic; a real
//!   BPE such as cl100k slots in behind the same trait). Reports both totals
//!   and the savings ratio.
//! - `plugin-overhead`: the cost the plugin subsystem adds per analyze cycle
//!   (M4, PRD §14) — per-cycle wall time of one feed+drain round trip
//!   against a live plugin host (the `samples/compat` compatibility plugin,
//!   launched through the daemon's `dart <entrypoint>` path) and the plugin
//!   sidecar's peak RSS (the runtime's own monitor): the "with plugins" side
//!   of the with/without comparison, since a run without plugins has no
//!   sidecar process. Per-cycle time includes the runtime's reply-burst
//!   settle window (the drain face's contract: the burst is handed over
//!   whole, so each cycle pays one quiet window, `BURST_SETTLE` in
//!   `dartq-plugin`).
//!
//! Every report carries `schema_version: 1` and its full input set (corpus
//! dir, LOC definition, tokenizer id, sample counts, commands) for PRD §19.5
//! reproducibility, written as `<out>/<metric>/report.json` (default out:
//! `docs/audits/bench`).
//!
//! Discipline: the process-driving functions are exercised only by the
//! serial verification phase — the unit tests cover the pure core
//! exclusively (canned ps output, canned samples, temp-dir LOC counting).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{Manifest, PluginDiagnostic, PluginRuntime, SpawnSpec};
use dartq_source::{FileId, FileTable};
use serde::{Deserialize, Serialize};

use crate::Outcome;
use crate::corpus;
use crate::dartq::DartqDriver;
use crate::machine::OutputFormat;
use crate::oracle::{driver_timeout_from_env, run_captured};

const DEFAULT_CORPUS: &str = "tests/corpus/m0";
const DEFAULT_OUT: &str = "docs/audits/bench";
const DEFAULT_DARTQ_CMD: &str = "cargo run -q -p dartq-cli -- check --format=machine";
/// Tree-RSS watchdog threshold: 8 GiB in KiB (memory-incident discipline —
/// the collector carries the same guard the serial phase runs under).
const WATCHDOG_THRESHOLD_KIB: u64 = 8 * 1024 * 1024;
const DEFAULT_RSS_INTERVAL_MS: u64 = 50;
/// `/bin/ps`, absolute: the sampler must not depend on PATH (a hijacked or
/// missing ps would silently produce zero samples and a fake low peak).
const PS: &str = "/bin/ps";
const METRICS: [&str; 4] = ["rss-100k", "incremental-p95", "token-cost", "plugin-overhead"];
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// The plugin the `plugin-overhead` axis drives: the compatibility sample (a
/// real plugin directory with its `dartq_plugin.json` manifest), launched
/// through the same `dart <entrypoint>` path the daemon's discovery uses.
const DEFAULT_PLUGIN_DIR: &str = "crates/dartq-plugin-host/samples/compat";
/// Measured analyze cycles of feed + drain per run.
const DEFAULT_PLUGIN_CYCLES: usize = 50;
/// Measurement-validity drain budget for one feed+drain cycle: a warm plugin
/// answers in milliseconds; missing it fails the metric instead of reporting
/// numbers off a stalled host.
const PLUGIN_DRAIN: Duration = Duration::from_secs(5);
/// The synthesized engine batch: deterministic rows spread over a fixed file
/// set (the plugin emits one finding per row on a `.dart` file).
const PLUGIN_BATCH_ROWS: usize = 64;
const PLUGIN_BATCH_FILES: usize = 8;

/// LOC definition recorded verbatim in every report that divides by LOC.
const LOC_DEFINITION: &str = "non-blank lines (at least one non-whitespace \
character) across .dart files under the corpus dir, .dart_tool excluded";

// ---------------------------------------------------------------------------
// CLI options
// ---------------------------------------------------------------------------

pub(crate) struct BenchOptions {
    /// Selected metrics; empty = all three, in canonical order.
    metrics: Vec<&'static str>,
    corpus_dir: PathBuf,
    out: PathBuf,
    /// Edit manifest for `incremental-p95`.
    manifest: Option<PathBuf>,
    dartq_cmd: String,
    oracle_dart: String,
    rss_interval_ms: u64,
    /// incremental-p95: also capture one full `dart analyze` wall time per
    /// step into `oracle_duration_ms`.
    with_oracle: bool,
    /// plugin-overhead: the plugin directory to drive (its
    /// `dartq_plugin.json` manifest is loaded from it).
    plugin_dir: PathBuf,
    /// plugin-overhead: measured feed+drain cycles.
    plugin_cycles: usize,
}

impl BenchOptions {
    fn parse(args: &[String]) -> Result<Self, Outcome> {
        let mut opts = Self {
            metrics: Vec::new(),
            corpus_dir: PathBuf::from(DEFAULT_CORPUS),
            out: PathBuf::from(DEFAULT_OUT),
            manifest: None,
            dartq_cmd: DEFAULT_DARTQ_CMD.to_string(),
            oracle_dart: "dart".to_string(),
            rss_interval_ms: DEFAULT_RSS_INTERVAL_MS,
            with_oracle: false,
            plugin_dir: PathBuf::from(DEFAULT_PLUGIN_DIR),
            plugin_cycles: DEFAULT_PLUGIN_CYCLES,
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--metric" => opts
                    .metrics
                    .push(resolve_metric(crate::value(args, &mut i)?)?),
                "--corpus-dir" => opts.corpus_dir = crate::value(args, &mut i)?.into(),
                "--out" => opts.out = crate::value(args, &mut i)?.into(),
                "--manifest" => opts.manifest = Some(crate::value(args, &mut i)?.into()),
                "--dartq-cmd" => opts.dartq_cmd = crate::value(args, &mut i)?.to_string(),
                "--oracle-dart" => opts.oracle_dart = crate::value(args, &mut i)?.to_string(),
                "--rss-interval-ms" => {
                    opts.rss_interval_ms = crate::value(args, &mut i)?
                        .parse()
                        .ok()
                        .filter(|ms| *ms > 0)
                        .ok_or_else(|| {
                            Outcome::Error("--rss-interval-ms must be an integer > 0".into())
                        })?;
                }
                "--with-oracle" => opts.with_oracle = true,
                "--plugin-dir" => opts.plugin_dir = crate::value(args, &mut i)?.into(),
                "--plugin-cycles" => {
                    opts.plugin_cycles = crate::value(args, &mut i)?
                        .parse()
                        .ok()
                        .filter(|cycles| *cycles > 0)
                        .ok_or_else(|| {
                            Outcome::Error("--plugin-cycles must be an integer > 0".into())
                        })?;
                }
                other => return Err(crate::unknown_flag(other, "bench")),
            }
            i += 1;
        }
        Ok(opts)
    }

    /// The resolved run order: selected metrics in selection order, or all
    /// three when `--metric` was not given.
    fn resolved_metrics(&self) -> Vec<&'static str> {
        if self.metrics.is_empty() {
            METRICS.to_vec()
        } else {
            self.metrics.clone()
        }
    }
}

fn resolve_metric(name: &str) -> Result<&'static str, Outcome> {
    METRICS
        .into_iter()
        .find(|m| m.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            Outcome::Error(format!(
                "invalid --metric `{name}` (one of: {})",
                METRICS.join(", ")
            ))
        })
}

// ---------------------------------------------------------------------------
// Pure core: percentiles, tokenizer, ps parsing, watchdog, LOC, manifest
// ---------------------------------------------------------------------------

/// Nearest-rank percentile over samples in any order (not mutated): the value
/// at rank ceil(p/100 · n) in ascending order, 1-based. The nearest-rank
/// convention keeps every reported percentile an actually observed sample
/// (no interpolation), P0 = min and P100 = max. Empty input -> `None`.
fn percentile(mut samples: Vec<f64>, p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    #[allow(clippy::cast_precision_loss)] // usize -> f64 for the rank formula (counts are small)
    let rank = ((p / 100.0) * samples.len() as f64).ceil().max(1.0);
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    // rank in [1, n] by construction
    let rank = rank as usize;
    samples.get(rank - 1).copied()
}

/// Output tokenizer for the token-cost metric (PRD §1.5). v1 is the chars/4
/// heuristic; a real BPE (cl100k) slots in behind this trait without touching
/// the metric code — only `id()` and `count()` change.
trait Tokenizer {
    /// Stable id recorded in the report (`tokenizer` input field).
    fn id(&self) -> &'static str;
    fn count(&self, text: &str) -> u64;
}

/// v1 heuristic: one token per 4 characters, rounded up (empty output = 0).
/// Deliberately crude — the metric's comparison is *relative* output volume
/// between the two CLIs, and a linear heuristic preserves the ratio; cl100k
/// is the calibration step for absolute numbers.
struct CharsPer4;

impl Tokenizer for CharsPer4 {
    fn id(&self) -> &'static str {
        "chars-per-4"
    }

    #[allow(clippy::cast_possible_truncation)] // char counts fit u64 on every supported target
    fn count(&self, text: &str) -> u64 {
        text.chars().count().div_ceil(4) as u64
    }
}

/// One `/bin/ps` snapshot reduced to the tree RSS of one process group.
struct TreeRss {
    total_kib: u64,
    /// Rows matched to the pgid (processes seen in the group).
    matched: usize,
    /// Non-blank lines that do not carry two integer columns; counted, not
    /// silently dropped — a torn read must never fake a low sample.
    malformed: usize,
}

/// Sum the RSS (KiB) of every process whose PGID equals `pgid`, from one
/// `/bin/ps -axo pgid=,rss=` snapshot. The sampled child is its own group
/// leader (setpgid pre-exec), so the filter covers the whole tree.
fn tree_rss_kib(ps_output: &str, pgid: u32) -> TreeRss {
    let mut result = TreeRss {
        total_kib: 0,
        matched: 0,
        malformed: 0,
    };
    for line in ps_output.lines() {
        let mut columns = line.split_whitespace();
        match (columns.next(), columns.next(), columns.next()) {
            (Some(row_pgid), Some(row_rss), None) => {
                match (row_pgid.parse::<u32>(), row_rss.parse::<u64>()) {
                    (Ok(row_pgid), Ok(row_rss)) if row_pgid == pgid => {
                        result.total_kib += row_rss;
                        result.matched += 1;
                    }
                    (Ok(_), Ok(_)) => {}
                    _ => result.malformed += 1,
                }
            }
            (None, None, None) => {} // blank line
            _ => result.malformed += 1,
        }
    }
    result
}

/// Tree-RSS watchdog (memory-incident discipline): one sample over the
/// threshold kills the whole process group immediately. The kill goes through
/// the injected closure so the trip path is unit-asserted with a spy — tests
/// never touch a real process.
struct RssWatchdog<F>
where
    F: FnMut(i32),
{
    threshold_kib: u64,
    kill: F,
}

impl<F> RssWatchdog<F>
where
    F: FnMut(i32),
{
    /// Feed one sample; `true` = tripped (the group was already signalled).
    fn sample(&mut self, pgid: i32, rss_kib: u64) -> bool {
        if rss_kib > self.threshold_kib {
            (self.kill)(pgid);
            true
        } else {
            false
        }
    }
}

/// File count + LOC over the corpus `.dart` files (the file universe is
/// [`corpus::dart_files`]: recursive, sorted, `.dart_tool` excluded).
struct LocStats {
    files: usize,
    loc: u64,
}

#[allow(clippy::cast_possible_truncation)] // line counts fit u64 on every supported target
fn count_loc(corpus_dir: &Path) -> Result<LocStats, String> {
    let files = corpus::dart_files(corpus_dir)?;
    let mut stats = LocStats {
        files: files.len(),
        loc: 0,
    };
    for rel in &files {
        let bytes = fs::read(corpus_dir.join(rel))
            .map_err(|e| format!("reading {}: {e}", corpus_dir.join(rel).display()))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| format!("{}: invalid UTF-8: {e}", rel.display()))?;
        stats.loc += text.lines().filter(|line| !line.trim().is_empty()).count() as u64;
    }
    Ok(stats)
}

/// Edit manifest for `incremental-p95` (JSON) — the hoisted shape of the
/// in-test tables in `crates/dartq-engine/tests/incremental_oracle.rs`:
///
/// ```json
/// {
///   "initial": {"lib/a.dart": "int f() => 1;\n"},
///   "steps": [
///     {"edits": [{"path": "lib/a.dart", "content": "int f() => 2;\n"}]},
///     {"edits": [{"path": "lib/p.dart", "remove": true}]}
///   ]
/// }
/// ```
///
/// `initial` is optional: present = materialize a fresh package from it
/// (`dart pub get --offline` included); absent = copy `--corpus-dir` and edit
/// the copy. Each edit is exactly one of `content` (set file) or `remove`.
#[derive(Deserialize)]
struct EditManifest {
    #[serde(default)]
    initial: BTreeMap<String, String>,
    steps: Vec<ManifestStep>,
}

#[derive(Deserialize)]
struct ManifestStep {
    edits: Vec<ManifestEdit>,
}

#[derive(Deserialize)]
struct ManifestEdit {
    path: String,
    content: Option<String>,
    #[serde(default)]
    remove: bool,
}

fn parse_manifest(text: &str) -> Result<EditManifest, Outcome> {
    let manifest: EditManifest = serde_json::from_str(text)
        .map_err(|e| Outcome::Error(format!("parsing --manifest JSON: {e}")))?;
    if manifest.steps.is_empty() {
        return Err(Outcome::Error(
            "edit manifest has no steps; nothing to time".into(),
        ));
    }
    for (index, step) in manifest.steps.iter().enumerate() {
        if step.edits.is_empty() {
            return Err(Outcome::Error(format!(
                "edit manifest step {} has an empty edits batch",
                index + 1
            )));
        }
        for edit in &step.edits {
            validate_edit_path(&edit.path).map_err(Outcome::Error)?;
            if (edit.content.is_some()) == edit.remove {
                return Err(Outcome::Error(format!(
                    "edit manifest step {} path `{}`: exactly one of `content` or `remove` \
                     is required",
                    index + 1,
                    edit.path
                )));
            }
        }
    }
    Ok(manifest)
}

/// Manifest paths are written under the materialized fixture: they must be
/// plain corpus-relative components — absolute paths and `..` traversal would
/// let a manifest write outside the fixture.
fn validate_edit_path(path: &str) -> Result<(), String> {
    let rel = Path::new(path);
    if path.is_empty() || rel.is_absolute() {
        return Err(format!("edit path `{path}` must be corpus-relative"));
    }
    for component in rel.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "edit path `{path}` must be plain relative components (no `.`, `..`, `/`)"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Report schema (schema_version 1; every metric records its full input set,
// PRD §19.5)
// ---------------------------------------------------------------------------

/// Report shapes follow the existing report.rs pattern: one nested `meta`
/// object per metric carrying the schema version plus the metric's full input
/// set (corpus dir, LOC definition, tokenizer id, sample counts, commands).
/// Flat structs, no serde flatten — the round-trip test pins the shape.

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct RssReport {
    meta: RssMeta,
    file_count: usize,
    loc: u64,
    samples: usize,
    /// ps rows that failed to parse across the whole run (accounted, not
    /// silently dropped).
    malformed_samples: usize,
    peak_rss_kib: u64,
    rss_per_100k_loc_kib: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct RssMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    dartq_cmd: String,
    /// Unix epoch seconds (std-only reporting; no calendar dependency).
    epoch_seconds: u64,
    loc_definition: String,
    sample_interval_ms: u64,
    watchdog_threshold_kib: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct IncrementalReport {
    meta: IncrementalMeta,
    /// Baseline (cold) analysis wall time, ms.
    baseline_duration_ms: f64,
    steps: Vec<StepTiming>,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct IncrementalMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    dartq_cmd: String,
    epoch_seconds: u64,
    manifest: String,
    steps: usize,
    /// Whether the oracle comparison column was captured.
    with_oracle: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct StepTiming {
    /// 1-based manifest step.
    index: usize,
    duration_ms: f64,
    /// Full `dart analyze` wall time for the same step; captured only with
    /// `--with-oracle` (null otherwise).
    oracle_duration_ms: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TokenCostReport {
    meta: TokenMeta,
    dartq_output_bytes: u64,
    dartq_tokens: u64,
    oracle_output_bytes: u64,
    oracle_tokens: u64,
    /// `1 - dartq_tokens/oracle_tokens`; null when the oracle side is empty
    /// (the ratio is undefined — a vacuous corpus says nothing about volume).
    savings_ratio: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TokenMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    dartq_cmd: String,
    epoch_seconds: u64,
    tokenizer: String,
    oracle_cmd: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct PluginOverheadReport {
    meta: PluginOverheadMeta,
    /// One wall-time entry (ms) per measured feed+drain cycle, in run order.
    round_trip_ms: Vec<f64>,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    /// The plugin sidecar process's peak resident set (the runtime's own
    /// RSS monitor, `VmHWM`/`proc_pid_rusage`): the incremental memory cost
    /// of having the plugin loaded — with no plugin loaded there is no
    /// sidecar and the delta is zero.
    plugin_peak_rss_kib: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct PluginOverheadMeta {
    schema_version: u32,
    metric: String,
    epoch_seconds: u64,
    /// The plugin directory driven (manifest + entrypoint live there).
    plugin_dir: String,
    cycles: usize,
    batch_rows: usize,
    batch_files: usize,
    /// Measurement-validity drain budget per cycle.
    drain_budget_ms: u64,
}

/// Unix epoch seconds (std-only reporting; no calendar dependency).
fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The existing report-helper pattern (report.rs): pretty JSON under the out
/// dir, one subdir per report kind.
fn write_report<T: Serialize>(
    metric: &str,
    out_root: &Path,
    report: &T,
) -> Result<PathBuf, Outcome> {
    let dir = out_root.join(metric);
    fs::create_dir_all(&dir)
        .map_err(|e| Outcome::Error(format!("creating {}: {e}", dir.display())))?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| Outcome::Error(format!("serializing {metric} report: {e}")))?;
    let path = dir.join("report.json");
    fs::write(&path, json)
        .map_err(|e| Outcome::Error(format!("writing {}: {e}", path.display())))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Process driving (serial-phase only; the unit tests never call these)
// ---------------------------------------------------------------------------

/// One RSS-sampled driver run (stdout is discarded — the metric reads only
/// the exit tier and stderr; the capture files exist to keep pipe buffers
/// from blocking a verbose driver).
struct SampledRun {
    status: std::process::ExitStatus,
    stderr: Vec<u8>,
    peak_rss_kib: u64,
    samples: usize,
    malformed_samples: usize,
}

/// Run `program` to completion while sampling its process-group tree RSS
/// every `interval`: same spawn/kill skeleton as [`crate::oracle::run_captured`]
/// (own process group via setpgid, temp-file capture, wall-clock budget, group
/// kill on timeout) plus the ps polling loop and the 8 GiB watchdog.
#[cfg(unix)]
fn run_sampled(
    name: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
    interval: Duration,
) -> Result<SampledRun, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use crate::oracle::{POLL_INTERVAL, TempCapture, kill_driver};

    let capture = TempCapture::new();
    let stdout = fs::File::create(&capture.stdout_path).map_err(|e| format!("[{name}] {e}"))?;
    let stderr = fs::File::create(&capture.stderr_path).map_err(|e| format!("[{name}] {e}"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // Own process group (same rationale as run_captured): the pgid filter
    // then covers the whole tree, and the watchdog/timeout kills reach
    // grandchildren (cargo -> rustc/dartq).
    unsafe {
        cmd.pre_exec(|| {
            let _ = libc::setpgid(0, 0);
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("[{name}] failed to launch `{program}`: {e}"))?;
    let pgid = i32::try_from(child.id()).map_err(|_| format!("[{name}] child pid overflow"))?;

    let kill_group = |group: i32| {
        // SAFETY: kill(2) with a negative PID signals the process group whose
        // leader is our own just-spawned child (see the pre_exec hook).
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
    };
    let mut watchdog = RssWatchdog {
        threshold_kib: WATCHDOG_THRESHOLD_KIB,
        kill: kill_group,
    };

    let deadline = Instant::now() + timeout;
    let mut next_sample = Instant::now();
    let mut peak_rss_kib = 0;
    let mut samples = 0;
    let mut malformed_samples = 0;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    kill_driver(&mut child);
                    let _ = child.wait();
                    return Err(format!(
                        "[{name}] `{program}` timed out after {}ms and was killed; \
                         peak tree RSS so far: {peak_rss_kib} KiB",
                        timeout.as_millis()
                    ));
                }
                if now >= next_sample {
                    next_sample = now + interval;
                    let snapshot = ps_snapshot()?;
                    let rss = tree_rss_kib(&snapshot, child.id());
                    malformed_samples += rss.malformed;
                    samples += 1;
                    peak_rss_kib = peak_rss_kib.max(rss.total_kib);
                    if watchdog.sample(pgid, rss.total_kib) {
                        let _ = child.wait();
                        return Err(format!(
                            "[{name}] RSS watchdog: tree RSS {} KiB exceeds the \
                             {WATCHDOG_THRESHOLD_KIB} KiB threshold; process group killed",
                            rss.total_kib
                        ));
                    }
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(format!("[{name}] waiting for `{program}`: {e}")),
        }
    };
    if samples == 0 {
        return Err(format!(
            "[{name}] `{program}` exited before any RSS sample could be taken"
        ));
    }
    Ok(SampledRun {
        status,
        stderr: capture.stderr_bytes(),
        peak_rss_kib,
        samples,
        malformed_samples,
    })
}

/// One `/bin/ps -axo pgid=,rss=` snapshot (KiB columns on macOS and Linux).
#[cfg(unix)]
fn ps_snapshot() -> Result<String, String> {
    let output = std::process::Command::new(PS)
        .args(["-axo", "pgid=,rss="])
        .output()
        .map_err(|e| format!("`{PS} -axo pgid=,rss=` failed to launch: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{PS} -axo pgid=,rss=` exited with {:?}",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("ps output is not UTF-8: {e}"))
}

/// Exit validation shared by all bench drivers: the analysis exit tiers
/// {0,1,2,3} are measurements (dartdev analyze contract, facts §3); anything
/// else fails the metric instead of reporting numbers off a crashed run.
fn validate_exit(
    name: &str,
    program: &str,
    status: std::process::ExitStatus,
    stderr: &[u8],
) -> Result<(), String> {
    let code = status.code();
    if code.is_some_and(|c| (0..=3).contains(&c)) {
        return Ok(());
    }
    Err(format!(
        "[{name}] `{program}` exited with {code:?} (valid: 0-3); stderr tail:\n{}",
        crate::oracle::stderr_tail(stderr)
    ))
}

// ---------------------------------------------------------------------------
// Metric collectors
// ---------------------------------------------------------------------------

/// Entry: `harness bench [--metric NAME]... [--corpus-dir DIR] [--out DIR]
/// [--manifest PATH] [--dartq-cmd TEMPLATE] [--oracle-dart PATH]
/// [--rss-interval-ms MS] [--with-oracle]`.
pub(crate) fn bench_entry(args: &[String]) -> Result<(String, bool), Outcome> {
    let opts = BenchOptions::parse(args)?;
    let metrics = opts.resolved_metrics();
    if metrics.contains(&"incremental-p95") && opts.manifest.is_none() {
        return Err(Outcome::Error(
            "--metric incremental-p95 requires --manifest PATH (the edit manifest)".into(),
        ));
    }
    // plugin-overhead drives a plugin process, not the corpus: the corpus
    // preflight runs only when a corpus metric was selected.
    let corpus = if metrics.iter().any(|m| *m != "plugin-overhead") {
        let corpus = crate::canonical_corpus(&opts.corpus_dir)?;
        corpus::check_language_version(&corpus).map_err(Outcome::Error)?;
        Some(corpus)
    } else {
        None
    };

    let mut lines = Vec::new();
    for metric in metrics {
        let line = match metric {
            "rss-100k" => metric_rss(&opts, corpus.as_ref().expect("preflighted"))?,
            "incremental-p95" => metric_incremental(&opts, corpus.as_ref().expect("preflighted"))?,
            "token-cost" => metric_tokens(&opts, corpus.as_ref().expect("preflighted"))?,
            "plugin-overhead" => metric_plugin_overhead(&opts)?,
            other => return Err(Outcome::Error(format!("unhandled metric `{other}`"))),
        };
        lines.push(line);
    }
    Ok((lines.join("\n"), true))
}

/// `rss-100k`: one full sampled analysis over the corpus, peak tree RSS
/// normalized by `LOC/100_000`.
#[cfg(unix)]
fn metric_rss(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let loc = count_loc(corpus).map_err(Outcome::Error)?;
    if loc.loc == 0 {
        return Err(Outcome::Error(format!(
            "corpus `{}` has zero LOC; the RSS/100k normalization is undefined",
            corpus.display()
        )));
    }
    let driver = DartqDriver::from_template(&opts.dartq_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let run = run_sampled(
        "bench-rss",
        program,
        &args,
        timeout,
        Duration::from_millis(opts.rss_interval_ms),
    )
    .map_err(Outcome::Error)?;
    validate_exit("bench-rss", program, run.status, &run.stderr).map_err(Outcome::Error)?;
    #[allow(clippy::cast_precision_loss)] // KiB counts -> f64 for the normalized metric
    let per_100k = run.peak_rss_kib as f64 / (loc.loc as f64 / 100_000.0);
    let report = RssReport {
        meta: RssMeta {
            schema_version: SCHEMA_VERSION,
            metric: "rss-100k".to_string(),
            corpus_dir: corpus.display().to_string(),
            dartq_cmd: opts.dartq_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            loc_definition: LOC_DEFINITION.to_string(),
            sample_interval_ms: opts.rss_interval_ms,
            watchdog_threshold_kib: WATCHDOG_THRESHOLD_KIB,
        },
        file_count: loc.files,
        loc: loc.loc,
        samples: run.samples,
        malformed_samples: run.malformed_samples,
        peak_rss_kib: run.peak_rss_kib,
        rss_per_100k_loc_kib: per_100k,
    };
    let path = write_report("rss-100k", &opts.out, &report)?;
    Ok(format!(
        "rss-100k: peak {} KiB / {} LOC ({} files) -> {:.1} KiB per 100k LOC \
         (samples: {}, interval: {}ms) -> {}",
        report.peak_rss_kib,
        report.loc,
        report.file_count,
        per_100k,
        report.samples,
        opts.rss_interval_ms,
        path.display()
    ))
}

#[cfg(not(unix))]
fn metric_rss(_opts: &BenchOptions, _corpus: &Path) -> Result<String, Outcome> {
    Err(Outcome::Error(
        "rss-100k sampling requires a POSIX host (process-group ps sampling)".into(),
    ))
}

/// `incremental-p95`: materialize the fixture package, run a timed baseline,
/// then drive the manifest's edit steps and time the driver per step.
fn metric_incremental(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let manifest_path = opts
        .manifest
        .as_ref()
        .expect("bench_entry validates the manifest flag for incremental-p95");
    let text = fs::read_to_string(manifest_path).map_err(|e| {
        Outcome::Error(format!(
            "reading --manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let manifest = parse_manifest(&text)?;

    let fixture = BenchFixture::new("dq_bench_fixture");
    fixture
        .materialize(corpus, &manifest, &opts.oracle_dart)
        .map_err(Outcome::Error)?;
    let driver = DartqDriver::from_template(&opts.dartq_cmd);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;

    let baseline_duration_ms =
        timed_run("bench-incremental", &driver, &fixture.dir, timeout).map_err(Outcome::Error)?;

    let mut steps = Vec::with_capacity(manifest.steps.len());
    for (index, step) in manifest.steps.iter().enumerate() {
        apply_step(&fixture.dir, step).map_err(Outcome::Error)?;
        let duration_ms = timed_run("bench-incremental", &driver, &fixture.dir, timeout)
            .map_err(Outcome::Error)?;
        let oracle_duration_ms = if opts.with_oracle {
            Some(
                timed_oracle_run(
                    "bench-incremental-oracle",
                    &opts.oracle_dart,
                    &fixture.dir,
                    timeout,
                )
                .map_err(Outcome::Error)?,
            )
        } else {
            None
        };
        steps.push(StepTiming {
            index: index + 1,
            duration_ms,
            oracle_duration_ms,
        });
    }

    let durations: Vec<f64> = steps.iter().map(|s| s.duration_ms).collect();
    let report = IncrementalReport {
        meta: IncrementalMeta {
            schema_version: SCHEMA_VERSION,
            metric: "incremental-p95".to_string(),
            corpus_dir: corpus.display().to_string(),
            dartq_cmd: opts.dartq_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            manifest: manifest_path.display().to_string(),
            steps: steps.len(),
            with_oracle: opts.with_oracle,
        },
        baseline_duration_ms,
        p50_ms: percentile(durations.clone(), 50.0).unwrap_or_default(),
        p95_ms: percentile(durations.clone(), 95.0).unwrap_or_default(),
        p99_ms: percentile(durations, 99.0).unwrap_or_default(),
        steps,
    };
    let path = write_report("incremental-p95", &opts.out, &report)?;
    Ok(format!(
        "incremental-p95: {} steps | p50 {:.1}ms p95 {:.1}ms p99 {:.1}ms \
         (baseline {:.1}ms) -> {}",
        report.meta.steps,
        report.p50_ms,
        report.p95_ms,
        report.p99_ms,
        baseline_duration_ms,
        path.display()
    ))
}

/// `token-cost`: `dartq check --format=machine` stdout vs
/// `dart analyze --format=machine` stdout, same corpus, same tokenizer.
#[allow(clippy::cast_possible_truncation)] // output byte counts fit u64 on every supported target
fn metric_tokens(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let driver = DartqDriver::from_template(&opts.dartq_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let dartq_out = run_captured("bench-tokens-dartq", program, &args, None, timeout)
        .map_err(Outcome::Error)?;
    validate_exit(
        "bench-tokens-dartq",
        program,
        dartq_out.status,
        &dartq_out.stderr,
    )
    .map_err(Outcome::Error)?;
    let dartq_text = std::str::from_utf8(&dartq_out.stdout)
        .map_err(|e| Outcome::Error(format!("dartq stdout is not UTF-8: {e}")))?;

    let oracle_args = vec![
        "analyze".to_string(),
        OutputFormat::Machine.flag().to_string(),
        "--no-fatal-warnings".to_string(),
        corpus.display().to_string(),
    ];
    let oracle_out = run_captured(
        "bench-tokens-oracle",
        &opts.oracle_dart,
        &oracle_args,
        None,
        timeout,
    )
    .map_err(Outcome::Error)?;
    validate_exit(
        "bench-tokens-oracle",
        &opts.oracle_dart,
        oracle_out.status,
        &oracle_out.stderr,
    )
    .map_err(Outcome::Error)?;
    let oracle_text = std::str::from_utf8(&oracle_out.stdout)
        .map_err(|e| Outcome::Error(format!("oracle stdout is not UTF-8: {e}")))?;

    let tokenizer = CharsPer4;
    let dartq_tokens = tokenizer.count(dartq_text);
    let oracle_tokens = tokenizer.count(oracle_text);
    let savings_ratio = if oracle_tokens == 0 {
        None
    } else {
        #[allow(clippy::cast_precision_loss)] // token counts -> f64 for the ratio
        Some(1.0 - dartq_tokens as f64 / oracle_tokens as f64)
    };
    let report = TokenCostReport {
        meta: TokenMeta {
            schema_version: SCHEMA_VERSION,
            metric: "token-cost".to_string(),
            corpus_dir: corpus.display().to_string(),
            dartq_cmd: opts.dartq_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            tokenizer: tokenizer.id().to_string(),
            oracle_cmd: format!(
                "{} analyze --format=machine --no-fatal-warnings",
                opts.oracle_dart
            ),
        },
        dartq_output_bytes: dartq_out.stdout.len() as u64,
        dartq_tokens,
        oracle_output_bytes: oracle_out.stdout.len() as u64,
        oracle_tokens,
        savings_ratio,
    };
    let path = write_report("token-cost", &opts.out, &report)?;
    let savings = report.savings_ratio.map_or_else(
        || "undefined (empty oracle output)".to_string(),
        |r| format!("{:.1}%", r * 100.0),
    );
    Ok(format!(
        "token-cost: dartq {} tok vs oracle {} tok ({}) | savings {savings} -> {}",
        report.dartq_tokens,
        report.oracle_tokens,
        tokenizer.id(),
        path.display()
    ))
}

/// `plugin-overhead`: the cost the plugin subsystem adds per analyze cycle —
/// the wall time of one feed+drain round trip against a live plugin host
/// (the `compat` sample by default), plus the sidecar process's peak RSS:
/// the "with plugins" side of the with/without comparison, since a run
/// without plugins has no sidecar and pays nothing.
fn metric_plugin_overhead(opts: &BenchOptions) -> Result<String, Outcome> {
    let manifest = Manifest::from_path(&opts.plugin_dir.join("dartq_plugin.json"))
        .map_err(|e| Outcome::Error(format!("loading the plugin manifest: {e}")))?;
    let mut runtime = PluginRuntime::spawn(SpawnSpec::dart(&opts.plugin_dir, manifest))
        .map_err(|e| Outcome::Error(format!("spawning the plugin host: {e}")))?;

    let (batch, files) = synthesized_batch();
    // One unmeasured warmup cycle: the first JIT-compiled pass would skew
    // the per-cycle distribution.
    let expected_rows = format!(
        "plugin emitted the wrong number of findings for a {PLUGIN_BATCH_ROWS}-row batch"
    );
    if drive_cycle(&mut runtime, &batch, &files)?.len() != PLUGIN_BATCH_ROWS {
        return Err(Outcome::Error(expected_rows));
    }

    let mut round_trip_ms = Vec::with_capacity(opts.plugin_cycles);
    for _ in 0..opts.plugin_cycles {
        let started = Instant::now();
        let rows = drive_cycle(&mut runtime, &batch, &files)?;
        round_trip_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        if rows.len() != PLUGIN_BATCH_ROWS {
            return Err(Outcome::Error(expected_rows));
        }
    }
    let stats = runtime.stats();
    runtime
        .shutdown()
        .map_err(|e| Outcome::Error(format!("plugin shutdown failed: {e}")))?;

    let p50_ms = percentile(round_trip_ms.clone(), 50.0).unwrap_or_default();
    let p95_ms = percentile(round_trip_ms.clone(), 95.0).unwrap_or_default();
    let p99_ms = percentile(round_trip_ms.clone(), 99.0).unwrap_or_default();
    let report = PluginOverheadReport {
        meta: PluginOverheadMeta {
            schema_version: SCHEMA_VERSION,
            metric: "plugin-overhead".to_string(),
            epoch_seconds: epoch_seconds(),
            plugin_dir: opts.plugin_dir.display().to_string(),
            cycles: opts.plugin_cycles,
            batch_rows: PLUGIN_BATCH_ROWS,
            batch_files: PLUGIN_BATCH_FILES,
            drain_budget_ms: u64::try_from(PLUGIN_DRAIN.as_millis()).unwrap_or(u64::MAX),
        },
        p50_ms,
        p95_ms,
        p99_ms,
        round_trip_ms,
        plugin_peak_rss_kib: stats.peak_rss_bytes / 1024,
    };
    let path = write_report("plugin-overhead", &opts.out, &report)?;
    let p50 = report.p50_ms;
    let p95 = report.p95_ms;
    let p99 = report.p99_ms;
    Ok(format!(
        "plugin-overhead: {} feed+drain cycles over {} rows ({} files) | \
         p50 {p50:.1}ms p95 {p95:.1}ms p99 {p99:.1}ms | plugin peak RSS {} KiB -> {}",
        report.meta.cycles,
        report.meta.batch_rows,
        report.meta.batch_files,
        report.plugin_peak_rss_kib,
        path.display()
    ))
}

/// One analyze cycle against the plugin: push the engine batch, wait for its
/// findings. The drain budget is a measurement-validity gate — a warm plugin
/// answers in milliseconds, so a miss fails the metric instead of reporting
/// numbers off a stalled host.
fn drive_cycle(
    runtime: &mut PluginRuntime,
    batch: &[Diagnostic],
    files: &FileTable,
) -> Result<Vec<PluginDiagnostic>, Outcome> {
    runtime
        .feed(batch, files)
        .map_err(|e| Outcome::Error(format!("plugin feed failed: {e}")))?;
    runtime
        .wait_diagnostics(PLUGIN_DRAIN)
        .map_err(|e| Outcome::Error(format!("plugin drain failed: {e}")))
}

/// The deterministic engine batch: [`PLUGIN_BATCH_ROWS`] diagnostics spread
/// round-robin over [`PLUGIN_BATCH_FILES`] `.dart` files.
fn synthesized_batch() -> (Vec<Diagnostic>, FileTable) {
    let mut files = FileTable::new();
    let paths: Vec<FileId> = (0..PLUGIN_BATCH_FILES)
        .map(|i| {
            files.intern(Path::new(&format!(
                "lib/plugin_overhead_{i}.dart"
            )))
        })
        .collect();
    let batch = (0..PLUGIN_BATCH_ROWS)
        .map(|i| {
            Diagnostic::new(
                Severity::Warning,
                URI_DOES_NOT_EXIST,
                "plugin overhead batch row",
                paths[i % PLUGIN_BATCH_FILES],
                SourceRange {
                    offset: u32::try_from(i * 8).unwrap_or(u32::MAX),
                    length: 4,
                },
                None,
            )
        })
        .collect();
    (batch, files)
}

// ---------------------------------------------------------------------------
// Fixture materialization (incremental-p95)
// ---------------------------------------------------------------------------

/// A scratch fixture package, removed on drop (every exit path).
struct BenchFixture {
    dir: PathBuf,
}

impl BenchFixture {
    fn new(prefix: &str) -> Self {
        Self {
            dir: std::env::temp_dir().join(format!("{prefix}_{}", std::process::id())),
        }
    }

    /// `initial` present: fresh package from it (pubspec + `dart pub get
    /// --offline`, the `corpus::pub_get` budget applies). Otherwise: a plain
    /// recursive copy of `corpus` (package config included — its rootUri is
    /// relative, so the copy stays a valid package).
    fn materialize(
        &self,
        corpus: &Path,
        manifest: &EditManifest,
        dart: &str,
    ) -> Result<(), String> {
        let _ = fs::remove_dir_all(&self.dir);
        fs::create_dir_all(&self.dir).map_err(|e| format!("creating fixture: {e}"))?;
        if manifest.initial.is_empty() {
            copy_tree(corpus, &self.dir)?;
        } else {
            fs::write(self.dir.join("pubspec.yaml"), corpus::PUBSPEC)
                .map_err(|e| format!("writing fixture pubspec: {e}"))?;
            for (path, content) in &manifest.initial {
                validate_edit_path(path)?;
                let target = self.dir.join(path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("creating {}: {e}", parent.display()))?;
                }
                fs::write(&target, content)
                    .map_err(|e| format!("writing fixture {}: {e}", target.display()))?;
            }
            corpus::pub_get(dart, &self.dir)?;
        }
        Ok(())
    }
}

impl Drop for BenchFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).ok();
    }
}

/// Apply one manifest step to the fixture on disk.
fn apply_step(fixture_dir: &Path, step: &ManifestStep) -> Result<(), String> {
    for edit in &step.edits {
        let target = fixture_dir.join(&edit.path);
        if edit.remove {
            fs::remove_file(&target).map_err(|e| format!("removing {}: {e}", target.display()))?;
        } else if let Some(content) = &edit.content {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("creating {}: {e}", parent.display()))?;
            }
            fs::write(&target, content)
                .map_err(|e| format!("writing {}: {e}", target.display()))?;
        }
    }
    Ok(())
}

/// Plain recursive copy (dirs and files; corpora carry no symlinks — corpus
/// assembly drops them, see `corpus::copy_tree`'s skip rule).
fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("reading {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("reading {}: {e}", src.display()))?;
        let ty = entry
            .file_type()
            .map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
        let target = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("copying {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// One timed dartq driver invocation (valid-exit tiers {0,1,2,3}; anything
/// else fails the metric instead of reporting a time off a crashed run).
fn timed_run(
    name: &str,
    driver: &DartqDriver,
    dir: &Path,
    timeout: Duration,
) -> Result<f64, String> {
    let (program, args) = driver.command(dir);
    let started = Instant::now();
    let out = run_captured(name, program, &args, None, timeout)?;
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    validate_exit(name, program, out.status, &out.stderr)?;
    Ok(duration_ms)
}

/// One timed full `dart analyze` over the fixture (the incremental metric's
/// oracle comparison column).
fn timed_oracle_run(name: &str, dart: &str, dir: &Path, timeout: Duration) -> Result<f64, String> {
    let args = vec![
        "analyze".to_string(),
        OutputFormat::Machine.flag().to_string(),
        "--no-fatal-warnings".to_string(),
        dir.display().to_string(),
    ];
    let started = Instant::now();
    let out = run_captured(name, dart, &args, None, timeout)?;
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    validate_exit(name, dart, out.status, &out.stderr)?;
    Ok(duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    // -- percentiles ---------------------------------------------------------

    #[test]
    fn percentile_nearest_rank_edges() {
        assert_eq!(percentile(vec![], 50.0), None, "empty input -> None");
        assert_eq!(percentile(vec![7.0], 50.0), Some(7.0));
        assert_eq!(percentile(vec![7.0], 0.0), Some(7.0), "P0 = min");
        assert_eq!(percentile(vec![7.0], 100.0), Some(7.0), "P100 = max");
        let samples = vec![30.0, 10.0, 40.0, 20.0]; // unsorted input
        assert_eq!(percentile(samples.clone(), 0.0), Some(10.0));
        // rank = ceil(0.5 * 4) = 2 -> the 2nd smallest (1-based)
        assert_eq!(percentile(samples.clone(), 50.0), Some(20.0));
        // rank = ceil(0.95 * 4) = 4 -> max
        assert_eq!(percentile(samples, 95.0), Some(40.0));
    }

    #[test]
    fn percentile_rank_is_one_based_nearest() {
        // n = 3: P50 -> ceil(1.5) = 2 -> middle; P99 -> ceil(2.97) = 3 -> max
        let samples = vec![1.0, 2.0, 3.0];
        assert_eq!(percentile(samples.clone(), 50.0), Some(2.0));
        assert_eq!(percentile(samples, 99.0), Some(3.0));
    }

    // -- tokenizer -----------------------------------------------------------

    #[test]
    fn chars_per4_tokenizer_counts_ceil_chars_div4() {
        let tokenizer = CharsPer4;
        assert_eq!(tokenizer.id(), "chars-per-4");
        assert_eq!(tokenizer.count(""), 0);
        assert_eq!(tokenizer.count("a"), 1);
        assert_eq!(tokenizer.count("abcd"), 1);
        assert_eq!(tokenizer.count("abcde"), 2);
        assert_eq!(tokenizer.count("abcdefgh"), 2);
        assert_eq!(tokenizer.count("abcdefghi"), 3);
        // characters, not bytes: a 3-byte CJK char counts as one character
        assert_eq!(tokenizer.count("水水水水"), 1);
    }

    // -- ps parsing ----------------------------------------------------------

    #[test]
    fn tree_rss_sums_matching_pgid_rows() {
        let canned = "  100  1200\n  100   340\n  200 99999\n";
        let rss = tree_rss_kib(canned, 100);
        assert_eq!(rss.total_kib, 1540, "group 100 = 1200 + 340; 200 excluded");
        assert_eq!(rss.matched, 2);
        assert_eq!(rss.malformed, 0);
    }

    #[test]
    fn tree_rss_counts_malformed_lines_and_skips_blanks() {
        let canned = "\n  100  100\n  junk\n  100 x\n  100\n  100  50  extra\n  100  40\n";
        let rss = tree_rss_kib(canned, 100);
        assert_eq!(rss.total_kib, 140);
        assert_eq!(rss.matched, 2);
        assert_eq!(
            rss.malformed, 4,
            "non-numeric pgid, non-numeric rss, single column, three columns"
        );
    }

    #[test]
    fn tree_rss_empty_output_is_all_zero() {
        let rss = tree_rss_kib("", 7);
        assert_eq!((rss.total_kib, rss.matched, rss.malformed), (0, 0, 0));
    }

    // -- watchdog ------------------------------------------------------------

    #[test]
    fn watchdog_trips_only_above_threshold_and_signals_group() {
        let mut killed: Vec<i32> = Vec::new();
        let mut watchdog = RssWatchdog {
            threshold_kib: 1_000,
            kill: |group| killed.push(group),
        };
        assert!(!watchdog.sample(42, 999), "below threshold: no trip");
        assert!(
            !watchdog.sample(42, 1_000),
            "exactly at the threshold is not exceeding"
        );
        assert!(watchdog.sample(42, 1_001), "above threshold trips");
        assert_eq!(killed, vec![42], "the process group was signalled once");
    }

    // -- LOC counting --------------------------------------------------------

    #[test]
    fn loc_counts_non_blank_dart_lines_and_skips_dart_tool() {
        let root = std::env::temp_dir().join(format!("dq_bench_loc_{}", std::process::id()));
        write(&root, "lib/a.dart", "void a() {}\n\n   \n\t\nint x = 1;\n");
        write(&root, "lib/sub/b.dart", "\nvoid b() {}\n");
        write(&root, ".dart_tool/hidden.dart", "void h() {}\n");
        write(&root, "notes.txt", "void n() {}\n");

        let stats = count_loc(&root).expect("loc");
        assert_eq!(stats.files, 2, ".dart_tool and non-.dart files excluded");
        assert_eq!(stats.loc, 3, "blank and whitespace-only lines skipped");

        fs::remove_dir_all(&root).ok();
    }

    // -- edit manifest -------------------------------------------------------

    #[test]
    fn manifest_parses_initial_and_steps() {
        let manifest = parse_manifest(
            r#"{"initial": {"lib/a.dart": "x"},
                "steps": [
                  {"edits": [{"path": "lib/a.dart", "content": "y"}]},
                  {"edits": [{"path": "lib/p.dart", "remove": true},
                             {"path": "lib/new.dart", "content": "z"}]}
                ]}"#,
        )
        .expect("valid manifest");
        assert_eq!(manifest.initial.len(), 1);
        assert_eq!(manifest.steps.len(), 2);
        assert_eq!(manifest.steps[1].edits.len(), 2);
    }

    #[test]
    fn manifest_rejects_bad_shape_and_paths() {
        for bad in [
            r#"{"steps": []}"#,
            r#"{"steps": [{"edits": []}]}"#,
            r#"{"steps": [{"edits": [{"path": "a.dart"}]}]}"#, // neither content nor remove
            r#"{"steps": [{"edits": [{"path": "a.dart", "content": "x", "remove": true}]}]}"#, // both
            r#"{"steps": [{"edits": [{"path": "", "content": "x"}]}]}"#,
            r#"{"steps": [{"edits": [{"path": "/abs/a.dart", "content": "x"}]}]}"#,
            r#"{"steps": [{"edits": [{"path": "../escape.dart", "content": "x"}]}]}"#,
            r#"{"steps": [{"edits": [{"path": "./here.dart", "content": "x"}]}]}"#,
            "not json",
        ] {
            assert!(parse_manifest(bad).is_err(), "must reject: {bad}");
        }
    }

    // -- flag parsing --------------------------------------------------------

    #[test]
    fn bench_flag_parsing() {
        let defaults = BenchOptions::parse(&args(&[])).expect("defaults");
        assert_eq!(defaults.resolved_metrics(), METRICS.to_vec());
        assert_eq!(defaults.corpus_dir, PathBuf::from(DEFAULT_CORPUS));
        assert_eq!(defaults.out, PathBuf::from(DEFAULT_OUT));
        assert_eq!(defaults.rss_interval_ms, DEFAULT_RSS_INTERVAL_MS);
        assert!(!defaults.with_oracle);
        assert!(defaults.manifest.is_none());
        assert_eq!(defaults.plugin_dir, PathBuf::from(DEFAULT_PLUGIN_DIR));
        assert_eq!(defaults.plugin_cycles, DEFAULT_PLUGIN_CYCLES);

        let opts = BenchOptions::parse(&args(&[
            "--metric",
            "RSS-100K",
            "--metric",
            "token-cost",
            "--metric",
            "PLUGIN-OVERHEAD",
            "--rss-interval-ms",
            "10",
            "--with-oracle",
            "--manifest",
            "m.json",
            "--out",
            "o",
            "--plugin-dir",
            "some/plugin",
            "--plugin-cycles",
            "7",
        ]))
        .expect("flags");
        assert_eq!(
            opts.resolved_metrics(),
            vec!["rss-100k", "token-cost", "plugin-overhead"]
        );
        assert_eq!(opts.rss_interval_ms, 10);
        assert!(opts.with_oracle);
        assert_eq!(opts.manifest, Some(PathBuf::from("m.json")));
        assert_eq!(opts.plugin_dir, PathBuf::from("some/plugin"));
        assert_eq!(opts.plugin_cycles, 7);

        assert!(
            BenchOptions::parse(&args(&["--metric", "nope"])).is_err(),
            "unknown metric must error"
        );
        assert!(BenchOptions::parse(&args(&["--rss-interval-ms", "0"])).is_err());
        assert!(BenchOptions::parse(&args(&["--rss-interval-ms", "x"])).is_err());
        assert!(BenchOptions::parse(&args(&["--plugin-cycles", "0"])).is_err());
        assert!(BenchOptions::parse(&args(&["--plugin-cycles", "x"])).is_err());
        assert!(matches!(
            BenchOptions::parse(&args(&["--bogus"])),
            Err(Outcome::Error(msg)) if msg.contains("bench")
        ));

        // incremental without a manifest is rejected before any corpus access
        assert!(matches!(
            bench_entry(&args(&["--corpus-dir", "/nonexistent", "--metric", "incremental-p95"])),
            Err(Outcome::Error(msg)) if msg.contains("--manifest")
        ));
    }

    // -- report schema round trip ---------------------------------------------

    #[test]
    fn report_schemas_round_trip_through_disk() {
        let out = std::env::temp_dir().join(format!("dq_bench_report_{}", std::process::id()));
        let rss = RssReport {
            meta: RssMeta {
                schema_version: SCHEMA_VERSION,
                metric: "rss-100k".to_string(),
                corpus_dir: "/corpus".to_string(),
                dartq_cmd: "cargo run -q -p dartq-cli -- check".to_string(),
                epoch_seconds: 1_700_000_000,
                loc_definition: LOC_DEFINITION.to_string(),
                sample_interval_ms: 50,
                watchdog_threshold_kib: WATCHDOG_THRESHOLD_KIB,
            },
            file_count: 2,
            loc: 100_000,
            samples: 7,
            malformed_samples: 1,
            peak_rss_kib: 2048,
            rss_per_100k_loc_kib: 2048.0,
        };
        let incremental = IncrementalReport {
            meta: IncrementalMeta {
                schema_version: SCHEMA_VERSION,
                metric: "incremental-p95".to_string(),
                corpus_dir: "/corpus".to_string(),
                dartq_cmd: "dartq check".to_string(),
                epoch_seconds: 1_700_000_000,
                manifest: "m.json".to_string(),
                steps: 2,
                with_oracle: true,
            },
            baseline_duration_ms: 100.5,
            steps: vec![
                StepTiming {
                    index: 1,
                    duration_ms: 12.5,
                    oracle_duration_ms: Some(900.25),
                },
                StepTiming {
                    index: 2,
                    duration_ms: 8.25,
                    oracle_duration_ms: None,
                },
            ],
            p50_ms: 12.5,
            p95_ms: 12.5,
            p99_ms: 12.5,
        };
        let tokens = TokenCostReport {
            meta: TokenMeta {
                schema_version: SCHEMA_VERSION,
                metric: "token-cost".to_string(),
                corpus_dir: "/corpus".to_string(),
                dartq_cmd: "dartq check".to_string(),
                epoch_seconds: 1_700_000_000,
                tokenizer: "chars-per-4".to_string(),
                oracle_cmd: "dart analyze --format=machine --no-fatal-warnings".to_string(),
            },
            dartq_output_bytes: 10,
            dartq_tokens: 3,
            oracle_output_bytes: 0,
            oracle_tokens: 0,
            savings_ratio: None,
        };

        let rss_path = write_report("rss-100k", &out, &rss).expect("rss report");
        let inc_path = write_report("incremental-p95", &out, &incremental).expect("inc report");
        let tok_path = write_report("token-cost", &out, &tokens).expect("tok report");
        assert_eq!(rss_path, out.join("rss-100k").join("report.json"));

        // rss-100k: typed round trip + the recorded inputs
        let text = fs::read_to_string(&rss_path).expect("report file");
        let parsed: RssReport = serde_json::from_str(&text).expect("rss parse");
        assert_eq!(parsed, rss);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            value["meta"]["loc_definition"], LOC_DEFINITION,
            "the LOC definition rides the report"
        );
        assert_eq!(
            value["meta"]["watchdog_threshold_kib"],
            WATCHDOG_THRESHOLD_KIB
        );

        // incremental-p95: typed round trip + the oracle column's null face
        let text = fs::read_to_string(&inc_path).expect("report file");
        let parsed: IncrementalReport = serde_json::from_str(&text).expect("inc parse");
        assert_eq!(parsed, incremental);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["steps"][0]["oracle_duration_ms"], 900.25);
        assert!(value["steps"][1]["oracle_duration_ms"].is_null());

        // token-cost: typed round trip + the recorded tokenizer
        let text = fs::read_to_string(&tok_path).expect("report file");
        let parsed: TokenCostReport = serde_json::from_str(&text).expect("tok parse");
        assert_eq!(parsed, tokens);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["tokenizer"], "chars-per-4");
        assert!(
            value["savings_ratio"].is_null(),
            "empty oracle output -> undefined ratio"
        );

        fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn plugin_overhead_report_round_trips_through_disk() {
        let out = std::env::temp_dir().join(format!("dq_bench_plugin_{}", std::process::id()));
        let report = PluginOverheadReport {
            meta: PluginOverheadMeta {
                schema_version: SCHEMA_VERSION,
                metric: "plugin-overhead".to_string(),
                epoch_seconds: 1_700_000_000,
                plugin_dir: "crates/dartq-plugin-host/samples/compat".to_string(),
                cycles: 50,
                batch_rows: PLUGIN_BATCH_ROWS,
                batch_files: PLUGIN_BATCH_FILES,
                drain_budget_ms: 5_000,
            },
            round_trip_ms: vec![1.5, 2.5, 3.0],
            p50_ms: 2.5,
            p95_ms: 3.0,
            p99_ms: 3.0,
            plugin_peak_rss_kib: 48_000,
        };

        let path = write_report("plugin-overhead", &out, &report).expect("report");
        assert_eq!(path, out.join("plugin-overhead").join("report.json"));

        let text = fs::read_to_string(&path).expect("report file");
        let parsed: PluginOverheadReport = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed, report);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["metric"], "plugin-overhead");
        assert_eq!(value["round_trip_ms"].as_array().map(Vec::len), Some(3));
        assert_eq!(value["plugin_peak_rss_kib"], 48_000);

        fs::remove_dir_all(&out).ok();
    }
}
