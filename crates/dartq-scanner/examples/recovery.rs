//! Recovery-option benchmark for [`dartq_scanner::scan`]: the cost of the
//! dual-option recovery comparison (lexer `drop_wins`, official
//! `_recoveryOptionScanner`) on an adversarial corpus versus a low-recovery
//! control of equal size.
//!
//! The adversarial corpus is synthesized so that a large fraction of its
//! closers are *deep matches* — the matching opener sits below other unclosed
//! openers on the grouping stack — which is exactly the arm that runs both
//! recovery postures (insert/drop) to EOF on snapshot state and picks the
//! strictly cheaper one. Every benchmark line stays self-contained and drains
//! its stack, so the open-group stack never exceeds 5 entries: the bench
//! measures the option runs, not stack cloning (an unbounded stack would turn
//! each snapshot into an O(depth) memcpy and the whole bench quadratic). The
//! option runs themselves remain bounded by the official 100-step cap. The
//! control corpus has the same nesting shape with every closer head-matching,
//! so it pays none of that path.
//!
//! Min-of-K wall-clock timing, same idiom as `examples/throughput.rs`: both
//! corpora are scanned K times, the fastest round per corpus determines the
//! reported number, no I/O inside timing. The adversarial corpus should
//! report a large diagnostic count and the control exactly zero — that is the
//! evidence the recovery path was actually exercised (per-scan recording is
//! capped at `MAX_DIAGNOSTICS`, so the count saturates there).
//!
//! Run (from the repository root):
//!
//! ```text
//! cargo run -p dartq-scanner --example recovery --release
//! cargo run -p dartq-scanner --example recovery --release -- 4
//! ```
//!
//! The optional positional argument is the total synthesized size in MB
//! (default 1), split evenly between the two corpora and then equalized to
//! the byte at a line boundary. The numbers to record per run: scans/s for
//! both corpora and the `ratio:` line (control scans/s divided by adversarial
//! scans/s; > 1 is the recovery-path cost factor).

use std::fmt::Write as _;
use std::path::Path;
use std::process::exit;
use std::time::{Duration, Instant};

use dartq_scanner::{ScanConfig, scan};
use dartq_source::FileTable;

/// Number of timed rounds; the fastest round determines throughput.
const ROUNDS: usize = 5;

/// One benchmark corpus, synthesized once before any timing starts.
struct Corpus {
    name: &'static str,
    source: String,
    bytes: u64,
    file_id: dartq_source::FileId,
    /// Shortest full-corpus scan time seen across all rounds.
    min_time: Duration,
    /// Diagnostics observed across all rounds (adversarial saturates the
    /// per-scan cap; control must stay at zero).
    diags: usize,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let target_mb = match (args.next(), args.next()) {
        (None, _) => 1u64,
        (Some(arg), None) => match arg.parse::<u64>() {
            Ok(mb @ 1..) => mb,
            _ => {
                eprintln!("error: expected a positive integer (total target MB)");
                usage();
                exit(2);
            }
        },
        (Some(_), Some(_)) => {
            eprintln!("error: expected at most one positional argument (total target MB)");
            usage();
            exit(2);
        }
    };

    // Latest-language defaults; the benchmark measures the scanner, not the gates.
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut table = FileTable::new();

    // Both corpora are built to half the target, then truncated to a common
    // line boundary so the comparison is byte-for-byte equal in size. Every
    // line is self-contained, so the cut never leaves an open group.
    let target = target_mb * 1_000_000 / 2;
    let mut adversarial = String::new();
    let mut control = String::new();
    let mut i = 0usize;
    while (adversarial.len() as u64) < target {
        adversarial_unit(i, &mut adversarial);
        control_unit(i, &mut control);
        i += 1;
    }
    truncate_to_line_boundary(&mut adversarial);
    truncate_to_line_boundary(&mut control);
    let common = adversarial.len().min(control.len());
    adversarial.truncate(common);
    control.truncate(common);

    let mut corpora = [
        build_corpus(
            "adversarial",
            adversarial,
            table.intern(Path::new("bench/recovery-adversarial.dart")),
        ),
        build_corpus(
            "control",
            control,
            table.intern(Path::new("bench/recovery-control.dart")),
        ),
    ];
    let bytes = corpora[0].bytes;

    println!("dartq-scanner recovery-option benchmark (min-of-K, K = {ROUNDS})");
    println!("corpus:      synthesized ({bytes} B per corpus, target {target_mb} MB total)");
    println!("convention:  1 MB/s = 10^6 bytes/s");
    println!("per-corpus (fastest of {ROUNDS} rounds):");

    for _ in 0..ROUNDS {
        for c in &mut corpora {
            let start = Instant::now();
            let result = scan(&c.source, config, c.file_id);
            let elapsed = start.elapsed();
            c.diags += result.diagnostics.len();
            if elapsed < c.min_time {
                c.min_time = elapsed;
            }
        }
    }

    for c in &corpora {
        let secs = c.min_time.as_secs_f64();
        println!(
            "  {:<12} {:>9} B  {:>9.1} scans/s  {:>7.1} MB/s  ({:.3} ms)  {} diags",
            c.name,
            c.bytes,
            scans_per_s(secs),
            mb_per_s(c.bytes, secs),
            secs * 1e3,
            c.diags,
        );
    }

    let [adversarial, control] = &corpora;
    let ratio = control.min_time.as_secs_f64() / adversarial.min_time.as_secs_f64();
    println!(
        "ratio:       {ratio:.2}x (control scans/s / adversarial scans/s; > 1 = recovery-option cost)"
    );
}

fn usage() {
    eprintln!("usage: cargo run -p dartq-scanner --example recovery --release -- [TOTAL_MB]");
}

/// Scans per second for one full-corpus scan taking `secs`.
fn scans_per_s(secs: f64) -> f64 {
    1.0 / secs
}

/// Throughput in MB/s, using the 10^6-byte convention.
#[allow(clippy::cast_precision_loss)]
fn mb_per_s(bytes: u64, secs: f64) -> f64 {
    bytes as f64 / 1_000_000.0 / secs
}

/// Drops the trailing partial line so both corpora cut at a line boundary.
fn truncate_to_line_boundary(s: &mut String) {
    if let Some(i) = s.rfind('\n') {
        s.truncate(i + 1);
    }
}

/// Assembles one corpus (synthesis is untimed).
fn build_corpus(name: &'static str, source: String, file_id: dartq_source::FileId) -> Corpus {
    Corpus {
        name,
        bytes: source.len() as u64,
        source,
        file_id,
        min_time: Duration::MAX,
        diags: 0,
    }
}

/// One adversarial unit: two bracket-mismatch shapes whose closers deep-match
/// an opener buried under other unclosed opens — the arm that runs the
/// dual-option comparison (`drop_wins`) — while draining the stack in both
/// postures:
///
/// - `var v{i}={[0};};]}`: the first `}` matches the `{` under the open `[`
///   (1 event). Insert wins empties the stack; drop wins leaves `{`+`[`, and
///   then the second `}` deep-matches again (2 events) before `]}` head-drains.
/// - `void d{i}(){{{{[0];}}}}]}}}}`: four `{` under an open `[`; each `}`
///   deep-matches the nearest `{` (1 event if insert wins, 4 if drop wins)
///   and the `]}}}}` tail head-drains the drop-winner's stack.
///
/// Worst-case stack depth is 5 (`{`x4 + `[`), so each of the two
/// `option_snapshot` clones per event is O(1).
fn adversarial_unit(i: usize, out: &mut String) {
    let _ = writeln!(out, "var v{i}={{[0}};}};]}}");
    let _ = writeln!(out, "void d{i}(){{{{{{{{[0];}}}}}}}}]}}}}}}}}");
}

/// Control unit: the same nesting shape with every closer head-matching —
/// identical bracket traffic, zero deep matches, zero diagnostics.
fn control_unit(i: usize, out: &mut String) {
    let _ = writeln!(out, "var v{i}={{[0]}};");
    let _ = writeln!(out, "void d{i}(){{{{{{{{[0]}}}}}}}};");
}
