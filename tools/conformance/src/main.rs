//! `conformance` — the conformance corpus tool.
//!
//! - `extract`: derives the diagnostics case corpus from a pinned Dart SDK
//!   source tree (doc examples from messages.yaml + analyzer package test
//!   files), writing `.case` files and `cases.manifest`.
//! - `mocks`: extracts the official mock packages into `conformance/mocks/`.
//! - `oracle`: produces the tracked expected outputs (`expected.tsv`) by
//!   running the pinned SDK's analyzer over materialized chunks.
//! - `run`: runs dartq over the same cases, compares with the oracle and
//!   writes the failure report and the tracked `status.tsv`.
//! - `lock` / `gate`: maintain `locked.txt` and enforce it.
//!
//! All inputs come from arguments/environment; no tracked file references a
//! machine-local path.

mod analyze_json;
mod case;
mod dart;
mod docs;
mod exclusions;
mod features_extract;
mod lints_extract;
mod manifest;
mod markers;
mod materialize;
mod messages_yaml;
mod mocks;
mod oracle;
mod realworld;
mod registry;
mod rows;
mod run;
mod runner;
mod setup;
mod status;
mod structure;
mod tests_extract;
mod watchdog;

use clap::{Parser, Subcommand};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Tracked conformance data files (repo-root relative).
const PIN: &str = "conformance/pin.toml";
const MOCKS: &str = "conformance/mocks";
const MANUAL: &str = "conformance/manual";
const EXPECTED: &str = "conformance/expected.tsv";
const STATUS: &str = "conformance/status.tsv";
const LOCKED: &str = "conformance/locked.txt";
const CODES_TSV: &str = "crates/dartq-diagnostics/data/analyzer-codes.tsv";
const RULE_META_RS: &str = "crates/dartq-lints/src/generated/rule_meta.rs";
const FEATURES_YAML: &str = "crates/dartq-language/data/experimental_features.yaml";
const CASES: &str = "target/conformance/cases";
const WORK: &str = "target/conformance/chunks";
const CACHE: &str = "target/conformance/oracle-cache";
const REPORT: &str = "target/conformance/report.tsv";

#[derive(Parser)]
#[command(
    name = "conformance",
    about = "Conformance corpus tool for the dartq analyzer"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract diagnostics cases from a pinned SDK source tree.
    Extract {
        /// The pinned SDK source root.
        #[arg(long)]
        sdk_src: PathBuf,
        /// Output directory for the `.case` tree. Required for a write run;
        /// a bare run without `--out` or `--check` is refused (it once
        /// wiped the default corpus by accident). With `--check` alone the
        /// default corpus location is verified.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The manifest file to write (or compare with `--check`).
        #[arg(long, default_value = "conformance/cases.manifest")]
        manifest: PathBuf,
        /// The co19 suite root (a checkout of the pinned `co19_rev`); when
        /// absent the co19 feature cases are counted as skipped.
        #[arg(long)]
        co19: Option<PathBuf>,
        /// Compare instead of write: fail when the corpus or manifest differs.
        #[arg(long)]
        check: bool,
    },
    /// Extract the official mock packages.
    Mocks {
        /// The pinned SDK source root.
        #[arg(long)]
        sdk_src: PathBuf,
        /// Output directory (the tracked `conformance/mocks/` tree).
        /// Required for a write run; a bare run without `--out` or
        /// `--check` is refused for the same reason as `extract`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Compare instead of write.
        #[arg(long)]
        check: bool,
    },
    /// Produce `expected.tsv` by analyzing cases with the pinned SDK's
    /// `dart analyze` (the oracle), cached per case.
    Oracle {
        /// The pinned released SDK root (providing `bin/dart`).
        #[arg(long)]
        oracle_sdk: PathBuf,
        /// Only analyze cases whose id starts with this prefix.
        #[arg(long)]
        filter: Option<String>,
        /// Skip the hand-written manual corpus (`conformance/manual`).
        #[arg(long)]
        no_manual: bool,
        /// Cases per chunk (one analyzer process per chunk).
        #[arg(long, default_value_t = 500)]
        chunk: usize,
    },
    /// Run dartq over the cases, compare with `expected.tsv`, and write the
    /// report plus `status.tsv` (unfiltered runs only).
    Run {
        /// The dartq binary to test.
        #[arg(long)]
        dartq: PathBuf,
        /// SDK root passed to dartq as `--sdk-path` (default: `$DART_SDK`).
        #[arg(long)]
        sdk: Option<PathBuf>,
        /// Only run cases whose id starts with this prefix.
        #[arg(long)]
        filter: Option<String>,
        /// Skip the hand-written manual corpus (`conformance/manual`).
        #[arg(long)]
        no_manual: bool,
        /// Cases per chunk (one dartq process per chunk).
        #[arg(long, default_value_t = 500)]
        chunk: usize,
    },
    /// Add every green entry of `status.tsv` to `locked.txt` (never removes).
    Lock,
    /// Fail when any locked entry is not green or any case crashed.
    Gate,
    /// Gate dartq against the oracle over real pub-cache packages
    /// (`conformance/realworld.toml`).
    Realworld {
        /// The dartq binary to test.
        #[arg(long)]
        dartq: PathBuf,
        /// The released SDK root: provides the oracle `bin/dart` and dartq's
        /// `--sdk-path`.
        #[arg(long)]
        oracle_sdk: PathBuf,
        /// Only run this one package from the list.
        #[arg(long)]
        package: Option<String>,
        /// Force-record the current numbers as the stored baseline (the tsv).
        #[arg(long)]
        baseline: bool,
        /// Record the current numbers, refusing when any package regressed
        /// against the stored baseline.
        #[arg(long, conflicts_with = "baseline")]
        update_baseline: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Extract {
            sdk_src,
            out,
            manifest,
            co19,
            check,
        } => extract_command(&sdk_src, out.as_deref(), &manifest, co19.as_deref(), check),
        Command::Mocks {
            sdk_src,
            out,
            check,
        } => mocks_command(&sdk_src, out.as_deref(), check),
        Command::Oracle {
            oracle_sdk,
            filter,
            no_manual,
            chunk,
        } => oracle::oracle_command(&oracle::OracleArgs {
            oracle_sdk,
            cases_dir: PathBuf::from(CASES),
            manual_dir: (!no_manual).then(|| PathBuf::from(MANUAL)),
            mocks_root: PathBuf::from(MOCKS),
            pin_path: PathBuf::from(PIN),
            out_path: PathBuf::from(EXPECTED),
            cache_dir: PathBuf::from(CACHE),
            work_root: PathBuf::from(WORK),
            filter,
            chunk,
        }),
        Command::Run {
            dartq,
            sdk,
            filter,
            no_manual,
            chunk,
        } => run::run_command(&run::RunArgs {
            dartq_bin: dartq,
            sdk,
            cases_dir: PathBuf::from(CASES),
            manual_dir: (!no_manual).then(|| PathBuf::from(MANUAL)),
            mocks_root: PathBuf::from(MOCKS),
            expected_path: PathBuf::from(EXPECTED),
            codes_tsv: PathBuf::from(CODES_TSV),
            rule_meta_rs: PathBuf::from(RULE_META_RS),
            features_yaml: PathBuf::from(FEATURES_YAML),
            report_path: PathBuf::from(REPORT),
            status_path: PathBuf::from(STATUS),
            work_root: PathBuf::from(WORK),
            filter,
            chunk,
        }),
        Command::Lock => lock_command(),
        Command::Gate => gate_command(),
        Command::Realworld {
            dartq,
            oracle_sdk,
            package,
            baseline,
            update_baseline,
        } => realworld::realworld_command(&realworld::RealworldArgs {
            dartq_bin: dartq,
            oracle_sdk,
            package,
            baseline,
            update_baseline,
            list_path: PathBuf::from(realworld::LIST),
            out_path: PathBuf::from(realworld::OUT),
            diff_root: PathBuf::from(realworld::DIFF_ROOT),
        }),
    };
    std::process::exit(code);
}

fn lock_command() -> i32 {
    let stats_text = match std::fs::read_to_string(STATUS) {
        Ok(text) => text,
        Err(_) => {
            eprintln!("error: {STATUS} not found; run `conformance run` first");
            return 2;
        }
    };
    let stats = match status::parse(&stats_text) {
        Ok(stats) => stats,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let existing = std::fs::read_to_string(LOCKED).unwrap_or_default();
    let merged = status::lock(&existing, &stats);
    if let Err(e) = std::fs::write(LOCKED, merged) {
        eprintln!("error: cannot write {LOCKED}: {e}");
        return 2;
    }
    0
}

fn gate_command() -> i32 {
    let locked = std::fs::read_to_string(LOCKED).unwrap_or_default();
    let locked_count = locked.lines().filter(|l| !l.trim().is_empty()).count();
    let stats_text = match std::fs::read_to_string(STATUS) {
        Ok(text) => text,
        Err(_) => {
            eprintln!("error: {STATUS} not found; run `conformance run` first");
            return 2;
        }
    };
    let stats = match status::parse(&stats_text) {
        Ok(stats) => stats,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let violations = status::gate(&stats, &locked);
    if violations.is_empty() {
        eprintln!("gate: {locked_count} locked entries all green, no crashes");
        0
    } else {
        for violation in &violations {
            eprintln!("gate FAILED: {violation}");
        }
        1
    }
}

const ANALYZER_PKG: &str = "pkg/analyzer";
const TEST_ROOT_REL: &str = "pkg/analyzer/test";
const LINTER_PKG: &str = "pkg/linter";

fn extract_command(
    sdk_src: &Path,
    out: Option<&Path>,
    manifest_path: &Path,
    co19: Option<&Path>,
    check: bool,
) -> i32 {
    let started = std::time::Instant::now();
    let Some(out) = resolve_write_target("extract", out, CASES, check) else {
        return 2;
    };
    if let Some(dir) = co19 {
        for warning in co19_pin_warnings(dir, Path::new(PIN)) {
            eprintln!("warn: {warning}");
        }
    }
    let analyzer_messages = sdk_src.join(format!("{ANALYZER_PKG}/messages.yaml"));
    let shared_messages = sdk_src.join("pkg/_fe_analyzer_shared/messages.yaml");
    let test_root = sdk_src.join(TEST_ROOT_REL);
    let linter_messages = sdk_src.join(format!("{LINTER_PKG}/messages.yaml"));
    let linter_test_root = sdk_src.join(format!("{LINTER_PKG}/test/rules"));
    let language_root = sdk_src.join("tests/language");
    let front_end_root = sdk_src.join("pkg/front_end/parser_testcases");
    for (path, label) in [
        (&analyzer_messages, "analyzer messages.yaml"),
        (&shared_messages, "shared messages.yaml"),
        (&test_root, "analyzer test root"),
        (&linter_messages, "linter messages.yaml"),
        (&linter_test_root, "linter test root"),
        (&language_root, "language test root"),
        (&front_end_root, "front_end parser_testcases root"),
    ] {
        if !path.exists() {
            eprintln!("error: missing {label}: {}", path.display());
            return 2;
        }
    }

    if !check {
        // Fresh corpus: remove stale cases so deletions cannot survive.
        if out.exists() {
            if let Err(e) = std::fs::remove_dir_all(out) {
                eprintln!("error: cannot clear {}: {e}", out.display());
                return 2;
            }
        }
        std::fs::create_dir_all(out).expect("creates out dir");
    }

    let mut rows: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stats = Stats::default();
    // Every dropped site as (face, reason, upstream id) — the skip
    // inventory persisted next to the manifest.
    let mut skip_triples: std::collections::BTreeSet<(String, String, String)> =
        std::collections::BTreeSet::new();

    // 1. Doc examples.
    for (path, origin) in [
        (&shared_messages, "pkg/_fe_analyzer_shared/messages.yaml"),
        (&analyzer_messages, "pkg/analyzer/messages.yaml"),
    ] {
        let yaml = std::fs::read_to_string(path).expect("reads messages.yaml");
        let extraction = docs::extract_messages_text(&yaml, origin);
        stats.doc_cases += extraction.cases.len();
        for skip in &extraction.skips {
            *stats.doc_skips.entry(skip.reason.clone()).or_insert(0) += 1;
            skip_triples.insert(("docs".to_string(), skip.reason.clone(), skip.id.clone()));
        }
        stats.doc_no_example += extraction.no_example_codes;
        stats.doc_multi_range += extraction.multi_range;
        for c in &extraction.cases {
            write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
        }
    }

    // 2. Analyzer test files.
    let mut files = Vec::new();
    collect_test_files(&test_root, &mut files);
    files.sort();
    for rel in &files {
        let content = match std::fs::read_to_string(test_root.join(rel)) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("warn: cannot read {rel}: {e}");
                continue;
            }
        };
        let extraction = tests_extract::extract_file(rel, &content);
        for c in &extraction.cases {
            if c.id.starts_with("parser/") {
                stats.parser_cases += 1;
            } else {
                stats.test_cases += 1;
            }
        }
        stats.test_files_scanned += 1;
        for skip in &extraction.skips {
            let reason = if skip.reason.starts_with("setup_") {
                "unmodeled_setup".to_string()
            } else {
                skip.reason.clone()
            };
            *stats.test_skips.entry(reason).or_insert(0) += 1;
        }
        for skip in extraction.skips {
            let face = if skip.id.starts_with("parser/") {
                "parser"
            } else {
                "diagnostics"
            };
            skip_triples.insert((face.to_string(), skip.reason, skip.id));
        }
        for c in &extraction.cases {
            write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
        }
    }

    // 3. Lint rule test files.
    let mut files = Vec::new();
    collect_test_files(&linter_test_root, &mut files);
    files.sort();
    for rel in &files {
        let content = match std::fs::read_to_string(linter_test_root.join(rel)) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("warn: cannot read {rel}: {e}");
                continue;
            }
        };
        let extraction = lints_extract::extract_file(rel, &content);
        stats.lint_test_cases += extraction.cases.len();
        stats.lint_files_scanned += 1;
        for skip in &extraction.skips {
            let reason = if skip.reason.starts_with("setup_") {
                "unmodeled_setup".to_string()
            } else {
                skip.reason.clone()
            };
            *stats.lint_skips.entry(reason).or_insert(0) += 1;
        }
        for skip in extraction.skips {
            skip_triples.insert(("lints".to_string(), skip.reason, skip.id));
        }
        for c in &extraction.cases {
            write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
        }
    }

    // 4. Lint doc snippets.
    let linter_yaml = std::fs::read_to_string(&linter_messages).expect("reads messages.yaml");
    let doc_extraction = lints_extract::extract_docs(&linter_yaml, "pkg/linter/messages.yaml");
    stats.lint_doc_cases = doc_extraction.cases.len();
    for skip in &doc_extraction.skips {
        *stats.lint_skips.entry(skip.reason.clone()).or_insert(0) += 1;
        skip_triples.insert(("lints".to_string(), skip.reason.clone(), skip.id.clone()));
    }
    for c in &doc_extraction.cases {
        write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
    }

    // 5. Language-feature suites (tests/language and, when provided, co19).
    // The co19 suite root is the co19 checkout: its test files live under
    // LanguageFeatures/ but their helpers (Utils/ and friends) sit at the
    // checkout root, so imports resolve against the checkout.
    let mut feature_extraction = features_extract::extract_suite(
        &language_root,
        "",
        "language",
        &features_extract::is_language_test,
        &features_extract::language_flag,
    );
    stats.feature_files_scanned += feature_extraction.scanned;
    match co19 {
        Some(dir) if dir.join("LanguageFeatures").is_dir() => {
            let co19_extraction = features_extract::extract_suite(
                dir,
                "LanguageFeatures",
                "co19",
                &features_extract::is_co19_test,
                &features_extract::co19_flag,
            );
            stats.feature_files_scanned += co19_extraction.scanned;
            feature_extraction.cases.extend(co19_extraction.cases);
            feature_extraction.skips.extend(co19_extraction.skips);
        }
        Some(dir) => {
            *stats
                .feature_skips
                .entry("co19_missing".to_string())
                .or_insert(0) += 1;
            eprintln!("warn: no LanguageFeatures under {}", dir.display());
        }
        None => {
            *stats
                .feature_skips
                .entry("co19_not_provided".to_string())
                .or_insert(0) += 1;
        }
    }
    stats.feature_cases = feature_extraction.cases.len();
    for skip in &feature_extraction.skips {
        *stats.feature_skips.entry(skip.reason.clone()).or_insert(0) += 1;
        skip_triples.insert(("features".to_string(), skip.reason.clone(), skip.id.clone()));
    }
    for c in &feature_extraction.cases {
        write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
    }

    // 6. front_end parser_testcases: whole-file source-only cases.
    let front_end_extraction = tests_extract::extract_front_end_parser(&front_end_root);
    stats.parserfe_cases = front_end_extraction.cases.len();
    stats.parserfe_files_scanned = front_end_extraction.scanned;
    for skip in &front_end_extraction.skips {
        *stats.parserfe_skips.entry(skip.reason.clone()).or_insert(0) += 1;
        skip_triples.insert((
            "parser-fe".to_string(),
            skip.reason.clone(),
            skip.id.clone(),
        ));
    }
    for c in &front_end_extraction.cases {
        write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
    }

    // 7. Manifest.

    // 6. Manifest.
    let built = manifest::build(rows.iter().map(|(id, bytes)| (id, bytes.as_slice())));
    if check {
        let existing = match std::fs::read_to_string(manifest_path) {
            Ok(text) => manifest::parse(&text).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let (missing, unexpected, changed) = manifest::diff(&existing, &built);
        // On --check, corpus files were not written; verify them by content.
        let mut content_mismatch = 0usize;
        for row in &built {
            let bytes = std::fs::read(case_path(out, &row.id)).unwrap_or_default();
            if blake3::hash(&bytes).to_string() != row.hash {
                content_mismatch += 1;
            }
        }
        if !missing.is_empty()
            || !unexpected.is_empty()
            || !changed.is_empty()
            || content_mismatch > 0
        {
            eprintln!(
                "check FAILED: {} missing, {} unexpected, {} changed, {} content-mismatch",
                missing.len(),
                unexpected.len(),
                changed.len(),
                content_mismatch
            );
            return 1;
        }
    } else {
        let text = manifest::render(&built);
        if let Some(parent) = manifest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(manifest_path, text) {
            eprintln!("error: cannot write {}: {e}", manifest_path.display());
            return 2;
        }
        write_skip_inventory(
            manifest_path.parent().unwrap_or(Path::new(".")),
            &skip_triples,
        );
    }

    stats.print(started);
    0
}

/// On-disk path for a case id. The filename carries a short hash of the full
/// id: the Dart test suites contain methods differing only in identifier
/// case (`...Function` / `...function`), which would collide on
/// case-insensitive filesystems. Derived from the id alone, so write and
/// check agree without shared state.
fn case_path(out: &Path, id: &str) -> PathBuf {
    let mut path = out.join(id.replace('/', std::path::MAIN_SEPARATOR_STR));
    let hash = blake3::hash(id.as_bytes());
    path.as_mut_os_string().push(".");
    path.as_mut_os_string().push(&hash.to_hex()[..12]);
    path.as_mut_os_string().push(".case");
    path
}

fn write_case(
    out: &Path,
    c: &case::Case,
    rows: &mut Vec<(String, Vec<u8>)>,
    check: bool,
    skipped_writes: &mut usize,
) {
    let bytes = case::write(c);
    rows.push((c.id.clone(), bytes.clone().into_bytes()));
    if check {
        *skipped_writes += 1;
        return;
    }
    let path = case_path(out, &c.id);
    let parent = path.parent().expect("case path has a parent");
    if let Err(e) = std::fs::create_dir_all(parent).and_then(|_| std::fs::write(&path, &bytes)) {
        eprintln!("error: cannot write {}: {e}", path.display());
    }
}

fn collect_test_files(dir: &Path, out: &mut Vec<String>) {
    collect_under(dir, dir, out);
}

fn collect_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_under(root, &path, out);
        } else if name.ends_with("_test.dart") {
            out.push(
                path.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
}

/// Persists the skip inventory next to the manifest: one row per
/// (face, reason) with the dropped-site count and representative upstream
/// ids, so every run's drops are auditable on disk.
fn write_skip_inventory(
    dir: &Path,
    triples: &std::collections::BTreeSet<(String, String, String)>,
) {
    let mut grouped: std::collections::BTreeMap<(String, String), Vec<String>> =
        std::collections::BTreeMap::new();
    for (face, reason, id) in triples {
        grouped
            .entry((face.clone(), reason.clone()))
            .or_default()
            .push(id.clone());
    }
    let mut out = String::from("face\treason\tcount\texamples\n");
    for ((face, reason), mut ids) in grouped {
        ids.sort();
        ids.dedup();
        let count = ids.len();
        ids.truncate(3);
        out.push_str(&format!("{face}\t{reason}\t{count}\t{}\n", ids.join(", ")));
    }
    let path = dir.join("skips.tsv");
    if let Err(e) = std::fs::write(&path, out) {
        eprintln!("error: cannot write {}: {e}", path.display());
    }
}

#[derive(Default)]
struct Stats {
    doc_cases: usize,
    doc_skips: std::collections::BTreeMap<String, usize>,
    doc_no_example: usize,
    doc_multi_range: usize,
    test_cases: usize,
    test_files_scanned: usize,
    test_skips: std::collections::BTreeMap<String, usize>,
    parser_cases: usize,
    lint_test_cases: usize,
    lint_files_scanned: usize,
    lint_doc_cases: usize,
    lint_skips: std::collections::BTreeMap<String, usize>,
    feature_cases: usize,
    feature_files_scanned: usize,
    feature_skips: std::collections::BTreeMap<String, usize>,
    parserfe_cases: usize,
    parserfe_files_scanned: usize,
    parserfe_skips: std::collections::BTreeMap<String, usize>,
    case_writes_skipped: usize,
}

impl Stats {
    fn print(&self, started: std::time::Instant) {
        let mut out = std::io::stderr().lock();
        let total = self.doc_cases
            + self.test_cases
            + self.parser_cases
            + self.lint_doc_cases
            + self.lint_test_cases
            + self.feature_cases
            + self.parserfe_cases;
        let _ = writeln!(
            out,
            "extract: {} doc cases, {} analyzer-test cases ({} files scanned), {} parser cases, {} lint doc cases, {} lint-test cases ({} files scanned), {} feature cases, {} parser-fe cases ({} files scanned), {} total",
            self.doc_cases,
            self.test_cases,
            self.test_files_scanned,
            self.parser_cases,
            self.lint_doc_cases,
            self.lint_test_cases,
            self.lint_files_scanned,
            self.feature_cases,
            self.parserfe_cases,
            self.parserfe_files_scanned,
            total
        );
        let _ = writeln!(out, "doc skips: {}", fmt_counts(&self.doc_skips));
        if self.doc_no_example > 0 {
            let _ = writeln!(out, "doc codes without examples: {}", self.doc_no_example);
        }
        if self.doc_multi_range > 0 {
            let _ = writeln!(
                out,
                "doc examples with more than one range: {}",
                self.doc_multi_range
            );
        }
        let _ = writeln!(out, "test skips: {}", fmt_counts(&self.test_skips));
        let _ = writeln!(out, "lint skips: {}", fmt_counts(&self.lint_skips));
        let _ = writeln!(out, "feature skips: {}", fmt_counts(&self.feature_skips));
        let _ = writeln!(out, "parser-fe skips: {}", fmt_counts(&self.parserfe_skips));
        let _ = writeln!(out, "done in {:.2}s", started.elapsed().as_secs_f64());
    }
}

fn fmt_counts(counts: &std::collections::BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn mocks_command(sdk_src: &Path, out: Option<&Path>, check: bool) -> i32 {
    let Some(out) = resolve_write_target("mocks", out, MOCKS, check) else {
        return 2;
    };
    let mock_root = sdk_src.join("pkg/analyzer_testing/lib/src/mock_packages");
    let context =
        sdk_src.join("pkg/analyzer/test/src/dart/resolution/context_collection_resolution.dart");
    for path in [&mock_root, &context] {
        if !path.exists() {
            eprintln!("error: missing {}", path.display());
            return 2;
        }
    }
    let packages = match mocks::extract(
        &mock_root,
        &std::fs::read_to_string(&context).expect("reads context"),
        sdk_src,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut total_files = 0usize;
    let mut diffs = 0usize;
    for (package, files) in &packages {
        total_files += files.len();
        for file in files {
            let path = out.join(package).join(&file.path);
            if check {
                let existing = std::fs::read_to_string(&path).unwrap_or_default();
                if existing != file.content {
                    diffs += 1;
                    eprintln!("check: differs: {}", path.display());
                }
            } else if let Err(e) = std::fs::create_dir_all(path.parent().expect("has parent"))
                .and_then(|_| std::fs::write(&path, &file.content))
            {
                eprintln!("error: cannot write {}: {e}", path.display());
                return 2;
            }
        }
    }
    if check {
        if diffs > 0 {
            eprintln!("mocks check FAILED: {diffs} files differ");
            return 1;
        }
    } else {
        // Remove files that the pin no longer produces.
        prune_mocks(out, &packages);
    }
    eprintln!(
        "mocks: {} packages, {} files{}",
        packages.len(),
        total_files,
        if check { " (checked)" } else { " written" }
    );
    0
}

fn prune_mocks(out: &Path, packages: &std::collections::BTreeMap<String, Vec<mocks::MockFile>>) {
    let Ok(entries) = std::fs::read_dir(out) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() {
            continue;
        }
        match packages.get(&name) {
            None => {
                let _ = std::fs::remove_dir_all(entry.path());
            }
            Some(files) => {
                prune_dir(&entry.path(), &entry.path(), files);
            }
        }
    }
}

/// `root` is the package directory; `keep` paths are relative to it.
fn prune_dir(dir: &Path, root: &Path, keep: &[mocks::MockFile]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_dir(&path, root, keep);
            let _ = std::fs::remove_dir(&path); // remove when empty
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .into_owned();
            if !keep.iter().any(|f| f.path == rel) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Resolves the write-vs-check mode. `--out` is the explicit write target
/// and `--check` verifies an existing tree (defaulting to `default`)
/// without writing; a bare run with neither is refused instead of writing
/// into the default location, which once wiped a derived corpus by
/// accident. `None` means the refusal message was printed.
fn resolve_write_target<'a>(
    command: &str,
    out: Option<&'a Path>,
    default: &'a str,
    check: bool,
) -> Option<&'a Path> {
    match (out, check) {
        (Some(out), _) => Some(out),
        (None, true) => Some(Path::new(default)),
        (None, false) => {
            eprintln!(
                "error: `{command}` without --out or --check would write by default; \
                 that default is refused\n\
                 modes: write a fresh tree with `--out <dir>` (the tree is cleared first)\n       \
                 verify without writing with `--check` (verifies `{default}`)\n       \
                 verify a specific tree with `--check --out <dir>`"
            );
            None
        }
    }
}

/// Reads one flat `key = "value"` entry from the pin file; the pin is a
/// flat string map, so this avoids a TOML dependency.
fn pin_value(pin_path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(pin_path).ok()?;
    text.lines().map(str::trim).find_map(|line| {
        let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string())
    })
}

/// The HEAD commit of a plain git clone, read from `.git` directly (no
/// subprocess): a detached SHA in `HEAD`, or `ref: <name>` resolved via the
/// loose ref file and then `packed-refs`. `None` when `.git` is not a
/// directory (e.g. a worktree pointer) or the layout is unrecognized.
fn read_git_head(dir: &Path) -> Option<String> {
    let git = dir.join(".git");
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(rev) = head.strip_prefix("ref:") {
        let rev = rev.trim();
        if let Ok(sha) = std::fs::read_to_string(git.join(rev)) {
            return Some(sha.trim().to_string());
        }
        let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
        packed.lines().find_map(|line| {
            let (sha, r) = line.trim().split_once(' ')?;
            (r.trim() == rev).then(|| sha.trim().to_string())
        })
    } else {
        Some(head.to_string())
    }
}

/// The co19 pin diagnostics for a `--co19` checkout: a missing pin entry, a
/// revision mismatch, or an unreadable checkout revision. Empty means the
/// checkout agrees with the pin. Mismatches warn but never block, since
/// local checkouts may legitimately be dirty or newer.
fn co19_pin_warnings(co19: &Path, pin_path: &Path) -> Vec<String> {
    let Some(pinned) = pin_value(pin_path, "co19_revision") else {
        return vec![format!(
            "{} has no co19_revision; the co19 corpus revision is unpinned",
            pin_path.display()
        )];
    };
    match read_git_head(co19) {
        Some(rev) if rev == pinned => Vec::new(),
        Some(rev) => vec![format!(
            "co19 checkout {} is at {rev} but the pin is {pinned}; \
             the extracted co19 cases may not match the pinned corpus",
            co19.display()
        )],
        None => vec![format!(
            "cannot determine the revision of co19 checkout {} (pin {pinned}); \
             the extracted co19 cases may not match the pinned corpus",
            co19.display()
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake co19 checkout whose HEAD resolves to `sha` via a loose ref.
    fn fake_checkout(dir: &Path, sha: &str) {
        let git = dir.join(".git");
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(git.join("refs/heads/master"), format!("{sha}\n")).unwrap();
    }

    #[test]
    fn read_git_head_resolves_detached_loose_and_packed() {
        let dir = std::env::temp_dir().join("dartq-conf-head-detached");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join(".git/HEAD"),
            "8c17c2fa3d018098494422557d195ad396a0daf8\n",
        )
        .unwrap();
        assert_eq!(
            read_git_head(&dir).as_deref(),
            Some("8c17c2fa3d018098494422557d195ad396a0daf8")
        );

        let dir = std::env::temp_dir().join("dartq-conf-head-loose");
        fake_checkout(&dir, "a83e218200000000000000000000000000000001");
        assert_eq!(
            read_git_head(&dir).as_deref(),
            Some("a83e218200000000000000000000000000000001")
        );

        let dir = std::env::temp_dir().join("dartq-conf-head-packed");
        std::fs::create_dir_all(dir.join(".git/refs/heads")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(
            dir.join(".git/packed-refs"),
            "# pack-refs\n\
             0000000000000000000000000000000000000009 refs/heads/main\n\
             a83e218200000000000000000000000000000002 refs/heads/master\n",
        )
        .unwrap();
        assert_eq!(
            read_git_head(&dir).as_deref(),
            Some("a83e218200000000000000000000000000000002")
        );

        // Not a git checkout at all.
        let dir = std::env::temp_dir().join("dartq-conf-head-none");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_git_head(&dir), None);
    }

    /// A bare `extract` (no `--out`, no `--check`) must fail fast with the
    /// tutorial exit code and leave the default case tree and the manifest
    /// untouched — the #2763 corpus-wipe footgun, pinned.
    #[test]
    fn extract_without_out_or_check_refuses_and_writes_nothing() {
        let work = std::env::temp_dir().join("dartq-conf-refuse");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(work.join("sdk/pkg/analyzer")).unwrap();
        std::fs::create_dir_all(work.join("sdk/pkg/_fe_analyzer_shared")).unwrap();
        std::fs::create_dir_all(work.join("sdk/pkg/analyzer/test")).unwrap();
        std::fs::create_dir_all(work.join("sdk/pkg/linter/test/rules")).unwrap();
        std::fs::create_dir_all(work.join("sdk/tests/language")).unwrap();
        std::fs::create_dir_all(work.join("sdk/pkg/front_end/parser_testcases")).unwrap();
        for yaml in [
            "sdk/pkg/analyzer/messages.yaml",
            "sdk/pkg/_fe_analyzer_shared/messages.yaml",
            "sdk/pkg/linter/messages.yaml",
        ] {
            std::fs::write(work.join(yaml), "").unwrap();
        }

        // A sentinel in the default case tree must survive untouched, and
        // no manifest may appear in the (temp) manifest location.
        let cases = PathBuf::from(CASES);
        std::fs::create_dir_all(&cases).unwrap();
        std::fs::write(cases.join("sentinel.case"), "untouched").unwrap();
        let before = dir_listing(&cases);

        let code = extract_command(
            &work.join("sdk"),
            None,
            &work.join("cases.manifest"),
            None,
            false,
        );

        assert_eq!(code, 2, "bare extract must be refused with exit 2");
        assert_eq!(dir_listing(&cases), before, "case tree must be untouched");
        assert!(
            !work.join("cases.manifest").exists(),
            "no manifest may be written by a refused run"
        );
        let _ = std::fs::remove_file(cases.join("sentinel.case"));
        let _ = std::fs::remove_dir_all(&work);
    }

    fn dir_listing(dir: &Path) -> Vec<String> {
        let mut paths = Vec::new();
        fn walk(dir: &Path, root: &Path, paths: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, paths);
                } else {
                    paths.push(
                        path.strip_prefix(root)
                            .expect("under root")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        walk(dir, dir, &mut paths);
        paths.sort();
        paths
    }

    #[test]
    fn co19_pin_warns_on_fake_revision_mismatch() {
        let work = std::env::temp_dir().join("dartq-conf-co19pin");
        let _ = std::fs::remove_dir_all(&work);
        let checkout = work.join("co19");
        fake_checkout(&checkout, "8c17c2fa3d018098494422557d195ad396a0daf8");
        let pin = work.join("pin.toml");
        std::fs::write(
            &pin,
            "sdk_version = \"3.13.7\"\nco19_revision = \"a83e2182\"\n",
        )
        .unwrap();

        let warnings = co19_pin_warnings(&checkout, &pin);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("8c17c2fa3d018098494422557d195ad396a0daf8")
                && warnings[0].contains("a83e2182"),
            "mismatch warning must carry both revisions: {warnings:?}"
        );

        // A matching checkout produces no warning.
        fake_checkout(&checkout, "a83e2182");
        assert!(co19_pin_warnings(&checkout, &pin).is_empty());

        // An unreadable checkout warns too.
        let bare = work.join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        let warnings = co19_pin_warnings(&bare, &pin);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("cannot determine"));

        // A pin without the co19 entry warns that the pin is incomplete.
        let pin = work.join("pin-no-co19.toml");
        std::fs::write(&pin, "sdk_version = \"3.13.7\"\n").unwrap();
        let warnings = co19_pin_warnings(&checkout, &pin);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no co19_revision"));
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn pin_value_reads_flat_entries() {
        let work = std::env::temp_dir().join("dartq-conf-pinvalue");
        std::fs::create_dir_all(&work).unwrap();
        let pin = work.join("pin.toml");
        std::fs::write(
            &pin,
            "# comment\nsdk_version = \"3.13.7\"\nsdk_revision = \"416e\"\n",
        )
        .unwrap();
        assert_eq!(pin_value(&pin, "sdk_version").as_deref(), Some("3.13.7"));
        assert_eq!(pin_value(&pin, "sdk_revision").as_deref(), Some("416e"));
        assert_eq!(pin_value(&pin, "co19_revision"), None);
        let _ = std::fs::remove_dir_all(&work);
    }
}
