//! Throughput benchmark for [`dartq_pub::glob::Glob::matches_path`], the
//! analysis-options exclude face the engine calls per discovered file per
//! exclude alternative.
//!
//! Measures min-of-K timing over a representative exclude-pattern set and
//! path corpus (mixing hits and misses, ASCII and multi-byte paths), plus
//! the adversarial multi-star case separately. The matcher performs zero
//! heap allocation per call (asserted by the
//! `matching_hot_path_allocates_nothing` test); these numbers quantify the
//! absolute per-call cost. The replaced implementation allocated at least
//! one `Vec<char>` per call plus a fresh `Vec<char>` per literal at every
//! recursion position, so per-call cost is dominated by allocation removal.
//!
//! Run (from the repository root):
//!
//! ```text
//! cargo run -p dartq-pub --example glob_throughput --release
//! ```

use std::time::{Duration, Instant};

use dartq_pub::glob::Glob;

/// Number of timed rounds; the fastest determines the reported number.
const ROUNDS: usize = 5;

/// Number of full pattern-set x path-corpus sweeps per timed round.
const SWEEPS: usize = 200;

/// Exclude patterns shaped like real `analysis_options` entries (at most
/// two stars — the common shapes).
const PATTERNS: &[&str] = &[
    "*.g.dart",
    "*.freezed.dart",
    "**/*.g.dart",
    "**/build/",
    "build/",
    "packages/*/lib/**",
    "?.dart",
    "[tf]ail*",
];

/// Relative POSIX paths as the analyzer produces them during discovery.
const PATHS: &[&str] = &[
    "lib/main.dart",
    "lib/src/deep/nested/module.dart",
    "a/b/c/deep/x.g.dart",
    "x.g.dart",
    "build/web/main.js",
    "sub/build/out.freezed.dart",
    "out.freezed.dart",
    "packages/foo/lib/foo.dart",
    "test/glob_test.dart",
    "tool/generate.dart",
    "é/漢字/naïve.dart",
];

/// Average nanoseconds per call across one timed round.
#[allow(clippy::cast_precision_loss)]
fn nanos_per_call(round: Duration, calls: usize) -> f64 {
    round.as_secs_f64() * 1e9 / calls as f64
}

fn main() {
    let globs: Vec<Glob> = PATTERNS
        .iter()
        .map(|p| Glob::new(p).expect("parse"))
        .collect();
    let calls = globs.len() * PATHS.len();

    // Sanity: the corpus must exercise both outcomes.
    let hits: usize = globs
        .iter()
        .map(|glob| PATHS.iter().filter(|path| glob.matches_path(path)).count())
        .sum();
    assert!(hits > 0 && hits < calls, "corpus must mix hits and misses");

    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..SWEEPS {
            for glob in &globs {
                for path in PATHS {
                    let _ = glob.matches_path(path);
                }
            }
        }
        best = best.min(start.elapsed());
    }
    let per_call = nanos_per_call(best, SWEEPS * calls);
    println!("exclude matching: {calls} calls/sweep x {SWEEPS} sweeps");
    println!("  best round: {best:?}");
    println!("  per call:   {per_call:.1}ns");
    println!("  throughput: {:.1}M matches/s", 1e3 / per_call);

    // Adversarial multi-star case, memoized to a polynomial bound.
    let adversarial = Glob::new("*a*a*a*a*a*a*a*a*b").expect("parse");
    let path = "a".repeat(400);
    let start = Instant::now();
    let hit = adversarial.matches_path(&path);
    let elapsed = start.elapsed();
    println!("adversarial 9-star x 400-char path (miss): {elapsed:?} -> {hit}");
}
