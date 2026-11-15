//! Memory-budget runtime pins: the ladder sheds capacity, never
//! truth — a bottom-tier-exhausted run still reports the complete diagnostic
//! set, and an unarmed engine's output face carries no budget rows.

#![allow(
    clippy::useless_conversion,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::semicolon_if_nothing_returned
)]
mod common;

use common::{Fixture, GREETER_0, diagnostic_summary};
use dartq_diagnostics::Severity;
use dartq_engine::CancellationToken;
use dartq_engine::EngineEvent;

/// `x }` inside a block is a missing-`;` statement terminator — a parse
/// error under every feature set (no gate exists for it), so this file
/// surfaces `EXPECTED_TOKEN` unconditionally: the run's truth marker,
/// independent of the semantic seam the ladder never touches. The same
/// shape is oracle-verified byte-for-byte by the CLI's
/// `analyze_runs_engine_over_temp_workspace`.
const MAIN_BROKEN: &str = "void f() { x }\n";

/// The `dartq/memory_degraded` code.
const MEMORY_DEGRADED: &str = "dartq/memory_degraded";

/// The tier names in ladder order, as the emitted messages carry them.
const TIERS: [&str; 3] = [
    "lazy-parse results evicted (LRU)",
    "non-active context semantic slices swapped out (L2)",
    "AST cold regions swapped out",
];

#[test]
fn bottom_tier_exhausted_run_stays_complete() {
    let fixture = Fixture::new(&[
        ("lib/main.dart", MAIN_BROKEN),
        ("lib/greeter.dart", "String greet() => 'hi';\n"),
    ]);

    // The unarmed baseline: the parse errors and nothing else — no budget
    // rows ever ride an unlimited run.
    let baseline = {
        let mut engine = fixture.engine();
        engine.discover_workspace().expect("discovery");
        engine.set_content(&fixture.path("lib/main.dart"), MAIN_BROKEN.as_bytes());
        engine.set_content(&fixture.path("lib/greeter.dart"), GREETER_0.as_bytes());
        let out = engine
            .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::default())
            .expect("baseline analyze");
        assert!(!out.is_empty(), "the parse errors exist unarmed");
        assert!(
            out.iter().all(|d| d.code.name != MEMORY_DEGRADED),
            "unlimited runs never degrade: {out:?}"
        );
        diagnostic_summary(&engine, &out)
    };

    // Armed at 1 MiB: real test-process RSS is far over, so every boundary
    // poll descends — the phase boundary plus two library boundaries walk
    // the full ladder inside this single run.
    let mut engine = fixture.engine();
    engine.set_memory_limit(Some(1));
    engine.discover_workspace().expect("discovery");
    engine.set_content(&fixture.path("lib/main.dart"), MAIN_BROKEN.as_bytes());
    engine.set_content(&fixture.path("lib/greeter.dart"), GREETER_0.as_bytes());
    let out = engine
        .analyze(&[], &mut |_: EngineEvent| {}, &CancellationToken::default())
        .expect("budgeted analyze");

    let degraded: Vec<_> = out
        .iter()
        .filter(|d| d.code.name == MEMORY_DEGRADED)
        .collect();
    assert_eq!(
        degraded.len(),
        TIERS.len(),
        "one row per memory tier: {degraded:?}"
    );
    for tier in TIERS {
        assert!(
            degraded.iter().any(|d| d.message.contains(tier)),
            "missing tier `{tier}`: {degraded:?}"
        );
    }
    assert!(
        degraded.iter().all(|d| d.severity == Severity::Info),
        "a capacity note never fails the run: {degraded:?}"
    );

    // Completeness at the bottom tier: strip the budget rows and what
    // remains is exactly the unarmed run's set.
    let remaining: Vec<_> = out
        .iter()
        .filter(|d| d.code.name != MEMORY_DEGRADED)
        .cloned()
        .collect();
    assert_eq!(
        diagnostic_summary(&engine, &remaining),
        baseline,
        "the ladder sheds capacity, never truth"
    );
}
