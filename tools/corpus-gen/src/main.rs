//! CLI entry point for the corpus generator.

use std::path::PathBuf;
use std::process::ExitCode;

use corpus_gen::generator::{generate, Config};
use corpus_gen::plan::Scale;

const USAGE: &str = "\
usage: corpus-gen --out <dir> --scale <1k|10k|100k> [--seed <u64>]

Generates a deterministic synthetic Dart corpus (a multi-package project)
under <dir>, plus a manifest.json with counters and a reproducibility
fingerprint. The same seed always produces byte-identical output.
";

struct Args {
    out: PathBuf,
    scale: Scale,
    seed: u64,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut out: Option<PathBuf> = None;
    let mut scale: Option<Scale> = None;
    let mut seed = 0u64;
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
            "--scale" => {
                if scale.is_some() {
                    return Err("duplicate --scale".to_string());
                }
                let value = take_value("--scale")?;
                scale = Some(Scale::parse(&value).ok_or_else(|| {
                    format!("unknown scale {value:?}; expected one of 1k, 10k, 100k")
                })?);
            }
            "--seed" => {
                let value = take_value("--seed")?;
                seed = value
                    .parse()
                    .map_err(|_| format!("invalid seed {value:?}; expected u64"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let out = out.ok_or("missing --out <dir>")?;
    let scale = scale.ok_or("missing --scale <1k|10k|100k>")?;
    Ok(Args { out, scale, seed })
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
    let config = Config {
        out: args.out,
        scale: args.scale,
        seed: args.seed,
    };
    match generate(&config) {
        Ok(stats) => {
            println!(
                "generated {} files / {} lines under {} (scale {}, seed {}, fingerprint fnv1a64:{:016x})",
                stats.manifest.files,
                stats.manifest.lines,
                config.out.display(),
                stats.manifest.scale,
                stats.manifest.seed,
                stats.manifest.fingerprint
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}
