//! Throughput benchmark for [`dartq_scanner::scan`] over a Dart corpus.
//!
//! Measures lexical scanning throughput with min-of-K timing: the full corpus
//! is scanned K times and the fastest round determines the reported number,
//! matching the "≈245 MB/s" release-build figure quoted in the README.
//!
//! Run (from the repository root):
//!
//! ```text
//! cargo run -p dartq-scanner --example throughput --release
//! DARTQ_BENCH_CORPUS=<corpus-dir> \
//!   cargo run -p dartq-scanner --example throughput --release
//! ```
//!
//! The corpus must be supplied (a directory scanned recursively for `*.dart`
//! files in sorted order, `.dart_tool` skipped — via DARTQ_BENCH_CORPUS or a
//! single positional argument); there is no vendored default. Files are
//! read once; the K rounds do no I/O. Throughput is reported in MB/s using
//! the 10^6-byte convention.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::{Duration, Instant};

use dartq_scanner::{ScanConfig, scan};
use dartq_source::FileTable;

/// Number of timed rounds; the fastest round determines throughput.
const ROUNDS: usize = 5;

/// One corpus file, read into memory once before any timing starts.
struct CorpusFile {
    /// Path for display (relative to the corpus root when possible).
    path: PathBuf,
    source: String,
    bytes: u64,
    file_id: dartq_source::FileId,
    /// Shortest per-file scan time seen across all rounds.
    min_time: Duration,
}

fn main() {
    let mut args = std::env::args().skip(1);
    // No default corpus: the repo no longer vendors a Dart corpus. Point
    // DARTQ_BENCH_CORPUS at a directory (or pass one positionally).
    let root = match (args.next(), args.next()) {
        (None, _) => match std::env::var("DARTQ_BENCH_CORPUS") {
            Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => {
                eprintln!(
                    "no corpus: set DARTQ_BENCH_CORPUS=<dir> (a directory of .dart files) \
                     or pass one as the single positional argument"
                );
                exit(2);
            }
        },
        (Some(arg), None) => PathBuf::from(arg),
        (Some(_), Some(_)) => {
            eprintln!("error: expected at most one positional argument (dir or file)");
            usage();
            exit(2);
        }
    };
    if !root.exists() {
        eprintln!("error: path not found: {}", root.display());
        usage();
        exit(2);
    }

    let mut files = load_corpus(&root);
    let total_bytes: u64 = files.iter().map(|f| f.bytes).sum();

    // Latest-language defaults; the benchmark measures the scanner, not the gates.
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };

    println!("dartq-scanner throughput benchmark (min-of-K, K = {ROUNDS})");
    println!(
        "corpus:      {} ({} files, {total_bytes} bytes)",
        root.display(),
        files.len()
    );
    println!("convention:  1 MB/s = 10^6 bytes/s");
    println!("per-file (fastest of {ROUNDS} rounds):");

    let mut min_round = Duration::MAX;
    let mut token_total = 0usize;
    for _ in 0..ROUNDS {
        let round_start = Instant::now();
        for f in &mut files {
            let start = Instant::now();
            let result = scan(&f.source, config, f.file_id);
            let elapsed = start.elapsed();
            token_total += result.tokens.len();
            if elapsed < f.min_time {
                f.min_time = elapsed;
            }
        }
        let round = round_start.elapsed();
        if round < min_round {
            min_round = round;
        }
    }

    for f in &files {
        let secs = f.min_time.as_secs_f64();
        println!(
            "  {}  {} B  {:.1} MB/s  ({:.3} ms)",
            f.path.display(),
            f.bytes,
            mb_per_s(f.bytes, secs),
            secs * 1e3
        );
    }

    let secs = min_round.as_secs_f64();
    println!(
        "aggregate:   {:.1} MB/s  ({total_bytes} bytes, {} files, K = {ROUNDS}, fastest round {:.3} ms)",
        mb_per_s(total_bytes, secs),
        files.len(),
        secs * 1e3,
    );
    println!("tokens:      {token_total} total across {ROUNDS} rounds");
}

fn usage() {
    eprintln!("usage: cargo run -p dartq-scanner --example throughput --release -- [DIR_OR_FILE]");
}

/// Throughput in MB/s, using the 10^6-byte convention.
#[expect(clippy::cast_precision_loss)] // display-only benchmark math; precision loss cannot affect the measurement
fn mb_per_s(bytes: u64, secs: f64) -> f64 {
    bytes as f64 / 1_000_000.0 / secs
}

/// Recursively collects and sorts all `*.dart` files under `root`, skipping
/// `.dart_tool` directories, then reads each file once into memory.
fn load_corpus(root: &Path) -> Vec<CorpusFile> {
    let mut paths = Vec::new();
    if root.is_file() {
        paths.push(root.to_path_buf());
    } else {
        collect_dart_files(root, &mut paths).unwrap_or_else(|e| {
            eprintln!("error: cannot walk {}: {e}", root.display());
            exit(2);
        });
        paths.sort();
    }

    let mut table = FileTable::new();
    let mut files = Vec::with_capacity(paths.len());
    let mut skipped = 0usize;
    for path in &paths {
        let file_id = table.intern(path);
        let display = path.strip_prefix(root).unwrap_or(path);
        let Ok(bytes_vec) = fs::read(path) else {
            eprintln!("warning: unreadable, skipping: {}", display.display());
            skipped += 1;
            continue;
        };
        let Ok(source) = String::from_utf8(bytes_vec) else {
            eprintln!("warning: not UTF-8, skipping: {}", display.display());
            skipped += 1;
            continue;
        };
        if source.is_empty() {
            skipped += 1;
            continue;
        }
        files.push(CorpusFile {
            path: display.to_path_buf(),
            bytes: source.len() as u64,
            source,
            file_id,
            min_time: Duration::MAX,
        });
    }
    if skipped > 0 {
        eprintln!("note: skipped {skipped} unreadable/non-UTF-8/empty files");
    }
    files
}

fn collect_dart_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if path.file_name().is_some_and(|name| name == ".dart_tool") {
                continue;
            }
            collect_dart_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "dart") {
            out.push(path);
        }
    }
    Ok(())
}
