//! `conformance` — the conformance corpus tool.
//!
//! - `extract`: derives the diagnostics case corpus from a pinned Dart SDK
//!   source tree (doc examples from messages.yaml + analyzer package test
//!   files), writing `.case` files and `cases.manifest`.
//! - `mocks`: extracts the official mock packages into `conformance/mocks/`.
//!
//! All inputs come from arguments/environment; no tracked file references a
//! machine-local path.

mod case;
mod dart;
mod docs;
mod manifest;
mod markers;
mod messages_yaml;
mod mocks;
mod setup;
mod structure;
mod tests_extract;

use clap::{Parser, Subcommand};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "conformance", about = "Conformance corpus tool for the dartq analyzer")]
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
        /// Output directory for the `.case` tree.
        #[arg(long, default_value = "target/conformance/cases")]
        out: PathBuf,
        /// The manifest file to write (or compare with `--check`).
        #[arg(long, default_value = "conformance/cases.manifest")]
        manifest: PathBuf,
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
        #[arg(long, default_value = "conformance/mocks")]
        out: PathBuf,
        /// Compare instead of write.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Extract { sdk_src, out, manifest, check } => {
            extract_command(&sdk_src, &out, &manifest, check)
        }
        Command::Mocks { sdk_src, out, check } => mocks_command(&sdk_src, &out, check),
    };
    std::process::exit(code);
}

const ANALYZER_PKG: &str = "pkg/analyzer";
const TEST_ROOT_REL: &str = "pkg/analyzer/test";

fn extract_command(sdk_src: &Path, out: &Path, manifest_path: &Path, check: bool) -> i32 {
    let started = std::time::Instant::now();
    let analyzer_messages = sdk_src.join(format!("{ANALYZER_PKG}/messages.yaml"));
    let shared_messages = sdk_src.join("pkg/_fe_analyzer_shared/messages.yaml");
    let test_root = sdk_src.join(TEST_ROOT_REL);
    for (path, label) in [
        (&analyzer_messages, "analyzer messages.yaml"),
        (&shared_messages, "shared messages.yaml"),
        (&test_root, "analyzer test root"),
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
        stats.test_cases += extraction.cases.len();
        stats.test_files_scanned += 1;
        for skip in &extraction.skips {
            let reason = if skip.reason.starts_with("setup_") {
                "unmodeled_setup".to_string()
            } else {
                skip.reason.clone()
            };
            *stats.test_skips.entry(reason).or_insert(0) += 1;
        }
        for c in &extraction.cases {
            write_case(out, c, &mut rows, check, &mut stats.case_writes_skipped);
        }
    }

    // 3. Manifest.
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
        if !missing.is_empty() || !unexpected.is_empty() || !changed.is_empty() || content_mismatch > 0 {
            eprintln!("check FAILED: {} missing, {} unexpected, {} changed, {} content-mismatch",
                missing.len(), unexpected.len(), changed.len(), content_mismatch);
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
    }

    stats.print(started);
    0
}

fn case_path(out: &Path, id: &str) -> PathBuf {
    let mut path = out.join(id.replace('/', std::path::MAIN_SEPARATOR_STR));
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
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_test_files(&path, out);
        } else if name.ends_with("_test.dart") {
            out.push(
                path.strip_prefix(dir).expect("under dir").to_string_lossy().into_owned(),
            );
        }
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
    case_writes_skipped: usize,
}

impl Stats {
    fn print(&self, started: std::time::Instant) {
        let mut out = std::io::stderr().lock();
        let _ = writeln!(out, "extract: {} doc cases, {} analyzer-test cases ({} files scanned), {} total",
            self.doc_cases, self.test_cases, self.test_files_scanned, self.doc_cases + self.test_cases);
        let _ = writeln!(out, "doc skips: {}", fmt_counts(&self.doc_skips));
        if self.doc_no_example > 0 {
            let _ = writeln!(out, "doc codes without examples: {}", self.doc_no_example);
        }
        if self.doc_multi_range > 0 {
            let _ = writeln!(out, "doc examples with more than one range: {}", self.doc_multi_range);
        }
        let _ = writeln!(out, "test skips: {}", fmt_counts(&self.test_skips));
        let _ = writeln!(out, "done in {:.2}s", started.elapsed().as_secs_f64());
    }
}

fn fmt_counts(counts: &std::collections::BTreeMap<String, usize>) -> String {
    counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")
}

fn mocks_command(sdk_src: &Path, out: &Path, check: bool) -> i32 {
    let mock_root = sdk_src.join("pkg/analyzer_testing/lib/src/mock_packages");
    let context = sdk_src.join("pkg/analyzer/test/src/dart/resolution/context_collection_resolution.dart");
    for path in [&mock_root, &context] {
        if !path.exists() {
            eprintln!("error: missing {}", path.display());
            return 2;
        }
    }
    let packages = match mocks::extract(&mock_root, &std::fs::read_to_string(&context).expect("reads context")) {
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
            } else if let Err(e) =
                std::fs::create_dir_all(path.parent().expect("has parent")).and_then(|_| std::fs::write(&path, &file.content))
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
    eprintln!("mocks: {} packages, {} files{}", packages.len(), total_files,
        if check { " (checked)" } else { " written" });
    0
}

fn prune_mocks(out: &Path, packages: &std::collections::BTreeMap<String, Vec<mocks::MockFile>>) {
    let Ok(entries) = std::fs::read_dir(out) else { return };
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
                prune_dir(&entry.path(), files);
            }
        }
    }
}

fn prune_dir(dir: &Path, keep: &[mocks::MockFile]) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_dir(&path, keep);
            let _ = std::fs::remove_dir(&path); // remove when empty
        } else {
            let rel = path.strip_prefix(dir).expect("under dir").to_string_lossy().into_owned();
            if !keep.iter().any(|f| f.path == rel) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}
