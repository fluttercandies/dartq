//! bench-corpus-gen — deterministic LOC-targeted synthetic Dart corpus for
//! the ten-target performance measurement batch (#2404, artifact of #2421).
//!
//! # What it emits
//!
//! A single self-contained Dart package (no external dependencies, `dart:`
//! core libraries only) under `--out`, directly analyzable by both dartq and
//! the pinned Dart oracle with no pub get: `pubspec.yaml` (SDK constraint
//! `^3.13.0`, matching the pinned Dart 3.13.1), a hand-written
//! `.dart_tool/package_config.json`, every `lib/src/**` source file, and a
//! `manifest.json` with counters plus a reproducibility fingerprint.
//!
//! # Shape mix (by emitted Dart line share)
//!
//! - ~35% switch/match-heavy (`lib/src/switch/`): sealed hierarchies plus
//!   exhaustive switch expressions, guarded pattern switch statements,
//!   if-case chains, and object/list/map/record/relational/cast patterns —
//!   the flow/pattern engine's worst case.
//! - ~25% barrel chains (`lib/src/chains/chainNN/`): hub modules under deep
//!   export spines that re-export overlapping name sets through multiple
//!   paths, with importer files at the leaves pulling the root barrel — the
//!   resolution/library-graph worst case.
//! - ~40% general mix (`lib/src/general/`): bounded generics, mixins with
//!   `on` constraints and mixin applications, extensions on generic types,
//!   typedef'd function types with closures, records, const/factory/operator
//!   classes, and pattern destructuring in declarations.
//!
//! File sizes mix ~200-line and ~2000-line files: every 5th file targets
//! ~2000 lines, the rest ~200.
//!
//! # Seed contract
//!
//! The same `--seed` and `--loc` always produce a byte-identical output tree
//! and an identical `manifest.json` fingerprint (FNV-1a 64 over per-file
//! path/size pairs in emission order). Std-only SplitMix64 PRNG, no time or
//! entropy sources. Defaults: seed 0, 100000 LOC.
//!
//! # Consumption
//!
//! ```text
//! cargo run -p bench-corpus-gen -- --out /tmp/corpus100k --loc 100000 --seed 0
//! ```
//!
//! Point dartq and the oracle at the output directory (the package config is
//! already materialized). Regenerate any time — determinism makes every
//! measurement reproducible. Never commit the generated tree; target a fresh
//! directory per run for a clean manifest. Output goes only where `--out`
//! points; the generator writes nothing inside the repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bench_corpus_gen::manifest_text;
use bench_corpus_gen::render;

const DEFAULT_OUT: &str = "/tmp/corpus100k";
const DEFAULT_LOC: usize = 100_000;
const DEFAULT_SEED: u64 = 0;

const USAGE: &str = "\
usage: bench-corpus-gen [--out <dir>] [--loc <n>] [--seed <u64>]

Generates a deterministic synthetic Dart corpus (a self-contained package)
under <dir>, plus a manifest.json with counters and a reproducibility
fingerprint. The same seed and loc always produce byte-identical output.

Defaults: --out /tmp/corpus100k --loc 100000 --seed 0
";

struct Args {
    out: PathBuf,
    loc: usize,
    seed: u64,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut out: Option<PathBuf> = None;
    let mut loc: Option<usize> = None;
    let mut seed: Option<u64> = None;
    let mut index = 1;
    while index < argv.len() {
        let raw = argv[index].clone();
        index += 1;
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (raw.as_str(), None),
        };
        let mut take_value = |what: &str| -> Result<String, String> {
            if let Some(value) = &inline {
                return Ok(value.clone());
            }
            if index < argv.len() {
                let value = argv[index].clone();
                index += 1;
                return Ok(value);
            }
            Err(format!("missing value for {what}"))
        };
        match name {
            "--out" => {
                if out.is_some() {
                    return Err("duplicate --out".to_string());
                }
                out = Some(PathBuf::from(take_value("--out")?));
            }
            "--loc" => {
                if loc.is_some() {
                    return Err("duplicate --loc".to_string());
                }
                let value = take_value("--loc")?;
                loc = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid --loc {value:?}; expected a positive integer"))?,
                );
            }
            "--seed" => {
                if seed.is_some() {
                    return Err("duplicate --seed".to_string());
                }
                let value = take_value("--seed")?;
                seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid --seed {value:?}; expected u64"))?,
                );
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Args {
        out: out.unwrap_or_else(|| PathBuf::from(DEFAULT_OUT)),
        loc: loc.unwrap_or(DEFAULT_LOC),
        seed: seed.unwrap_or(DEFAULT_SEED),
    })
}

fn write_corpus(out: &Path, files: &[bench_corpus_gen::File]) -> std::io::Result<()> {
    for file in files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &file.body)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().skip(1).any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_args(&argv) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.loc == 0 {
        eprintln!("error: --loc must be positive");
        return ExitCode::from(2);
    }
    let stats = render(args.seed, args.loc);
    if let Err(error) = write_corpus(&args.out, &stats.files) {
        eprintln!("error: writing corpus failed: {error}");
        return ExitCode::FAILURE;
    }
    let manifest = manifest_text(&stats);
    if let Err(error) = fs::write(args.out.join("manifest.json"), manifest) {
        eprintln!("error: writing manifest failed: {error}");
        return ExitCode::FAILURE;
    }
    let total_lines =
        stats.switch_heavy.lines + stats.barrel_chains.lines + stats.general_mix.lines;
    println!(
        "generated {} dart files / {} dart lines under {} (loc target {}, seed {}, fingerprint fnv1a64:{:016x})",
        stats.files.len() - 2,
        total_lines,
        args.out.display(),
        stats.loc_target,
        stats.seed,
        stats.fingerprint
    );
    println!(
        "  switch_heavy:  {} files / {} lines",
        stats.switch_heavy.files, stats.switch_heavy.lines
    );
    println!(
        "  barrel_chains: {} files / {} lines",
        stats.barrel_chains.files, stats.barrel_chains.lines
    );
    println!(
        "  general_mix:   {} files / {} lines",
        stats.general_mix.files, stats.general_mix.lines
    );
    ExitCode::SUCCESS
}
