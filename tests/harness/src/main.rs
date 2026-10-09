//! Differential-testing driver CLI:
//! - `harness corpus`: assemble the M0 synthetic corpus (tests/corpus/m0/)
//! - `harness run`: oracle vs dartiq differential run, (code, file) entry Jaccard
//!
//! Exit codes: 0 = ERROR-tier Jaccard meets the gate; 1 = below gate; 2 = harness error of its own.

mod compare;
mod corpus;
mod dartiq;
mod machine;
mod oracle;
mod report;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compare::{Side, build_side_labeled, compare, merge_sides};
use dartiq::DartiqDriver;
use machine::{DIAG_TYPES, MachineDiag, OutputFormat, Severity};
use oracle::OracleDriver;

const DEFAULT_SOURCE: &str = ".third/dart-sdk/pkg/_fe_analyzer_shared/test";
const DEFAULT_TARGET: &str = "tests/corpus/m0";
const DEFAULT_OUT: &str = "target/harness-report";
const DEFAULT_THRESHOLD: f64 = 0.99;

#[derive(Debug)]
enum Outcome {
    Error(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        // (summary, gate passed); the corpus subcommand always passes
        Ok((summary, passed)) => {
            if !summary.is_empty() {
                println!("{summary}");
            }
            if passed {
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
        Some("--help" | "-h" | "help") | None => {
            print_usage();
            Ok((String::new(), true))
        }
        Some(other) => Err(Outcome::Error(format!(
            "unknown subcommand `{other}`; see `harness --help`"
        ))),
    }
}

fn print_usage() {
    println!(
        "usage:
  harness corpus [--source DIR] [--target DIR] [--rebuild] [--dart PATH]
      Assemble the M0 synthetic corpus (self-contained .dart files from the
      analyzer test suite, relative layout preserved) into a zero-dependency
      package. Default source: {DEFAULT_SOURCE}; default target: {DEFAULT_TARGET}.

  harness run [--corpus-dir DIR]... [--oracle-dart PATH] [--oracle-format machine|json]
              [--dartiq-cmd TEMPLATE] [--e2e-oracle] [--severity-filter LIST]
              [--type-filter LIST] [--codes-file PATH] [--threshold F] [--out DIR]
      Run oracle and dartiq over the corpus, compare (code, file) entry sets.
      --corpus-dir is repeatable: each dir must be a corpus package; every dir
      is analyzed with one invocation per dir and the entry sets are merged
      (keys prefixed with the corpus dir name to stay unambiguous).
      TEMPLATE: whitespace-split argv; {{dir}} is replaced by the corpus dir
      (appended as last arg when absent), e.g.
      --dartiq-cmd \"cargo run -q -p dartiq-cli -- check --format=machine\".
      --e2e-oracle: point the dartiq side at dart analyze itself (both sides
      identical -> jaccard must be 1.000; strongest comparator self-check).
      --severity-filter LIST: all | ERROR[,WARNING[,INFO]] — severities that
      enter the comparison. --type-filter LIST: all |
      syntactic_error[,compile_time_error[,static_warning[,lint]]] — machine
      TYPE values that enter the comparison (case-insensitive). --codes-file
      PATH: diagnostic code names, one per line, case-insensitive; `#` comment
      lines are skipped and a TAB-separated line contributes its first column
      (TSV compatible — the authoritative table is crates/dartiq-diagnostics/
      data/syntax-codes.tsv). Only entries whose
      code is in the set enter the comparison on either side — oracle-only
      out-of-set entries are dropped before scoring and never inflate the
      denominator. The three filters stack; per-severity jaccards and the
      gate verdict are computed from the filtered sets only. M0 syntax
      caliber: codes-file (the 262 scanner/parser codes, tests/corpus/
      syntax-codes.txt = the TSV minus the 3 language-version-family codes)
      ∩ severity ERROR. An empty codes file is a usage error (zero
      discrimination). If a filter empties BOTH sides, the
      existing rule applies: empty vs empty = full agreement (jaccard 1.000).
      Preflight: every corpus dir must carry a .dart_tool/package_config.json
      whose own languageVersion is >= 3.13 (Plan A, docs/facts/
      version-skew.md §4) — a stale pin floods the gate with
      experiment_not_enabled assembly noise and fails the run up front.
      Exit codes: 0 = ERROR jaccard >= threshold (default {DEFAULT_THRESHOLD}),
      1 = below threshold, 2 = harness error."
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
    dartiq_cmd: Option<String>,
    severity_filter: String,
    type_filter: String,
    codes_file: Option<PathBuf>,
    threshold: f64,
    out: PathBuf,
    e2e_oracle: bool,
}

impl RunOptions {
    fn parse(args: &[String]) -> Result<Self, Outcome> {
        let mut opts = Self {
            corpus_dirs: Vec::new(),
            oracle_dart: "dart".to_string(),
            oracle_format: OutputFormat::Machine,
            dartiq_cmd: None,
            severity_filter: "all".to_string(),
            type_filter: "all".to_string(),
            codes_file: None,
            threshold: DEFAULT_THRESHOLD,
            out: PathBuf::from(DEFAULT_OUT),
            e2e_oracle: false,
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
                "--dartiq-cmd" => opts.dartiq_cmd = Some(value(args, &mut i)?.to_string()),
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
                other => return Err(unknown_flag(other, "run")),
            }
            i += 1;
        }
        Ok(opts)
    }
}

fn run_compare(args: &[String]) -> Result<(String, bool), Outcome> {
    let mut opts = RunOptions::parse(args)?;
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
    let mut canonical: Vec<PathBuf> = Vec::new();
    for dir in &opts.corpus_dirs {
        let canon = canonical_corpus(dir)?;
        // Plan A preflight (docs/facts/version-skew.md §4): a stale corpus
        // languageVersion re-enables experiment_not_enabled skew; fail before
        // any oracle run so the noise can never reach a report.
        corpus::check_language_version(&canon).map_err(Outcome::Error)?;
        canonical.push(canon);
    }
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
    let oracle_diags = analyze_corpora(&corpora, |dir| oracle.analyze(dir, opts.oracle_format))?;
    // The dartiq contract is always machine; only the e2e self-check (dartiq side = dart analyze itself) follows the oracle format
    let dartiq_format = if opts.e2e_oracle {
        opts.oracle_format
    } else {
        OutputFormat::Machine
    };
    let dartiq_driver = build_dartiq_driver(&opts, opts.oracle_format)?;
    let dartiq_diags = analyze_corpora(&corpora, |dir| dartiq_driver.analyze(dir, dartiq_format))?;

    let oracle_side = build_filtered_side(&corpora, oracle_diags, &filters);
    let dartiq_side = build_filtered_side(&corpora, dartiq_diags, &filters);
    let cmp = compare(&oracle_side, &dartiq_side);

    let corpus_display = corpora
        .iter()
        .map(|(_, canon)| canon.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let passed = cmp.error_jaccard() >= opts.threshold;
    let meta = report::RunMeta {
        corpus_dir: corpus_display,
        oracle_cmd: format!("{} analyze {}", opts.oracle_dart, opts.oracle_format.flag()),
        dartiq_cmd: if opts.e2e_oracle {
            format!(
                "{} analyze {} (e2e self-check)",
                opts.oracle_dart,
                opts.oracle_format.flag()
            )
        } else {
            opts.dartiq_cmd.clone().unwrap_or_default()
        },
        threshold: opts.threshold,
        severity_filter: opts.severity_filter.clone(),
        type_filter: opts.type_filter.clone(),
        codes_file: opts
            .codes_file
            .as_ref()
            .map_or(String::new(), |p| p.display().to_string()),
    };
    let (json_path, md_path) =
        report::write_reports(&cmp, &meta, passed, &opts.out).map_err(Outcome::Error)?;

    let stats: Vec<String> = cmp
        .severities
        .iter()
        .map(|s| format!("{}={:.3}", s.severity.as_str(), s.jaccard))
        .collect();
    let verdict = if passed { "PASS" } else { "FAIL" };
    let summary = format!(
        "corpus: {} (files with diagnostics: {})\n\
         oracle: {} diags | dartiq: {} diags\n\
         jaccard: {} (threshold {} on ERROR)\n\
         diffs: oracle-only {} entries, dartiq-only {} entries\n\
         reports: {} {}\n\
         result: {verdict}",
        meta.corpus_dir,
        cmp.file_count,
        cmp.oracle_total,
        cmp.dartiq_total,
        stats.join("  "),
        opts.threshold,
        cmp.oracle_only.len(),
        cmp.dartiq_only.len(),
        json_path.display(),
        md_path.display(),
    );
    Ok((summary, passed))
}

fn canonical_corpus(corpus_dir: &Path) -> Result<PathBuf, Outcome> {
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

/// Build the dartiq-side driver: e2e self-check = dart analyze itself; otherwise a command template is required.
fn build_dartiq_driver(
    opts: &RunOptions,
    oracle_format: OutputFormat,
) -> Result<DartiqDriver, Outcome> {
    if opts.e2e_oracle {
        return Ok(DartiqDriver::from_program(
            opts.oracle_dart.clone(),
            vec![
                "analyze".into(),
                oracle_format.flag().into(),
                "--no-fatal-warnings".into(),
            ],
        ));
    }
    let template = opts
        .dartiq_cmd
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            Outcome::Error("run needs --dartiq-cmd TEMPLATE (or --e2e-oracle)".into())
        })?;
    Ok(DartiqDriver::from_template(template))
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
/// M0 syntax-tier criterion: **codes-file (the 262 scanner/parser codes, see
/// tests/corpus/syntax-codes.txt) ∩ severity ERROR**; the analyzer reports `ParserErrorCode` as
/// `COMPILE_TIME_ERROR` (docs/facts/corpus-baseline.md §1), so the syntax tier cannot be obtained by
/// `--type-filter syntactic_error` alone and must be filtered by code set.
///
/// Empty-set semantics: a filter may remove all diagnostics from both sides -- then both entry sets
/// are empty and Jaccard keeps the existing rule "both empty = full agreement = 1.0" (see
/// `compare::jaccard`), so the gate still passes; same as the pure-report mode when severity-filter
/// drops the ERROR tier.
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
}

/// Filter per corpus dir, build a side per dir, then merge; key prefixes come from the corpora labels (None for a single corpus).
fn build_filtered_side(
    corpora: &[(Option<String>, PathBuf)],
    per_corpus_diags: Vec<Vec<MachineDiag>>,
    filters: &Filters,
) -> Side {
    let sides = corpora
        .iter()
        .zip(per_corpus_diags)
        .map(|((label, canon), diags)| {
            build_side_labeled(&diags, canon, label.as_deref(), |d| filters.keep(d))
        })
        .collect();
    merge_sides(sides)
}

/// --codes-file content: one code name per line, case-insensitive. TSV
/// compatible: `#` comment lines are skipped and a TAB-separated line
/// contributes its first column — the authoritative table
/// `crates/dartiq-diagnostics/data/syntax-codes.tsv` (265 rows = 262
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

fn value<'a>(args: &'a [String], i: &mut usize) -> Result<&'a str, Outcome> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| Outcome::Error(format!("flag `{}` requires a value", args[*i - 1])))
}

fn unknown_flag(flag: &str, subcommand: &str) -> Outcome {
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
    /// (crates/dartiq-diagnostics/data/syntax-codes.tsv) minus the 3
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
            std::fs::read_to_string(root.join("crates/dartiq-diagnostics/data/syntax-codes.tsv"))
                .expect("authoritative TSV present");
        let txt = std::fs::read_to_string(root.join("tests/corpus/syntax-codes.txt"))
            .expect("gate codes file present");
        let tsv_codes = parse_codes_file(&tsv).expect("TSV parses");
        let gate_codes = parse_codes_file(&txt).expect("gate file parses");
        assert_eq!(tsv_codes.len(), 265, "TSV = 262 syntax + 3 family codes");
        assert_eq!(gate_codes.len(), 262, "gate file = 262 syntax codes");
        // 262 raw data lines for the gate file (comments/blanks excluded): a
        // duplicated line would hide in the set comparison but still be drift.
        assert_eq!(
            txt.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .count(),
            262
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
        // oracle: 1 in-set + 1 out-of-set (an analyzer semantic code); dartiq: only the 1 in-set entry
        let oracle = vec![mk("EXPECTED_TOKEN"), mk("UNDEFINED_CLASS")];
        let dartiq = vec![mk("expected_token")];
        let filters = Filters {
            severities: None,
            types: None,
            codes: Some(BTreeSet::from(["expected_token".to_string()])),
        };
        let root = Path::new("/corpus");
        let cmp = compare(
            &build_side_labeled(&oracle, root, None, |d| filters.keep(d)),
            &build_side_labeled(&dartiq, root, None, |d| filters.keep(d)),
        );
        assert_eq!(cmp.oracle_total, 1, "out-of-set oracle-only entry dropped");
        assert_eq!(cmp.dartiq_total, 1);
        assert_eq!(cmp.oracle_dropped, 1, "the drop is accounted, not silent");
        assert_eq!(cmp.oracle_only.len(), 0);
        assert_eq!(cmp.dartiq_only.len(), 0);
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
        // Missing --dartiq-cmd without --e2e-oracle: errors after corpus checks -- a nonexistent corpus reports the corpus error first
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
}
