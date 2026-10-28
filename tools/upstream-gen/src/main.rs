//! `analyzer-codes-gen` — regenerates the checked-in analyzer code table
//! `crates/dartq-diagnostics/data/analyzer-codes.tsv` from the pinned
//! upstream Dart SDK tree (#2677).
//!
//! The SDK tree is a local read-only checkout that tracked code must
//! never reference silently: the source root comes from `--sdk-src` or
//! the `DART_SDK_SRC` environment variable, and the run fails with a
//! usage message when neither is set.
//!
//! Usage (from the workspace root):
//!
//! ```text
//! cargo run -p upstream-gen --bin analyzer-codes-gen -- --sdk-src <dart-sdk-tree> [--check] [--verbose]
//! ```
//!
//! Modes:
//! - default: writes the TSV to `crates/dartq-diagnostics/data/analyzer-codes.tsv`
//!   (or `--out <path>`);
//! - `--check`: re-derives the table and diffs it against the tracked
//!   file, exiting 1 with a regeneration hint on drift — the CI face.
//!
//! Determinism: the emitted document is a pure function of the SDK tree
//! (no timestamps, sorted rows), so `--check` is byte-exact.

mod registry;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use registry::{Entry, Row, class_map, fold, parse_registry, render_tsv};

const USAGE: &str = "usage: analyzer-codes-gen [--sdk-src <dart-sdk-tree>] [--out <path>] [--check] [--verbose]\n\
                     the SDK tree also resolves from the DART_SDK_SRC environment variable";

/// `(message, exit code)`: 2 marks usage errors, 1 run failures.
type Failure = (String, u8);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((message, code)) => {
            eprintln!("analyzer-codes-gen: {message}");
            ExitCode::from(code)
        }
    }
}

fn run() -> Result<(), Failure> {
    let mut sdk_src: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut check = false;
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sdk-src" => {
                sdk_src = Some(PathBuf::from(
                    args.next()
                        .ok_or(("--sdk-src needs a path".to_owned(), 2))?,
                ));
            }
            "--out" => {
                out = Some(PathBuf::from(
                    args.next().ok_or(("--out needs a path".to_owned(), 2))?,
                ));
            }
            "--check" => check = true,
            "--verbose" => verbose = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => {
                return Err((format!("unknown argument {other:?}; {USAGE}"), 2));
            }
        }
    }
    let Some(sdk_src) = sdk_src.or_else(|| std::env::var("DART_SDK_SRC").ok().map(PathBuf::from))
    else {
        return Err((format!("no SDK source given; {USAGE}"), 2));
    };
    let out = out.unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/dartq-diagnostics/data/analyzer-codes.tsv")
    });

    let registry_path = sdk_src.join("pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart");
    let analyzer_yaml_path = sdk_src.join("pkg/analyzer/messages.yaml");
    let fe_yaml_path = sdk_src.join("pkg/_fe_analyzer_shared/messages.yaml");
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|e| (format!("reading {}: {e}", path.display()), 1))
    };
    let version = sdk_version(&sdk_src)?;

    let entries = parse_registry(&read(&registry_path)?)
        .map_err(|e| (format!("{}:{e}", registry_path.display()), 1))?;
    let classes =
        class_map(&read(&analyzer_yaml_path)?, &read(&fe_yaml_path)?).map_err(|e| (e, 1))?;
    let rows = fold(&entries, &classes).map_err(|e| (e, 1))?;
    if verbose {
        print_tally(&entries, &rows, &classes);
    }
    let document = render_tsv(&rows, entries.len(), &version);

    if check {
        let tracked = read(&out).map_err(|(message, _)| (format!("--check {message}"), 1))?;
        if tracked == document {
            println!(
                "analyzer-codes-gen: ok — {} rows match {}",
                rows.len(),
                out.display()
            );
            return Ok(());
        }
        let first_diff = tracked
            .lines()
            .zip(document.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map_or(
                tracked.lines().count().max(document.lines().count()) + 1,
                |(no, _)| no + 1,
            );
        return Err((
            format!(
                "{}: drift from the pinned SDK registry (first difference at line \
                 {first_diff}); regenerate with: cargo run -p upstream-gen --bin \
                 analyzer-codes-gen -- --sdk-src <dart-sdk-tree>",
                out.display()
            ),
            1,
        ));
    }

    fs::write(&out, document).map_err(|e| (format!("writing {}: {e}", out.display()), 1))?;
    println!(
        "analyzer-codes-gen: wrote {} rows ({} registry consts) to {}",
        rows.len(),
        entries.len(),
        out.display()
    );
    Ok(())
}

/// The SDK identity stamp for the TSV header, from the checkout's
/// `tools/VERSION` (the file the SDK's own version tooling generates
/// from) — e.g. `dart 3.15.0 (main channel)`.
fn sdk_version(sdk_src: &Path) -> Result<String, Failure> {
    let version_path = sdk_src.join("tools/VERSION");
    let text = fs::read_to_string(&version_path).map_err(|e| {
        (
            format!(
                "reading {}: {e} (is this a Dart SDK tree?)",
                version_path.display()
            ),
            1,
        )
    })?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(' ') {
            fields.insert(key.to_owned(), value.trim().to_owned());
        }
    }
    let get = |key: &str| fields.get(key).cloned().unwrap_or_default();
    let channel = get("CHANNEL");
    let version = format!("{}.{}.{}", get("MAJOR"), get("MINOR"), get("PATCH"));
    if version == ".." || channel.is_empty() {
        return Err((
            format!(
                "{}: malformed VERSION file (channel {channel:?}, version {version:?})",
                version_path.display()
            ),
            1,
        ));
    }
    Ok(format!("dart {version} ({channel} channel)"))
}

/// The `--verbose` face: per-class row tallies cross-checked against the
/// yaml sources, plus the fold statistics.
fn print_tally(entries: &[Entry], rows: &[Row], classes: &BTreeMap<String, String>) {
    let mut by_class: BTreeMap<&str, u32> = BTreeMap::new();
    for row in rows {
        *by_class.entry(row.class.as_str()).or_default() += 1;
    }
    let registry_consts: BTreeSet<&str> = entries
        .iter()
        .map(|entry| entry.const_name.as_str())
        .collect();
    let yaml_only = classes
        .keys()
        .filter(|const_name| !registry_consts.contains(const_name.as_str()))
        .count();
    let mut mixed_types: Vec<&str> = Vec::new();
    let mut first_per_name: BTreeMap<&str, &str> = BTreeMap::new();
    for entry in entries {
        match first_per_name.get(entry.name.as_str()) {
            Some(first) if *first != entry.dtype => mixed_types.push(entry.name.as_str()),
            None => {
                first_per_name.insert(entry.name.as_str(), entry.dtype.as_str());
            }
            _ => {}
        }
    }
    mixed_types.sort_unstable();
    mixed_types.dedup();

    let mut report = String::new();
    let _ = writeln!(report, "registry entries: {}", entries.len());
    let _ = writeln!(report, "folded name rows: {}", rows.len());
    let _ = writeln!(report, "rows per class:");
    for (class, count) in by_class {
        let _ = writeln!(report, "  {class}: {count}");
    }
    let _ = writeln!(
        report,
        "yaml consts without a registry entry (inactive upstream): {yaml_only}"
    );
    let _ = writeln!(
        report,
        "mixed-type names (first registry entry wins): {}",
        if mixed_types.is_empty() {
            "none".to_owned()
        } else {
            mixed_types.join(", ")
        }
    );
    print!("{report}");
}
