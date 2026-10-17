//! CLI entry point for the corpus generator.

use std::path::PathBuf;
use std::process::ExitCode;

use corpus_gen::generator::{Config, generate};
use corpus_gen::plan::{Layout, Options, Scale};

const USAGE: &str = "\
usage: corpus-gen --out <dir> --scale <slice|1k|10k|100k> [--seed <u64>] [--layout <flat|workspace>] [--version-ladder] [--emit-replay-seq <n>]

Generates a deterministic synthetic Dart corpus (a multi-package project)
under <dir>, plus a manifest.json with counters and a reproducibility
fingerprint. The same seed always produces byte-identical output.
`slice` is the ~200-file preset whose seed-0 output is tracked under
tests/corpus/mega-x/slice/ (the golden byte-stability pin).

--layout workspace emits a pub-workspace tree instead of the flat default:
a root `workspace:` pubspec, `resolution: workspace` members at two
depths, per-member .dart_tool/pub/workspace_ref.json, vendored
non-member path deps under third_party/, and the hand-written shared
.dart_tool/package_config.json (the nested-versions corpus shape).

--version-ladder rotates feature-package sdk constraints across the
2.12 -> 3.15 ladder, pins `// @dart =` banners on a deterministic subset
of rotation files (below and above the package floor), emits per-package
analysis_options.yaml carrying an enable-experiment list, and wires
cross-version package import edges.

--flutter-app (#2063) adds an in-corpus stub `packages/flutter` package
(pubspec + lib/widgets.dart + part files declaring StatelessWidget,
StatefulWidget/State, BuildContext, Key and the MaterialApp/Scaffold/
Text/... concrete surface), makes every feature package path-depend on it,
turns the widget host/part slots into real StatefulWidget/State pairs with
nested build trees, and renders every 4th rotation file as a widget scene.
Both the oracle and dartq analyze the same stub, so parity stays
dart-analyze-over-identical-sources with no Flutter SDK dependency.

--emit-replay-seq <n> additionally writes <dir>/edits.jsonl: an n-step
deterministic replay sequence (splice/remove/create) that round-trips the
tree to its initial state.
";

struct Args {
    out: PathBuf,
    scale: Scale,
    seed: u64,
    options: Options,
    emit_replay_seq: Option<u64>,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut out: Option<PathBuf> = None;
    let mut scale: Option<Scale> = None;
    let mut seed = 0u64;
    let mut options = Options::default();
    let mut emit_replay_seq: Option<u64> = None;
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
                    format!("unknown scale {value:?}; expected one of slice, 1k, 10k, 100k")
                })?);
            }
            "--seed" => {
                let value = take_value("--seed")?;
                seed = value
                    .parse()
                    .map_err(|_| format!("invalid seed {value:?}; expected u64"))?;
            }
            "--layout" => {
                let value = take_value("--layout")?;
                options.layout = Layout::parse(&value).ok_or_else(|| {
                    format!("unknown layout {value:?}; expected one of flat, workspace")
                })?;
            }
            "--version-ladder" => {
                options.version_ladder = true;
            }
            "--flutter-app" => {
                options.flutter_app = true;
            }
            "--emit-replay-seq" => {
                if emit_replay_seq.is_some() {
                    return Err("duplicate --emit-replay-seq".to_string());
                }
                let value = take_value("--emit-replay-seq")?;
                emit_replay_seq =
                    Some(value.parse().map_err(|_| {
                        format!("invalid --emit-replay-seq {value:?}; expected u64")
                    })?);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let out = out.ok_or("missing --out <dir>")?;
    let scale = scale.ok_or("missing --scale <slice|1k|10k|100k>")?;
    Ok(Args {
        out,
        scale,
        seed,
        options,
        emit_replay_seq,
    })
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    if argv
        .iter()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let parsed = match parse_args(&argv) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let config = Config {
        out: parsed.out,
        scale: parsed.scale,
        seed: parsed.seed,
        options: parsed.options,
        emit_replay_seq: parsed.emit_replay_seq,
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
