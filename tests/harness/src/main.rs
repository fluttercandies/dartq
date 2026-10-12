//! Differential-testing driver CLI:
//! - `harness corpus`: assemble the M0 synthetic corpus (tests/corpus/m0/)
//! - `harness run`: oracle vs dartq differential run, (code, file) entry Jaccard
//!
//! Exit codes: 0 = ERROR-tier Jaccard meets the gate (or the gate is not in scope
//! under the severity filter — pure-report mode, verdict SKIPPED); 1 = below gate;
//! 2 = harness error of its own.

mod bench;
mod bisect;
mod compare;
mod corpus;
mod dartq;
mod machine;
mod oracle;
mod replay;
mod replay_edit;
mod report;
mod repro;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compare::{Side, build_side_labeled, compare, merge_sides};
use dartq::DartqDriver;
use machine::{DIAG_TYPES, MachineDiag, OutputFormat, Severity};
use oracle::OracleDriver;

const DEFAULT_SOURCE: &str = ".third/dart-sdk/pkg/_fe_analyzer_shared/test";
const DEFAULT_TARGET: &str = "tests/corpus/m0";
const DEFAULT_OUT: &str = "target/harness-report";
const DEFAULT_THRESHOLD: f64 = 1.0;

#[derive(Debug)]
enum Outcome {
    Error(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        // (summary, exit success); the corpus subcommand always exits 0
        Ok((summary, exit_success)) => {
            if !summary.is_empty() {
                println!("{summary}");
            }
            if exit_success {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(Outcome::Error(msg)) => {
            eprintln!("harness error: {msg}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: &[String]) -> Result<(String, bool), Outcome> {
    match args.first().map(String::as_str) {
        Some("corpus") => run_corpus(&args[1..]).map(|s| (s, true)),
        Some("run") => run_compare(&args[1..]),
        Some("bench") => bench::bench_entry(&args[1..]),
        Some("replay") => replay::replay_entry(&args[1..]),
        Some("repro") => repro::repro_entry(&args[1..]),
        Some("--help" | "-h" | "help") | None => {
            print_usage();
            Ok((String::new(), true))
        }
        Some(other) => Err(Outcome::Error(format!(
            "unknown subcommand `{other}`; see `harness --help`"
        ))),
    }
}

// The usage text is one literal block; splitting it would fragment the
// exact output the CLI contract pins.
#[expect(clippy::too_many_lines, reason = "see above")]
fn print_usage() {
    println!(
        "usage:
  harness corpus [--source DIR] [--target DIR] [--rebuild] [--dart PATH]
      Assemble the M0 synthetic corpus (self-contained .dart files from the
      analyzer test suite, relative layout preserved) into a zero-dependency
      package. Default source: {DEFAULT_SOURCE}; default target: {DEFAULT_TARGET}.

  harness run [--corpus-dir DIR]... [--oracle-dart PATH] [--oracle-format machine|json]
              [--dartq-cmd TEMPLATE] [--e2e-oracle] [--severity-filter LIST]
              [--type-filter LIST] [--codes-file PATH] [--threshold F] [--out DIR]
              [--allow-vacuous] [--persist-regressions DIR] [--list-known]
      Run oracle and dartq over the corpus, compare (code, file) entry sets.
      --corpus-dir is repeatable: each dir must be a corpus package; every dir
      is analyzed with one invocation per dir and the entry sets are merged
      (keys prefixed with the corpus dir name to stay unambiguous).
      TEMPLATE: whitespace-split argv; {{dir}} is replaced by the corpus dir
      (appended as last arg when absent), e.g.
      --dartq-cmd \"cargo run -q -p dartq-cli -- check --format=machine\".
      --e2e-oracle: point the dartq side at dart analyze itself (both sides
      identical -> jaccard must be 1.000; strongest comparator self-check).
      Mutually exclusive with --dartq-cmd: e2e would silently win and the
      run would measure dart vs dart at 1.000.
      --severity-filter LIST: all | ERROR[,WARNING[,INFO]] — severities that
      enter the comparison. --type-filter LIST: all |
      syntactic_error[,compile_time_error[,static_warning[,lint]]] — machine
      TYPE values that enter the comparison (case-insensitive). --codes-file
      PATH: diagnostic code names, one per line, case-insensitive; `#` comment
      lines are skipped and a TAB-separated line contributes its first column
      (TSV compatible — the authoritative table is crates/dartq-diagnostics/
      data/syntax-codes.tsv). Only entries whose
      code is in the set enter the comparison on either side — oracle-only
      out-of-set entries are dropped before scoring and never inflate the
      denominator. The three filters stack; per-severity jaccards and the
      gate verdict are computed from the filtered sets only. M0 syntax
      caliber: codes-file (the 255 scanner/parser codes, tests/corpus/
      syntax-codes.txt = the TSV minus the 3 language-version-family codes)
      ∩ severity ERROR. An empty codes file is a usage error (zero
      discrimination). Non-vacuity guard: if the oracle side has no
      ERROR-tier entries after filtering, the ERROR gate would pass at 1.000
      via the empty-vs-empty rule without judging anything — the run fails as
      a harness error unless --allow-vacuous is given. --allow-vacuous:
      accept such a run (e.g. a genuinely clean corpus).
      Preflight: every corpus dir must carry a .dart_tool/package_config.json
      whose own languageVersion is >= 3.13 (Plan A, docs/facts/
      version-skew.md §4) — a stale pin floods the gate with
      experiment_not_enabled assembly noise and fails the run up front.
      Panic isolation: if the dartq side crashes (exit 4 with empty stdout),
      the corpus is auto-bisected to attribute the panicking files; the run
      still fails as a harness error (exit 2) and writes a dartq_panic_files
      attribution report under --out.
      --persist-regressions DIR: materialize every post-filter diffing (code,
      file) entry as a self-contained regression case under DIR —
      <hash8>.json (both sides' verbatim rows, the Dart source inline, the
      capturing run, and a replay_command argv) plus <hash8>.edits.jsonl (a
      content-identical whole-file replay step). Same failure -> same file
      (overwrite, no growth). Known divergences listed in tests/corpus/
      regressions/known-diffs.json (code + corpus-relative file, optional
      message_pattern substring) are suppressed; see --list-known.
      --list-known: offline triage face — prints the exemption table with
      ACTIVE / STALE / NO-EVIDENCE status derived from the persisted cases
      (cases dir: the --persist-regressions value, default tests/corpus/
      regressions/cases). Runs no analysis; needs no corpus.
      Raw outputs: every run persists both sides' complete machine outputs
      (all severities, unfiltered) to raw-outputs.json under --out, so
      post-hoc parity attribution (e.g. WARNING) works even when the gate
      filters to ERROR-only; report.json carries a raw_outputs pointer.
      Exit codes: 0 = ERROR jaccard >= threshold (default {DEFAULT_THRESHOLD})
      or the gate not in scope under --severity-filter (verdict SKIPPED,
      pure-report mode), 1 = below threshold, 2 = harness error.

  harness replay --corpus-dir DIR --edits FILE.jsonl --dartq-cmd TEMPLATE
                 --daemon-cmd TEMPLATE [--label NAME] [--out DIR]
      PRD §19.4 oracle incremental consistency: drive one live `dartq
      daemon` session with an edit sequence (overlay didChange + the
      `dartq/analyze` pull = the incremental result) while the same edits
      land on a materialized disk copy re-analyzed fresh per step (the
      `--dartq-cmd` full-recompute result); per step both sides normalize
      to one canonical shape and compare byte-wise. Report:
      <out>/<label>/{{report.json,report.md}}. Exit codes: 0 all steps
      equal, 1 any mismatch, 2 harness error.

  harness repro --corpus-dir DIR --dartq-cmd TEMPLATE [--runs N] [--out DIR]
      PRD §19.5 reproducibility: run the dartq side N times (default 2) over
      the corpus with fresh processes — the same template resolution the run
      face uses — normalize each run's machine rows through the differential
      face's normalizer (corpus-relative paths, lowercased codes,
      order-normalized), and byte-compare every run against run 1. Report:
      <out>/repro-report.json, whose `output_digest` is the run-1 normalized
      digest — distinct from the `output_sha256` scripts/repro-tuple.sh seals
      (the script hashes the raw output file, this the normalized comparison
      form) — so CI asserts same tuple ⇒ same digest across runs and
      machines. Exit codes: 0
      byte-identical, 1 divergence (first divergent row in the summary), 2
      harness error. Runs real dartq processes; serial verification phase
      only.

  harness bench [--metric NAME]... [--corpus-dir DIR] [--out DIR]
                [--manifest PATH] [--dartq-cmd TEMPLATE] [--oracle-dart PATH]
                [--rss-interval-ms MS] [--with-oracle] [--plugin-dir DIR]
                [--plugin-cycles N] [--sdk PATH]
      §19.3 benchmark metric collectors (PRD §1.5), one JSON report per
      metric under <out>/<metric>/report.json (schema_version 1; every
      report records its inputs for reproducibility). --metric is
      repeatable (rss-100k | incremental-p95 | incremental-p95-resident |
      token-cost | plugin-overhead | cold); default = all nine, in that
      order. Default corpus: {DEFAULT_TARGET}; default out: docs/audits/bench.
      rss-100k: peak tree-RSS of one full dartq analysis over the corpus
      (/bin/ps polling at --rss-interval-ms, default 50ms, process-group
      tree walk; an 8 GiB tree-RSS watchdog kills the sampled group),
      divided by LOC/100_000. LOC = non-blank lines (>= 1 non-whitespace
      character) across the corpus .dart files, .dart_tool excluded; the
      verbatim definition rides every report.
      incremental-p95: per-step wall time of the dartq driver over a
      materialized fixture package driven through --manifest (edit
      manifest JSON, incremental_oracle batch shape:
      {{\"initial\": {{path: content}}?, \"steps\": [{{\"edits\": [{{path,
      content | remove}}]}}]}}; initial present = fresh package from it,
      absent = a copy of the corpus). Reports P50/P95/P99 (nearest-rank)
      over the edit steps; --with-oracle additionally captures one full
      `dart analyze` wall time per step into the oracle column. This is the
      batch arm: one full driver process per step.
      incremental-p95-resident: the same --manifest driven through one
      in-process engine that stays warm across steps (no process restart;
      the resident face PRD §1.5's incremental target describes). Edits
      land as in-memory overlays (the daemon edit shape); each step is one
      timed `analyze` pass over the `dartq analyze` face's own workspace
      assembly. Reports the same P50/P95/P99 shape, comparable against the
      batch arm. SDK root: --sdk PATH > <root>/bin/dart derived from
      --oracle-dart > DART_SDK.
      token-cost: `dartq check --format=machine` stdout vs `dart analyze
      --format=machine` stdout on the same corpus, tokenized with the
      chars-per-4 heuristic (pluggable tokenizer trait; cl100k slots in
      behind it); reports both totals and the savings ratio.
      plugin-overhead: the cost the plugin subsystem adds per analyze cycle
      — per-cycle wall time of one feed+drain round trip against a live
      plugin host (--plugin-dir, default the samples/compat compatibility
      plugin; --plugin-cycles measured cycles, default 50, plus one
      unmeasured warmup), P50/P95/P99, and the plugin sidecar's peak RSS
      (the with-plugins side of the with/without comparison: no plugin, no
      sidecar). Needs `dart` on PATH and the samples' host package pub-getted.
      cold: one cold full-analysis wall time per side over the same corpus —
      the dartq run under the same sampled spawn as rss-100k (watchdog
      attached), the oracle as one full `dart analyze` — plus the oracle/dartq
      ratio (the cold-analysis charter face). Single run per side: at small
      corpora the times are process-boot dominated and the report carries a
      caveat note; cite only large-corpus ratios as milestone evidence.
      The collectors drive real analysis processes; run them in the serial
      verification phase only (memory-incident discipline).
      Exit codes: 0 = every requested metric produced a report, 2 = harness
      error (bad flags, corpus preflight, driver failure, watchdog trip)."
    );
}

fn run_corpus(args: &[String]) -> Result<String, Outcome> {
    let mut source = PathBuf::from(DEFAULT_SOURCE);
    let mut target = PathBuf::from(DEFAULT_TARGET);
    let mut rebuild = false;
    let mut dart = "dart".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => source = value(args, &mut i)?.into(),
            "--target" => target = value(args, &mut i)?.into(),
            "--dart" => dart = value(args, &mut i)?.to_string(),
            "--rebuild" => rebuild = true,
            other => return Err(unknown_flag(other, "corpus")),
        }
        i += 1;
    }

    if !source.is_dir() {
        return Err(Outcome::Error(format!(
            "source dir `{}` does not exist (clone .third/dart-sdk first)",
            source.display()
        )));
    }
    let report = corpus::build_corpus(&source, &target, rebuild).map_err(Outcome::Error)?;
    corpus::pub_get(&dart, &target).map_err(Outcome::Error)?;
    // Same preflight as `run`: assembly is only valid when pub recorded a
    // languageVersion >= 3.13 (Plan A, docs/facts/version-skew.md §4).
    corpus::check_language_version(&target).map_err(Outcome::Error)?;
    let target_display = target.canonicalize().map_or_else(
        |_| target.display().to_string(),
        |p| p.display().to_string(),
    );
    Ok(format!(
        "corpus ready: {target_display} (files: {}, loc: {}, skipped: {}{})",
        report.files,
        report.loc,
        report.skipped,
        if rebuild { ", rebuilt" } else { "" }
    ))
}

struct RunOptions {
    /// Corpus package dirs, repeatable; default = `tests/corpus/m0`.
    corpus_dirs: Vec<PathBuf>,
    oracle_dart: String,
    oracle_format: OutputFormat,
    dartq_cmd: Option<String>,
    severity_filter: String,
    type_filter: String,
    codes_file: Option<PathBuf>,
    threshold: f64,
    out: PathBuf,
    e2e_oracle: bool,
    /// Escape hatch for the non-vacuity guard: accept a run whose oracle side
    /// has no ERROR-tier entries post-filter (would otherwise pass at 1.0
    /// without judging anything).
    allow_vacuous: bool,
    /// `--persist-regressions <dir>`: materialize every post-filter diffing
    /// entry as a self-contained regression case under `dir` (PRD §19.1).
    persist_regressions: Option<PathBuf>,
    /// `--list-known`: offline face printing the exemption table
    /// (tests/corpus/regressions/known-diffs.json) with stale entries marked;
    /// runs no analysis.
    list_known: bool,
}

impl RunOptions {
    fn parse(args: &[String]) -> Result<Self, Outcome> {
        let mut opts = Self {
            corpus_dirs: Vec::new(),
            oracle_dart: "dart".to_string(),
            oracle_format: OutputFormat::Machine,
            dartq_cmd: None,
            severity_filter: "all".to_string(),
            type_filter: "all".to_string(),
            codes_file: None,
            threshold: DEFAULT_THRESHOLD,
            out: PathBuf::from(DEFAULT_OUT),
            e2e_oracle: false,
            allow_vacuous: false,
            persist_regressions: None,
            list_known: false,
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--corpus-dir" => opts.corpus_dirs.push(value(args, &mut i)?.into()),
                "--oracle-dart" => opts.oracle_dart = value(args, &mut i)?.to_string(),
                "--oracle-format" => {
                    opts.oracle_format =
                        OutputFormat::parse(value(args, &mut i)?).ok_or_else(|| {
                            Outcome::Error("invalid --oracle-format (machine|json)".into())
                        })?;
                }
                "--dartq-cmd" => opts.dartq_cmd = Some(value(args, &mut i)?.to_string()),
                "--severity-filter" => opts.severity_filter = value(args, &mut i)?.to_string(),
                "--type-filter" => opts.type_filter = value(args, &mut i)?.to_string(),
                "--codes-file" => opts.codes_file = Some(value(args, &mut i)?.into()),
                "--threshold" => {
                    opts.threshold = value(args, &mut i)?
                        .parse()
                        .ok()
                        .filter(|t| (0.0..=1.0).contains(t))
                        .ok_or_else(|| {
                            Outcome::Error("--threshold must be in [0.0, 1.0]".into())
                        })?;
                }
                "--out" => opts.out = value(args, &mut i)?.into(),
                "--e2e-oracle" => opts.e2e_oracle = true,
                "--allow-vacuous" => opts.allow_vacuous = true,
                "--persist-regressions" => {
                    opts.persist_regressions = Some(value(args, &mut i)?.into());
                }
                "--list-known" => opts.list_known = true,
                other => return Err(unknown_flag(other, "run")),
            }
            i += 1;
        }
        if opts.e2e_oracle && opts.dartq_cmd.is_some() {
            return Err(Outcome::Error(
                "--e2e-oracle and --dartq-cmd are mutually exclusive: e2e pins the dartq \
                 side to dart analyze itself, so passing both would silently measure dart vs \
                 dart at 1.000"
                    .into(),
            ));
        }
        Ok(opts)
    }
}

// Subcommand dispatcher: one arm per face, mirroring the CLI surface.
#[expect(clippy::too_many_lines, reason = "see above")]
fn run_compare(args: &[String]) -> Result<(String, bool), Outcome> {
    let opts = RunOptions::parse(args)?;
    // --list-known is an offline triage face: it reads the exemption table
    // and the persisted cases, runs no analysis, and needs no corpus.
    if opts.list_known {
        let cases_dir = opts
            .persist_regressions
            .clone()
            .unwrap_or_else(|| PathBuf::from(report::REGRESSIONS_ROOT).join(report::CASES_DIR));
        let known_path = PathBuf::from(report::REGRESSIONS_ROOT).join(report::KNOWN_DIFFS_FILE);
        let summary = report::list_known(&known_path, &cases_dir).map_err(Outcome::Error)?;
        return Ok((summary, true));
    }
    let mut opts = opts;
    if opts.corpus_dirs.is_empty() {
        opts.corpus_dirs.push(PathBuf::from(DEFAULT_TARGET));
    }
    let codes = match &opts.codes_file {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                Outcome::Error(format!("reading --codes-file {}: {e}", path.display()))
            })?;
            Some(parse_codes_file(&text)?)
        }
        None => None,
    };
    let canonical = canonical_corpus_dirs(&opts.corpus_dirs)?;
    let labels = corpus_labels(&canonical);
    let corpora: Vec<(Option<String>, PathBuf)> = labels.into_iter().zip(canonical).collect();
    let filters = Filters {
        severities: parse_severity_filter(&opts.severity_filter)?,
        types: parse_type_filter(&opts.type_filter)?,
        codes,
    };

    let oracle = OracleDriver {
        dart: opts.oracle_dart.clone(),
    };
    // Oracle reference version, captured once per run (PRD §19.1: every report records it)
    let oracle_version = oracle::dart_version(&opts.oracle_dart).map_err(Outcome::Error)?;
    let oracle_diags = analyze_corpora(&corpora, |dir| oracle.analyze(dir, opts.oracle_format))?;
    let oracle_side = build_filtered_side(&corpora, &oracle_diags, &filters);
    // Fail before the dartq run: a vacuous oracle side would pass at 1.0 without judging anything
    check_oracle_vacuity(&oracle_side, filters.error_in_scope(), opts.allow_vacuous)?;
    // The dartq contract is always machine; only the e2e self-check (dartq side = dart analyze itself) follows the oracle format
    let dartq_format = if opts.e2e_oracle {
        opts.oracle_format
    } else {
        OutputFormat::Machine
    };
    let dartq_driver = build_dartq_driver(&opts, opts.oracle_format)?;

    // Meta is built before the dartq phase: the panic path reports it too.
    let meta = build_run_meta(&opts, &corpora, oracle_version);

    // dartq phase with panic isolation: a crash tier (exit 4, empty stdout)
    // triggers a corpus auto-bisect; the run still fails as a harness error,
    // but the failure carries the attributed panicking files.
    let dartq_diags = match bisect::dartq_phase(&corpora, &dartq_driver, dartq_format) {
        Ok(diags) => diags,
        Err(bisect::DartqPhaseError::Hard(msg)) => return Err(Outcome::Error(msg)),
        Err(bisect::DartqPhaseError::Panicked(attribution)) => {
            let json_path = report::write_panic_report(&meta, &attribution, &opts.out)
                .map_err(Outcome::Error)?;
            return Err(Outcome::Error(attribution.error_message(&json_path)));
        }
    };

    let dartq_side = build_filtered_side(&corpora, &dartq_diags, &filters);
    let cmp = compare(&oracle_side, &dartq_side);

    let verdict = gate_verdict(
        filters.error_in_scope(),
        cmp.error_jaccard(),
        opts.threshold,
    );
    // Every gate run persists both sides' raw machine outputs (all severities,
    // unfiltered) next to the reports — the post-hoc attribution data plane.
    let raw = report::RawSideOutputs {
        corpora: &corpora,
        oracle: &oracle_diags,
        dartq: &dartq_diags,
    };
    let (json_path, md_path) =
        report::write_reports(&cmp, &meta, verdict, &raw, &opts.out).map_err(Outcome::Error)?;

    // Regression persistence (PRD §19.1): when asked, every post-filter
    // diffing entry is materialized as a self-contained case under the
    // persist dir — known divergences suppressed via the exemption table.
    let persist_note = match &opts.persist_regressions {
        Some(dir) => {
            let known_path = PathBuf::from(report::REGRESSIONS_ROOT).join(report::KNOWN_DIFFS_FILE);
            let known = report::load_known_diffs(&known_path).map_err(Outcome::Error)?;
            let captured_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let stats = report::persist_regressions(
                &cmp,
                &raw,
                &report::PersistOptions {
                    dir,
                    known: &known,
                    report_path: &json_path,
                    dartq_cmd: &meta.dartq_cmd,
                    oracle_cmd: &meta.oracle_cmd,
                    captured_at,
                },
            )
            .map_err(Outcome::Error)?;
            format!(
                "\nregressions: {} case(s) persisted to {} ({} file(s) fully exempt, \
                 {} diff entr{} exempt, {} skipped)",
                stats.files_persisted,
                dir.display(),
                stats.files_exempt,
                stats.entries_exempt,
                if stats.entries_exempt == 1 {
                    "y"
                } else {
                    "ies"
                },
                stats.files_skipped,
            )
        }
        None => String::new(),
    };

    let stats: Vec<String> = cmp
        .severities
        .iter()
        .map(|s| format!("{}={:.3}", s.severity.as_str(), s.jaccard))
        .collect();
    let summary = format!(
        "corpus: {} (files with diagnostics: {})\n\
         oracle: {} diags | dartq: {} diags\n\
         jaccard: {} (threshold {} on ERROR)\n\
         diffs: oracle-only {} entries, dartq-only {} entries\n\
         reports: {} {}\n\
         result: {}{}",
        meta.corpus_dir,
        cmp.file_count,
        cmp.oracle_total,
        cmp.dartq_total,
        stats.join("  "),
        opts.threshold,
        cmp.oracle_only.len(),
        cmp.dartq_only.len(),
        json_path.display(),
        md_path.display(),
        verdict.token(),
        persist_note,
    );
    Ok((summary, verdict.exit_success()))
}

/// Report meta: everything known before the comparison runs; both report
/// shapes (standard run + panic attribution) record it verbatim.
fn build_run_meta(
    opts: &RunOptions,
    corpora: &[(Option<String>, PathBuf)],
    oracle_version: String,
) -> report::RunMeta {
    let corpus_display = corpora
        .iter()
        .map(|(_, canon)| canon.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    report::RunMeta {
        corpus_dir: corpus_display,
        oracle_cmd: format!("{} analyze {}", opts.oracle_dart, opts.oracle_format.flag()),
        dartq_cmd: if opts.e2e_oracle {
            format!(
                "{} analyze {} (e2e self-check)",
                opts.oracle_dart,
                opts.oracle_format.flag()
            )
        } else {
            opts.dartq_cmd.clone().unwrap_or_default()
        },
        oracle_version,
        threshold: opts.threshold,
        severity_filter: opts.severity_filter.clone(),
        type_filter: opts.type_filter.clone(),
        codes_file: opts
            .codes_file
            .as_ref()
            .map_or(String::new(), |p| p.display().to_string()),
    }
}

/// Map the ERROR gate to its verdict: judged against the threshold when the ERROR
/// tier is in scope, otherwise skipped — a gate that never ran must not print
/// PASS (or set `passed: true`) indistinguishably from a judged pass.
fn gate_verdict(error_in_scope: bool, error_jaccard: f64, threshold: f64) -> report::Verdict {
    if !error_in_scope {
        report::Verdict::Skipped
    } else if error_jaccard >= threshold {
        report::Verdict::Pass
    } else {
        report::Verdict::Fail
    }
}

pub(crate) fn canonical_corpus(corpus_dir: &Path) -> Result<PathBuf, Outcome> {
    if !corpus_dir.join("pubspec.yaml").is_file() {
        return Err(Outcome::Error(format!(
            "`{}` is not a corpus package; run `harness corpus` first",
            corpus_dir.display()
        )));
    }
    corpus_dir.canonicalize().map_err(|e| {
        Outcome::Error(format!(
            "resolving corpus dir {}: {e}",
            corpus_dir.display()
        ))
    })
}

/// Canonicalize every corpus dir in declaration order, failing fast on the Plan A
/// languageVersion preflight (docs/facts/version-skew.md §4: a stale corpus
/// languageVersion re-enables `experiment_not_enabled` skew; fail before any oracle
/// run so the noise can never reach a report).
///
/// Duplicates — the same canonical dir passed twice, e.g. via differing spellings —
/// are dropped: a repeated dir would run each driver twice and double every
/// instance count in the merged sides.
fn canonical_corpus_dirs(dirs: &[PathBuf]) -> Result<Vec<PathBuf>, Outcome> {
    let mut canonical: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let canon = canonical_corpus(dir)?;
        if canonical.contains(&canon) {
            continue;
        }
        corpus::check_language_version(&canon).map_err(Outcome::Error)?;
        canonical.push(canon);
    }
    Ok(canonical)
}

/// Key prefixes for multi-corpus merges: the corpus dir basename. Two corpora
/// sharing a basename (e.g. two checkouts both named `m0`) would merge their
/// entry keys into one bucket, so colliding basenames fall back to the full
/// canonical path — the prefixes are then trivially disjoint. Single corpus =
/// all `None`, keeping the key shape identical to history.
fn corpus_labels(canonical_dirs: &[PathBuf]) -> Vec<Option<String>> {
    if canonical_dirs.len() <= 1 {
        return vec![None];
    }
    let basename = |dir: &Path| {
        dir.file_name()
            .expect("canonicalized dir has a file name")
            .to_string_lossy()
            .into_owned()
    };
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for dir in canonical_dirs {
        *counts.entry(basename(dir)).or_default() += 1;
    }
    canonical_dirs
        .iter()
        .map(|dir| {
            let name = basename(dir);
            Some(if counts[&name] > 1 {
                dir.display().to_string()
            } else {
                name
            })
        })
        .collect()
}

/// Build the dartq-side driver: e2e self-check = dart analyze itself; otherwise a command template is required.
fn build_dartq_driver(
    opts: &RunOptions,
    oracle_format: OutputFormat,
) -> Result<DartqDriver, Outcome> {
    if opts.e2e_oracle {
        return Ok(DartqDriver::from_program(
            opts.oracle_dart.clone(),
            vec![
                "analyze".into(),
                oracle_format.flag().into(),
                "--no-fatal-warnings".into(),
            ],
        ));
    }
    let template = opts
        .dartq_cmd
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Outcome::Error("run needs --dartq-cmd TEMPLATE (or --e2e-oracle)".into()))?;
    Ok(DartqDriver::from_template(template))
}

/// Invoke the driver once per corpus dir (one process per dir), returning diagnostic groups parallel to corpora.
fn analyze_corpora(
    corpora: &[(Option<String>, PathBuf)],
    analyze: impl Fn(&Path) -> Result<Vec<MachineDiag>, String>,
) -> Result<Vec<Vec<MachineDiag>>, Outcome> {
    corpora
        .iter()
        .map(|(_, canon)| analyze(canon).map_err(Outcome::Error))
        .collect()
}

/// `None` = all; `Some(list)` = only these tiers enter the comparison. The threshold is permanently bound to the
/// ERROR tier (task semantics); when a filter drops ERROR, run degrades to pure-report mode (exit 0).
fn parse_severity_filter(spec: &str) -> Result<Option<Vec<Severity>>, Outcome> {
    if spec.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    let list = spec
        .split(',')
        .map(str::trim)
        .map(|s| {
            Severity::parse(&s.to_ascii_uppercase()).ok_or_else(|| {
                Outcome::Error(format!(
                    "invalid --severity-filter `{spec}` (all | ERROR[,WARNING[,INFO]])"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(list))
}

/// `None` = all; `Some(list)` = only these machine TYPEs enter the comparison (case-insensitive,
/// domain [`machine::DIAG_TYPES`]).
fn parse_type_filter(spec: &str) -> Result<Option<Vec<String>>, Outcome> {
    if spec.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    let list = spec
        .split(',')
        .map(str::trim)
        .map(|s| {
            let upper = s.to_ascii_uppercase();
            if DIAG_TYPES.contains(&upper.as_str()) {
                Ok(upper)
            } else {
                Err(Outcome::Error(format!(
                    "invalid --type-filter `{spec}` (all | {})",
                    DIAG_TYPES
                        .iter()
                        .map(|t| t.to_ascii_lowercase())
                        .collect::<Vec<_>>()
                        .join(",")
                )))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(list))
}

/// Three orthogonal, stacking filters: severity × type × codes; `None` = no filtering on that dimension.
///
/// codes (--codes-file): comparison and gate scoring keep only entries whose code is in the set --
/// oracle-only out-of-set entries are dropped and never enter the denominator. This is exactly the
/// M0 syntax-tier criterion: **codes-file (the 255 scanner/parser codes, see
/// tests/corpus/syntax-codes.txt) ∩ severity ERROR**; the analyzer reports `ParserErrorCode` as
/// `COMPILE_TIME_ERROR` (docs/facts/corpus-baseline.md §1), so the syntax tier cannot be obtained by
/// `--type-filter syntactic_error` alone and must be filtered by code set.
///
/// Empty-set semantics: a filter may remove all diagnostics from both sides -- then both entry sets
/// are empty and Jaccard keeps the existing rule "both empty = full agreement = 1.0" (see
/// `compare::jaccard`), so the gate would still pass; same as the pure-report mode when
/// severity-filter drops the ERROR tier. On top of that rule stands the non-vacuity guard
/// ([`check_oracle_vacuity`]): a run whose oracle side has no ERROR-tier entries post-filter is a
/// harness error unless `--allow-vacuous` is given.
struct Filters {
    severities: Option<Vec<Severity>>,
    types: Option<Vec<String>>,
    codes: Option<BTreeSet<String>>,
}

impl Filters {
    fn keep(&self, d: &MachineDiag) -> bool {
        self.severities
            .as_ref()
            .is_none_or(|s| s.contains(&d.severity))
            && self.types.as_ref().is_none_or(|t| t.contains(&d.dtype))
            && self
                .codes
                .as_ref()
                .is_none_or(|c| c.contains(&d.code.to_lowercase()))
    }

    /// Whether the ERROR tier — the gate metric's tier — enters the comparison
    /// under the severity filter (`false` = pure-report mode, no gate to be vacuous about).
    fn error_in_scope(&self) -> bool {
        self.severities
            .as_ref()
            .is_none_or(|s| s.contains(&Severity::Error))
    }
}

/// Non-vacuity guard: the ERROR gate scores "both empty = 1.0", so an oracle side that
/// survives the filters with zero ERROR-tier entries would pass at 1.0 without judging
/// anything (a broken --codes-file, a wrong corpus dir, a severity/type filter mismatch,
/// or an oracle that silently produced nothing). Such a run is a harness error unless
/// `--allow-vacuous` accepts it explicitly.
fn check_oracle_vacuity(
    oracle_side: &Side,
    error_in_scope: bool,
    allow_vacuous: bool,
) -> Result<(), Outcome> {
    if allow_vacuous || !error_in_scope {
        return Ok(());
    }
    let error_tier_empty = oracle_side
        .per_severity
        .get(&Severity::Error)
        .is_none_or(BTreeSet::is_empty);
    if !error_tier_empty {
        return Ok(());
    }
    Err(Outcome::Error(format!(
        "oracle side is vacuous post-filter: {} pre-filter entries, {} dropped by the filters, \
         0 ERROR-tier entries survive — the ERROR gate would pass at 1.000 without judging \
         anything (pass --allow-vacuous to accept)",
        oracle_side.pre_filter_total, oracle_side.dropped_by_filter
    )))
}

/// Filter per corpus dir, build a side per dir, then merge; key prefixes come from the corpora labels (None for a single corpus).
/// Borrows the per-corpus diagnostic lists: the unfiltered raw outputs stay alive for
/// [`report::write_reports`], which persists them on every run.
fn build_filtered_side(
    corpora: &[(Option<String>, PathBuf)],
    per_corpus_diags: &[Vec<MachineDiag>],
    filters: &Filters,
) -> Side {
    let sides = corpora
        .iter()
        .zip(per_corpus_diags)
        .map(|((label, canon), diags)| {
            build_side_labeled(diags, canon, label.as_deref(), |d| filters.keep(d))
        })
        .collect();
    merge_sides(sides)
}

/// --codes-file content: one code name per line, case-insensitive. TSV
/// compatible: `#` comment lines are skipped and a TAB-separated line
/// contributes its first column — the authoritative table
/// `crates/dartq-diagnostics/data/syntax-codes.tsv` (258 rows = 255
/// scanner/parser codes + 3 language-version-family codes) can be passed
/// directly. The M0 gate input is `tests/corpus/syntax-codes.txt`, the TSV
/// minus the 3 language-version-family codes (docs/facts/version-skew.md §5.3:
/// skew codes must not enter the gate denominator); the derivation is pinned
/// by `gate_codes_file_matches_authoritative_tsv` below.
///
/// Zero codes (no data lines) = usage error: an empty code set has zero gate
/// discrimination — fail fast instead of silently passing everything.
fn parse_codes_file(text: &str) -> Result<BTreeSet<String>, Outcome> {
    // A UTF-8 BOM (U+FEFF) on the first line must not glue itself onto the
    // first code (editors add BOMs); a BOM elsewhere is content and stays.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let codes: BTreeSet<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').next().unwrap_or("").trim().to_lowercase())
        .filter(|code| !code.is_empty())
        .collect();
    if codes.is_empty() {
        return Err(Outcome::Error(
            "codes file is empty; expected one diagnostic code name per line".into(),
        ));
    }
    Ok(codes)
}

pub(crate) fn value<'a>(args: &'a [String], i: &mut usize) -> Result<&'a str, Outcome> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| Outcome::Error(format!("flag `{}` requires a value", args[*i - 1])))
}

pub(crate) fn unknown_flag(flag: &str, subcommand: &str) -> Outcome {
    Outcome::Error(format!(
        "unknown flag `{flag}` for `{subcommand}`; see `harness --help`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn severity_filter_parsing() {
        assert!(parse_severity_filter("all").unwrap().is_none());
        assert!(parse_severity_filter("ALL").unwrap().is_none());
        let f = parse_severity_filter("ERROR,warning").unwrap().unwrap();
        assert_eq!(f, vec![Severity::Error, Severity::Warning]);
        assert!(parse_severity_filter("FATAL").is_err());
        assert!(parse_severity_filter("").is_err());
    }

    #[test]
    fn type_filter_parsing() {
        assert!(parse_type_filter("all").unwrap().is_none());
        assert!(parse_type_filter("ALL").unwrap().is_none());
        let single = parse_type_filter("syntactic_error").unwrap().unwrap();
        assert_eq!(single, vec!["SYNTACTIC_ERROR".to_string()]);
        let stacked = parse_type_filter("Syntactic_Error,lint").unwrap().unwrap();
        assert_eq!(
            stacked,
            vec!["SYNTACTIC_ERROR".to_string(), "LINT".to_string()]
        );
        assert!(
            parse_type_filter("FATAL").is_err(),
            "unknown TYPE must error"
        );
        assert!(parse_type_filter("").is_err());
    }

    /// Two filters stacked orthogonally: with severity=ERROR and `type=SYNTACTIC_ERROR`, semantic
    /// errors and static warnings are both dropped; only syntactic errors enter the comparison.
    #[test]
    fn stacked_filters_keep_only_matching_diag() {
        let filters = Filters {
            severities: Some(vec![Severity::Error]),
            types: Some(vec!["SYNTACTIC_ERROR".to_string()]),
            codes: None,
        };
        let mk = |severity: Severity, dtype: &str| MachineDiag {
            severity,
            dtype: dtype.to_string(),
            code: "X".to_string(),
            file: std::path::PathBuf::from("a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".to_string(),
        };
        assert!(filters.keep(&mk(Severity::Error, "SYNTACTIC_ERROR")));
        assert!(
            !filters.keep(&mk(Severity::Error, "COMPILE_TIME_ERROR")),
            "semantic error dropped by type filter"
        );
        assert!(
            !filters.keep(&mk(Severity::Warning, "SYNTACTIC_ERROR")),
            "warning dropped by severity filter"
        );
        // A dimension with no filter = everything passes on that dimension
        let only_type = Filters {
            severities: None,
            types: filters.types.clone(),
            codes: None,
        };
        assert!(only_type.keep(&mk(Severity::Info, "SYNTACTIC_ERROR")));
        let none = Filters {
            severities: None,
            types: None,
            codes: None,
        };
        assert!(none.keep(&mk(Severity::Warning, "STATIC_WARNING")));
    }

    /// --codes-file parsing: one code name per line, case-normalized, blank
    /// and `#` comment lines skipped, TSV rows contribute the first column;
    /// empty file = usage error (an empty code set has zero gate
    /// discrimination — fail fast).
    #[test]
    fn codes_file_parsing() {
        let codes = parse_codes_file("expected_token\n\nmissing_identifier  \nMISSING_HEX_DIGIT\n")
            .expect("valid codes file");
        assert_eq!(
            codes,
            BTreeSet::from([
                "expected_token".to_string(),
                "missing_identifier".to_string(),
                "missing_hex_digit".to_string(),
            ])
        );
        // TSV rows: first TAB column only; comment lines skipped.
        let tsv = "# header comment\n\
                   expected_token\tEXPECTED_TOKEN\tERROR\tP\tmessage\n\
                   \n\
                   missing_identifier\tMISSING_IDENTIFIER\tERROR\tP\tmessage\n";
        let codes = parse_codes_file(tsv).expect("valid TSV codes file");
        assert_eq!(
            codes,
            BTreeSet::from([
                "expected_token".to_string(),
                "missing_identifier".to_string(),
            ])
        );
        assert!(parse_codes_file("").is_err(), "empty file must error");
        assert!(
            parse_codes_file("\n  \n").is_err(),
            "blank-only file must error"
        );
        assert!(
            parse_codes_file("# only comments\n\n").is_err(),
            "comment-only file must error"
        );
    }

    /// A UTF-8 BOM on the first line must not glue itself onto the first code
    /// (editors add BOMs); the first code still enters the set verbatim.
    #[test]
    fn codes_file_bom_is_stripped() {
        let codes = parse_codes_file("\u{feff}expected_token\nmissing_identifier\n")
            .expect("BOM-prefixed file parses");
        assert_eq!(
            codes,
            BTreeSet::from([
                "expected_token".to_string(),
                "missing_identifier".to_string(),
            ])
        );
        // A BOM not at the start is content: it makes the code unmatchable,
        // which stays visible as an (empty-matching) set entry rather than
        // being silently stripped.
        let codes = parse_codes_file("a\n\u{feff}expected_token\n").expect("parses");
        assert!(codes.contains("\u{feff}expected_token"));
    }

    /// Two corpora with the same basename (e.g. two checkouts both named
    /// `m0`) must not merge their entry keys: colliding basenames fall back
    /// to the full canonical path so the prefixes are disjoint; unique
    /// basenames keep the short form and a single corpus stays unlabeled.
    #[test]
    fn same_basename_corpora_get_disjoint_labels() {
        let root = std::env::temp_dir().join(format!("dq_labels_{}", std::process::id()));
        let a = root.join("a").join("m0");
        let b = root.join("b").join("m0");
        let other = root.join("other");
        for dir in [&a, &b, &other] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let a_canon = a.canonicalize().expect("canonicalize a");
        let b_canon = b.canonicalize().expect("canonicalize b");

        let labels = corpus_labels(&[a_canon.clone(), b_canon.clone()]);
        assert_ne!(
            labels[0], labels[1],
            "colliding basenames must disambiguate"
        );
        assert_eq!(
            labels[0].as_deref(),
            Some(a_canon.to_str().expect("utf8 temp path"))
        );
        assert_eq!(
            labels[1].as_deref(),
            Some(b_canon.to_str().expect("utf8 temp path"))
        );

        // Unique basenames keep the short, readable form
        let labels = corpus_labels(&[a_canon.clone(), other.canonicalize().expect("canonicalize")]);
        assert_eq!(labels[0].as_deref(), Some("m0"));
        assert_eq!(labels[1].as_deref(), Some("other"));

        // Single corpus = unlabeled (historical key shape)
        assert_eq!(corpus_labels(&[a_canon]), vec![None]);

        std::fs::remove_dir_all(&root).ok();
    }

    /// Derivation pin: the M0 gate codes file (tests/corpus/syntax-codes.txt)
    /// must stay exactly the authoritative TSV
    /// (crates/dartq-diagnostics/data/syntax-codes.tsv) minus the 3
    /// language-version-family codes (docs/facts/version-skew.md §5.3: skew
    /// codes must not enter the gate denominator). Catches silent drift
    /// between the two files and parser regression in one shot.
    #[test]
    fn gate_codes_file_matches_authoritative_tsv() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness");
        let tsv =
            std::fs::read_to_string(root.join("crates/dartq-diagnostics/data/syntax-codes.tsv"))
                .expect("authoritative TSV present");
        let txt = std::fs::read_to_string(root.join("tests/corpus/syntax-codes.txt"))
            .expect("gate codes file present");
        let tsv_codes = parse_codes_file(&tsv).expect("TSV parses");
        let gate_codes = parse_codes_file(&txt).expect("gate file parses");
        assert_eq!(tsv_codes.len(), 258, "TSV = 255 syntax + 3 family codes");
        assert_eq!(gate_codes.len(), 255, "gate file = 255 syntax codes");
        // 255 raw data lines for the gate file (comments/blanks excluded): a
        // duplicated line would hide in the set comparison but still be drift.
        assert_eq!(
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
            255
        );
        let family: BTreeSet<String> = [
            "illegal_language_version_override",
            "inconsistent_language_version_override",
            "invalid_language_version_override",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            tsv_codes
                .difference(&gate_codes)
                .cloned()
                .collect::<BTreeSet<_>>(),
            family,
            "gate file = TSV minus exactly the language-version family"
        );
        assert!(
            gate_codes.is_subset(&tsv_codes),
            "gate codes must all exist in the authoritative TSV"
        );
    }

    /// Derivation pin: the M1 gate codes file (`tests/harness/m1_codes.txt`)
    /// must equal its documented derivation (file header, PRD section 20.3 M1
    /// row): the `undefined_*` and `wrong_number_of_*` wildcard families expanded
    /// over the SDK main unified registry (.third/dart-sdk pkg/analyzer
    /// lib/src/diagnostic/diagnostic.g.dart, `name:` literals), unioned with
    /// the explicit picks (`uri_does_not_exist`, `duplicate_definition`, the
    /// static-nullability four), plus `undefined_annotation`, minus
    /// `undefined_lint` (an analysis-options code, not a Dart-language
    /// diagnostic; present in the registry, so the exclusion is load-bearing).
    ///
    /// The analyzer-8.4.1 side of the header's union contributes exactly one
    /// gate code (`undefined_annotation`, `lib/src/error/codes.g.dart`); it is
    /// pinned as a frozen const because a published package version is
    /// immutable — re-reading it at test time detects nothing — and no
    /// workspace lockfile pins 8.4.1 into pub-cache, so a live read would be
    /// a spurious environmental failure. The SDK main registry is the live
    /// drift surface: a .third re-clone that adds or removes a wildcard
    /// family member fails this pin and forces a conscious re-freeze.
    /// `wrong_number_of_type_arguments_method` exists only in analyzer-8.4.1,
    /// never in the SDK main registry, and is intentionally not gated; the
    /// SDK-main-anchored derivation needs no exclusion for it.
    #[test]
    fn m1_codes_file_matches_derivation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness");
        let registry = std::fs::read_to_string(
            root.join(".third/dart-sdk/pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart"),
        )
        .expect("SDK main diagnostic registry present (.third/dart-sdk checkout)");
        let txt = std::fs::read_to_string(root.join("tests/harness/m1_codes.txt"))
            .expect("gate codes file present");
        let gate_codes = parse_codes_file(&txt).expect("gate file parses");

        // `name: 'snake_case',` at line start; `uniqueName:` never matches
        // because the anchor sits directly after leading whitespace.
        let registry_codes: BTreeSet<String> = registry
            .lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("name: '")?;
                rest.strip_suffix("',")
            })
            .map(str::to_string)
            .collect();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for prefix in ["undefined_", "wrong_number_of_"] {
            expected.extend(
                registry_codes
                    .iter()
                    .filter(|code| code.starts_with(prefix))
                    .cloned(),
            );
        }
        expected.extend(
            [
                "uri_does_not_exist",
                "duplicate_definition",
                "invalid_assignment",
                "argument_type_not_assignable",
                "return_of_invalid_type",
                "invalid_null_aware_operator",
                // Frozen analyzer-8.4.1-only addition (see doc above).
                "undefined_annotation",
            ]
            .into_iter()
            .map(str::to_string),
        );
        expected.remove("undefined_lint");

        assert_eq!(gate_codes.len(), 36, "M1 gate freezes 36 codes");
        // 36 raw data lines (comments/blanks excluded): a duplicated line
        // would hide in the set comparison but still be drift.
        assert_eq!(
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
            36
        );
        assert_eq!(
            gate_codes, expected,
            "gate file = documented M1 derivation over the SDK main registry"
        );
    }

    /// Derivation pin: the M2 WARNING-tier gate codes file
    /// (`tests/harness/m2_warning_codes.txt`) must equal its documented
    /// derivation (file header, PRD section 20.3 M2 row): the SDK main
    /// unified registry's WARNING severity family (`type:
    /// DiagnosticType.STATIC_WARNING`, what `dart analyze --format=machine`
    /// reports as WARNING) intersected with the per-bucket freeze list
    /// (M1 frozen-set continuation, flow/null-safety soundness,
    /// patterns/records/switch exhaustiveness, class modifiers, const and
    /// literal semantics, combinator duplicates, override semantics).
    /// Unlike the M1 pin, the derivation pairs each `name:` literal with
    /// the following `type:` line: severity is per-entry, not per-name
    /// (`multiple_combinators` is registered under two types; it is
    /// excluded by the tooling family either way). Three long-named
    /// entries wrap the `name:` value onto the next line and are skipped
    /// by the line-anchored parse; all three are COMPILE_TIME_ERROR, so
    /// the WARNING family is unaffected.
    #[test]
    fn m2_warning_codes_file_matches_derivation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness");
        let registry = std::fs::read_to_string(
            root.join(".third/dart-sdk/pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart"),
        )
        .expect("SDK main diagnostic registry present (.third/dart-sdk checkout)");
        let txt = std::fs::read_to_string(root.join("tests/harness/m2_warning_codes.txt"))
            .expect("gate codes file present");
        let gate_codes = parse_codes_file(&txt).expect("gate file parses");

        // Pair each `name: 'snake_case',` (line-anchored) with the next
        // `type: DiagnosticType.X,` line; collect the STATIC_WARNING names.
        let mut pending: Option<String> = None;
        let mut warning_family: BTreeSet<String> = BTreeSet::new();
        for line in registry.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("name: '") {
                pending = rest.strip_suffix("',").map(str::to_string);
            } else if let Some(rest) = trimmed.strip_prefix("type: DiagnosticType.") {
                if let Some(name) = pending.take() {
                    if rest.strip_suffix(',') == Some("STATIC_WARNING") {
                        warning_family.insert(name);
                    }
                }
            }
        }
        let frozen: BTreeSet<String> = [
            // (a) M1 frozen-set continuation
            "invalid_null_aware_operator",
            "undefined_hidden_name",
            "undefined_referenced_parameter",
            "undefined_shown_name",
            // (b) flow analysis & null-safety soundness
            "body_might_complete_normally_nullable",
            "cast_from_null_always_fails",
            "cast_from_nullable_always_fails",
            "dead_code",
            "dead_code_catch_following_catch",
            "dead_code_on_catch_subtype",
            "dead_null_aware_expression",
            "null_argument_to_non_null_type",
            "null_check_always_fails",
            "nullable_type_in_catch_clause",
            "receiver_of_type_never",
            "type_check_with_null",
            "unnecessary_cast",
            "unnecessary_nan_comparison",
            "unnecessary_non_null_assertion",
            "unnecessary_null_comparison",
            "unnecessary_question_mark",
            "unnecessary_type_check",
            // (c) patterns / records / switch exhaustiveness & reachability
            "constant_pattern_never_matches_value_type",
            "missing_enum_constant_in_switch",
            "pattern_never_matches_value_type",
            "unreachable_switch_case",
            "unreachable_switch_default",
            "unnecessary_cast_pattern",
            "unnecessary_null_assert_pattern",
            "unnecessary_null_check_pattern",
            "unnecessary_wildcard_pattern",
            // (d) class modifiers / sealed
            "mixin_on_sealed_class",
            "subtype_of_sealed_class",
            // (e) const & literal semantics
            "equal_elements_in_set",
            "equal_keys_in_map",
            "non_const_call_to_literal_constructor",
            // (f) import/export combinator duplicates
            "duplicate_export",
            "duplicate_hidden_name",
            "duplicate_import",
            "duplicate_shown_name",
            // (g) override & interface-member semantics
            "non_nullable_equals_parameter",
            "override_on_non_overriding_member",
            "unnecessary_no_such_method",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            frozen.len(),
            43,
            "the documented bucket freeze holds 43 codes"
        );
        // Registry drift in the removing direction (a frozen name re-typed
        // or dropped from the STATIC_WARNING family) shrinks the
        // intersection and fails here before the set comparison.
        let expected: BTreeSet<String> = frozen.intersection(&warning_family).cloned().collect();
        assert_eq!(
            expected.len(),
            43,
            "every frozen bucket member must still be STATIC_WARNING in the registry"
        );
        assert_eq!(gate_codes.len(), 43, "M2 WARNING tier freezes 43 codes");
        // 43 raw data lines (comments/blanks excluded): a duplicated line
        // would hide in the set comparison but still be drift.
        assert_eq!(
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
            43
        );
        assert_eq!(
            gate_codes, expected,
            "gate file = documented M2 WARNING derivation over the SDK main registry"
        );
        // Load-bearing exclusions: family members kept out on purpose
        // (file-header exclusion families). `undefined_lint` is the M1
        // precedent (analysis-options code); `unused_import` is
        // WARNING-severity yet M3 usage analysis; `strict_raw_type` is
        // opt-in via analysis options; `multiple_combinators` is the
        // dual-type ambiguity case.
        for excluded in [
            "undefined_lint",
            "unused_import",
            "strict_raw_type",
            "multiple_combinators",
        ] {
            assert!(
                warning_family.contains(excluded),
                "{excluded} must still be in the WARNING family for this guard to be load-bearing"
            );
            assert!(
                !gate_codes.contains(excluded),
                "{excluded} must stay excluded from the M2 WARNING tier"
            );
        }
    }

    /// Corpus-hygiene pin for the M2 WARNING-tier gate corpus
    /// (`tests/corpus/m2-warning`, the run of `scripts/m2-gate.sh`): the
    /// gate's 1.000 bar over the 43-code universe holds only while the
    /// corpus never triggers the excluded STATIC_WARNING families whose
    /// codes are oracle-emittable from plain Dart (the #905 exemption
    /// re-audit §5 S6 conditional keeps and §7 P2 watchlist: the
    /// usage-analysis `unused_*` set, the deprecation machinery, doc-comment
    /// directives & bidi, the async error-handler family,
    /// `unnecessary_set_literal`). This pin makes that hygiene
    /// load-bearing: it scans every corpus source for the trigger shapes
    /// and fails naming each offending file and shape.
    ///
    /// Shape table (shape -> the excluded codes it keeps unreachable):
    /// - `@Deprecated` / `@deprecated` declarations -> the `deprecated_*`
    ///   kind family (12): one deprecated element makes plain usage
    ///   (extend / implement / mixin / instantiate / field type / export)
    ///   oracle-emittable. (Re-probe 2026-10-12: the plain use-kind face is
    ///   itself never emitted by the 3.13.1 CLI; the kind-carrying family
    ///   is the exposure.)
    /// - `{@` doc-directive markers, `@docImport` -> the doc-comment
    ///   directive family (`doc_directive_*`, `doc_import_*`,
    ///   `uri_does_not_exist_in_doc_import`).
    /// - bidi code points U+202E / U+200F / U+200E ->
    ///   `text_direction_code_point_in_comment` /
    ///   `text_direction_code_point_in_literal`.
    /// - `.then(` / `.catchError(` -> the async error-handler family
    ///   (`argument_type_not_assignable_to_error_handler`,
    ///   `body_might_complete_normally_catch_error`,
    ///   `invalid_return_type_for_catch_error`,
    ///   `invalid_return_type_for_then`, `unawaited_return_in_try_block`).
    /// - `Map()` / `Set()` bare constructor calls -> the
    ///   `non_const_call_to_literal_constructor` trigger: a WARNING-tier
    ///   universe member with no dartq implementation and no fixture yet
    ///   (#749), so a stray call mints oracle-only rows. Legal only inside
    ///   that code's dedicated targeted fixture (the corpus's "one surgical
    ///   fixture per included code" contract).
    /// - `=> {` arrow-body collection literals -> the
    ///   `unnecessary_set_literal` face (a built-in WARNING at the oracle
    ///   generation; fires on a void-returning arrow body producing a set
    ///   literal). This textual shape is the deliberate superset: no
    ///   return-type resolution happens here.
    /// - a `catch` clause whose exception or stack-trace variable is never
    ///   referenced again in the file -> `unused_catch_clause` /
    ///   `unused_catch_stack` (the two textually pinnable `unused_*`
    ///   faces).
    ///
    /// Not textually pinnable: the remaining `unused_*` usage-analysis
    /// faces (`unused_local_variable`, `unused_field`, `unused_import`,
    /// ...) fire on unreferenced declarations, whose absence only a real
    /// usage analysis can prove — they stay under the re-audit's contract
    /// that any corpus growth re-runs the P2 out-of-universe sweep. The
    /// strict-analysis opt-in family (8 codes) is guarded structurally:
    /// the corpus must carry no `analysis_options.yaml` at all.
    #[test]
    fn m2_warning_corpus_carries_no_excluded_code_trigger_shapes() {
        fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // `.dart_tool` is pub-get output, not corpus content.
                    if path.file_name().map(|n| n.to_string_lossy().into_owned())
                        != Some(".dart_tool".to_owned())
                    {
                        collect_files(&path, out);
                    }
                } else {
                    out.push(path);
                }
            }
        }

        fn is_ident_byte(b: u8) -> bool {
            b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
        }

        /// Whole-word occurrence count of `word` in `text` (Dart
        /// identifiers are ASCII letters/digits/`_`/`$` — the official
        /// scanner's identifier rule).
        fn word_occurrences(text: &str, word: &str) -> usize {
            let b = text.as_bytes();
            let w = word.as_bytes();
            let mut count = 0;
            let mut i = 0;
            while i + w.len() <= b.len() {
                if &b[i..i + w.len()] == w
                    && (i == 0 || !is_ident_byte(b[i - 1]))
                    && (i + w.len() == b.len() || !is_ident_byte(b[i + w.len()]))
                {
                    count += 1;
                    i += w.len();
                } else {
                    i += 1;
                }
            }
            count
        }

        /// The `catch`-clause variables declared but never referenced
        /// anywhere else in the file: the `unused_catch_clause` /
        /// `unused_catch_stack` trigger shape. A mention in a comment or
        /// string counts as a use here — under-flagging is the safe
        /// direction for a tripwire backed by the gate itself.
        fn unused_catch_variables(text: &str) -> Vec<String> {
            let b = text.as_bytes();
            let mut unused = Vec::new();
            let mut i = 0;
            while let Some(found) = text[i..].find("catch (") {
                let mut cursor = i + found + "catch (".len();
                let mut idents: Vec<String> = Vec::new();
                for _ in 0..2 {
                    while cursor < b.len() && b[cursor] == b' ' {
                        cursor += 1;
                    }
                    let start = cursor;
                    while cursor < b.len() && is_ident_byte(b[cursor]) {
                        cursor += 1;
                    }
                    if cursor == start {
                        break;
                    }
                    idents.push(text[start..cursor].to_owned());
                    while cursor < b.len() && b[cursor] == b' ' {
                        cursor += 1;
                    }
                    if cursor < b.len() && b[cursor] == b',' {
                        cursor += 1;
                    } else {
                        break;
                    }
                }
                for ident in &idents {
                    if word_occurrences(text, ident) == 1 {
                        unused.push(ident.clone());
                    }
                }
                i = cursor.max(i + found + 1);
            }
            unused
        }

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness");
        let corpus = root.join("tests/corpus/m2-warning");

        let mut files = Vec::new();
        collect_files(&corpus, &mut files);
        files.sort();
        assert!(
            !files.is_empty(),
            "{} carries no files (corpus missing?)",
            corpus.display()
        );

        let literal_constructor_fixture = "lib/targeted/non_const_call_to_literal_constructor.dart";
        let substring_shapes: &[(&[&str], &str)] = &[
            (
                &["@Deprecated", "@deprecated"],
                "deprecated declaration — deprecated_* kind family (12 excluded codes)",
            ),
            (&["{@"], "{@ doc directive — doc_directive_* family"),
            (
                &["@docImport"],
                "@docImport directive — doc_import_* / uri_does_not_exist_in_doc_import",
            ),
            (
                &[".then(", ".catchError("],
                "Future error-handler call — async error-handler family (5 excluded codes)",
            ),
            (
                &["=> {"],
                "arrow-body collection literal — unnecessary_set_literal face \
                 (void-arrow set literal)",
            ),
        ];

        let mut violations: Vec<String> = Vec::new();
        for file in &files {
            let rel = file
                .strip_prefix(&corpus)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            if rel.ends_with("analysis_options.yaml") {
                violations.push(format!(
                    "{rel}: analysis_options.yaml must not exist in the m2-warning \
                     corpus (the strict_raw_type / inference_failure_on_* exclusions \
                     rest on its absence)"
                ));
                continue;
            }
            if !rel.ends_with(".dart") {
                continue;
            }
            let text = match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(e) => {
                    violations.push(format!("{rel}: unreadable as UTF-8 ({e})"));
                    continue;
                }
            };
            for (needles, description) in substring_shapes {
                if needles.iter().any(|needle| text.contains(needle)) {
                    violations.push(format!("{rel}: {description}"));
                }
            }
            if (word_occurrences(&text, "Map()") > 0 || word_occurrences(&text, "Set()") > 0)
                && rel != literal_constructor_fixture
            {
                violations.push(format!(
                    "{rel}: Map()/Set() bare constructor call — \
                     non_const_call_to_literal_constructor trigger (unimplemented, \
                     no fixture until #749); legal only inside {literal_constructor_fixture}"
                ));
            }
            if text.contains(['\u{202E}', '\u{200F}', '\u{200E}']) {
                violations.push(format!(
                    "{rel}: bidi code point U+202E/U+200F/U+200E — \
                     text_direction_code_point_in_comment/_in_literal"
                ));
            }
            let unused = unused_catch_variables(&text);
            if !unused.is_empty() {
                violations.push(format!(
                    "{rel}: catch clause variable(s) {unused:?} never referenced — \
                     unused_catch_clause/unused_catch_stack trigger"
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "the m2-warning corpus carries excluded-code trigger shapes; the M2 \
             WARNING gate's 1.000 bar holds only without them (remove the shape, \
             or re-run the #905 P2 out-of-universe sweep and extend the audit \
             trail first):\n  {}",
            violations.join("\n  ")
        );
    }

    /// Core codes-filter semantics: out-of-set entries are dropped on both sides --
    /// oracle-only out-of-set entries never enter the denominator; the gate judges only in-set entries.
    #[test]
    #[allow(clippy::float_cmp)] // the count ratio 1.0 is exact
    fn codes_filter_drops_out_of_set_oracle_entries() {
        let mk = |code: &str| MachineDiag {
            severity: Severity::Error,
            dtype: "COMPILE_TIME_ERROR".into(),
            code: code.into(),
            file: std::path::PathBuf::from("lib/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        // oracle: 1 in-set + 1 out-of-set (an analyzer semantic code); dartq: only the 1 in-set entry
        let oracle = vec![mk("EXPECTED_TOKEN"), mk("UNDEFINED_CLASS")];
        let dartq = vec![mk("expected_token")];
        let filters = Filters {
            severities: None,
            types: None,
            codes: Some(BTreeSet::from(["expected_token".to_string()])),
        };
        let root = Path::new("/corpus");
        let cmp = compare(
            &build_side_labeled(&oracle, root, None, |d| filters.keep(d)),
            &build_side_labeled(&dartq, root, None, |d| filters.keep(d)),
        );
        assert_eq!(cmp.oracle_total, 1, "out-of-set oracle-only entry dropped");
        assert_eq!(cmp.dartq_total, 1);
        assert_eq!(cmp.oracle_dropped, 1, "the drop is accounted, not silent");
        assert_eq!(cmp.oracle_only.len(), 0);
        assert_eq!(cmp.dartq_only.len(), 0);
        assert_eq!(cmp.error_jaccard(), 1.0, "in-set entries agree exactly");
    }

    /// Three filters stacked orthogonally: codes ∩ severity ERROR ∩ type; a miss on any dimension drops the entry.
    #[test]
    fn codes_filter_stacks_with_severity_and_type() {
        let mk = |code: &str, dtype: &str| MachineDiag {
            severity: Severity::Error,
            dtype: dtype.into(),
            code: code.into(),
            file: std::path::PathBuf::from("lib/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        let filters = Filters {
            severities: Some(vec![Severity::Error]),
            types: Some(vec!["COMPILE_TIME_ERROR".to_string()]),
            codes: Some(BTreeSet::from(["expected_token".to_string()])),
        };
        // Hit: the analyzer reports EXPECTED_TOKEN (ParserErrorCode) as COMPILE_TIME_ERROR
        assert!(filters.keep(&mk("EXPECTED_TOKEN", "COMPILE_TIME_ERROR")));
        // Code in set but TYPE mismatched (e.g. the other side routes it as SYNTACTIC_ERROR) -> dropped
        assert!(!filters.keep(&mk("EXPECTED_TOKEN", "SYNTACTIC_ERROR")));
        // Code not in set -> dropped (even when severity/type both hit)
        assert!(!filters.keep(&mk("UNDEFINED_CLASS", "COMPILE_TIME_ERROR")));
        // Static warning -> dropped by severity
        assert!(!filters.keep(&mk("EXPECTED_TOKEN", "STATIC_WARNING")));
    }

    /// --codes-file / --corpus-dir flag parsing and validation.
    #[test]
    fn codes_file_flag_parsing_and_validation() {
        // Repeatable --corpus-dir and --codes-file recorded into options
        let opts = RunOptions::parse(&args(&[
            "--corpus-dir",
            "tests/corpus/m0",
            "--corpus-dir",
            "tests/corpus/m0-syntax",
            "--codes-file",
            "tests/corpus/syntax-codes.txt",
        ]))
        .expect("valid flags");
        assert_eq!(opts.corpus_dirs.len(), 2);
        assert_eq!(
            opts.codes_file.as_deref(),
            Some(Path::new("tests/corpus/syntax-codes.txt"))
        );
        // Defaults: single corpus, no code set
        let defaults = RunOptions::parse(&[]).expect("defaults");
        assert!(defaults.corpus_dirs.is_empty());
        assert!(defaults.codes_file.is_none());
        // Missing codes file -> harness error (at run_compare's file-reading stage)
        assert!(matches!(
            run_compare(&args(&["--codes-file", "/nonexistent/codes.txt"])),
            Err(Outcome::Error(msg)) if msg.contains("--codes-file")
        ));
    }

    #[test]
    fn run_flag_parsing_and_validation() {
        // Unknown flag
        assert!(matches!(
            run_compare(&args(&["--bogus"])),
            Err(Outcome::Error(_))
        ));
        // Missing --dartq-cmd without --e2e-oracle: errors after corpus checks -- a nonexistent corpus reports the corpus error first
        assert!(matches!(
            run_compare(&args(&["--corpus-dir", "/nonexistent"])),
            Err(Outcome::Error(msg)) if msg.contains("harness corpus")
        ));
        // Threshold boundaries
        assert!(matches!(
            run_compare(&args(&["--corpus-dir", "/nonexistent", "--threshold", "1.5"])),
            Err(Outcome::Error(msg)) if msg.contains("--threshold")
        ));
    }

    /// --e2e-oracle and --dartq-cmd are mutually exclusive at flag-parsing
    /// time: e2e would silently win and the run would measure dart vs dart at
    /// 1.000 (a "pass" that judges nothing).
    #[test]
    fn e2e_oracle_and_dartq_cmd_are_mutually_exclusive() {
        assert!(matches!(
            RunOptions::parse(&args(&["--e2e-oracle", "--dartq-cmd", "dartq check"])),
            Err(Outcome::Error(msg)) if msg.contains("mutually exclusive")
        ));
        // Either flag alone parses
        assert!(RunOptions::parse(&args(&["--e2e-oracle"])).is_ok());
        assert!(
            RunOptions::parse(&args(&["--dartq-cmd", "dartq check"])).is_ok(),
            "dartq-cmd alone is the measurement mode"
        );
    }

    /// Non-vacuity guard: an oracle side whose ERROR tier is empty post-filter
    /// (all entries dropped, or only non-ERROR entries) would pass the ERROR
    /// gate at 1.0 via the both-empty rule -- rejected unless --allow-vacuous;
    /// pure-report mode (severity filter without ERROR) has no gate to be
    /// vacuous about and is never rejected.
    #[test]
    fn vacuity_guard_rejects_empty_oracle_error_tier() {
        let mk = |severity: Severity, code: &str| MachineDiag {
            severity,
            dtype: "SYNTACTIC_ERROR".into(),
            code: code.into(),
            file: std::path::PathBuf::from("lib/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };

        // All oracle entries dropped by the codes filter -> vacuous
        let filters = Filters {
            severities: None,
            types: None,
            codes: Some(BTreeSet::from(["expected_token".to_string()])),
        };
        assert!(filters.error_in_scope());
        let diags = vec![mk(Severity::Error, "UNDEFINED_CLASS")];
        let side = build_side_labeled(&diags, Path::new("/corpus"), None, |d| filters.keep(d));
        assert!(
            matches!(
                check_oracle_vacuity(&side, true, false),
                Err(Outcome::Error(msg)) if msg.contains("vacuous")
                    && msg.contains("1 pre-filter entries, 1 dropped")
            ),
            "error carries the accounting"
        );
        // The escape hatch accepts exactly this run
        assert!(check_oracle_vacuity(&side, true, true).is_ok());

        // Only WARNING entries survive -> ERROR tier empty while in scope -> vacuous
        let diags = vec![mk(Severity::Warning, "UNUSED_LOCAL_VARIABLE")];
        let side = build_side_labeled(&diags, Path::new("/corpus"), None, |_| true);
        assert!(check_oracle_vacuity(&side, true, false).is_err());

        // One ERROR entry survives -> non-vacuous
        let diags = vec![mk(Severity::Error, "EXPECTED_TOKEN")];
        let side = build_side_labeled(&diags, Path::new("/corpus"), None, |_| true);
        assert!(check_oracle_vacuity(&side, true, false).is_ok());

        // Pure-report mode: severity filter drops ERROR, gate not in scope -> never rejected
        let filters = Filters {
            severities: Some(vec![Severity::Warning]),
            types: None,
            codes: None,
        };
        assert!(!filters.error_in_scope());
        let side = build_side_labeled(&diags, Path::new("/corpus"), None, |d| filters.keep(d));
        assert!(check_oracle_vacuity(&side, false, false).is_ok());
    }

    #[test]
    fn value_helper_takes_next_arg() {
        let a = args(&["--out", "x"]);
        let mut i = 0;
        assert_eq!(value(&a, &mut i).unwrap(), "x");
        assert_eq!(i, 1);
        let b = args(&["--out"]);
        let mut j = 0;
        assert!(value(&b, &mut j).is_err());
    }

    /// Verdict mapping: judged against the threshold when the ERROR tier is in
    /// scope; skipped (never PASS) when the severity filter drops it, even though
    /// the empty-vs-empty jaccard rule scores 1.0 there.
    #[test]
    fn gate_verdict_skips_only_when_error_not_in_scope() {
        use report::Verdict;
        assert_eq!(gate_verdict(true, 1.0, 0.99), Verdict::Pass);
        assert_eq!(
            gate_verdict(true, 0.99, 0.99),
            Verdict::Pass,
            "boundary meets the gate"
        );
        assert_eq!(gate_verdict(true, 0.5, 0.99), Verdict::Fail);
        assert_eq!(
            gate_verdict(false, 1.0, 0.99),
            Verdict::Skipped,
            "the informational 1.0 must not print as a judged pass"
        );
        assert_eq!(gate_verdict(false, 0.0, 0.99), Verdict::Skipped);
        // Exit semantics: only a judged FAIL exits 1
        assert!(Verdict::Pass.exit_success());
        assert!(
            Verdict::Skipped.exit_success(),
            "pure-report mode is exit 0"
        );
        assert!(!Verdict::Fail.exit_success());
    }

    /// Duplicate `--corpus-dir` values (same canonical dir through differing
    /// spellings) must collapse to one entry: a repeated dir would run each
    /// driver twice and double every instance count in the merged sides.
    #[test]
    fn duplicate_corpus_dirs_dedup_by_canonical_path() {
        let root = std::env::temp_dir().join(format!("dq_dedup_{}", std::process::id()));
        let a = root.join("a").join("m0");
        let b = root.join("b").join("m0");
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir.join(".dart_tool")).expect("mkdir");
            std::fs::write(dir.join("pubspec.yaml"), "name: corpus_m0\n").expect("pubspec");
            let config = r#"{"configVersion":2,"packages":[{"name":"c","rootUri":"../","packageUri":"lib/","languageVersion":"3.13"}],"generator":"pub"}"#;
            std::fs::write(dir.join(".dart_tool").join("package_config.json"), config)
                .expect("package config");
        }
        // Second spelling of `a` via a `.` path segment: same canonical dir,
        // different string form — dedup must key on the canonical path.
        let a_via_dot = root.join("a").join(".").join("m0");
        let canonical =
            canonical_corpus_dirs(&[a.clone(), a_via_dot, b.clone()]).expect("canonical dirs");
        assert_eq!(
            canonical,
            vec![
                a.canonicalize().expect("canon a"),
                b.canonicalize().expect("canon b")
            ],
            "duplicate dropped, order preserved"
        );

        // A nonexistent corpus still fails before dedup can matter
        assert!(canonical_corpus_dirs(&[PathBuf::from("/nonexistent/corpus")]).is_err());

        std::fs::remove_dir_all(&root).ok();
    }
}
