//! `linter-meta-gen` — regenerates the checked-in lint rule metadata
//! table `crates/dartq-lints/src/generated/rule_meta.rs` and the lint
//! documentation rows
//! `crates/dartq-diagnostics/src/generated/lint_docs_table.rs` from the
//! pinned upstream sources (#2676).
//!
//! Both source trees are local read-only checkouts that tracked code must
//! never reference silently: the SDK root resolves from `--sdk-src` or
//! `DART_SDK_SRC`, the `package:lints` snapshot from `--lints-src` or
//! `DART_LINTS_SRC`, and the run fails with a usage message when either
//! is missing.
//!
//! Usage (from the workspace root):
//!
//! ```text
//! cargo run -p upstream-gen --bin linter-meta-gen -- --sdk-src <dart-sdk-tree> --lints-src <lints-tree> [--check] [--verbose]
//! ```
//!
//! Modes:
//! - default: writes both outputs;
//! - `--check`: re-derives both documents and byte-compares them against
//!   the tracked files, exiting 1 with a regeneration hint on drift —
//!   the CI face.
//!
//! Determinism: both emitted documents are pure functions of the source
//! trees (no timestamps, sorted rows), so `--check` is byte-exact.

mod linter_meta;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use linter_meta::{
    RuleRow, fold, lints_set_names, parse_messages, render_lint_docs_rows, render_rule_meta,
    scan_rule_files, validate_names, version_consts,
};

const USAGE: &str = "usage: linter-meta-gen [--sdk-src <dart-sdk-tree>] [--lints-src <lints-tree>]\
                     \n                     [--out <path>] [--docs-out <path>] [--check] [--verbose]\
                     \n                     the SDK tree resolves from DART_SDK_SRC, the lints tree from DART_LINTS_SRC";

/// `(message, exit code)`: 2 marks usage errors, 1 run failures.
type Failure = (String, u8);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((message, code)) => {
            eprintln!("linter-meta-gen: {message}");
            ExitCode::from(code)
        }
    }
}

fn run() -> Result<(), Failure> {
    let mut sdk_src: Option<PathBuf> = None;
    let mut lints_src: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut docs_out: Option<PathBuf> = None;
    let mut check = false;
    let mut verbose = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| -> Result<PathBuf, Failure> {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| (format!("{flag} needs a path"), 2))
        };
        match arg.as_str() {
            "--sdk-src" => sdk_src = Some(value("--sdk-src")?),
            "--lints-src" => lints_src = Some(value("--lints-src")?),
            "--out" => out = Some(value("--out")?),
            "--docs-out" => docs_out = Some(value("--docs-out")?),
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
    let Some(lints_src) =
        lints_src.or_else(|| std::env::var("DART_LINTS_SRC").ok().map(PathBuf::from))
    else {
        return Err((format!("no lints source given; {USAGE}"), 2));
    };
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = out.unwrap_or_else(|| {
        manifest.join("../../crates/dartq-lints/src/generated/rule_meta.rs")
    });
    let docs_out =
        docs_out.unwrap_or_else(|| {
            manifest.join("../../crates/dartq-diagnostics/src/generated/lint_docs_table.rs")
        });

    let read = |path: &Path| -> Result<String, Failure> {
        fs::read_to_string(path).map_err(|e| (format!("reading {}: {e}", path.display()), 1))
    };
    let version = sdk_version(&sdk_src)?;
    let messages = read(&sdk_src.join("pkg/linter/messages.yaml"))?;
    let entries = parse_messages(&messages).map_err(|e| (e, 1))?;
    let lint_names = read(&sdk_src.join("pkg/linter/lib/src/lint_names.g.dart"))?;
    validate_names(&entries, &lint_names).map_err(|e| (e, 1))?;
    let versions = version_consts(&read(&sdk_src.join(
        "pkg/analyzer/lib/analysis_rule/rule_state.dart",
    ))?);
    let mut rule_files: Vec<(String, String)> = Vec::new();
    let rules_dir = sdk_src.join("pkg/linter/lib/src/rules");
    let mut names: Vec<String> = fs::read_dir(&rules_dir)
        .map_err(|e| (format!("reading {}: {e}", rules_dir.display()), 1))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.ends_with(".dart").then_some(name)
        })
        .collect();
    names.sort();
    for name in &names {
        rule_files.push((name.clone(), read(&rules_dir.join(name))?));
    }
    let facts = scan_rule_files(&rule_files);
    let core = lints_set_names(&read(&lints_src.join("lib/core.yaml"))?).map_err(|e| (e, 1))?;
    let recommended =
        lints_set_names(&read(&lints_src.join("lib/recommended.yaml"))?).map_err(|e| (e, 1))?;
    let rows = fold(&entries, &facts, &versions, &core, &recommended).map_err(|e| (e, 1))?;
    if verbose {
        print_tally(&rows, &entries);
    }
    let document = render_rule_meta(&rows, entries.len(), &version);
    let docs = render_lint_docs_rows(&rows);

    for (path, document) in [(&out, document.as_str()), (&docs_out, docs.as_str())] {
        if check {
            let tracked = read(path).map_err(|(message, _)| (format!("--check {message}"), 1))?;
            if tracked != document {
                return Err((
                    format!(
                        "{}: drift from the pinned upstream sources; regenerate with: \
                         cargo run -p upstream-gen --bin linter-meta-gen -- --sdk-src \
                         <dart-sdk-tree> --lints-src <lints-tree>",
                        path.display()
                    ),
                    1,
                ));
            }
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| (format!("creating {}: {e}", parent.display()), 1))?;
            }
            fs::write(path, document)
                .map_err(|e| (format!("writing {}: {e}", path.display()), 1))?;
        }
    }
    if check {
        println!(
            "linter-meta-gen: ok — {} rows match {} and {}",
            rows.len(),
            out.display(),
            docs_out.display()
        );
    } else {
        println!(
            "linter-meta-gen: wrote {} rows ({} registry consts) to {} and {}",
            rows.len(),
            entries.len(),
            out.display(),
            docs_out.display()
        );
    }
    Ok(())
}

/// The SDK identity stamp for the table header, from the checkout's
/// `tools/VERSION` — the same stamp `analyzer-codes-gen` writes.
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
    let mut fields = std::collections::BTreeMap::new();
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

/// The `--verbose` face: the fold tally by group and state.
fn print_tally(rows: &[RuleRow], entries: &[linter_meta::ConstEntry]) {
    let mut groups = std::collections::BTreeMap::new();
    let mut states = std::collections::BTreeMap::new();
    let mut multi = 0;
    for row in rows {
        *groups.entry(row.group).or_insert(0u32) += 1;
        *states.entry(row.state).or_insert(0u32) += 1;
    }
    let mut per_name = std::collections::BTreeMap::new();
    for entry in entries {
        *per_name.entry(entry.name.clone()).or_insert(0u32) += 1;
    }
    for (_, count) in &per_name {
        if *count > 1 {
            multi += 1;
        }
    }
    println!("rows: {}", rows.len());
    println!("consts: {} ({} multi-variant families)", entries.len(), multi);
    println!("by group: {groups:?}");
    println!("by state: {states:?}");
}
