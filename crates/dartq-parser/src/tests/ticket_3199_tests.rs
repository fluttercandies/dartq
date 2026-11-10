//! #3199 pins: operator chains fold in a loop, never one native frame per
//! link — the written-out-operator recovery included.
//!
//! The official precedence walk (`_parsePrecedenceExpressionLoop`,
//! `parser_impl.dart:7182`) is a loop that folds every left-associative tier
//! in place: the plain-binary arm recurses into the right operand at
//! `level + 1`, that frame consumes exactly one operand (the next
//! same-or-lower operator is below its floor) and returns, and the SAME loop
//! iteration folds the next link. Native depth stays constant across an
//! unbounded `a + 1 + 1 ...` chain; only right-associative tiers (`=`, `??=`,
//! the `?:` middle) recurse by design.
//!
//! The written-out recovery (`_attemptPrecedenceLevelRecovery`, :7594) keeps
//! that shape upstream by rewriting the identifier into the replacement token
//! and handing it back to the loop. Ours consumes the operator from the
//! attempt and parses the right operand through `parse_precedence`; the
//! attempt used to run ungated at the entry floor, so every nested
//! right-operand frame (floor `replacement + 1`) accepted the NEXT link
//! itself and the chain recursed once per link — the #1071 fixture (50k
//! `xor` links) overflowed the default 8MB test-thread stack before the
//! 500-deep expression fuse could trip (four fat debug-opt frames per link
//! lose that race). Upstream never grows here: its spec reparse runs at the
//! frame's floor and makes no progress on a below-floor rewritten operator
//! (`token == afterExpression` rejects), so the nested frame returns and the
//! frame whose floor admits the operator folds it in its loop.
//!
//! The fix gates the attempt by exactly those two official levels — the
//! frame floor (the no-progress face) at both call sites, and the walk's
//! current level (`newLevel > currentLevel → continue`) on the mid-walk
//! call — so chains now fold link by link in one frame with flat native
//! depth. These pins run on the DEFAULT test-thread stack: no
//! `RUST_MIN_STACK` anywhere in the run.

use super::collector::{Collector, latest_features, scan_ok};
use crate::parser::{MAX_DIAGNOSTICS, Parser};

/// Parses a unit and returns `(collector, bailed)`.
fn parse_unit(source: &'static str) -> (Collector, bool) {
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    let bailed = {
        let mut parser = Parser::new(text, table, latest_features(), &mut listener);
        parser.parse_unit();
        parser.is_bailed()
    };
    (listener, bailed)
}

/// `void f() { var x = a<link>…; }` with `link` repeated `n` times.
fn chain(link: &str, n: usize) -> &'static str {
    let mut source = String::from("void f() {\n  var x = a");
    for _ in 0..n {
        source.push_str(link);
    }
    source.push_str(";\n}\n");
    Box::leak(source.into_boxed_str())
}

fn binary_begins(c: &Collector) -> usize {
    c.log
        .iter()
        .filter(|e| **e == ("begin", "binary_expression"))
        .count()
}

fn binary_ends(c: &Collector) -> usize {
    c.log
        .iter()
        .filter(|e| **e == ("end", "binary_expression"))
        .count()
}

/// The #1071 fixture on the default stack: 50 000 `xor` links. Every link
/// is accepted at the ORIGIN frame's mid-walk attempt and folded there; the
/// per-link `binary_operator_written_out` flood latches the diagnostics
/// cap, which is the bounded terminal, and the depth fuse never trips — the
/// walk's native depth did not move with the chain.
#[test]
fn written_out_chain_50k_folds_flat_on_default_stack() {
    let (c, bailed) = parse_unit(chain(" xor 1", 50_000));
    let codes = c.error_codes();
    assert!(
        !codes.iter().any(|c| c == "stack_overflow"),
        "the depth fuse must never trip: the chain folds in one frame's loop"
    );
    assert!(
        bailed,
        "the per-link report flood latches the diagnostics cap"
    );
    assert_eq!(
        c.errors.len(),
        MAX_DIAGNOSTICS,
        "the cap bounds the flood: {} - 1 written-out rows + the terminal",
        MAX_DIAGNOSTICS
    );
    assert_eq!(
        codes.last().map(String::as_str),
        Some("dartq/internal_error"),
        "the terminal is the diagnostics-cap bailout, not a fuse"
    );
    assert!(
        codes[..codes.len() - 1]
            .iter()
            .all(|c| c == "binary_operator_written_out"),
        "the flood is one written-out row per accepted link"
    );
    // Extent: one begin/end pair per link up to the cap — the walk reached
    // ten thousand links deep into the chain in a single frame.
    assert_eq!(binary_begins(&c), MAX_DIAGNOSTICS);
    assert_eq!(binary_ends(&c), MAX_DIAGNOSTICS);
}

/// A moderate written-out chain well under every cap: all 1000 links are
/// accepted (one report each, in chain order), every link is a complete
/// binary expression, the parse completes without bailing, and no fuse
/// fires. Every link's spec reparse consumes exactly one operand — the
/// chain costs one attempt per link, not one per remaining link.
#[test]
fn written_out_chain_1000_links_folds_completely() {
    let (c, bailed) = parse_unit(chain(" xor 1", 1_000));
    assert!(!bailed, "a 1000-link chain is far under every cap");
    let codes = c.error_codes();
    assert_eq!(codes.len(), 1_000, "exactly one written-out row per link");
    assert!(
        codes.iter().all(|c| c == "binary_operator_written_out"),
        "no other diagnostic: {:?}",
        codes
            .iter()
            .filter(|c| *c != "binary_operator_written_out")
            .collect::<Vec<_>>()
    );
    assert_eq!(binary_begins(&c), 1_000);
    assert_eq!(binary_ends(&c), 1_000);
    // The rows anchor on each `xor` in source order: strictly increasing
    // offsets prove each link was reported exactly once at its own token.
    let starts: Vec<u32> = c.errors.iter().map(|(_, s, _)| *s).collect();
    assert!(
        starts.windows(2).all(|w| w[0] < w[1]),
        "written-out rows anchor on successive operators"
    );
}

/// The plain left-associative tiers fold the same way: 50 000 real `+`
/// links parse to completion with one begin/end pair per link, zero
/// diagnostics and no fuse — the tier law the written-out recovery now
/// shares.
#[test]
fn real_operator_chain_50k_folds_flat_on_default_stack() {
    let (c, bailed) = parse_unit(chain(" + 1", 50_000));
    assert!(!bailed, "a healthy chain never bails");
    assert!(c.errors.is_empty(), "no diagnostics: {:?}", c.errors);
    assert_eq!(binary_begins(&c), 50_000);
    assert_eq!(binary_ends(&c), 50_000);
}

/// Mixed written-out tiers follow the official floor gate: in
/// `a xor 1 and 2 xor 3` the `and` is accepted inside the first `xor`'s
/// right-operand frame (floor 11 admits `&` at 11) while the second `xor`
/// is rejected there (10 < 12) and folds at the origin frame instead —
/// three reports, three complete binaries, no bail.
#[test]
fn mixed_written_out_tiers_fold_without_nesting_per_link() {
    let (c, bailed) = parse_unit("void f() {\n  var x = a xor 1 and 2 xor 3;\n}\n");
    assert!(!bailed);
    let codes = c.error_codes();
    assert_eq!(
        codes, ["binary_operator_written_out"; 3],
        "one row per written-out operator: {codes:?}"
    );
    assert_eq!(binary_begins(&c), 3);
    assert_eq!(binary_ends(&c), 3);
}
