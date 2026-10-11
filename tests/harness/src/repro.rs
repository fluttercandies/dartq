//! PRD §19.5 reproducibility face: identical run tuples answer byte-identical
//! normalized output.
//!
//! The engine's in-process pin (crates/dartq-engine/tests/reproducibility.rs)
//! covers determinism inside one process. This face covers the across-fresh-
//! process guarantee: every run launches the `--dartq-cmd` from scratch, the
//! way the differential and gate faces invoke it, so process-level
//! nondeterminism (map iteration leaking into output order, env-dependent
//! paths, per-process state) surfaces here and nowhere else.
//!
//! # Normalization
//!
//! Rows go through the differential face's own normalizer
//! ([`EntryKeyBuilder`]: memoized canonicalization to corpus-relative paths,
//! lowercased codes) and render to one canonical line per row
//! (`code|severity|type|file|line|col|length|message`). Rows sort before the
//! byte compare — the differential face compares (code, file) sets, so the
//! repro contract is order-normalized too: identical rows in a different
//! emission order are not a §19.5 divergence, any content divergence is.
//!
//! # Tuple
//!
//! `scripts/repro-tuple.sh` pins the environment identity (git head, worktree
//! patch, toolchain) and hashes a run output into its `output_sha256` field;
//! this face's report carries the byte identity of what was compared (the
//! run-1 normalized digest, `output_digest`), so the script's `--output` hook
//! seals the run this face judged. The
//! per-`Context` tuple faces (SDK, `PackageGraph`, `LanguageVersions`, `Options`,
//! `Target`, `ConditionalEnv`) are the run inputs themselves: one canonical corpus
//! dir plus one dartq command, recorded verbatim in the report.
//!
//! Exit codes: 0 = all runs byte-identical, 1 = divergence (the first
//! divergent row rides the summary and the report), 2 = harness error.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::json;

use crate::Outcome;
use crate::dartq::{DartqDriver, DartqProbe};
use crate::machine::{EntryKeyBuilder, MachineDiag, OutputFormat};
use crate::{canonical_corpus, corpus};

/// Same default report root as the run face (the reports are siblings under it).
const DEFAULT_OUT: &str = "target/harness-report";

struct ReproOptions {
    corpus_dir: PathBuf,
    dartq_cmd: String,
    runs: usize,
    out: PathBuf,
}

impl ReproOptions {
    fn parse(args: &[String]) -> Result<Self, Outcome> {
        let mut opts = Self {
            corpus_dir: PathBuf::new(),
            dartq_cmd: String::new(),
            runs: 2,
            out: PathBuf::from(DEFAULT_OUT),
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--corpus-dir" => opts.corpus_dir = value(args, &mut i)?.into(),
                "--dartq-cmd" => opts.dartq_cmd = value(args, &mut i)?.to_string(),
                "--runs" => {
                    opts.runs = value(args, &mut i)?
                        .parse()
                        .ok()
                        .filter(|&n| n >= 2)
                        .ok_or_else(|| Outcome::Error("--runs must be an integer >= 2".into()))?;
                }
                "--out" => opts.out = value(args, &mut i)?.into(),
                other => {
                    return Err(Outcome::Error(format!(
                        "unknown repro flag `{other}`; see `harness repro --help`"
                    )));
                }
            }
            i += 1;
        }
        if opts.corpus_dir.as_os_str().is_empty() {
            return Err(Outcome::Error("repro needs --corpus-dir DIR".into()));
        }
        if opts.dartq_cmd.trim().is_empty() {
            return Err(Outcome::Error(
                "repro needs --dartq-cmd TEMPLATE (the same template the run face takes)".into(),
            ));
        }
        Ok(opts)
    }
}

fn value<'a>(args: &'a [String], i: &mut usize) -> Result<&'a str, Outcome> {
    let name = &args[*i];
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| Outcome::Error(format!("{name} needs a value")))
}

/// One normalized output: the sorted canonical row lines plus their digest.
struct NormalizedRun {
    bytes: String,
    digest: String,
    rows: usize,
}

pub(crate) fn repro_entry(args: &[String]) -> Result<(String, bool), Outcome> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok((usage().to_string(), true));
    }
    let opts = ReproOptions::parse(args)?;
    let corpus = canonical_corpus(&opts.corpus_dir)?;
    corpus::check_language_version(&corpus).map_err(Outcome::Error)?;
    let driver = DartqDriver::from_template(&opts.dartq_cmd);

    // Run 1 is the baseline; every later run compares against its bytes. The
    // render buffer is built once and reused (cleared) per run — per run only
    // the divergent-line lookup and the report allocate.
    let baseline = capture(&driver, &corpus, 1)?;
    let output_digest = baseline.digest.clone();
    let mut digest = output_digest.clone();
    let mut row_counts = vec![baseline.rows];
    let mut divergence = None;
    for run in 2..=opts.runs {
        let attempt = capture(&driver, &corpus, run)?;
        row_counts.push(attempt.rows);
        if divergence.is_none() && attempt.bytes != baseline.bytes {
            divergence = first_divergence(&baseline.bytes, &attempt.bytes, run);
        }
        digest = attempt.digest;
    }

    let path = opts.out.join("repro-report.json");
    let (verdict, exit_success) = match &divergence {
        None => ("match", true),
        Some(_) => ("diverged", false),
    };
    let report = json!({
        "record": "repro_report",
        "schema_version": 1,
        "corpus_dir": corpus.display().to_string(),
        "dartq_cmd": opts.dartq_cmd,
        "runs": opts.runs,
        "row_counts": row_counts,
        // The run-1 normalized-output digest (blake3-128, the workspace's
        // content-hash family) — the byte identity `scripts/repro-tuple.sh`
        // seals alongside its environment fields (its `output_sha256` hashes
        // the raw output file; this digests the normalized comparison form).
        "output_digest": output_digest,
        "final_run_digest": digest,
        "verdict": verdict,
        "first_divergence": divergence.as_ref().map(|d| {
            json!({
                "run": d.run,
                "row": d.row,
                "baseline": d.baseline,
                "repro": d.repro,
            })
        }),
    });
    std::fs::create_dir_all(&opts.out)
        .map_err(|e| Outcome::Error(format!("creating {}: {e}", opts.out.display())))?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&report).expect("report serializes"),
    )
    .map_err(|e| Outcome::Error(format!("writing {}: {e}", path.display())))?;

    let summary = match &divergence {
        None => format!(
            "repro: {}/{} runs byte-identical ({} rows, output digest {})\
             \nreport: {}",
            opts.runs,
            opts.runs,
            baseline.rows,
            output_digest,
            path.display()
        ),
        Some(d) => format!(
            "repro: DIVERGED at run {} row {}\
             \n  baseline: {}\
             \n  repro:    {}\
             \nreport: {}",
            d.run,
            d.row,
            d.baseline,
            d.repro,
            path.display()
        ),
    };
    Ok((summary, exit_success))
}

fn usage() -> &'static str {
    "usage:
  harness repro --corpus-dir DIR --dartq-cmd TEMPLATE [--runs N] [--out DIR]
      PRD §19.5 reproducibility: run the dartq side N times (default 2) over
      the corpus with fresh processes, normalize each run's machine rows
      through the differential face's normalizer (corpus-relative paths,
      lowercased codes, order-normalized), and byte-compare every run against
      run 1. Exit codes: 0 = byte-identical, 1 = divergence (first divergent
      row reported), 2 = harness error. The report under --out carries the
      run-1 normalized digest in `output_digest` — the blake3-128 of the
      normalized comparison form, distinct from the `output_sha256`
      scripts/repro-tuple.sh seals (the script hashes the raw output file) —
      so CI can assert same tuple ⇒ same digest."
}

/// One dartq run, normalized into the canonical byte form.
fn capture(
    driver: &DartqDriver,
    corpus: &std::path::Path,
    run: usize,
) -> Result<NormalizedRun, Outcome> {
    let diags = match driver.probe(corpus, OutputFormat::Machine) {
        DartqProbe::Measured(diags) => diags,
        DartqProbe::Panic => {
            return Err(Outcome::Error(format!(
                "run {run} panicked (exit 4, empty stdout): reproducibility needs a measurement"
            )));
        }
        DartqProbe::Failed(msg) => {
            return Err(Outcome::Error(format!("run {run} failed: {msg}")));
        }
    };
    let mut bytes = String::new();
    let rows = normalize_rows(corpus, &diags, &mut bytes);
    Ok(NormalizedRun {
        digest: digest_hex(bytes.as_bytes()),
        bytes,
        rows,
    })
}

/// Render every row through the shared normalizer, sort, and append the
/// canonical lines (each `\n`-terminated) to `out`. Returns the row count.
fn normalize_rows(corpus_root: &std::path::Path, diags: &[MachineDiag], out: &mut String) -> usize {
    let mut keys = EntryKeyBuilder::new(corpus_root);
    let mut rows: Vec<String> = Vec::with_capacity(diags.len());
    for diag in diags {
        let (code, file) = keys.key(diag);
        rows.push(format!(
            "{code}|{}|{}|{file}|{}|{}|{}|{}",
            diag.severity.as_str(),
            diag.dtype,
            diag.line,
            diag.col,
            diag.length,
            diag.message
        ));
    }
    rows.sort_unstable();
    for row in &rows {
        out.push_str(row);
        out.push('\n');
    }
    rows.len()
}

/// The first line position where two sorted normalized outputs disagree.
/// Equal prefixes with a length mismatch report the first extra row; full
/// equality answers `None`.
fn first_divergence(baseline: &str, candidate: &str, run: usize) -> Option<Divergence> {
    let mut baseline_lines = baseline.lines();
    let mut candidate_lines = candidate.lines();
    let mut row = 0;
    loop {
        let a = baseline_lines.next();
        let b = candidate_lines.next();
        match (a, b) {
            (None, None) => return None,
            (Some(x), Some(y)) if x == y => row += 1,
            (a, b) => {
                return Some(Divergence {
                    run,
                    row,
                    baseline: a.unwrap_or("<absent>").to_owned(),
                    repro: b.unwrap_or("<absent>").to_owned(),
                });
            }
        }
    }
}

struct Divergence {
    run: usize,
    row: usize,
    baseline: String,
    repro: String,
}

/// blake3-128 over the normalized bytes, hex — the workspace's content-hash
/// family, the same one the regression cases key on.
fn digest_hex(bytes: &[u8]) -> String {
    let digest = dartq_source::content_hash(bytes);
    let mut hex = String::with_capacity(digest.as_bytes().len() * 2);
    for byte in digest.as_bytes() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Severity;

    fn diag(code: &str, severity: Severity, file: &str, line: u32, message: &str) -> MachineDiag {
        MachineDiag {
            severity,
            dtype: "COMPILE_TIME_ERROR".to_owned(),
            code: code.to_owned(),
            file: file.into(),
            line,
            col: 7,
            length: 3,
            message: message.to_owned(),
        }
    }

    /// Same rows in a different emission order normalize to identical bytes:
    /// the contract is order-normalized (mirroring the differential face's
    /// set comparison), so a benign reorder is not a §19.5 divergence.
    #[test]
    fn normalized_rows_are_order_normalized() {
        let corpus = std::path::Path::new("/w");
        let mut first = String::new();
        let rows = normalize_rows(
            corpus,
            &[
                diag("A_CODE", Severity::Error, "/w/lib/a.dart", 2, "first"),
                diag("b_code", Severity::Warning, "/w/lib/b.dart", 5, "second"),
            ],
            &mut first,
        );
        assert_eq!(rows, 2);

        let mut second = String::new();
        normalize_rows(
            corpus,
            &[
                diag("b_code", Severity::Warning, "/w/lib/b.dart", 5, "second"),
                diag("A_CODE", Severity::Error, "/w/lib/a.dart", 2, "first"),
            ],
            &mut second,
        );
        assert_eq!(first, second, "identical rows sort to identical bytes");
        assert_eq!(
            first,
            "a_code|ERROR|COMPILE_TIME_ERROR|lib/a.dart|2|7|3|first\n\
             b_code|WARNING|COMPILE_TIME_ERROR|lib/b.dart|5|7|3|second\n",
            "rows render code-lowercased, corpus-relative, field-ordered"
        );
    }

    /// The normalizer is the differential face's: absolute-vs-relative
    /// spellings of the same on-disk file collapse to one corpus-relative row,
    /// and the digest is stable across re-renders.
    #[test]
    fn path_spellings_collapse_through_the_shared_normalizer() {
        let root = std::env::temp_dir().join(format!("dq_repro_norm_{}", std::process::id()));
        std::fs::create_dir_all(root.join("lib")).expect("mkdir");
        let corpus = root.canonicalize().expect("canonical corpus");
        let direct = corpus.join("lib/a.dart");
        std::fs::write(&direct, "void a() {}\n").expect("fixture");

        let mut straight = String::new();
        normalize_rows(
            &corpus,
            &[diag(
                "X",
                Severity::Error,
                direct.display().to_string().as_str(),
                1,
                "m",
            )],
            &mut straight,
        );
        let mut winding = String::new();
        normalize_rows(
            &corpus,
            &[diag(
                "x",
                Severity::Error,
                corpus
                    .join("lib")
                    .join("..")
                    .join("lib")
                    .join("a.dart")
                    .display()
                    .to_string()
                    .as_str(),
                1,
                "m",
            )],
            &mut winding,
        );
        assert_eq!(
            straight, winding,
            "one file, one row — whatever spelling arrived"
        );
        assert_eq!(
            digest_hex(straight.as_bytes()),
            digest_hex(winding.as_bytes()),
            "equal bytes hash equal"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The divergence finder names the first differing row position with both
    /// sides' text, and equal outputs yield none.
    #[test]
    fn first_divergence_pins_the_row() {
        let baseline = "a\nb\nc\n";
        assert!(first_divergence(baseline, baseline, 2).is_none());

        let d = first_divergence("a\nb\nc\n", "a\nx\nc\n", 3).expect("diverges");
        assert_eq!((d.run, d.row), (3, 1));
        assert_eq!(d.baseline, "b");
        assert_eq!(d.repro, "x");

        // A missing row is a divergence against `<absent>`, not a panic.
        let d = first_divergence("a\nb\n", "a\n", 2).expect("length divergence");
        assert_eq!(d.row, 1);
        assert_eq!(d.repro, "<absent>");
    }
}
