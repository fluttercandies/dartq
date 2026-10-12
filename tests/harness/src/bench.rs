//! §19.3 benchmark metric substrate (M3 gate arbiter tooling): eight
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
//!   `dart analyze` per step) is captured only with `--with-oracle`. This is
//!   the *batch* arm: one full driver process per step.
//! - `incremental-p95-resident`: the same edit manifest driven through one
//!   in-process engine that stays warm across steps (the resident face
//!   PRD §1.5's incremental target describes — edits land as in-memory
//!   overlays, the daemon edit shape, and each step is one `analyze` pass,
//!   no process restart). Reports the same P50/P95/P99 shape over the same
//!   step set, comparable against the batch arm.
//! - `token-cost`: `dartq check --format=machine` stdout vs
//!   `dart analyze --format=machine` stdout on the same corpus, tokenized
//!   with a pluggable [`Tokenizer`] (v1: the `chars-per-4` heuristic; a real
//!   BPE such as cl100k slots in behind the same trait). Reports both totals
//!   and the savings ratio, plus the per-file distribution (machine lines
//!   grouped by their file field): median + P95 (nearest-rank) per side.
//!   The verbosity-tier dimension is recorded honestly: the driven faces
//!   (check/analyze machine output) are fixed-tier today — the CLI's `-v`
//!   lives only on `explain` — so the report carries `tiers: ["fixed"]`
//!   instead of inventing flags; real tiers are swept where they exist.
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
//! - `cold`: one cold full-analysis wall time per side over the same corpus
//!   (the dartq run through the same sampled spawn as `rss-100k`, the oracle
//!   as one full `dart analyze`) and the oracle/dartq ratio — the charter's
//!   cold-analysis face. Single run per side and no warmup, by definition;
//!   at small corpora the times are process-boot dominated, so every report
//!   carries a `note` caveat: small-corpus ratios are not milestone evidence.
//!   The meta also carries the env descriptor `oracle_baseline.sh` records
//!   (dart version / OS / chip / cores, #798), so the two instruments'
//!   reports are comparable side by side.
//! - `cache-hit-rate`: PRD §19.3's Cache Hit Rate over a defined warm run —
//!   two `dartq analyze` processes over the same corpus sharing one fresh
//!   cache root (`XDG_CACHE_HOME` pinned to an empty scratch dir): run 1
//!   untraced (cold, populates the cross-session cache), run 2 with
//!   `--trace-events`; the engine's own aggregator
//!   (`dartq_engine::event::cache_hit_rate`) scores the capture, and the raw
//!   `LibraryAnalyzed` counts ride beside the rate so a rate over a tiny
//!   total is visible as such. The verbatim warm-run definition rides every
//!   report — the rate is meaningless without the run shape that produced it.
//! - `snapshot-size`: the bytes one analyze run persists under the cache
//!   root (`$XDG_CACHE_HOME/dartq` else `~/.cache/dartq`, PRD §17.2). The
//!   measured run pins `XDG_CACHE_HOME` to an empty scratch dir, so a
//!   per-file stat walk (no `du`) sums exactly this run's writes:
//!   content-store blobs under `<root>/store` today; snapshot generations
//!   under `<root>/snapshots` are written by the daemon session face and do
//!   not occur in a CLI-only run — the path definition rides every report.
//! - `startup`: spawn → first stdout byte, wall clock, over N serial spawns
//!   of the light CLI face (`check --format=json`: the envelope is always
//!   emitted, even clean, so a first byte is guaranteed). Same spawn
//!   skeleton as the rss/cold collectors (own process group, group kill on
//!   timeout, single child at a time); a piped-stdout reader thread
//!   timestamps the first successful read the moment it happens. Reports
//!   median + P95 (nearest-rank) over the sample set.
//!
//! Every report carries `schema_version: 1` and its full input set (corpus
//! dir, LOC definition, tokenizer id, sample counts, commands) for PRD §19.5
//! reproducibility, written as `<out>/<metric>/report.json` (default out:
//! `target/evidence/bench`).
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
const DEFAULT_OUT: &str = "target/evidence/bench";
const DEFAULT_DARTQ_CMD: &str = "cargo run -q -p dartq-cli -- check --format=machine";
/// Tree-RSS watchdog threshold: 8 GiB in KiB (memory-incident discipline —
/// the collector carries the same guard the serial phase runs under).
const WATCHDOG_THRESHOLD_KIB: u64 = 8 * 1024 * 1024;
const DEFAULT_RSS_INTERVAL_MS: u64 = 50;
/// `/bin/ps`, absolute: the sampler must not depend on PATH (a hijacked or
/// missing ps would silently produce zero samples and a fake low peak).
const PS: &str = "/bin/ps";
const METRICS: [&str; 9] = [
    "rss-100k",
    "incremental-p95",
    "incremental-p95-resident",
    "token-cost",
    "plugin-overhead",
    "cold",
    "cache-hit-rate",
    "snapshot-size",
    "startup",
];
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

/// Methodology caveat recorded verbatim in every `cold` report (the report
/// must carry its own validity boundary — a ratio read without the note
/// invites small-corpus numbers to be cited as milestone evidence).
const COLD_NOTE: &str = "single cold run per side; at small corpora elapsed \
time is dominated by process boot (cargo/dart VM startup), not analysis \
throughput — cite only large-corpus ratios as milestone evidence";

/// The engine-face driver template (`cache-hit-rate` / `snapshot-size`): the
/// trace sink and the content store are wired only on the `analyze` path
/// (`analyze_plain` in dartq-cli) — `check` bypasses the engine entirely.
const DEFAULT_ANALYZE_CMD: &str = "cargo run -q -p dartq-cli -- analyze --format=machine";
/// The startup driver template: the light CLI face in the json format — the
/// envelope is always emitted even with zero diagnostics, so spawn → first
/// stdout byte is measurable on any corpus (machine writes nothing when
/// clean).
const DEFAULT_STARTUP_CMD: &str = "cargo run -q -p dartq-cli -- check --format=json";
/// `startup` spawns (median + P95 need a distribution); mirrors the
/// plugin-overhead collector's measured-cycle count.
const DEFAULT_STARTUP_RUNS: usize = 50;

/// The verbatim warm-run definition riding every `cache-hit-rate` report.
const WARM_RUN_DEFINITION: &str = "two consecutive `analyze` processes over \
the same corpus sharing one fresh cache root (XDG_CACHE_HOME pinned to an \
empty scratch dir): run 1 untraced (cold, populates the cross-session \
cache), run 2 with --trace-events; the rate aggregates run 2's \
LibraryAnalyzed lines (from_cache=true over all of them)";

/// The verbatim resident-arm definition riding every
/// `incremental-p95-resident` report (the number is meaningless without the
/// run shape and the assembly that produced it).
const RESIDENT_DEFINITION: &str = "one in-process engine that stays warm \
across the whole manifest (the dartq_commands workspace assembly the \
`dartq analyze` face opens: same pub faces, package table, SDK snapshot, \
semdiag diagnoser included), no process restart per step: discovery plus \
one full analyze pass as the timed baseline, then per manifest step the \
edits applied as in-memory overlays (engine set_content/remove_file — the \
daemon edit shape; edits never touch the fixture on disk) immediately \
followed by one timed analyze. Step wall time = overlay application + the \
incremental pass (edit -> diagnostics). The batch sibling \
(incremental-p95) times a full driver process per step over the same \
manifest instead.";

/// The verbatim cache-path definition riding every `snapshot-size` report.
const CACHE_PATH_DEFINITION: &str = "`$XDG_CACHE_HOME/dartq` when \
XDG_CACHE_HOME is set non-empty, else `~/.cache/dartq` \
(dartq_engine::store::default_root, PRD §17.2); the measured run pins \
XDG_CACHE_HOME to an empty scratch dir, so the sum covers exactly one \
analyze run's writes: content-store blobs under <root>/store (the CLI \
analyze face); snapshot generations under <root>/snapshots are written by \
the daemon session face and do not occur in a CLI-only run";

/// The verbosity-tier surface of the faces the token-cost metric drives
/// (#801): `check`/`analyze` machine output is fixed-tier today — the CLI's
/// `-v` lives only on `explain` — so the sweep is the single fixed tier.
/// Real tiers (`minimal`/`compact`/`full`) are swept where a driven face
/// actually accepts them; none of the driven faces do, and no flags are
/// invented to pretend otherwise.
const FIXED_TIERS: [&str; 1] = ["fixed"];

/// The verbatim tier definitions riding every token-cost report (the input
/// set must define its dimensions): the schema's tier table
/// (`dartq_schema::Verbosity`, §15.8) as the CLI spells it, plus the
/// `fixed` marker for the tierless faces this metric drives.
const TIER_DEFINITIONS: &str = "minimal = the red-line floor (schema §15.8): \
locating fields only — a diagnostic's code + location, nothing prose-like; \
compact = the default tier, each producer's baseline shape; full = compact \
plus the cheaply-available extras named per face; `fixed` = the driven face \
(check/analyze machine output) has no -v surface today, so the report's one \
tier and its savings_ratio are the fixed face's";

// ---------------------------------------------------------------------------
// CLI options
// ---------------------------------------------------------------------------

pub(crate) struct BenchOptions {
    /// Selected metrics; empty = all of them, in canonical order.
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
    /// Engine-face driver template for `cache-hit-rate` / `snapshot-size`
    /// (the trace sink and the content store live only on `analyze`).
    analyze_cmd: String,
    /// startup: driver template (the json format guarantees a first stdout
    /// byte even on a clean corpus).
    startup_cmd: String,
    /// startup: measured spawns (median + P95 over the sample set).
    startup_runs: usize,
    /// incremental-p95-resident: the Dart SDK root the in-process engine
    /// analyzes against. Explicit `--sdk` wins; otherwise derived from
    /// `--oracle-dart`'s `<sdk-root>/bin/dart` layout; otherwise
    /// `DART_SDK` (resolved inside the workspace assembly).
    sdk: Option<PathBuf>,
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
            analyze_cmd: DEFAULT_ANALYZE_CMD.to_string(),
            startup_cmd: DEFAULT_STARTUP_CMD.to_string(),
            startup_runs: DEFAULT_STARTUP_RUNS,
            sdk: None,
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
                "--analyze-cmd" => opts.analyze_cmd = crate::value(args, &mut i)?.to_string(),
                "--sdk" => opts.sdk = Some(crate::value(args, &mut i)?.into()),
                "--startup-cmd" => opts.startup_cmd = crate::value(args, &mut i)?.to_string(),
                "--startup-runs" => {
                    opts.startup_runs = crate::value(args, &mut i)?
                        .parse()
                        .ok()
                        .filter(|runs| *runs > 0)
                        .ok_or_else(|| {
                            Outcome::Error("--startup-runs must be an integer > 0".into())
                        })?;
                }
                other => return Err(crate::unknown_flag(other, "bench")),
            }
            i += 1;
        }
        Ok(opts)
    }

    /// The resolved run order: selected metrics in selection order, or all
    /// of them when `--metric` was not given.
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

/// Per-file token counts over machine-format output (#801): stdout split to
/// lines, each line attributed to its 4th `|` field (the file URI — the
/// machine wire shape both CLIs share) and tokenized whole. Per-file numbers
/// sum to slightly more than the whole-stdout total (ceil rounding is not
/// additive); the totals keep their whole-stdout definition so reports stay
/// comparable across schema revisions.
struct FileTokens {
    /// Tokens per file URI, in URI order (BTreeMap: deterministic reports).
    per_file: BTreeMap<String, u64>,
    /// Lines with no file field (fewer than four `|` fields — contract
    /// drift, summary prose): outside the distribution, counted for honesty,
    /// never silently dropped (the ps `malformed_samples` precedent).
    unattributed_lines: u64,
}

fn per_file_tokens(text: &str, tokenizer: &impl Tokenizer) -> FileTokens {
    let mut result = FileTokens {
        per_file: BTreeMap::new(),
        unattributed_lines: 0,
    };
    for line in text.lines() {
        match line.split('|').nth(3) {
            Some(file) => {
                *result.per_file.entry(file.to_string()).or_default() += tokenizer.count(line);
            }
            None => result.unattributed_lines += 1,
        }
    }
    result
}

/// Median + P95 over a per-file token distribution (nearest-rank, so both
/// are actually observed per-file samples); `(0, 0)` when no files.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // u64 token counts <-> f64 percentile domain; nearest-rank results are integral
fn token_distribution(tokens: &FileTokens) -> (u64, u64) {
    let counts: Vec<f64> = tokens
        .per_file
        .values()
        .map(|count| *count as f64)
        .collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // percentile() of non-negative integers returns a non-negative integer
    let at = |p: f64| percentile(counts.clone(), p).map_or(0, |v| v as u64);
    (at(50.0), at(95.0))
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

/// Raw §19.3 event counts over a `--trace-events` capture: (cache hits,
/// `LibraryAnalyzed` lines admitted). Same line admissibility as the
/// engine's [`dartq_engine::event::cache_hit_rate`] — malformed lines and
/// lines without a boolean `from_cache` don't count; the predicate is the
/// §17.4-pinned wire shape, and the round-trip test pins the two faces
/// together (`rate == hits / total`).
fn library_hit_counts(jsonl: &str) -> (u64, u64) {
    jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            let hit = value.get("from_cache")?.as_bool()?;
            value
                .get("event")?
                .as_str()?
                .eq("LibraryAnalyzed")
                .then_some(hit)
        })
        .fold((0u64, 0u64), |(hits, total), hit| {
            (hits + u64::from(hit), total + 1)
        })
}

/// Recursive byte footprint of one directory: `symlink_metadata` per entry
/// (never follow links), files summed, subdirs recursed. The
/// `snapshot-size` walk — stat per file, no `du` subprocess.
struct DirSize {
    bytes: u64,
    files: usize,
}

fn dir_size(dir: &Path) -> Result<DirSize, String> {
    let mut size = DirSize { bytes: 0, files: 0 };
    let entries = fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let meta = entry
            .path()
            .symlink_metadata()
            .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
        if meta.is_dir() {
            let sub = dir_size(&entry.path())?;
            size.bytes += sub.bytes;
            size.files += sub.files;
        } else {
            size.bytes += meta.len();
            size.files += 1;
        }
    }
    Ok(size)
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

/// The SDK root implied by a `dart` binary path: `<sdk-root>/bin/dart` is the
/// stock and fvm layout, so `.../bin/dart` derives `...`. Pure path
/// arithmetic — a bare command name (`dart` on PATH) yields `None`, and a
/// derived root that is not actually an SDK fails loudly downstream
/// (`SdkSnapshot::load` refuses it). Degenerate roots are refused here
/// rather than emitted: `bin/dart` has no parent at all (empty), and
/// `./bin/dart` would derive the cwd-relative `.` — neither is an SDK-root
/// identity. The `--sdk` flag overrides.
fn sdk_root_from_dart_path(dart: &str) -> Option<PathBuf> {
    let bin = Path::new(dart).parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let root = bin.parent()?;
    if root.as_os_str().is_empty() || root == Path::new(".") {
        return None;
    }
    Some(root.to_path_buf())
}

/// The resident arm's SDK resolution: explicit `--sdk`, else the
/// `--oracle-dart` derivation (both faces should analyze against the same
/// SDK), else `None` — the workspace assembly then consults `DART_SDK`
/// itself and its error names both remaining options.
fn resolve_sdk_root(opts: &BenchOptions) -> Option<PathBuf> {
    opts.sdk
        .clone()
        .or_else(|| sdk_root_from_dart_path(&opts.oracle_dart))
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
    /// Wall-clock time of the same run (spawn → exit, output consumed).
    elapsed_ms: f64,
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

/// The resident incremental arm (same step shape and percentiles as
/// [`IncrementalReport`], one warm engine instead of a driver process per
/// step). Separate report under `incremental-p95-resident/` so the batch
/// metric's report shape and the mega ladder's consumers stay untouched.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct IncrementalResidentReport {
    meta: IncrementalResidentMeta,
    /// Discovery plus the first full analyze pass over the fixture, one
    /// clock (the batch arm's baseline times a driver cold start, which
    /// also discovers from scratch).
    baseline_duration_ms: f64,
    steps: Vec<StepTiming>,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct IncrementalResidentMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    epoch_seconds: u64,
    manifest: String,
    steps: usize,
    /// Whether the oracle comparison column was captured.
    with_oracle: bool,
    /// The SDK root the in-process engine analyzed against (empty = the
    /// workspace assembly resolved `DART_SDK` itself).
    sdk_root: String,
    /// The verbatim resident-arm definition ([`RESIDENT_DEFINITION`]).
    resident_definition: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TokenCostReport {
    meta: TokenMeta,
    dartq_output_bytes: u64,
    dartq_tokens: u64,
    /// Median per-file token count over the dartq output's machine lines
    /// grouped by file (nearest-rank, an observed per-file sample); 0 when
    /// no file groups exist. Absent in pre-#801 reports (`serde(default)`).
    #[serde(default)]
    dartq_tokens_median: u64,
    /// P95 over the same per-file distribution.
    #[serde(default)]
    dartq_tokens_p95: u64,
    oracle_output_bytes: u64,
    oracle_tokens: u64,
    #[serde(default)]
    oracle_tokens_median: u64,
    #[serde(default)]
    oracle_tokens_p95: u64,
    /// dartq output lines that matched no file (fewer than four `|` fields):
    /// excluded from the per-file distribution, still inside the whole-stdout
    /// totals.
    #[serde(default)]
    dartq_unattributed_lines: u64,
    #[serde(default)]
    oracle_unattributed_lines: u64,
    /// `1 - dartq_tokens/oracle_tokens`; null when the oracle side is empty
    /// (the ratio is undefined — a vacuous corpus says nothing about volume).
    /// The per-tier savings ratio of the single tier in `meta.tiers`.
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
    /// Verbosity tiers the measured numbers cover (#801): `["fixed"]` while
    /// the driven faces have no `-v` surface. Absent in pre-#801 reports
    /// (`serde(default)` keeps them parseable).
    #[serde(default)]
    tiers: Vec<String>,
    /// The verbatim tier definitions ([`TIER_DEFINITIONS`]).
    #[serde(default)]
    tier_definitions: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ColdReport {
    meta: ColdMeta,
    file_count: usize,
    loc: u64,
    /// One cold dartq run: full-analysis wall time (spawn → exit, output
    /// consumed), process boot included.
    dartq_elapsed_ms: f64,
    /// One cold `dart analyze` over the same corpus, timed the same way.
    oracle_elapsed_ms: f64,
    /// `oracle_elapsed_ms / dartq_elapsed_ms` — the charter's cold face
    /// (≤ 0.5 means dartq cold is at least 2x faster); null when the dartq
    /// side elapsed zero (undefined division; unreachable for a real process
    /// run).
    cold_ratio: Option<f64>,
    /// Methodology caveat recorded verbatim ([`COLD_NOTE`]): single run per
    /// side, process-boot dominated at small corpora.
    note: String,
}

/// Host + oracle-tool descriptor on every `cold` meta — the same fields
/// `tests/bench/oracle_baseline.sh` records in its summary line, probed the
/// same way, so the two instruments' reports read side by side. `Default`
/// backs the `serde(default)` on [`ColdMeta::env`]: the field is additive
/// (#798), pre-descriptor reports stay parseable.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
struct ColdEnv {
    /// The oracle dart's `--version` banner (the script's `dart` field);
    /// null when the probe failed — the times are the payload, a failed
    /// banner does not fail the metric.
    dart_version: Option<String>,
    /// OS + version (macOS: "macOS <sw_vers -productVersion>", the script's
    /// rendering; elsewhere `std::env::consts::OS`).
    os: String,
    /// CPU identifier (macOS: `sysctl -n machdep.cpu.brand_string`; elsewhere
    /// `std::env::consts::ARCH`).
    chip: String,
    /// Logical cores (`hw.ncpu` semantics; 0 when unavailable).
    cores: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ColdMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    dartq_cmd: String,
    epoch_seconds: u64,
    loc_definition: String,
    oracle_cmd: String,
    /// Host + tool descriptor (see [`ColdEnv`]); absent in pre-#798
    /// reports (`serde(default)` keeps them parseable).
    #[serde(default)]
    env: ColdEnv,
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

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct CacheHitRateReport {
    meta: CacheHitRateMeta,
    /// `LibraryAnalyzed` lines admitted in the traced warm run (the §17.4
    /// admissibility: event kind + boolean `from_cache`).
    library_analyzed: u64,
    /// Of which `from_cache=true`.
    library_cache_hits: u64,
    /// [`dartq_engine::event::cache_hit_rate`] over the traced warm run.
    hit_rate: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct CacheHitRateMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    /// The engine-face driver template (the trace sink lives only on
    /// `analyze`).
    analyze_cmd: String,
    epoch_seconds: u64,
    /// The verbatim warm-run definition ([`WARM_RUN_DEFINITION`]) — the
    /// rate is meaningless without the run shape that produced it.
    warm_run_definition: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct SnapshotSizeReport {
    meta: SnapshotSizeMeta,
    /// Whether the cache root existed after the run; `false` = the store
    /// stayed unarmed / wrote nothing — recorded, never faked as a low
    /// sample.
    cache_dir_present: bool,
    file_count: usize,
    total_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct SnapshotSizeMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    analyze_cmd: String,
    epoch_seconds: u64,
    /// The verbatim cache-path definition ([`CACHE_PATH_DEFINITION`]).
    cache_path_definition: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct StartupReport {
    meta: StartupMeta,
    /// One spawn → first-stdout-byte wall time (ms) per spawn, in run order.
    first_byte_ms: Vec<f64>,
    p50_ms: f64,
    p95_ms: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct StartupMeta {
    schema_version: u32,
    metric: String,
    corpus_dir: String,
    startup_cmd: String,
    epoch_seconds: u64,
    runs: usize,
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
/// from blocking a verbose driver). Also carries the run's wall-clock elapsed
/// (the `cold` metric's dartq-side time base).
struct SampledRun {
    status: std::process::ExitStatus,
    stderr: Vec<u8>,
    peak_rss_kib: u64,
    samples: usize,
    malformed_samples: usize,
    /// Wall-clock spawn → exit, stderr consumed (f64 ms, the file's duration
    /// convention).
    elapsed_ms: f64,
}

/// Run `program` to completion while sampling its process-group tree RSS
/// every `interval`: same spawn/kill skeleton as [`crate::oracle::run_captured`]
/// (own process group via setpgid, temp-file capture, wall-clock budget, group
/// kill on timeout) plus the ps polling loop, the 8 GiB watchdog, and the
/// wall-clock elapsed time.
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
    let started = Instant::now();
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
    let stderr = capture.stderr_bytes();
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(SampledRun {
        status,
        stderr,
        peak_rss_kib,
        samples,
        malformed_samples,
        elapsed_ms,
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

/// One captured driver run under an explicit environment overlay (the
/// scratch cache root the `cache-hit-rate` / `snapshot-size` collectors pin
/// via `XDG_CACHE_HOME`): the [`crate::oracle::run_captured`] skeleton (own
/// process group, temp-file capture, wall-clock budget, group kill on
/// timeout) plus `envs`. Stdout is captured and discarded — these metrics
/// read the exit tier, stderr, and on-disk state, not the output face.
struct EnvRun {
    status: std::process::ExitStatus,
    stderr: Vec<u8>,
}

fn run_with_env(
    name: &str,
    program: &str,
    args: &[String],
    envs: &[(&str, &Path)],
    timeout: Duration,
) -> Result<EnvRun, String> {
    use std::process::{Command, Stdio};

    use crate::oracle::{TempCapture, kill_driver};

    let capture = TempCapture::new();
    let stdout = fs::File::create(&capture.stdout_path).map_err(|e| format!("[{name}] {e}"))?;
    let stderr = fs::File::create(&capture.stderr_path).map_err(|e| format!("[{name}] {e}"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    for (key, value) in envs {
        cmd.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("[{name}] failed to launch `{program}`: {e}"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_driver(&mut child);
                    let _ = child.wait();
                    return Err(format!(
                        "[{name}] `{program}` timed out after {}ms and was killed",
                        timeout.as_millis()
                    ));
                }
                std::thread::sleep(crate::oracle::POLL_INTERVAL);
            }
            Err(e) => return Err(format!("[{name}] waiting for `{program}`: {e}")),
        }
    };
    Ok(EnvRun {
        status,
        stderr: capture.stderr_bytes(),
    })
}

/// Spawn → first stdout byte, wall clock. The child runs in its own process
/// group (group kill on timeout), stderr goes to a capture file so a verbose
/// driver can never block on a full pipe, and stdout is a pipe drained by a
/// reader thread that timestamps the first successful read the moment it
/// happens (a byte can sit in the pipe long before this thread gets
/// scheduled). The run is then drained to exit and the exit tier validated —
/// a crashed run never scores.
fn first_stdout_byte_ms(
    name: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<f64, String> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    use crate::oracle::{TempCapture, kill_driver};

    let stderr_capture = TempCapture::new();
    let stderr =
        fs::File::create(&stderr_capture.stderr_path).map_err(|e| format!("[{name}] {e}"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        unsafe {
            cmd.pre_exec(|| {
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("[{name}] failed to launch `{program}`: {e}"))?;
    let stdout = child.stdout.take().expect("stdout was piped");

    // The reader sends exactly once: at the first byte (immediately, so the
    // main thread's timing gate opens mid-run), or `None` at EOF when the
    // driver never wrote a byte.
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut first_byte: Option<Instant> = None;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if first_byte.is_none() {
                        first_byte = Some(Instant::now());
                        let _ = tx.send(first_byte);
                    }
                }
                Err(_) => break,
            }
        }
        if first_byte.is_none() {
            let _ = tx.send(None);
        }
    });

    let deadline = Instant::now() + timeout;
    let first_byte = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            kill_driver(&mut child);
            let _ = child.wait();
            return Err(format!(
                "[{name}] `{program}` produced no stdout within {}ms and was killed",
                timeout.as_millis()
            ));
        }
        match rx.recv_timeout(remaining) {
            Ok(first_byte) => break first_byte,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        }
    };

    // Drain to exit (the budget still applies): validate_exit below refuses
    // to score a crashed run.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_driver(&mut child);
                    let _ = child.wait();
                    return Err(format!(
                        "[{name}] `{program}` did not exit within {}ms after its first \
                         stdout byte and was killed",
                        timeout.as_millis()
                    ));
                }
                std::thread::sleep(crate::oracle::POLL_INTERVAL);
            }
            Err(e) => return Err(format!("[{name}] waiting for `{program}`: {e}")),
        }
    };
    let _ = reader.join();
    let stderr = stderr_capture.stderr_bytes();
    validate_exit(name, program, status, &stderr)?;
    let Some(first_byte) = first_byte else {
        return Err(format!(
            "[{name}] `{program}` exited {status:?} without writing a single stdout byte"
        ));
    };
    Ok(first_byte.duration_since(started).as_secs_f64() * 1000.0)
}

// ---------------------------------------------------------------------------
// Metric collectors
// ---------------------------------------------------------------------------

/// Entry: `harness bench [--metric NAME]... [--corpus-dir DIR] [--out DIR]
/// [--manifest PATH] [--dartq-cmd TEMPLATE] [--oracle-dart PATH]
/// [--rss-interval-ms MS] [--with-oracle] [--analyze-cmd TEMPLATE]
/// [--startup-cmd TEMPLATE] [--startup-runs N]`.
pub(crate) fn bench_entry(args: &[String]) -> Result<(String, bool), Outcome> {
    let opts = BenchOptions::parse(args)?;
    let metrics = opts.resolved_metrics();
    let wants_manifest = metrics
        .iter()
        .any(|m| *m == "incremental-p95" || *m == "incremental-p95-resident");
    if wants_manifest && opts.manifest.is_none() {
        return Err(Outcome::Error(
            "--metric incremental-p95[-resident] requires --manifest PATH (the edit manifest)"
                .into(),
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
            "incremental-p95-resident" => {
                metric_incremental_resident(&opts, corpus.as_ref().expect("preflighted"))?
            }
            "token-cost" => metric_tokens(&opts, corpus.as_ref().expect("preflighted"))?,
            "plugin-overhead" => metric_plugin_overhead(&opts)?,
            "cold" => metric_cold(&opts, corpus.as_ref().expect("preflighted"))?,
            "cache-hit-rate" => {
                metric_cache_hit_rate(&opts, corpus.as_ref().expect("preflighted"))?
            }
            "snapshot-size" => metric_snapshot_size(&opts, corpus.as_ref().expect("preflighted"))?,
            "startup" => metric_startup(&opts, corpus.as_ref().expect("preflighted"))?,
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
        elapsed_ms: run.elapsed_ms,
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

/// `incremental-p95-resident`: the same manifest driven through one
/// in-process engine that stays warm across steps — no process restart, the
/// resident face PRD §1.5's incremental target describes. The engine is the
/// `dartq analyze` face's own workspace assembly (semdiag included); edits
/// land as in-memory overlays (the daemon edit shape) and each step is one
/// timed `analyze` pass ([`RESIDENT_DEFINITION`] rides the report).
fn metric_incremental_resident(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let manifest_path = opts
        .manifest
        .as_ref()
        .expect("bench_entry validates the manifest flag for incremental-p95-resident");
    let text = fs::read_to_string(manifest_path).map_err(|e| {
        Outcome::Error(format!(
            "reading --manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let manifest = parse_manifest(&text)?;

    let fixture = BenchFixture::new("dq_bench_fixture_resident");
    fixture
        .materialize(corpus, &manifest, &opts.oracle_dart)
        .map_err(Outcome::Error)?;
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let sdk_root = resolve_sdk_root(opts);
    // The returned dir is canonicalized by the assembly; edit paths are
    // built from it so overlays address the spellings discovery interned.
    let (mut engine, fixture_dir) =
        dartq_commands::workspace::open_engine(&fixture.dir, sdk_root.as_deref())
            .map_err(|e| Outcome::Error(e.0))?;

    // Baseline: discovery plus the first full pass, one clock.
    let baseline_started = Instant::now();
    engine
        .discover_workspace()
        .map_err(|e| Outcome::Error(format!("discovering the fixture workspace: {e}")))?;
    engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .map_err(|e| Outcome::Error(format!("baseline analyze: {e:?}")))?;
    let baseline_duration_ms = baseline_started.elapsed().as_secs_f64() * 1000.0;

    let mut steps = Vec::with_capacity(manifest.steps.len());
    for (index, step) in manifest.steps.iter().enumerate() {
        let started = Instant::now();
        apply_step_overlays(&mut engine, &fixture_dir, step).map_err(Outcome::Error)?;
        engine
            .analyze(
                &[],
                &mut |_| {},
                &dartq_engine::CancellationToken::default(),
            )
            .map_err(|e| {
                Outcome::Error(format!("incremental analyze (step {}): {e:?}", index + 1))
            })?;
        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        let oracle_duration_ms = if opts.with_oracle {
            Some(
                timed_oracle_run(
                    "bench-incremental-resident-oracle",
                    &opts.oracle_dart,
                    &fixture_dir,
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
    let report = IncrementalResidentReport {
        meta: IncrementalResidentMeta {
            schema_version: SCHEMA_VERSION,
            metric: "incremental-p95-resident".to_string(),
            corpus_dir: corpus.display().to_string(),
            epoch_seconds: epoch_seconds(),
            manifest: manifest_path.display().to_string(),
            steps: steps.len(),
            with_oracle: opts.with_oracle,
            sdk_root: sdk_root
                .as_deref()
                .map_or_else(String::new, |root| root.display().to_string()),
            resident_definition: RESIDENT_DEFINITION.to_string(),
        },
        baseline_duration_ms,
        p50_ms: percentile(durations.clone(), 50.0).unwrap_or_default(),
        p95_ms: percentile(durations.clone(), 95.0).unwrap_or_default(),
        p99_ms: percentile(durations, 99.0).unwrap_or_default(),
        steps,
    };
    let path = write_report("incremental-p95-resident", &opts.out, &report)?;
    Ok(format!(
        "incremental-p95-resident: {} steps | p50 {:.1}ms p95 {:.1}ms p99 {:.1}ms \
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
/// Whole-stdout totals plus the per-file distribution (machine lines grouped
/// by file field, median + P95 per side); the tier dimension is honestly
/// `fixed` — the driven faces accept no `-v` ([`FIXED_TIERS`]).
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

    let dartq_files = per_file_tokens(dartq_text, &tokenizer);
    let oracle_files = per_file_tokens(oracle_text, &tokenizer);
    let (dartq_tokens_median, dartq_tokens_p95) = token_distribution(&dartq_files);
    let (oracle_tokens_median, oracle_tokens_p95) = token_distribution(&oracle_files);

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
            tiers: FIXED_TIERS.iter().map(|tier| tier.to_string()).collect(),
            tier_definitions: TIER_DEFINITIONS.to_string(),
        },
        dartq_output_bytes: dartq_out.stdout.len() as u64,
        dartq_tokens,
        dartq_tokens_median,
        dartq_tokens_p95,
        oracle_output_bytes: oracle_out.stdout.len() as u64,
        oracle_tokens,
        oracle_tokens_median,
        oracle_tokens_p95,
        dartq_unattributed_lines: dartq_files.unattributed_lines,
        oracle_unattributed_lines: oracle_files.unattributed_lines,
        savings_ratio,
    };
    let path = write_report("token-cost", &opts.out, &report)?;
    let savings = report.savings_ratio.map_or_else(
        || "undefined (empty oracle output)".to_string(),
        |r| format!("{:.1}%", r * 100.0),
    );
    Ok(format!(
        "token-cost: dartq {} tok vs oracle {} tok ({}) | per-file median/p95 \
         dartq {}/{}, oracle {}/{} | savings {savings} -> {}",
        report.dartq_tokens,
        report.oracle_tokens,
        tokenizer.id(),
        report.dartq_tokens_median,
        report.dartq_tokens_p95,
        report.oracle_tokens_median,
        report.oracle_tokens_p95,
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
    let expected_rows =
        format!("plugin emitted the wrong number of findings for a {PLUGIN_BATCH_ROWS}-row batch");
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
        .map(|i| files.intern(Path::new(&format!("lib/plugin_overhead_{i}.dart"))))
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

/// Probe the `cold` env descriptor: one `<oracle-dart> --version` spawn plus
/// the host fields oracle_baseline.sh records (macOS sw_vers/sysctl, same
/// sources; elsewhere the std consts). Serial-phase only. A failed sub-probe
/// degrades to a fallback/None field — the descriptor is context, never a
/// reason to fail the metric.
#[cfg(unix)]
fn probe_cold_env(oracle_dart: &str) -> ColdEnv {
    let dart_version = std::process::Command::new(oracle_dart)
        .arg("--version")
        .output()
        .ok()
        .map(|out| {
            let banner = String::from_utf8_lossy(&out.stdout);
            let banner = banner.trim();
            if banner.is_empty() {
                // Older darts banner on stderr; the script captures 2>&1 too.
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            } else {
                banner.to_string()
            }
        })
        .filter(|banner| !banner.is_empty());
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    #[cfg(target_os = "macos")]
    {
        let probe = |program: &str, args: &[&str]| {
            std::process::Command::new(program)
                .args(args)
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let os_version = probe("sw_vers", &["-productVersion"]);
        let chip = probe("sysctl", &["-n", "machdep.cpu.brand_string"]);
        ColdEnv {
            dart_version,
            os: os_version.map_or_else(|| "macos".to_string(), |v| format!("macOS {v}")),
            chip: chip.unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            cores,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        ColdEnv {
            dart_version,
            os: std::env::consts::OS.to_string(),
            chip: std::env::consts::ARCH.to_string(),
            cores,
        }
    }
}

/// `cold`: one cold full-analysis wall time per side over the same corpus —
/// the dartq run through the sampled spawn (watchdog attached, ≥ 1 RSS sample
/// required by [`run_sampled`]), the oracle through the incremental metric's
/// timed-oracle helper — plus the oracle/dartq ratio. One run per side and no
/// warmup: the metric is deliberately the cold-process measurement (see the
/// `note` field on the report before citing ratios).
#[cfg(unix)]
fn metric_cold(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let loc = count_loc(corpus).map_err(Outcome::Error)?;
    let driver = DartqDriver::from_template(&opts.dartq_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;

    let dartq = run_sampled(
        "bench-cold-dartq",
        program,
        &args,
        timeout,
        Duration::from_millis(opts.rss_interval_ms),
    )
    .map_err(Outcome::Error)?;
    validate_exit("bench-cold-dartq", program, dartq.status, &dartq.stderr)
        .map_err(Outcome::Error)?;
    let oracle_elapsed_ms =
        timed_oracle_run("bench-cold-oracle", &opts.oracle_dart, corpus, timeout)
            .map_err(Outcome::Error)?;

    let cold_ratio = (dartq.elapsed_ms > 0.0).then(|| oracle_elapsed_ms / dartq.elapsed_ms);
    let report = ColdReport {
        meta: ColdMeta {
            schema_version: SCHEMA_VERSION,
            metric: "cold".to_string(),
            corpus_dir: corpus.display().to_string(),
            dartq_cmd: opts.dartq_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            loc_definition: LOC_DEFINITION.to_string(),
            oracle_cmd: format!(
                "{} analyze --format=machine --no-fatal-warnings",
                opts.oracle_dart
            ),
            env: probe_cold_env(&opts.oracle_dart),
        },
        file_count: loc.files,
        loc: loc.loc,
        dartq_elapsed_ms: dartq.elapsed_ms,
        oracle_elapsed_ms,
        cold_ratio,
        note: COLD_NOTE.to_string(),
    };
    let path = write_report("cold", &opts.out, &report)?;
    let ratio = report.cold_ratio.map_or_else(
        || "undefined (zero dartq elapsed)".to_string(),
        |r| format!("{r:.2}"),
    );
    Ok(format!(
        "cold: dartq {:.1}ms vs oracle {:.1}ms over {} LOC ({} files) -> \
         oracle/dartq {ratio} -> {}",
        report.dartq_elapsed_ms,
        report.oracle_elapsed_ms,
        report.loc,
        report.file_count,
        path.display()
    ))
}

#[cfg(not(unix))]
fn metric_cold(_opts: &BenchOptions, _corpus: &Path) -> Result<String, Outcome> {
    Err(Outcome::Error(
        "cold sampling requires a POSIX host (process-group ps sampling)".into(),
    ))
}

/// `cache-hit-rate`: PRD §19.3's Cache Hit Rate over the defined warm run —
/// two `analyze` processes over the same corpus sharing one fresh cache
/// root, run 2 traced, the engine's own aggregator scoring the capture. A
/// traced run without a single `LibraryAnalyzed` line fails the metric
/// instead of reporting a vacuous rate.
fn metric_cache_hit_rate(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let scratch = BenchFixture::new("dq_bench_chr");
    let _ = fs::remove_dir_all(&scratch.dir);
    fs::create_dir_all(&scratch.dir)
        .map_err(|e| Outcome::Error(format!("creating {}: {e}", scratch.dir.display())))?;
    let cache_root = scratch.dir.join("cache");
    let driver = DartqDriver::from_template(&opts.analyze_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let envs = [("XDG_CACHE_HOME", cache_root.as_path())];

    let cold =
        run_with_env("bench-chr-cold", program, &args, &envs, timeout).map_err(Outcome::Error)?;
    validate_exit("bench-chr-cold", program, cold.status, &cold.stderr).map_err(Outcome::Error)?;

    let trace_path = scratch.dir.join("trace.jsonl");
    let mut warm_args = args.clone();
    warm_args.push("--trace-events".to_string());
    warm_args.push(trace_path.display().to_string());
    let warm = run_with_env("bench-chr-warm", program, &warm_args, &envs, timeout)
        .map_err(Outcome::Error)?;
    validate_exit("bench-chr-warm", program, warm.status, &warm.stderr).map_err(Outcome::Error)?;

    let jsonl = fs::read_to_string(&trace_path).map_err(|e| {
        Outcome::Error(format!(
            "reading the warm run's trace {}: {e}",
            trace_path.display()
        ))
    })?;
    let hit_rate = dartq_engine::event::cache_hit_rate(&jsonl).ok_or_else(|| {
        Outcome::Error(
            "the traced warm run emitted no LibraryAnalyzed events; nothing to rate".into(),
        )
    })?;
    let (library_cache_hits, library_analyzed) = library_hit_counts(&jsonl);

    let report = CacheHitRateReport {
        meta: CacheHitRateMeta {
            schema_version: SCHEMA_VERSION,
            metric: "cache-hit-rate".to_string(),
            corpus_dir: corpus.display().to_string(),
            analyze_cmd: opts.analyze_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            warm_run_definition: WARM_RUN_DEFINITION.to_string(),
        },
        library_analyzed,
        library_cache_hits,
        hit_rate,
    };
    let path = write_report("cache-hit-rate", &opts.out, &report)?;
    Ok(format!(
        "cache-hit-rate: {:.1}% ({} of {} library lines) over the defined warm run -> {}",
        report.hit_rate * 100.0,
        report.library_cache_hits,
        report.library_analyzed,
        path.display()
    ))
}

/// `snapshot-size`: the bytes one analyze run persists under the cache root.
/// The run's `XDG_CACHE_HOME` is pinned to a fresh scratch dir, so the
/// per-file stat walk sums exactly this run's writes (never pre-existing
/// state, never the user's live cache).
fn metric_snapshot_size(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let scratch = BenchFixture::new("dq_bench_snapsize");
    let _ = fs::remove_dir_all(&scratch.dir);
    fs::create_dir_all(&scratch.dir)
        .map_err(|e| Outcome::Error(format!("creating {}: {e}", scratch.dir.display())))?;
    let cache_root = scratch.dir.join("cache");
    let driver = DartqDriver::from_template(&opts.analyze_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let run = run_with_env(
        "bench-snapsize",
        program,
        &args,
        &[("XDG_CACHE_HOME", cache_root.as_path())],
        timeout,
    )
    .map_err(Outcome::Error)?;
    validate_exit("bench-snapsize", program, run.status, &run.stderr).map_err(Outcome::Error)?;

    let store_dir = cache_root.join("dartq");
    let cache_dir_present = store_dir.is_dir();
    let size = if cache_dir_present {
        dir_size(&store_dir).map_err(Outcome::Error)?
    } else {
        DirSize { bytes: 0, files: 0 }
    };

    let report = SnapshotSizeReport {
        meta: SnapshotSizeMeta {
            schema_version: SCHEMA_VERSION,
            metric: "snapshot-size".to_string(),
            corpus_dir: corpus.display().to_string(),
            analyze_cmd: opts.analyze_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            cache_path_definition: CACHE_PATH_DEFINITION.to_string(),
        },
        cache_dir_present,
        file_count: size.files,
        total_bytes: size.bytes,
    };
    let path = write_report("snapshot-size", &opts.out, &report)?;
    Ok(format!(
        "snapshot-size: {} bytes over {} files (cache root present: {}) -> {}",
        report.total_bytes,
        report.file_count,
        report.cache_dir_present,
        path.display()
    ))
}

/// `startup`: spawn → first stdout byte over N serial spawns of the light
/// CLI face (single instance at a time — one child is reaped before the next
/// spawns). Median + P95 over the sample set.
fn metric_startup(opts: &BenchOptions, corpus: &Path) -> Result<String, Outcome> {
    let driver = DartqDriver::from_template(&opts.startup_cmd);
    let (program, args) = driver.command(corpus);
    let timeout = driver_timeout_from_env().map_err(Outcome::Error)?;
    let mut first_byte_ms = Vec::with_capacity(opts.startup_runs);
    for index in 0..opts.startup_runs {
        let name = format!("bench-startup-{}", index + 1);
        first_byte_ms
            .push(first_stdout_byte_ms(&name, program, &args, timeout).map_err(Outcome::Error)?);
    }
    let p50_ms = percentile(first_byte_ms.clone(), 50.0).unwrap_or_default();
    let p95_ms = percentile(first_byte_ms.clone(), 95.0).unwrap_or_default();

    let report = StartupReport {
        meta: StartupMeta {
            schema_version: SCHEMA_VERSION,
            metric: "startup".to_string(),
            corpus_dir: corpus.display().to_string(),
            startup_cmd: opts.startup_cmd.clone(),
            epoch_seconds: epoch_seconds(),
            runs: opts.startup_runs,
        },
        first_byte_ms,
        p50_ms,
        p95_ms,
    };
    let path = write_report("startup", &opts.out, &report)?;
    Ok(format!(
        "startup: first stdout byte over {} spawns | p50 {p50_ms:.1}ms p95 {p95_ms:.1}ms -> {}",
        report.meta.runs,
        path.display()
    ))
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

/// Apply one manifest step as in-memory overlays (the resident arm): the
/// daemon edit shape — `set_content` / `remove_file`, the fixture on disk
/// stays at its materialized state. Paths are built from the assembly's
/// canonicalized package dir so overlays address the spellings discovery
/// interned (`remove_file` on a differently spelled twin would be an
/// unknown path).
fn apply_step_overlays(
    engine: &mut dartq_engine::Engine,
    fixture_dir: &Path,
    step: &ManifestStep,
) -> Result<(), String> {
    for edit in &step.edits {
        let target = fixture_dir.join(&edit.path);
        if edit.remove {
            engine
                .remove_file(&target)
                .map_err(|e| format!("removing {}: {e}", target.display()))?;
        } else if let Some(content) = &edit.content {
            engine.set_content(&target, content.as_bytes());
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

    // -- per-file token distribution (#801) -----------------------------------

    #[test]
    fn per_file_tokens_groups_machine_lines_by_file_field() {
        let out = concat!(
            "ERROR|SYNTACTIC_ERROR|E1|/c/lib/a.dart|2|11|1|Expected to find ';'.\n",
            "WARNING|STATIC_WARNING|W1|/c/lib/a.dart|3|7|1|The value of 'x' isn't used.\n",
            "INFO|INFO|I1|/c/lib/b.dart|1|1|1|Prefer `const`.\n",
        );
        let files = per_file_tokens(out, &CharsPer4);
        assert_eq!(files.per_file.len(), 2, "lines grouped by the 4th field");
        assert!(
            files.per_file["/c/lib/a.dart"] > files.per_file["/c/lib/b.dart"],
            "a.dart carries the two longer lines"
        );
        assert_eq!(files.unattributed_lines, 0);
    }

    #[test]
    fn per_file_tokens_counts_unattributed_lines() {
        // Fewer than four `|` fields (summary prose, blank lines) matches no
        // file: outside the distribution, accounted in the count.
        let files = per_file_tokens("not machine output\n\nAnalysis completed.\n", &CharsPer4);
        assert!(files.per_file.is_empty());
        assert_eq!(files.unattributed_lines, 3);
    }

    #[test]
    fn token_distribution_median_and_p95_are_observed_file_samples() {
        // Whole-line char counts: a = 14 -> 4 tok, b = 10 -> 3 tok, c = 7 -> 2 tok.
        let out = "E|E|E|a|aaaaaa\nE|E|E|b|aa\nE|E|E|c\n";
        let files = per_file_tokens(out, &CharsPer4);
        let (median, p95) = token_distribution(&files);
        assert_eq!(
            (median, p95),
            (3, 4),
            "nearest-rank over [2, 3, 4]: median = 2nd smallest, P95 (n=3) = max"
        );
        // The empty-output face: no file groups, zeroed distribution.
        assert_eq!(token_distribution(&per_file_tokens("", &CharsPer4)), (0, 0));
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

    // -- §19.3 event counts --------------------------------------------------

    #[test]
    fn library_hit_counts_admits_only_library_lines() {
        // The same capture shape the engine's own aggregator test pins:
        // other event kinds, malformed lines, and library lines without a
        // boolean `from_cache` don't count.
        let capture = concat!(
            "{\"event\":\"OverlayApplied\",\"changed\":1,\"removed\":0}\n",
            "not json at all\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"a\",\"from_cache\":false,\"duration_us\":10}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"b\",\"from_cache\":true,\"duration_us\":0}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"c\",\"from_cache\":true,\"duration_us\":0}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"d\"}\n",
            "{\"event\":\"FileRevalidated\",\"uri\":\"a\",\"gate\":\"parsed\",\"ast_cache_hit\":true}\n",
        );
        assert_eq!(library_hit_counts(capture), (2, 3));
        assert_eq!(library_hit_counts(""), (0, 0));
    }

    #[test]
    fn library_hit_counts_agree_with_the_engine_rate() {
        // One source of truth for the rate's semantics: hits / total from
        // the local counts must equal the engine aggregator's rate on the
        // same capture (the §17.4 wire predicate is shared, this pins it).
        let capture = concat!(
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"a\",\"from_cache\":false,\"duration_us\":1}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"b\",\"from_cache\":true,\"duration_us\":0}\n",
            "{\"event\":\"LibraryAnalyzed\",\"uri\":\"c\",\"from_cache\":true,\"duration_us\":0}\n",
        );
        let (hits, total) = library_hit_counts(capture);
        let rate = dartq_engine::event::cache_hit_rate(capture).expect("library lines exist");
        assert!((rate - hits as f64 / total as f64).abs() < 1e-12);
    }

    // -- directory byte footprint ---------------------------------------------

    #[test]
    fn dir_size_sums_files_recursively() {
        let root = std::env::temp_dir().join(format!("dq_bench_dirsz_{}", std::process::id()));
        write(&root, "a.txt", "0123456789"); // 10 bytes
        write(&root, "sub/b.txt", "abc"); // 3 bytes
        fs::create_dir_all(root.join("empty")).expect("mkdir");

        let size = dir_size(&root).expect("size");
        assert_eq!(size.bytes, 13);
        assert_eq!(size.files, 2, "directories are walked, not counted");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn dir_size_missing_dir_is_an_error() {
        let missing =
            std::env::temp_dir().join(format!("dq_bench_dirsz_absent_{}", std::process::id()));
        let _ = fs::remove_dir_all(&missing);
        assert!(dir_size(&missing).is_err());
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
        assert_eq!(defaults.analyze_cmd, DEFAULT_ANALYZE_CMD);
        assert_eq!(defaults.startup_cmd, DEFAULT_STARTUP_CMD);
        assert_eq!(defaults.startup_runs, DEFAULT_STARTUP_RUNS);
        assert_eq!(defaults.sdk, None);

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
            "--analyze-cmd",
            "dartq analyze",
            "--startup-cmd",
            "dartq check --format=json",
            "--startup-runs",
            "5",
            "--sdk",
            "/sdk/root",
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
        assert_eq!(opts.analyze_cmd, "dartq analyze");
        assert_eq!(opts.startup_cmd, "dartq check --format=json");
        assert_eq!(opts.startup_runs, 5);
        assert_eq!(opts.sdk, Some(PathBuf::from("/sdk/root")));

        assert!(
            BenchOptions::parse(&args(&["--metric", "nope"])).is_err(),
            "unknown metric must error"
        );
        assert!(BenchOptions::parse(&args(&["--rss-interval-ms", "0"])).is_err());
        assert!(BenchOptions::parse(&args(&["--rss-interval-ms", "x"])).is_err());
        assert!(BenchOptions::parse(&args(&["--plugin-cycles", "0"])).is_err());
        assert!(BenchOptions::parse(&args(&["--plugin-cycles", "x"])).is_err());
        assert!(BenchOptions::parse(&args(&["--startup-runs", "0"])).is_err());
        assert!(BenchOptions::parse(&args(&["--startup-runs", "x"])).is_err());
        assert!(matches!(
            BenchOptions::parse(&args(&["--bogus"])),
            Err(Outcome::Error(msg)) if msg.contains("bench")
        ));

        // incremental without a manifest is rejected before any corpus access
        assert!(matches!(
            bench_entry(&args(&["--corpus-dir", "/nonexistent", "--metric", "incremental-p95"])),
            Err(Outcome::Error(msg)) if msg.contains("--manifest")
        ));
        assert!(matches!(
            bench_entry(&args(&[
                "--corpus-dir",
                "/nonexistent",
                "--metric",
                "incremental-p95-resident"
            ])),
            Err(Outcome::Error(msg)) if msg.contains("--manifest")
        ));
    }

    // -- resident arm: SDK resolution ----------------------------------------

    #[test]
    fn sdk_root_derives_from_bin_dart_layout_only() {
        // The stock and fvm layouts: <sdk-root>/bin/dart.
        assert_eq!(
            sdk_root_from_dart_path("/home/u/fvm/default/bin/dart"),
            Some(PathBuf::from("/home/u/fvm/default"))
        );
        assert_eq!(
            sdk_root_from_dart_path("/usr/lib/dart/bin/dart"),
            Some(PathBuf::from("/usr/lib/dart"))
        );
        // A bare command name (no path) derives nothing — DART_SDK or --sdk
        // must speak up instead.
        assert_eq!(sdk_root_from_dart_path("dart"), None);
        assert_eq!(sdk_root_from_dart_path(""), None);
        // Not under bin/: no derivation.
        assert_eq!(sdk_root_from_dart_path("/some/dart"), None);
        // Degenerate roots: "bin/dart" has no parent (empty), "./bin/dart"
        // derives the cwd-relative "." — neither is an SDK-root identity.
        assert_eq!(sdk_root_from_dart_path("bin/dart"), None);
        assert_eq!(sdk_root_from_dart_path("./bin/dart"), None);
    }

    #[test]
    fn sdk_root_resolution_prefers_explicit_flag_over_derivation() {
        let mut opts = BenchOptions::parse(&args(&[
            "--oracle-dart",
            "/fvm/default/bin/dart",
            "--sdk",
            "/explicit/sdk",
        ]))
        .expect("flags");
        assert_eq!(
            resolve_sdk_root(&opts),
            Some(PathBuf::from("/explicit/sdk")),
            "--sdk wins over the --oracle-dart derivation"
        );
        // No --sdk: derive from the oracle dart's layout.
        opts.sdk = None;
        assert_eq!(resolve_sdk_root(&opts), Some(PathBuf::from("/fvm/default")));
        // Neither: None — the workspace assembly consults DART_SDK itself.
        opts.oracle_dart = "dart".to_string();
        assert_eq!(resolve_sdk_root(&opts), None);
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
            elapsed_ms: 812.5,
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
                tiers: FIXED_TIERS.iter().map(|tier| tier.to_string()).collect(),
                tier_definitions: TIER_DEFINITIONS.to_string(),
            },
            dartq_output_bytes: 10,
            dartq_tokens: 3,
            dartq_tokens_median: 2,
            dartq_tokens_p95: 3,
            oracle_output_bytes: 0,
            oracle_tokens: 0,
            oracle_tokens_median: 0,
            oracle_tokens_p95: 0,
            dartq_unattributed_lines: 1,
            oracle_unattributed_lines: 0,
            savings_ratio: None,
        };
        let cold = ColdReport {
            meta: ColdMeta {
                schema_version: SCHEMA_VERSION,
                metric: "cold".to_string(),
                corpus_dir: "/corpus".to_string(),
                dartq_cmd: "dartq check".to_string(),
                epoch_seconds: 1_700_000_000,
                loc_definition: LOC_DEFINITION.to_string(),
                oracle_cmd: "dart analyze --format=machine --no-fatal-warnings".to_string(),
                env: ColdEnv {
                    dart_version: Some("Dart SDK version: 3.13.1 (stable)".to_string()),
                    os: "macOS 15.6".to_string(),
                    chip: "Apple M1 Pro".to_string(),
                    cores: 10,
                },
            },
            file_count: 2,
            loc: 100_000,
            dartq_elapsed_ms: 40.0,
            oracle_elapsed_ms: 120.0,
            cold_ratio: Some(3.0),
            note: COLD_NOTE.to_string(),
        };

        let rss_path = write_report("rss-100k", &out, &rss).expect("rss report");
        let inc_path = write_report("incremental-p95", &out, &incremental).expect("inc report");
        let tok_path = write_report("token-cost", &out, &tokens).expect("tok report");
        let cold_path = write_report("cold", &out, &cold).expect("cold report");
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
        assert_eq!(value["elapsed_ms"], 812.5, "the run's wall clock rides too");

        // incremental-p95: typed round trip + the oracle column's null face
        let text = fs::read_to_string(&inc_path).expect("report file");
        let parsed: IncrementalReport = serde_json::from_str(&text).expect("inc parse");
        assert_eq!(parsed, incremental);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["steps"][0]["oracle_duration_ms"], 900.25);
        assert!(value["steps"][1]["oracle_duration_ms"].is_null());

        // token-cost: typed round trip + the recorded tokenizer, the honest
        // fixed-tier dimension, and the per-file distribution fields
        let text = fs::read_to_string(&tok_path).expect("report file");
        let parsed: TokenCostReport = serde_json::from_str(&text).expect("tok parse");
        assert_eq!(parsed, tokens);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["tokenizer"], "chars-per-4");
        assert_eq!(
            value["meta"]["tiers"],
            serde_json::json!(["fixed"]),
            "the driven faces are fixed-tier; no flags invented"
        );
        assert_eq!(value["meta"]["tier_definitions"], TIER_DEFINITIONS);
        assert_eq!(value["dartq_tokens_median"], 2);
        assert_eq!(value["dartq_tokens_p95"], 3);
        assert_eq!(value["dartq_unattributed_lines"], 1);
        assert!(
            value["savings_ratio"].is_null(),
            "empty oracle output -> undefined ratio"
        );

        // cold: typed round trip + the methodology note riding the report
        let text = fs::read_to_string(&cold_path).expect("report file");
        let parsed: ColdReport = serde_json::from_str(&text).expect("cold parse");
        assert_eq!(parsed, cold);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["env"]["cores"], 10, "env descriptor rides");
        assert_eq!(value["cold_ratio"], 3.0);
        assert_eq!(value["note"], COLD_NOTE);

        fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn cold_report_parses_without_env_descriptor() {
        // Pre-#798 cold reports carry no meta.env; the field is additive
        // (serde default), so existing reports must stay parseable.
        let legacy = r#"{"meta":{"schema_version":1,"metric":"cold",
            "corpus_dir":"/corpus","dartq_cmd":"dartq check","epoch_seconds":1,
            "loc_definition":"x","oracle_cmd":"dart analyze --format=machine"},
            "file_count":1,"loc":1,"dartq_elapsed_ms":1.0,"oracle_elapsed_ms":2.0,
            "cold_ratio":2.0,"note":"n"}"#;
        let parsed: ColdReport = serde_json::from_str(legacy).expect("legacy cold parse");
        assert_eq!(parsed.meta.env, ColdEnv::default());
        assert!(parsed.meta.env.dart_version.is_none());
    }

    #[test]
    fn token_cost_report_parses_without_tier_and_distribution_fields() {
        // Pre-#801 reports carry no tier dimension and no per-file
        // distribution; every new field is additive (`serde(default)`), so
        // existing reports must stay parseable.
        let legacy = r#"{"meta":{"schema_version":1,"metric":"token-cost",
            "corpus_dir":"/corpus","dartq_cmd":"dartq check","epoch_seconds":1,
            "tokenizer":"chars-per-4","oracle_cmd":"dart analyze --format=machine"},
            "dartq_output_bytes":10,"dartq_tokens":3,"oracle_output_bytes":6,
            "oracle_tokens":2,"savings_ratio":-0.5}"#;
        let parsed: TokenCostReport = serde_json::from_str(legacy).expect("legacy token parse");
        assert!(parsed.meta.tiers.is_empty());
        assert_eq!(parsed.meta.tier_definitions, "");
        assert_eq!(parsed.dartq_tokens_median, 0);
        assert_eq!(parsed.oracle_tokens_p95, 0);
        assert_eq!(parsed.dartq_unattributed_lines, 0);
        assert_eq!(parsed.savings_ratio, Some(-0.5));
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

    #[test]
    fn new_metric_reports_round_trip_through_disk() {
        let out = std::env::temp_dir().join(format!("dq_bench_new_{}", std::process::id()));
        let hit_rate = CacheHitRateReport {
            meta: CacheHitRateMeta {
                schema_version: SCHEMA_VERSION,
                metric: "cache-hit-rate".to_string(),
                corpus_dir: "/corpus".to_string(),
                analyze_cmd: DEFAULT_ANALYZE_CMD.to_string(),
                epoch_seconds: 1_700_000_000,
                warm_run_definition: WARM_RUN_DEFINITION.to_string(),
            },
            library_analyzed: 3,
            library_cache_hits: 2,
            hit_rate: 2.0 / 3.0,
        };
        let snapshot = SnapshotSizeReport {
            meta: SnapshotSizeMeta {
                schema_version: SCHEMA_VERSION,
                metric: "snapshot-size".to_string(),
                corpus_dir: "/corpus".to_string(),
                analyze_cmd: DEFAULT_ANALYZE_CMD.to_string(),
                epoch_seconds: 1_700_000_000,
                cache_path_definition: CACHE_PATH_DEFINITION.to_string(),
            },
            cache_dir_present: false,
            file_count: 0,
            total_bytes: 0,
        };
        let startup = StartupReport {
            meta: StartupMeta {
                schema_version: SCHEMA_VERSION,
                metric: "startup".to_string(),
                corpus_dir: "/corpus".to_string(),
                startup_cmd: DEFAULT_STARTUP_CMD.to_string(),
                epoch_seconds: 1_700_000_000,
                runs: 3,
            },
            first_byte_ms: vec![1.5, 2.5, 9.0],
            p50_ms: 2.5,
            p95_ms: 9.0,
        };
        let resident = IncrementalResidentReport {
            meta: IncrementalResidentMeta {
                schema_version: SCHEMA_VERSION,
                metric: "incremental-p95-resident".to_string(),
                corpus_dir: "/corpus".to_string(),
                epoch_seconds: 1_700_000_000,
                manifest: "m.json".to_string(),
                steps: 2,
                with_oracle: true,
                sdk_root: "/fvm/default".to_string(),
                resident_definition: RESIDENT_DEFINITION.to_string(),
            },
            baseline_duration_ms: 90.0,
            steps: vec![
                StepTiming {
                    index: 1,
                    duration_ms: 3.5,
                    oracle_duration_ms: Some(880.0),
                },
                StepTiming {
                    index: 2,
                    duration_ms: 2.25,
                    oracle_duration_ms: None,
                },
            ],
            p50_ms: 3.5,
            p95_ms: 3.5,
            p99_ms: 3.5,
        };

        let rate_path = write_report("cache-hit-rate", &out, &hit_rate).expect("rate report");
        let size_path = write_report("snapshot-size", &out, &snapshot).expect("size report");
        let startup_path = write_report("startup", &out, &startup).expect("startup report");
        let resident_path =
            write_report("incremental-p95-resident", &out, &resident).expect("resident report");

        // cache-hit-rate: typed round trip + the warm-run definition riding
        // the input set.
        let text = fs::read_to_string(&rate_path).expect("report file");
        let parsed: CacheHitRateReport = serde_json::from_str(&text).expect("rate parse");
        assert_eq!(parsed, hit_rate);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["warm_run_definition"], WARM_RUN_DEFINITION);
        assert_eq!(value["hit_rate"], 2.0 / 3.0);

        // snapshot-size: typed round trip + the path definition riding the
        // input set; the unarmed-store face (absent dir) is representable.
        let text = fs::read_to_string(&size_path).expect("report file");
        let parsed: SnapshotSizeReport = serde_json::from_str(&text).expect("size parse");
        assert_eq!(parsed, snapshot);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            value["meta"]["cache_path_definition"],
            CACHE_PATH_DEFINITION
        );
        assert_eq!(value["cache_dir_present"], false);

        // startup: typed round trip + the per-spawn sample set.
        let text = fs::read_to_string(&startup_path).expect("report file");
        let parsed: StartupReport = serde_json::from_str(&text).expect("startup parse");
        assert_eq!(parsed, startup);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["first_byte_ms"].as_array().map(Vec::len), Some(3));
        assert_eq!(value["p95_ms"], 9.0);

        // incremental-p95-resident: typed round trip + the resident-arm
        // definition and SDK root riding the input set.
        let text = fs::read_to_string(&resident_path).expect("report file");
        let parsed: IncrementalResidentReport = serde_json::from_str(&text).expect("res parse");
        assert_eq!(parsed, resident);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["meta"]["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["meta"]["sdk_root"], "/fvm/default");
        assert_eq!(value["meta"]["resident_definition"], RESIDENT_DEFINITION);
        assert_eq!(value["steps"][0]["oracle_duration_ms"], 880.0);
        assert!(value["steps"][1]["oracle_duration_ms"].is_null());

        fs::remove_dir_all(&out).ok();
    }
}
