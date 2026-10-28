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

// Frozen gate-universe artifacts (m1/m2_warning/m3_unused codes): the
// derivations, --check pins, and regen entry points live in freeze.rs —
// test-only, like the pins in `mod tests` below.
#[cfg(test)]
mod freeze;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compare::{Side, build_side_labeled, compare, merge_sides};
use dartq::DartqDriver;
use machine::{DIAG_TYPES, MachineDiag, OutputFormat, Severity};
use oracle::OracleDriver;

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
      package. Default source: $DARTQ_SDK_CHECKOUT/pkg/_fe_analyzer_shared/test;
      default target: {DEFAULT_TARGET}.

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
      --dartq-cmd \"cargo run -q -p dartq-cli -- check --format=json\".
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
      whose own languageVersion is >= 3.13 (the Plan A preflight) — a stale pin floods the gate with
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
      oracle incremental consistency: drive one live `dartq
      daemon` session with an edit sequence (overlay didChange + the
      `dartq/analyze` pull = the incremental result) while the same edits
      land on a materialized disk copy re-analyzed fresh per step (the
      `--dartq-cmd` full-recompute result); per step both sides normalize
      to one canonical shape and compare byte-wise. Report:
      <out>/<label>/{{report.json,report.md}}. Exit codes: 0 all steps
      equal, 1 any mismatch, 2 harness error.

  harness repro --corpus-dir DIR --dartq-cmd TEMPLATE [--runs N] [--out DIR]
      reproducibility: run the dartq side N times (default 2) over
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
      benchmark metric collectors, one JSON report per
      metric under <out>/<metric>/report.json (rss-100k:
      <out>/rss-100k/<corpus-name>/report.json so multi-size runs cannot
      overwrite each other; schema_version 1; every
      report records its inputs for reproducibility). --metric is
      repeatable (rss-100k | incremental-p95 | incremental-p95-resident |
      token-cost | plugin-overhead | cold); default = all nine, in that
      order. Default corpus: {DEFAULT_TARGET}; default out: target/evidence/bench.
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
      the resident face the incremental target describes). Edits
      land as in-memory overlays (the daemon edit shape); each step is one
      timed `analyze` pass over the `dartq analyze` face's own workspace
      assembly. Reports the same P50/P95/P99 shape, comparable against the
      batch arm. SDK root: --sdk PATH > <root>/bin/dart derived from
      --oracle-dart > DART_SDK.
      token-cost: `dartq check --format=json` stdout (the oracle-parity
      envelope) vs `dart analyze --format=machine` stdout on the same corpus,
      tokenized with the
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
    let mut source: Option<PathBuf> = None;
    let mut target = PathBuf::from(DEFAULT_TARGET);
    let mut rebuild = false;
    let mut dart = "dart".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => source = Some(value(args, &mut i)?.into()),
            "--target" => target = value(args, &mut i)?.into(),
            "--dart" => dart = value(args, &mut i)?.to_string(),
            "--rebuild" => rebuild = true,
            other => return Err(unknown_flag(other, "corpus")),
        }
        i += 1;
    }

    let source = source.map_or_else(
        || {
            std::env::var("DARTQ_SDK_CHECKOUT")
                .map(|root| PathBuf::from(root).join("pkg/_fe_analyzer_shared/test"))
                .map_err(|_| {
                    Outcome::Error(
                        "corpus source not set: pass --source DIR or export \
                         DARTQ_SDK_CHECKOUT=<dart-lang/sdk checkout>"
                            .into(),
                    )
                })
        },
        Ok,
    )?;
    if !source.is_dir() {
        return Err(Outcome::Error(format!(
            "source dir `{}` does not exist (point --source or DARTQ_SDK_CHECKOUT at a dart-lang/sdk checkout)",
            source.display()
        )));
    }
    let report = corpus::build_corpus(&source, &target, rebuild).map_err(Outcome::Error)?;
    corpus::pub_get(&dart, &target).map_err(Outcome::Error)?;
    // Same preflight as `run`: assembly is only valid when pub recorded a
    // languageVersion >= 3.13.
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
    /// entry as a self-contained regression case under `dir`.
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
    // Oracle reference version, captured once per run
    let oracle_version = oracle::dart_version(&opts.oracle_dart).map_err(Outcome::Error)?;
    let oracle_diags = analyze_corpora(&corpora, |dir| oracle.analyze(dir, opts.oracle_format))?;
    let oracle_side = build_filtered_side(&corpora, &oracle_diags, &filters);
    // Fail before the dartq run: a vacuous oracle side would pass at 1.0 without judging anything
    check_oracle_vacuity(&oracle_side, filters.error_in_scope(), opts.allow_vacuous)?;
    // The dartq contract is the oracle-parity json envelope (`--format=json`,
    // byte-identical to `dart analyze --format=json`; the CLI's machine face
    // is retired — `--format=machine` is a usage error there). Only the e2e
    // self-check (dartq side = dart analyze itself) follows the oracle format.
    let dartq_format = if opts.e2e_oracle {
        opts.oracle_format
    } else {
        OutputFormat::Json
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

    // Regression persistence: when asked, every post-filter
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
/// languageVersion preflight (the Plan A rule: a stale corpus
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
/// `COMPILE_TIME_ERROR`, so the syntax tier cannot be obtained by
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
/// minus the 3 language-version-family codes (skew codes must not enter
/// the gate denominator); the derivation is pinned
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

    // fixture-coverage lint-tier universe: the registry's live gate face
    // (registered_gate_codes below).
    use dartq_lints::emittable_rule_names;

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
    /// (crates/dartq-diagnostics/data/syntax-codes.tsv) minus the 4
    /// supplemental codes (the 3-code language-version family plus
    /// `invalid_covariant_modifier_in_primary_constructor`; skew codes must
    /// not enter the gate denominator). Catches
    /// silent drift
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
        assert_eq!(
            tsv_codes.len(),
            259,
            "TSV = 255 syntax + 4 supplemental codes"
        );
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
        let supplemental: BTreeSet<String> = [
            "illegal_language_version_override",
            "inconsistent_language_version_override",
            "invalid_language_version_override",
            "invalid_covariant_modifier_in_primary_constructor",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            tsv_codes
                .difference(&gate_codes)
                .cloned()
                .collect::<BTreeSet<_>>(),
            supplemental,
            "gate file = TSV minus exactly the supplemental codes"
        );
        assert!(
            gate_codes.is_subset(&tsv_codes),
            "gate codes must all exist in the authoritative TSV"
        );
    }

    /// Corpus-hygiene pin for the M2 WARNING-tier gate corpus
    /// (`tests/corpus/m2-warning`, the run of `scripts/m2-gate.sh`): the
    /// gate's 1.000 bar over the 43-code universe holds only while the
    /// corpus never triggers the excluded `STATIC_WARNING` families whose
    /// codes are oracle-emittable from plain Dart (the #905 exemption
    /// re-audit's S6 conditional keeps and P2 watchlist: the
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
    ///   universe member whose dartq producer exists (the @literal class
    ///   face, dartq-semantic annotation.rs) but whose dedicated targeted
    ///   fixture does not yet (#749), so a stray call mints gate entries
    ///   outside that fixture. Legal only inside the corpus's "one surgical
    ///   fixture per included code" contract.
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
    #[expect(clippy::too_many_lines)] // walks the corpus tree once per excluded shape; each walk is independent
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
            if Path::new(&rel).extension() != Some(std::ffi::OsStr::new("dart")) {
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

    /// Derivation pin: the semantic wave-2 gate codes file
    /// (`tests/harness/sem2_codes.txt`) must equal its documented
    /// derivation (file header): the union of the oracle and dartq row
    /// codes over every persisted regression nail
    /// (`tests/corpus/regressions/cases/*.json`, schema
    /// regression-case-v1) minus the five frozen gate universes
    /// (`tests/corpus/syntax-codes.txt`, `m1_codes.txt`,
    /// `m2_warning_codes.txt`, `m3_unused_codes.txt`, `lint_codes.txt`).
    /// Unlike the m1/m2 registry pins this derivation is fully local (the
    /// nails and the frozen files are in-repo), so it always runs: a fresh
    /// nail carrying a new divergence code, or a frozen-file expansion
    /// that absorbs one, fails here and forces a conscious re-freeze.
    #[test]
    fn sem2_codes_file_matches_derivation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness");
        let txt = std::fs::read_to_string(root.join("tests/harness/sem2_codes.txt"))
            .expect("gate codes file present");
        let gate_codes = parse_codes_file(&txt).expect("gate file parses");

        let mut nail_codes: BTreeSet<String> = BTreeSet::new();
        let cases = root.join("tests/corpus/regressions/cases");
        let mut count = 0usize;
        for entry in std::fs::read_dir(&cases).expect("regression nails present") {
            let path = entry.expect("readable nail entry").path();
            // `.edits.jsonl` companions carry edit scripts, not rows; only
            // the per-case `.json` snapshots do.
            if path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref()
                != Some("json")
            {
                continue;
            }
            let case: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("nail readable"))
                    .expect("nail JSON parses");
            assert_eq!(
                case["schema"],
                "regression-case-v1",
                "unexpected nail schema in {}",
                path.display()
            );
            for side in ["oracle", "dartq"] {
                for row in case[side].as_array().unwrap_or_else(|| {
                    panic!("nail {} carries no {side} row array", path.display())
                }) {
                    nail_codes.insert(
                        row["code"]
                            .as_str()
                            .unwrap_or_else(|| panic!("nail row without code: {}", path.display()))
                            .to_owned(),
                    );
                }
            }
            count += 1;
        }
        assert!(
            count >= 50,
            "regression nails largely missing ({count} cases); derivation void"
        );

        let mut frozen: BTreeSet<String> = BTreeSet::new();
        for file in [
            "tests/corpus/syntax-codes.txt",
            "tests/harness/m1_codes.txt",
            "tests/harness/m2_warning_codes.txt",
            "tests/harness/m3_unused_codes.txt",
            "tests/harness/lint_codes.txt",
        ] {
            let content = std::fs::read_to_string(root.join(file))
                .unwrap_or_else(|e| panic!("frozen gate codes file {file} must be present: {e}"));
            frozen.extend(parse_codes_file(&content).expect("frozen file parses"));
        }

        let expected: BTreeSet<String> = nail_codes.difference(&frozen).cloned().collect();

        assert_eq!(
            gate_codes.len(),
            77,
            "sem2 freezes 77 codes at the 2026-10-15 re-freeze"
        );
        // 77 raw data lines (comments/blanks excluded): a duplicated line
        // would hide in the set comparison but still be drift.
        assert_eq!(
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
            77
        );
        assert_eq!(
            gate_codes, expected,
            "gate file = nail codes minus the five frozen gate universes"
        );

        // Rename retirement pin (2026-10-15): dartq adopted the oracle
        // names const_not_initialized / final_not_initialized, retiring the
        // two dartq-only rename codes this tier used to carry (see the
        // codes-file header's historical note). They must stay retired:
        // absent from the tier and from every nail row.
        for retired in [
            "const_field_without_initializer",
            "final_field_without_initializer",
        ] {
            assert!(
                !gate_codes.contains(retired),
                "{retired} must stay retired (dartq adopted the oracle name)"
            );
            assert!(
                !nail_codes.contains(retired),
                "{retired} resurfaced in nail rows -- a rename regression"
            );
        }
        assert!(
            gate_codes.contains("const_not_initialized")
                && gate_codes.contains("final_not_initialized"),
            "the adopted oracle names must carry the tier's const/final init rows"
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

    // ─── Fixture-coverage manifest (tests/harness/fixture-coverage.tsv) ────

    /// One gate tier of the fixture-coverage manifest: where its universe
    /// codes come from, the gate corpora the tier's run scores (the
    /// `--corpus-dir` values the tier's gate script passes), and whether
    /// the scanner-goldens column applies (the syntax tier only — the
    /// weaker dartq-only scanner face, never full coverage on its own).
    struct FixtureCoverageTier {
        tier: &'static str,
        codes: TierCodes,
        corpus_dirs: &'static [&'static str],
        scanner_goldens: bool,
    }

    /// Where a tier's universe codes come from.
    enum TierCodes {
        /// The frozen codes file, relative to the repo root. The five
        /// diagnostic tiers: the codes file is itself the tier's universe
        /// definition (no separate registry face exists for it).
        File(&'static str),
        /// The lint tier: derived live from the rule registry — every
        /// registered emittable rule's code minus the divergence ledger's
        /// `gate_status=excluded` rows (`registered_gate_codes` below).
        /// Deliberately NOT the frozen `lint_codes.txt`: the freeze lags
        /// the registry by design while a rule wave is in flight (the
        /// re-freeze is a deliberate single-shot, N1), and the freeze is
        /// governed independently by `lint_codes_freeze_matches_the_registry`
        /// (crates/dartq-lints). Anchoring the coverage pin to the same
        /// registered face the governance pin reconciles against keeps this
        /// manifest green through wave-time lag and re-freezes alike, while
        /// still forcing a conscious re-derive whenever the registry grows.
        RegisteredLintGate,
    }

    const FIXTURE_COVERAGE_TIERS: &[FixtureCoverageTier] = &[
        FixtureCoverageTier {
            tier: "syntax",
            codes: TierCodes::File("tests/corpus/syntax-codes.txt"),
            corpus_dirs: &["tests/corpus/m0", "tests/corpus/m0-syntax"],
            scanner_goldens: true,
        },
        FixtureCoverageTier {
            tier: "m1",
            codes: TierCodes::File("tests/harness/m1_codes.txt"),
            corpus_dirs: &["tests/corpus/m1-semantic"],
            scanner_goldens: false,
        },
        FixtureCoverageTier {
            tier: "m2",
            codes: TierCodes::File("tests/harness/m2_warning_codes.txt"),
            corpus_dirs: &["tests/corpus/m2-warning"],
            scanner_goldens: false,
        },
        FixtureCoverageTier {
            tier: "m3",
            codes: TierCodes::File("tests/harness/m3_unused_codes.txt"),
            corpus_dirs: &["tests/corpus/m3-unused"],
            scanner_goldens: false,
        },
        FixtureCoverageTier {
            tier: "lint",
            codes: TierCodes::RegisteredLintGate,
            corpus_dirs: &["tests/corpus/lints"],
            scanner_goldens: false,
        },
        FixtureCoverageTier {
            tier: "sem2",
            codes: TierCodes::File("tests/harness/sem2_codes.txt"),
            corpus_dirs: &["tests/corpus/sem2"],
            scanner_goldens: false,
        },
    ];

    const FIXTURE_COVERAGE_MANIFEST: &str = "tests/harness/fixture-coverage.tsv";

    /// The divergence ledger `tests/harness/lint_divergences.tsv` — the
    /// source of truth for gate exclusions, shared with the registry
    /// governance pin.
    const LINT_DIVERGENCES: &str = "tests/harness/lint_divergences.tsv";

    /// The `gate_status` tokens the ledger's column prose defines. The TSV
    /// carries no header row (the columns live in its `#` comment block),
    /// so the status column is located by this vocabulary, not by position.
    const GATE_STATUSES: &[&str] = &["excluded", "in_universe", "fixed"];

    /// The ledger's `gate_status=excluded` rule names (lowercase). A data
    /// row without exactly one status token fails loudly — format drift
    /// surfaces here as a tutorial panic, never as a silent misparse.
    fn ledger_excluded(text: &str) -> BTreeSet<String> {
        let mut excluded = BTreeSet::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let rule = line.split('\t').next().unwrap_or_default();
            let statuses: Vec<&str> = line
                .split('\t')
                .filter(|field| GATE_STATUSES.contains(field))
                .collect();
            assert!(
                statuses.len() == 1,
                "{LINT_DIVERGENCES}: row {rule:?} carries {statuses:?} where exactly one \
                 gate_status token belongs ({GATE_STATUSES:?}) — the ledger format drifted; \
                 fix the row or this parser"
            );
            if statuses[0] == "excluded" {
                excluded.insert(rule.to_ascii_lowercase());
            }
        }
        excluded
    }

    /// The lint tier's live universe: every registered rule the
    /// implementation can emit (non-diagnostic, non-removed, AST-dispatched
    /// or pubspec-pass mounted) minus the divergence ledger's
    /// `gate_status=excluded` rows. The emittable half is the lints crate's
    /// public derivation (`emittable_rule_names`) — the exact face
    /// `lint_codes_freeze_matches_the_registry` (crates/dartq-lints) holds
    /// `lint_codes.txt` to; the ledger parse stays here (the TSV lives in
    /// this tree).
    ///
    /// Why the coverage pin anchors here and not to the frozen file: the
    /// freeze deliberately lags the registry while a rule wave is in flight
    /// (its re-freeze is a conscious single-shot), so a frozen-file anchor
    /// reds on every wave. The registry face is the source of truth; the
    /// freeze follows it under its own independent enforcement — never the
    /// reverse. Deriving live keeps the manifest green through wave-time
    /// lag and re-freezes alike, while a newly registered rule still fails
    /// the pin until a manifest row (fixture or hand-authored gap note)
    /// exists for it.
    fn registered_gate_codes(root: &Path) -> BTreeSet<String> {
        let emittable: BTreeSet<String> = emittable_rule_names()
            .map(str::to_ascii_lowercase)
            .collect();
        let ledger = std::fs::read_to_string(root.join(LINT_DIVERGENCES)).unwrap_or_else(|e| {
            panic!("divergence ledger {LINT_DIVERGENCES} must be present: {e}")
        });
        &emittable - &ledger_excluded(&ledger)
    }

    /// A tier's universe codes (sorted, lowercase, deduped — the same shape
    /// `parse_codes_file` yields for the frozen files).
    fn tier_codes(tier: &FixtureCoverageTier, root: &Path) -> BTreeSet<String> {
        match tier.codes {
            TierCodes::File(path) => {
                let text = std::fs::read_to_string(root.join(path))
                    .unwrap_or_else(|e| panic!("frozen codes file {path} must be present: {e}"));
                parse_codes_file(&text).expect("frozen codes file parses")
            }
            TierCodes::RegisteredLintGate => registered_gate_codes(root),
        }
    }

    const SCANNER_GOLDENS_MANIFEST: &str = "tests/corpus/scanner-goldens/manifest.json";

    /// One manifest row: tier, code, coverage, targeted, annotation,
    /// `scanner_goldens`, note — `-` marks an empty cell.
    type CoverageRow = [String; 7];

    /// The lowercase whole-word set of a corpus text: maximal
    /// `[A-Za-z0-9_$]` runs (the Dart identifier rule; every other code
    /// point separates), lowercased. A code is "mentioned" iff it equals
    /// one of the runs — the whole-word, case-insensitive match the
    /// coverage-audit method note defines.
    fn word_set(text: &str) -> BTreeSet<String> {
        text.to_lowercase()
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Every `.dart` file under a corpus dir, sorted; `.dart_tool` subtrees
    /// are pub-get output, not corpus content.
    fn collect_corpus_files(root: &Path, corpus: &str) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map(|n| n.to_string_lossy().into_owned())
                        != Some(".dart_tool".to_owned())
                    {
                        walk(&path, out);
                    }
                } else if path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .as_deref()
                    == Some("dart")
                {
                    out.push(path);
                }
            }
        }
        let dir = root.join(corpus);
        assert!(dir.is_dir(), "tier corpus {corpus} must exist");
        let mut out = Vec::new();
        walk(&dir, &mut out);
        out.sort();
        out
    }

    /// The live derivation of the fixture-coverage manifest: for every
    /// frozen (tier, code), the three evidence cells — same-named targeted
    /// fixtures (file stem == code), annotation mentions (whole-word,
    /// case-insensitive), and (syntax tier) the scanner-goldens case ids —
    /// plus the covered / `documented_gap` verdict. Rows follow the tier
    /// table order, codes sorted within a tier; covered rows emit `-` in
    /// the note cell (the pin's schema face), while a gap note is
    /// deliberate prose, carried over by the regen test.
    fn derive_fixture_coverage(root: &Path) -> Vec<CoverageRow> {
        let goldens_raw = std::fs::read(root.join(SCANNER_GOLDENS_MANIFEST))
            .expect("scanner-goldens manifest present");
        let goldens_json: serde_json::Value =
            serde_json::from_slice(&goldens_raw).expect("scanner-goldens manifest parses");
        let mut goldens: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for case in goldens_json["cases"]
            .as_array()
            .expect("goldens cases array")
        {
            let id = case["id"].as_str().expect("golden case id").to_string();
            for diag in case["diagnostics"].as_array().unwrap_or(&Vec::new()) {
                // Diagnostic rows are [code, offset, args?]; the first
                // element is the code.
                let code = match diag {
                    serde_json::Value::Array(rows) => rows.first().and_then(|v| v.as_str()),
                    serde_json::Value::String(s) => Some(s.as_str()),
                    _ => None,
                }
                .unwrap_or_else(|| panic!("golden diagnostic without a code: {case:?}"));
                goldens
                    .entry(code.to_ascii_lowercase())
                    .or_default()
                    .insert(id.clone());
            }
        }

        let mut rows = Vec::new();
        for tier in FIXTURE_COVERAGE_TIERS {
            let codes = tier_codes(tier, root);

            let mut files: Vec<(String, String, BTreeSet<String>)> = Vec::new();
            for corpus in tier.corpus_dirs {
                for path in collect_corpus_files(root, corpus) {
                    let rel = path
                        .strip_prefix(root)
                        .expect("corpus file under repo root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let bytes = std::fs::read(&path).expect("corpus file readable");
                    // Lossy decode: the corpus deliberately carries a
                    // non-UTF-8 fixture (the encoding face); its ASCII
                    // comment text must still be readable.
                    let stem = path
                        .file_stem()
                        .expect("file stem")
                        .to_string_lossy()
                        .into_owned();
                    files.push((rel, stem, word_set(&String::from_utf8_lossy(&bytes))));
                }
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));

            for code in &codes {
                let targeted: Vec<String> = files
                    .iter()
                    .filter(|(_, stem, _)| stem == code)
                    .map(|(rel, _, _)| rel.clone())
                    .collect();
                let annotated: Vec<String> = files
                    .iter()
                    .filter(|(_, _, words)| words.contains(code))
                    .map(|(rel, _, _)| rel.clone())
                    .collect();
                let golden: Vec<String> = if tier.scanner_goldens {
                    goldens
                        .get(code)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let cell = |v: &[String]| {
                    if v.is_empty() {
                        "-".to_string()
                    } else {
                        v.join(";")
                    }
                };
                let covered = !targeted.is_empty() || !annotated.is_empty() || !golden.is_empty();
                rows.push([
                    tier.tier.to_string(),
                    code.clone(),
                    if covered { "covered" } else { "documented_gap" }.to_string(),
                    cell(&targeted),
                    cell(&annotated),
                    cell(&golden),
                    // The pin's schema face: covered rows carry no note.
                    // A gap note is deliberate prose, carried over by the
                    // regen test before anything is written.
                    if covered {
                        "-".to_string()
                    } else {
                        String::new()
                    },
                ]);
            }
        }
        rows
    }

    /// The manifest's data rows (7 TSV cells each); `#` comments and blanks
    /// skipped.
    fn parse_coverage_manifest(text: &str) -> Vec<CoverageRow> {
        text.lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let cells: Vec<String> = l.split('\t').map(str::to_string).collect();
                assert_eq!(
                    cells.len(),
                    7,
                    "manifest row must carry 7 TSV cells (tier, code, coverage, targeted, \
                     annotation, scanner_goldens, note): {l}"
                );
                cells.try_into().expect("exactly 7 cells")
            })
            .collect()
    }

    fn fixture_coverage_repo_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("harness lives at <repo>/tests/harness")
    }

    /// The manifest's canonical text — the single serializer the check
    /// below and its regen share, so the checked-in file and the regen
    /// output cannot diverge: the committed `#` header verbatim, then the
    /// live derivation's rows in tier-table order (codes sorted within a
    /// tier), with `documented_gap` notes carried over from the committed
    /// table (a derived gap the committed table has no note for fails
    /// here — the note is deliberate prose, authored never derived).
    fn coverage_manifest_canonical(root: &Path) -> String {
        let old_text = std::fs::read_to_string(root.join(FIXTURE_COVERAGE_MANIFEST))
            .expect("coverage manifest present");
        let old_notes: BTreeMap<(String, String), String> = parse_coverage_manifest(&old_text)
            .into_iter()
            .filter(|r| r[2] == "documented_gap")
            .map(|r| {
                assert_ne!(
                    r[6], "-",
                    "documented_gap row {}::{} must state its reason",
                    r[0], r[1]
                );
                ((r[0].clone(), r[1].clone()), r[6].clone())
            })
            .collect();
        // The committed header block: every leading comment line, verbatim.
        let header_len = old_text.lines().take_while(|l| l.starts_with('#')).count();
        let mut out: String = old_text
            .lines()
            .take(header_len)
            .map(|l| l.to_owned() + "\n")
            .collect();

        for mut row in derive_fixture_coverage(root) {
            if row[2] == "documented_gap" {
                let key = (row[0].clone(), row[1].clone());
                row[6] = old_notes.get(&key).cloned().unwrap_or_else(|| {
                    panic!(
                        "new documented_gap code {}::{} needs a hand-authored note in {} \
                         (why this code has no fixture yet)",
                        row[0], row[1], FIXTURE_COVERAGE_MANIFEST
                    )
                });
            }
            out.push_str(&row.join("\t"));
            out.push('\n');
        }
        out
    }

    /// Coverage-manifest pin (audit finding: no committed per-code exposure
    /// artifact — coverage claims were prose, not pins): the committed
    /// table must equal the live derivation over the gate universe, byte
    /// for byte — one row per (tier, code) over the five diagnostic tiers'
    /// frozen codes files and the lint tier's registry gate face
    /// (`registered_gate_codes`), every evidence cell equal to the
    /// corpora-derived value, gap notes carried over (schema and universe
    /// equality are subsumed: the canonical text IS the derived rows under
    /// the committed header). Any corpus, frozen-file or registry edit
    /// that moves coverage fails here and forces a conscious re-derive;
    /// the regen test below writes this same canonical text.
    #[test]
    fn fixture_coverage_manifest_matches_corpus_derivation() {
        let root = fixture_coverage_repo_root();
        let manifest_text = std::fs::read_to_string(root.join(FIXTURE_COVERAGE_MANIFEST))
            .expect("coverage manifest present");
        crate::freeze::assert_freeze_bytes(
            FIXTURE_COVERAGE_MANIFEST,
            &manifest_text,
            &coverage_manifest_canonical(root),
            "cargo test -p harness regen_fixture_coverage_manifest -- --ignored",
        );
    }

    /// Regen (writes the manifest; not run by default):
    /// `cargo test -p harness regen_fixture_coverage_manifest -- --ignored`
    /// writes the canonical text the check above asserts — the derivation
    /// live, the committed header verbatim, the `documented_gap` notes of
    /// still-dark codes carried over from the current table. A new dark
    /// code without an existing note fails here — the note is deliberate
    /// prose (the reason a code has no fixture) and must be authored,
    /// never derived.
    #[test]
    #[ignore = "regenerates the manifest in place; run explicitly with --ignored"]
    fn regen_fixture_coverage_manifest() {
        let root = fixture_coverage_repo_root();
        let canonical = coverage_manifest_canonical(root);
        let data_rows = canonical
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count();
        std::fs::write(root.join(FIXTURE_COVERAGE_MANIFEST), canonical)
            .expect("rewrite coverage manifest");
        println!("regenerated {FIXTURE_COVERAGE_MANIFEST} ({data_rows} data rows)");
    }
}
