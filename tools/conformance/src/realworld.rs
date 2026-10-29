//! `conformance realworld`: release gate 4 over real packages straight from
//! the local pub cache (never vendored). The package list is
//! `conformance/realworld.toml`; each entry pins one package at an exact
//! version. For every package the oracle (`dart analyze --format=json`) and
//! dartq (`analyze --cold --format=json --sdk-path <sdk>`) each run once over
//! the package directory under the shared watchdog, the diagnostics are
//! attributed to files relative to the package dir (rows outside it are
//! dep-graph noise and are dropped, the same class of filter the chunked
//! runner applies), and the full canonical rows are compared.
//!
//! Baseline: the tracked `conformance/realworld.tsv` records the current
//! numbers per package. A plain run compares against it and enforces the
//! gate rule — no new dartq-only row versus the stored baseline. The tracked
//! artifact stores counts and jaccard, so "no new dartq-only row" is
//! enforced as monotonicity: the extra count may not grow, and the row-set
//! jaccard may not drop (a dartq-only row swapped for another keeps the
//! count but lowers the jaccard). Regressions fail; improvements and other
//! shifts are reported and only recorded through the explicit
//! `--update-baseline` flag; `--baseline` force-records (bootstrap or
//! deliberate re-record). A plain run never writes the file.

use crate::analyze_json;
use crate::rows::{Row, sort_rows};
use crate::runner;
use crate::watchdog;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const LIST: &str = "conformance/realworld.toml";
pub const OUT: &str = "conformance/realworld.tsv";
pub const DIFF_ROOT: &str = "target/conformance/realworld";
pub const HEADER: &str =
    "package\tcases\toracle_rows\tdartq_rows\tmissing\textra\tmessage_only\tjaccard\tverdict";

/// One analyzer process per side per package; the watchdog kills a hung or
/// oversized run instead of stalling the sweep.
const TIMEOUT: Duration = Duration::from_secs(600);

/// Jaccard is stored rounded to 4 decimals; baseline comparisons tolerate
/// that storage precision so rounding alone never reads as a regression.
const JACCARD_EPS: f64 = 1e-4;

pub struct RealworldArgs {
    /// The dartq binary to test.
    pub dartq_bin: PathBuf,
    /// The released SDK root: provides the oracle `bin/dart` and dartq's
    /// `--sdk-path`.
    pub oracle_sdk: PathBuf,
    /// Only run this one package from the list.
    pub package: Option<String>,
    /// Force-record the current numbers as the stored baseline.
    pub baseline: bool,
    /// Record the current numbers, refusing when any package regressed.
    pub update_baseline: bool,
    pub list_path: PathBuf,
    pub out_path: PathBuf,
    pub diff_root: PathBuf,
}

pub fn realworld_command(args: &RealworldArgs) -> i32 {
    let started = std::time::Instant::now();
    if !args.dartq_bin.is_file() {
        eprintln!("error: dartq binary {} not found", args.dartq_bin.display());
        return 2;
    }
    let dart_bin = args.oracle_sdk.join("bin").join("dart");
    if !dart_bin.is_file() {
        eprintln!("error: no `bin/dart` under {}", args.oracle_sdk.display());
        return 2;
    }
    let sdk_version = match dart_version(&dart_bin) {
        Ok(version) => version,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let list_text = match std::fs::read_to_string(&args.list_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", args.list_path.display());
            return 2;
        }
    };
    let packages = match parse_packages(&list_text) {
        Ok(packages) => packages,
        Err(e) => {
            eprintln!("error: {}: {e}", args.list_path.display());
            return 2;
        }
    };
    let selected: Vec<&(String, String)> = match &args.package {
        Some(name) => match packages.iter().find(|(n, _)| n == name) {
            Some(entry) => vec![entry],
            None => {
                eprintln!(
                    "error: package {name} is not in {}; known packages: {}",
                    args.list_path.display(),
                    packages
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return 2;
            }
        },
        None => packages.iter().collect(),
    };

    let cache = match pub_cache_dir() {
        Ok(cache) => cache,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (name, version) in selected {
        match resolve_package(&cache, name, version) {
            Ok(dir) => dirs.push((name.clone(), dir)),
            Err(e) => {
                // An explicitly requested package that is absent is a usage
                // error; a sweep skips it explicitly and carries on.
                if args.package.is_some() {
                    eprintln!("error: {e}");
                    return 2;
                }
                eprintln!("realworld: {name} SKIP: {e}");
                skipped.push(name.clone());
            }
        }
    }
    if dirs.is_empty() {
        eprintln!(
            "realworld: nothing measured — every selected package is absent from the pub cache (explicit skip)"
        );
        return 77;
    }
    eprintln!(
        "realworld: oracle SDK reports {sdk_version}; {} package(s) to measure",
        dirs.len()
    );

    let stored = match load_baseline(&args.out_path) {
        Ok(stored) => stored,
        Err(e) => {
            eprintln!("error: {}: {e}", args.out_path.display());
            return 2;
        }
    };
    let recording = args.baseline || args.update_baseline;

    let mut fail_n = 0usize;
    let mut pass_n = 0usize;
    let mut diff_n = 0usize;
    let mut no_baseline: Vec<String> = Vec::new();
    let mut crashes: Vec<String> = Vec::new();
    let mut results: Vec<(String, PackageDiff, String)> = Vec::new();

    for (name, dir) in &dirs {
        let canonical = match std::fs::canonicalize(dir) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("error: cannot canonicalize {}: {e}", dir.display());
                return 2;
            }
        };
        // The oracle failing is an environment problem, not a dartq verdict.
        let oracle_diags =
            match run_analyzer(&mut runner::oracle_command(&dart_bin, &canonical), "oracle") {
                Ok(diags) => diags,
                Err(e) => {
                    eprintln!("error: the oracle failed on {name}: {e}");
                    return 2;
                }
            };
        let (oracle_rows, oracle_outside) = attribute(&oracle_diags, &canonical);
        if oracle_outside > 0 {
            warn(&format!(
                "{name}: {oracle_outside} oracle diagnostics outside the package dir were dropped"
            ));
        }
        let diff = match run_analyzer(
            &mut runner::dartq_command(&args.dartq_bin, Some(&args.oracle_sdk), &canonical),
            "dartq",
        ) {
            Ok(diags) => {
                let (dartq_rows, dartq_outside) = attribute(&diags, &canonical);
                if dartq_outside > 0 {
                    warn(&format!(
                        "{name}: {dartq_outside} dartq diagnostics outside the package dir were dropped"
                    ));
                }
                compare(&oracle_rows, &dartq_rows)
            }
            Err(e) => {
                eprintln!("realworld: {name} FAIL: dartq crashed: {e}");
                crashes.push(name.clone());
                PackageDiff::crashed(&oracle_rows, e)
            }
        };

        let verdict = if diff.crash.is_some() {
            fail_n += 1;
            "fail".to_string()
        } else if recording {
            let verdict = if diff.is_green() { "pass" } else { "diff" };
            match verdict {
                "pass" => pass_n += 1,
                _ => diff_n += 1,
            }
            verdict.to_string()
        } else {
            match stored.get(name).filter(|row| row.has_numbers()) {
                Some(baseline) => match gate_check(baseline, &diff) {
                    Err(reason) => {
                        fail_n += 1;
                        eprintln!("realworld: {name} FAIL: {reason}");
                        "fail".to_string()
                    }
                    Ok(()) if diff.is_green() => {
                        pass_n += 1;
                        eprintln!("realworld: {name} PASS{}", diff.stats_line());
                        "pass".to_string()
                    }
                    Ok(()) => {
                        diff_n += 1;
                        let hint = if *baseline == BaselineRow::recorded(name, &diff, "diff") {
                            " (unchanged divergence)".to_string()
                        } else {
                            " — run with --update-baseline to record".to_string()
                        };
                        eprintln!("realworld: {name} DIFF{}{hint}", diff.stats_line());
                        "diff".to_string()
                    }
                },
                None => {
                    no_baseline.push(name.clone());
                    "no-baseline".to_string()
                }
            }
        };
        results.push((name.clone(), diff, verdict));
    }

    if recording {
        if !crashes.is_empty() || (args.update_baseline && fail_n > 0) {
            eprintln!(
                "error: baseline not recorded: {}",
                if crashes.is_empty() {
                    format!(
                        "{} package(s) regressed against the stored baseline",
                        fail_n
                    )
                } else {
                    format!("dartq crashed on: {}", crashes.join(", "))
                }
            );
            return 1;
        }
        let merged = merge_baseline(&packages, &stored, &skipped, &results);
        if let Err(e) = std::fs::write(&args.out_path, render_baseline(&merged)) {
            eprintln!("error: cannot write {}: {e}", args.out_path.display());
            return 2;
        }
    } else if !no_baseline.is_empty() {
        eprintln!(
            "error: no stored baseline for: {}; record one with --baseline first",
            no_baseline.join(", ")
        );
        return 2;
    }

    if let Err(e) = std::fs::create_dir_all(&args.diff_root) {
        eprintln!("error: cannot create {}: {e}", args.diff_root.display());
        return 2;
    }
    for (name, diff, _) in &results {
        let path = args.diff_root.join(format!("{name}.diff"));
        if let Err(e) = std::fs::write(&path, render_diff(name, diff)) {
            eprintln!("error: cannot write {}: {e}", path.display());
            return 2;
        }
    }

    let _ = writeln!(
        std::io::stderr().lock(),
        "realworld: {} measured ({} pass, {} divergent, {} failed), {} skipped in {:.1}s{}",
        results.len(),
        pass_n,
        diff_n,
        fail_n,
        skipped.len(),
        started.elapsed().as_secs_f64(),
        if recording {
            format!("; baseline written to {}", args.out_path.display())
        } else {
            String::new()
        }
    );
    i32::from(fail_n > 0)
}

/// The gate rule over the stored baseline: no new dartq-only row. See the
/// module docs for why this is enforced as extra-count and jaccard
/// monotonicity.
fn gate_check(stored: &BaselineRow, current: &PackageDiff) -> Result<(), String> {
    if current.extra > stored.extra {
        return Err(format!(
            "new dartq-only rows: extra {} > baseline {}",
            current.extra, stored.extra
        ));
    }
    match stored.jaccard {
        Some(baseline_j) if current.jaccard + JACCARD_EPS < baseline_j => Err(format!(
            "jaccard dropped below the baseline: {:.4} < {:.4} (a dartq-only row changed)",
            current.jaccard, baseline_j
        )),
        _ => Ok(()),
    }
}

// ---- package list --------------------------------------------------------

/// Parses the `[packages]` section of the realworld list: `name = "version"`
/// lines, returned sorted by name. Other sections and comments are ignored;
/// a malformed entry inside `[packages]` is an error so a broken tracked
/// edit cannot silently shrink the sweep.
fn parse_packages(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut in_packages = false;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_packages = line == "[packages]";
            continue;
        }
        if !in_packages {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "malformed entry (expected name = \"version\"): {line}"
            ));
        };
        let name = name.trim();
        let value = value.trim();
        let version = if let Some(rest) = value.strip_prefix('"') {
            let Some(end) = rest.find('"') else {
                return Err(format!("malformed entry (unterminated quote): {line}"));
            };
            &rest[..end]
        } else {
            // Bare value: an inline comment runs from the first `#`.
            value.split('#').next().unwrap_or("").trim()
        };
        if name.is_empty() || version.is_empty() {
            return Err(format!("malformed entry (empty name or version): {line}"));
        }
        if map.insert(name.to_string(), version.to_string()).is_some() {
            return Err(format!("duplicate package: {name}"));
        }
    }
    if map.is_empty() {
        return Err("no entries under [packages]".to_string());
    }
    Ok(map.into_iter().collect())
}

// ---- pub cache -----------------------------------------------------------

/// The local pub cache root: `$PUB_CACHE` when set (pub's own override),
/// else `$HOME/.pub-cache`.
fn pub_cache_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("PUB_CACHE") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").map_err(|_| {
        "cannot locate the pub cache: neither $PUB_CACHE nor $HOME is set".to_string()
    })?;
    Ok(PathBuf::from(home).join(".pub-cache"))
}

/// The on-disk directory of one pinned package version, or the fix when it
/// is not in the local cache.
fn resolve_package(cache: &Path, name: &str, version: &str) -> Result<PathBuf, String> {
    let dir = cache
        .join("hosted")
        .join("pub.dev")
        .join(format!("{name}-{version}"));
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!(
            "{} is absent from the local pub cache; fetch it with: dart pub cache add {name} --version {version}",
            dir.display()
        ))
    }
}

// ---- analysis ------------------------------------------------------------

/// Runs one analyzer process under the watchdog and parses its JSON
/// envelope. A watchdog kill or an output stream without any envelope is an
/// error; the stderr tail rides along for diagnosis.
fn run_analyzer(command: &mut Command, label: &str) -> Result<Vec<analyze_json::Diag>, String> {
    let result = watchdog::run(command, TIMEOUT, watchdog::RSS_LIMIT_KB)
        .map_err(|e| format!("cannot spawn {label}: {e}"))?;
    if result.killed {
        return Err(format!(
            "{label} was killed by the watchdog (RSS cap or {}s timeout)",
            TIMEOUT.as_secs()
        ));
    }
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let diags = analyze_json::parse_output(&stdout);
    let envelope = stdout
        .lines()
        .any(|line| line.trim_start().starts_with("{\"version\""));
    if diags.is_empty() && !envelope {
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        let tail: String = {
            let start = stderr.len().saturating_sub(300);
            stderr[start..].replace('\n', " | ")
        };
        return Err(format!(
            "{label} produced no diagnostics envelope; stderr tail: {tail}"
        ));
    }
    Ok(diags)
}

/// Canonical rows for one package: diagnostics under the package dir become
/// rows with package-relative files and `<pkg>` in place of the absolute
/// root inside messages (both sides share the root, so the rows stay
/// comparable and machine-independent); anything outside is dep-graph noise
/// and is counted and dropped — the same class of filter the chunked runner
/// applies to rows outside a chunk's package roots.
fn attribute(diags: &[analyze_json::Diag], pkg_dir: &Path) -> (Vec<Row>, usize) {
    let root = pkg_dir.to_string_lossy().into_owned();
    let mut rows = Vec::new();
    let mut outside = 0usize;
    for diag in diags {
        let Ok(rel) = Path::new(&diag.file).strip_prefix(pkg_dir) else {
            outside += 1;
            continue;
        };
        let comps: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if comps.is_empty() {
            outside += 1;
            continue;
        }
        rows.push(Row {
            severity: diag.severity.clone(),
            code: diag.code.clone(),
            file: comps.join("/"),
            line: diag.line,
            column: diag.column,
            length: diag.length,
            message: diag.message.replace(root.as_str(), "<pkg>"),
        });
    }
    sort_rows(&mut rows);
    (rows, outside)
}

// ---- comparison ----------------------------------------------------------

/// One package's oracle-vs-dartq comparison, decomposed the way the tsv
/// reports it: rows whose key (`severity|code|file|line:col`) is unmatched
/// on the other side are missing/extra; key-matched rows differing in
/// message or length are message_only pairs; jaccard runs over the full row
/// text sets.
#[derive(Default, Debug, PartialEq, Clone)]
struct PackageDiff {
    files: usize,
    oracle_rows: usize,
    dartq_rows: usize,
    missing: usize,
    extra: usize,
    message_only: usize,
    jaccard: f64,
    missing_rows: Vec<String>,
    extra_rows: Vec<String>,
    message_pairs: Vec<(String, String)>,
    crash: Option<String>,
}

impl PackageDiff {
    fn is_green(&self) -> bool {
        self.crash.is_none() && self.missing == 0 && self.extra == 0 && self.message_only == 0
    }

    /// A dartq crash: every oracle row is unreported and the package can
    /// never look green (jaccard pins to 0 even on a clean oracle side).
    fn crashed(oracle: &[Row], detail: String) -> PackageDiff {
        PackageDiff {
            files: oracle
                .iter()
                .map(|r| r.file.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            oracle_rows: oracle.len(),
            missing: oracle.len(),
            jaccard: 0.0,
            missing_rows: oracle.iter().map(Row::text).collect(),
            crash: Some(detail),
            ..PackageDiff::default()
        }
    }

    fn stats_line(&self) -> String {
        format!(
            " files={} oracle={} dartq={} missing={} extra={} message_only={} jaccard={:.4}",
            self.files,
            self.oracle_rows,
            self.dartq_rows,
            self.missing,
            self.extra,
            self.message_only,
            self.jaccard
        )
    }
}

/// Set jaccard over the full row texts; two empty sides are identical (1.0).
fn jaccard(oracle: &[Row], dartq: &[Row]) -> f64 {
    let oracle_texts: BTreeSet<String> = oracle.iter().map(Row::text).collect();
    let dartq_texts: BTreeSet<String> = dartq.iter().map(Row::text).collect();
    let union = oracle_texts.union(&dartq_texts).count();
    if union == 0 {
        return 1.0;
    }
    oracle_texts.intersection(&dartq_texts).count() as f64 / union as f64
}

fn compare(oracle: &[Row], dartq: &[Row]) -> PackageDiff {
    // Group the full row texts by key; within a key, exact text matches
    // cancel and the leftovers pair up as message_only, with any unpaired
    // tail counting as missing/extra.
    let mut groups: BTreeMap<(String, String, String, u32, u32), (Vec<String>, Vec<String>)> =
        BTreeMap::new();
    for row in oracle {
        groups.entry(row.key()).or_default().0.push(row.text());
    }
    for row in dartq {
        groups.entry(row.key()).or_default().1.push(row.text());
    }
    let mut diff = PackageDiff {
        files: oracle
            .iter()
            .chain(dartq)
            .map(|r| r.file.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        oracle_rows: oracle.len(),
        dartq_rows: dartq.len(),
        jaccard: jaccard(oracle, dartq),
        ..PackageDiff::default()
    };
    for (_, (mut oracle_texts, mut dartq_texts)) in groups {
        oracle_texts.sort();
        dartq_texts.sort();
        let mut oracle_left: Vec<String> = Vec::new();
        let mut dartq_left: Vec<String> = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < oracle_texts.len() && j < dartq_texts.len() {
            match oracle_texts[i].cmp(&dartq_texts[j]) {
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => {
                    oracle_left.push(oracle_texts[i].clone());
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    dartq_left.push(dartq_texts[j].clone());
                    j += 1;
                }
            }
        }
        oracle_left.extend(oracle_texts[i..].iter().cloned());
        dartq_left.extend(dartq_texts[j..].iter().cloned());
        let pairs = oracle_left.len().min(dartq_left.len());
        diff.message_only += pairs;
        diff.message_pairs.extend(
            oracle_left[..pairs]
                .iter()
                .zip(dartq_left[..pairs].iter())
                .map(|(o, d)| (o.clone(), d.clone())),
        );
        diff.missing += oracle_left.len() - pairs;
        diff.missing_rows
            .extend(oracle_left[pairs..].iter().cloned());
        diff.extra += dartq_left.len() - pairs;
        diff.extra_rows.extend(dartq_left[pairs..].iter().cloned());
    }
    diff
}

// ---- baseline ------------------------------------------------------------

/// One stored baseline row. `verdict` is `pass`, `diff`, `fail` or `skip`;
/// a `skip` row carries no numbers (the package was absent from the pub
/// cache when the baseline was written) and blocks plain-run gating until a
/// real baseline is recorded.
#[derive(Debug, PartialEq, Clone)]
struct BaselineRow {
    cases: usize,
    oracle_rows: usize,
    dartq_rows: usize,
    missing: usize,
    extra: usize,
    message_only: usize,
    jaccard: Option<f64>,
    verdict: String,
}

impl BaselineRow {
    fn has_numbers(&self) -> bool {
        self.verdict != "skip"
    }

    fn recorded(name: &str, diff: &PackageDiff, verdict: &str) -> BaselineRow {
        let _ = name;
        BaselineRow {
            cases: diff.files,
            oracle_rows: diff.oracle_rows,
            dartq_rows: diff.dartq_rows,
            missing: diff.missing,
            extra: diff.extra,
            message_only: diff.message_only,
            jaccard: Some(diff.jaccard),
            verdict: verdict.to_string(),
        }
    }

    fn skip() -> BaselineRow {
        BaselineRow {
            cases: 0,
            oracle_rows: 0,
            dartq_rows: 0,
            missing: 0,
            extra: 0,
            message_only: 0,
            jaccard: None,
            verdict: "skip".to_string(),
        }
    }

    fn render(&self, name: &str) -> String {
        let jaccard = self
            .jaccard
            .map(|j| format!("{j:.4}"))
            .unwrap_or_else(|| "-".to_string());
        format!(
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{jaccard}\t{}",
            self.cases,
            self.oracle_rows,
            self.dartq_rows,
            self.missing,
            self.extra,
            self.message_only,
            self.verdict
        )
    }

    fn parse(line: &str) -> Result<(String, BaselineRow), String> {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 9 {
            return Err(format!("expected 9 tab-separated fields: {line}"));
        }
        let numbers = |field: &str| -> Result<usize, String> {
            field
                .parse()
                .map_err(|_| format!("not a count: {field:?} in {line}"))
        };
        let jaccard = match fields[7] {
            "-" => None,
            text => Some(
                text.parse::<f64>()
                    .map_err(|_| format!("not a jaccard: {text:?} in {line}"))?,
            ),
        };
        Ok((
            fields[0].to_string(),
            BaselineRow {
                cases: numbers(fields[1])?,
                oracle_rows: numbers(fields[2])?,
                dartq_rows: numbers(fields[3])?,
                missing: numbers(fields[4])?,
                extra: numbers(fields[5])?,
                message_only: numbers(fields[6])?,
                jaccard,
                verdict: fields[8].to_string(),
            },
        ))
    }
}

/// Loads the stored baseline; a missing file is an empty baseline (first
/// record), a corrupt one is an error.
fn load_baseline(path: &Path) -> Result<BTreeMap<String, BaselineRow>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("cannot read: {e}")),
    };
    let mut map = BTreeMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        if line.starts_with("package\t") {
            continue; // header
        }
        let (name, row) = BaselineRow::parse(line).map_err(|e| format!("corrupt baseline: {e}"))?;
        map.insert(name, row);
    }
    Ok(map)
}

/// Rebuilds the baseline from the package list: measured packages get their
/// new row, skipped packages keep their stored row (or a `skip` row when
/// never recorded), and packages no longer in the list drop out — the tsv
/// always mirrors the toml inventory.
fn merge_baseline(
    packages: &[(String, String)],
    stored: &BTreeMap<String, BaselineRow>,
    skipped: &[String],
    results: &[(String, PackageDiff, String)],
) -> BTreeMap<String, BaselineRow> {
    let mut merged = BTreeMap::new();
    for (name, _) in packages {
        if let Some((_, diff, verdict)) = results.iter().find(|(n, _, _)| n == name) {
            merged.insert(name.clone(), BaselineRow::recorded(name, diff, verdict));
        } else if skipped.contains(name) {
            merged.insert(
                name.clone(),
                stored.get(name).cloned().unwrap_or_else(BaselineRow::skip),
            );
        }
    }
    merged
}

fn render_baseline(map: &BTreeMap<String, BaselineRow>) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for (name, row) in map {
        out.push_str(&row.render(name));
        out.push('\n');
    }
    out
}

fn render_diff(name: &str, diff: &PackageDiff) -> String {
    let mut out = format!(
        "# realworld diff: {name}{}{}\n",
        diff.stats_line(),
        match &diff.crash {
            Some(detail) => format!("\n# dartq crash: {detail}"),
            None => String::new(),
        }
    );
    if !diff.message_pairs.is_empty() {
        out.push_str("== message_only (same position and code, differing message)\n");
        for (oracle_text, dartq_text) in &diff.message_pairs {
            out.push_str(&format!("- {oracle_text}\n+ {dartq_text}\n"));
        }
    }
    if !diff.missing_rows.is_empty() {
        out.push_str("== missing (oracle-only)\n");
        for text in &diff.missing_rows {
            out.push_str(&format!("{text}\n"));
        }
    }
    if !diff.extra_rows.is_empty() {
        out.push_str("== extra (dartq-only)\n");
        for text in &diff.extra_rows {
            out.push_str(&format!("{text}\n"));
        }
    }
    out
}

// ---- SDK version guard ---------------------------------------------------

/// The version token of a `dart --version` banner
/// (`Dart SDK version: 3.13.7 (stable) … on "macos_arm64"`).
fn version_from_banner(banner: &str) -> Option<String> {
    let rest = banner.strip_prefix("Dart SDK version: ")?;
    let version = rest.split_whitespace().next()?;
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Runs `<sdk>/bin/dart --version` and extracts its version token — the same
/// guard the oracle face applies before it trusts a released SDK.
fn dart_version(dart_bin: &Path) -> Result<String, String> {
    let output = Command::new(dart_bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", dart_bin.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    version_from_banner(stderr.trim())
        .or_else(|| version_from_banner(stdout.trim()))
        .ok_or_else(|| {
            format!(
                "{} --version printed no recognizable version banner (stderr: {})",
                dart_bin.display(),
                stderr.trim()
            )
        })
}

fn warn(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "warn: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sev: &str, code: &str, file: &str, line: u32, col: u32, len: u64, msg: &str) -> Row {
        Row {
            severity: sev.into(),
            code: code.into(),
            file: file.into(),
            line,
            column: col,
            length: len,
            message: msg.into(),
        }
    }

    #[test]
    fn package_list_parses_sorted_and_ignores_other_sections() {
        let text = "# header comment\n\n[packages]\nanalyzer = \"14.5.0\"\nmeta = \"1.19.0\"\nargs = \"2.7.0\" # trailing\n\n[other]\njunk = \"ignored\"\n";
        let packages = parse_packages(text).expect("parses");
        assert_eq!(
            packages,
            vec![
                ("analyzer".to_string(), "14.5.0".to_string()),
                ("args".to_string(), "2.7.0".to_string()),
                ("meta".to_string(), "1.19.0".to_string()),
            ]
        );
        assert!(parse_packages("# only comments\n").is_err());
    }

    #[test]
    fn package_list_rejects_malformed_and_duplicate_entries() {
        let text = "[packages]\nmeta = \"1.19.0\"\nbroken line without equals\n";
        let err = parse_packages(text).unwrap_err();
        assert!(err.contains("malformed"), "{err}");
        let text = "[packages]\nmeta = \"1\"\nmeta = \"2\"\n";
        assert!(parse_packages(text).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn cache_resolution_errors_name_the_fix() {
        let cache = std::env::temp_dir().join(format!("conf-rw-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let pkg = cache.join("hosted").join("pub.dev").join("meta-1.19.0");
        std::fs::create_dir_all(&pkg).expect("creates");

        let found = resolve_package(&cache, "meta", "1.19.0").expect("present");
        assert_eq!(found, pkg);
        let err = resolve_package(&cache, "drift", "2.34.0").unwrap_err();
        assert!(
            err.contains("dart pub cache add drift --version 2.34.0"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn attribution_relativizes_and_drops_outside_rows() {
        let pkg = Path::new("/cache/pub.dev/meta-1.19.0");
        let mut inside = analyze_json::Diag {
            severity: "ERROR".into(),
            code: "undefined_identifier".into(),
            file: "/cache/pub.dev/meta-1.19.0/lib/meta.dart".into(),
            line: 3,
            column: 9,
            length: 1,
            message: "issue in '/cache/pub.dev/meta-1.19.0'".into(),
        };
        let outside = analyze_json::Diag {
            file: "/elsewhere/lib/dep.dart".into(),
            ..inside.clone()
        };
        let on_root = analyze_json::Diag {
            file: "/cache/pub.dev/meta-1.19.0".into(),
            ..inside.clone()
        };
        let (rows, dropped) = attribute(&[outside, on_root, inside.clone()], pkg);
        assert_eq!(dropped, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file, "lib/meta.dart");
        assert_eq!(rows[0].message, "issue in '<pkg>'");
        // Diagnostics on the same file sort by position; the mutable clone
        // keeps the compiler honest about the field set.
        inside.line += 1;
        assert_eq!(inside.line, 4);
    }

    #[test]
    fn compare_decomposes_missing_extra_and_message_only() {
        let oracle = vec![
            row("ERROR", "a", "lib/x.dart", 1, 1, 1, "one"),
            row("ERROR", "b", "lib/x.dart", 2, 1, 1, "m"),
            row("WARNING", "c", "lib/y.dart", 3, 1, 1, "m"),
        ];
        let dartq = vec![
            row("ERROR", "a", "lib/x.dart", 1, 1, 1, "one"),
            row("ERROR", "b", "lib/x.dart", 2, 1, 1, "different"),
            row("ERROR", "d", "lib/z.dart", 4, 1, 1, "m"),
        ];
        let diff = compare(&oracle, &dartq);
        assert_eq!(diff.files, 3);
        assert_eq!(diff.oracle_rows, 3);
        assert_eq!(diff.dartq_rows, 3);
        assert_eq!(diff.message_only, 1);
        assert_eq!(
            diff.message_pairs,
            vec![(
                "ERROR|b|lib/x.dart|2:1|1|m".to_string(),
                "ERROR|b|lib/x.dart|2:1|1|different".to_string()
            )]
        );
        assert_eq!(diff.missing, 1);
        assert_eq!(
            diff.missing_rows,
            vec!["WARNING|c|lib/y.dart|3:1|1|m".to_string()]
        );
        assert_eq!(diff.extra, 1);
        assert_eq!(
            diff.extra_rows,
            vec!["ERROR|d|lib/z.dart|4:1|1|m".to_string()]
        );
        // jaccard over full row texts: {a,b,c} vs {a,b',d} -> 1/5.
        assert!((diff.jaccard - 0.2).abs() < 1e-9, "{}", diff.jaccard);
        assert!(!diff.is_green());
        assert!(compare(&[], &[]).is_green());
        assert!((compare(&[], &[]).jaccard - 1.0).abs() < 1e-9);
        // Duplicates are multiset-aware: a doubled oracle row is one missing.
        let doubled = vec![row("ERROR", "a", "f.dart", 1, 1, 1, "m"); 2];
        let diff = compare(&doubled, &[row("ERROR", "a", "f.dart", 1, 1, 1, "m")]);
        assert_eq!(diff.missing, 1);
        assert_eq!(diff.extra, 0);
        assert_eq!(diff.message_only, 0);
    }

    #[test]
    fn crashed_packages_never_look_green() {
        let oracle = vec![row("INFO", "a", "f.dart", 1, 1, 1, "m")];
        let diff = PackageDiff::crashed(&oracle, "killed by the watchdog".into());
        assert!(!diff.is_green());
        assert_eq!(diff.missing, 1);
        assert_eq!(diff.jaccard, 0.0);
        assert!(diff.crash.is_some());
    }

    #[test]
    fn gate_is_monotonic_on_extra_and_jaccard() {
        let baseline = BaselineRow {
            cases: 2,
            oracle_rows: 3,
            dartq_rows: 3,
            missing: 1,
            extra: 1,
            message_only: 1,
            jaccard: Some(0.5),
            verdict: "diff".into(),
        };
        // Same numbers: fine.
        let current = PackageDiff {
            files: 2,
            oracle_rows: 3,
            dartq_rows: 3,
            missing: 1,
            extra: 1,
            message_only: 1,
            jaccard: 0.5,
            ..PackageDiff::default()
        };
        assert!(gate_check(&baseline, &current).is_ok());
        // Improvement: fine.
        let improved = PackageDiff {
            extra: 0,
            jaccard: 0.8,
            ..current.clone()
        };
        assert!(gate_check(&baseline, &improved).is_ok());
        // Regression: extra grew.
        let regressed = PackageDiff {
            extra: 2,
            ..current.clone()
        };
        assert!(gate_check(&baseline, &regressed).is_err());
        // Regression: a dartq-only row swapped for another (extra count
        // equal, jaccard dropped beyond the storage tolerance).
        let swapped = PackageDiff {
            jaccard: 0.4,
            ..current.clone()
        };
        assert!(gate_check(&baseline, &swapped).is_err());
        // Rounding noise within the storage precision is not a regression.
        let noisy = PackageDiff {
            jaccard: 0.49995,
            ..current
        };
        assert!(gate_check(&baseline, &noisy).is_ok());
    }

    #[test]
    fn baseline_tsv_roundtrips_including_skip_rows() {
        let mut map = BTreeMap::new();
        map.insert(
            "meta".to_string(),
            BaselineRow {
                cases: 8,
                oracle_rows: 42,
                dartq_rows: 42,
                missing: 0,
                extra: 0,
                message_only: 0,
                jaccard: Some(1.0),
                verdict: "pass".into(),
            },
        );
        map.insert("drift".to_string(), BaselineRow::skip());
        let text = render_baseline(&map);
        assert!(text.starts_with(HEADER));
        let parsed = load_baseline_from_text(&text).expect("parses");
        assert_eq!(parsed, map);
        assert!(parsed["drift"].jaccard.is_none());
        assert!(!parsed["drift"].has_numbers());
        // Corrupt rows are hard errors, never silent data loss.
        assert!(load_baseline_from_text("meta\t1\t2\n").is_err());
        assert!(load_baseline_from_text("meta\t1\t2\t3\t4\t5\t6\tbad\tfail\n").is_err());
    }

    fn load_baseline_from_text(text: &str) -> Result<BTreeMap<String, BaselineRow>, String> {
        let mut map = BTreeMap::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            if line.starts_with("package\t") {
                continue;
            }
            let (name, row) =
                BaselineRow::parse(line).map_err(|e| format!("corrupt baseline: {e}"))?;
            map.insert(name, row);
        }
        Ok(map)
    }
}
