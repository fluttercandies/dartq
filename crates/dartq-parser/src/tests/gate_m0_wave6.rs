//! M0-gate wave-6 regression pins (#606): the run of stranded closers behind
//! a primary bracket error drains silently instead of minting one
//! `Unexpected text` row per token (the `bracket_mismatch_*.dart` m0-syntax
//! fixture class, where the oracle emits zero `unexpectedToken` rows). The
//! official decision order (`parser_impl.dart`, Dart 3.15 main):
//!
//! * Under the official dual-option recovery (`abstract_scanner.dart`
//!   `discardBeginGroupUntil` :787-826) the mismatching `]` of these
//!   fixtures DROPS: both postures run to EOF on cloned state and dropping
//!   is strictly cheaper (insert pays a synthetic closer plus no-match
//!   closers; drop pays only its +1 penalty — upstream
//!   `bracket_mismatch_00.dart.expect` reports no scanner problem). The
//!   closer stands alone and every begin pairs with a later real closer.
//! * `ensureCloseParen` (:5028-5050): with no scanner synthetic parked, the
//!   `(`'s inner expression sees the junk `]` — `ensureIdentifier` reports
//!   `missing_identifier` there and leaves it current
//!   (`identifier_context_impl.dart`:427-458), then `ensureCloseParen`
//!   reports `expectedButGot` once (`EXPECTED_TOKEN` per the 3.13.1 oracle)
//!   and jumps to the REAL `)` behind the junk — the junk region between is
//!   consumed by the jump, never row-per-token.
//! * The collection drains share the shape: the list literal jumps to the
//!   opener's endGroup for a non-entry follower (:8569-8575), the argument
//!   list routes through `ensureCloseParen` (:9588-9600), and both re-enter
//!   parsing instead when the follower looks like the next construct
//!   (`looksLikeLiteralEntry` :8563 / `looksLikeExpressionStart` :9588).
//! * The block statement loop's no-progress guard (:6618-6630) therefore
//!   never fires on this class in the official chain; our chain consumes the
//!   junk through the same jumps, so the statement loop drains at the loop
//!   head — before a stranded token can enter `parse_statement` and mint
//!   the expression/semicolon rows the official jumps never emit.

use super::collector::{Collector, latest_features};
use crate::Parser;
use dartq_scanner::{ScanConfig, scan};

/// Scans `source` keeping scanner diagnostics visible and parses the table.
/// Unmatched groups ride the table as (begin, synthetic closer) pairs — the
/// scanner emits no rows for them; the parser reports the pairs post-parse
/// into the collector. Returns the collector and the scanner diagnostics.
fn parse_with_scan_diags(source: &'static str) -> (Collector, Vec<dartq_diagnostics::Diagnostic>) {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("t.dart"));
    let result = scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let diags = result.diagnostics;
    let mut listener = Collector::default();
    {
        let mut p = Parser::new(source, table, latest_features(), &mut listener);
        p.parse_unit();
    }
    (listener, diags)
}

/// Asserts the exact in-order (code, start, end) parser stream.
fn assert_stream(source: &'static str, expected: &[(&str, u32, u32)], label: &str) -> Collector {
    let (listener, _) = parse_with_scan_diags(source);
    let got: Vec<(String, u32, u32)> = listener.errors.clone();
    let want: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| (ToString::to_string(c), *s, *e))
        .collect();
    assert_eq!(got, want, "{label}: parser stream diverged");
    listener
}

// —— the wave fixture (bracket_mismatch_00 / scanner_goldens/076) —————————

/// The `bracket_mismatch_00.dart` shape: the `(` opens inside a list literal
/// and the `]` mismatches it, stranding `)` and `]` behind the primary
/// error. The scanner's dual-option law DROPS the `]` (insert = synthetic
/// `)` + two no-match closers = 3 recoveries; drop = all closers pair, +1 =
/// 1 — strictly cheaper; upstream `bracket_mismatch_00.dart.expect` reports
/// no scanner problem), so zero scanner diagnostics arise and every closer
/// stays a real token. The parser then faces the official pair at the junk
/// head: the `(`'s inner expression reports `missing_identifier` at the `]`
/// via `ensureIdentifier` (offsets: `]`@67), which leaves the `]` current,
/// and `ensureCloseParen` (:5028-5050) reports `expectedButGot` ')' once and
/// jumps to the REAL `)`@68 — no scanner synthetic, no `moveSynthetic` spill
/// mark, no per-token rows. Pre-port (always-insert) this face minted a
/// single `ensureSemicolon` row plus a scanner `expected_token`.
#[test]
fn bracket_mismatch_drop_official_pair() {
    let source = "void good() {\n  [\n    ([\n    ])\n  ];\n}\n\nvoid bad() {\n  [\n    (\n    ])\n  ];\n}\n";
    let (listener, scan_diags) = parse_with_scan_diags(source);
    let got: Vec<(String, u32, u32)> = listener.errors.clone();
    assert_eq!(
        got,
        vec![
            ("missing_identifier".to_string(), 67, 68),
            ("expected_token".to_string(), 67, 68),
        ],
        "bracket mismatch drop face: parser stream diverged"
    );
    assert_eq!(
        listener.texts,
        ["Expected an identifier.", "Expected to find ')'.",],
        "message templates diverged"
    );
    assert!(
        scan_diags.is_empty(),
        "the dual-option law drops: no scanner rows, got {scan_diags:?}"
    );
}

/// The well-formed half of the same fixture class stays fully silent, on
/// both sides (scanner and parser).
#[test]
fn wellformed_bracket_nesting_stays_silent() {
    let source = "void good() {\n  [\n    ([\n    ])\n  ];\n}\n";
    let (listener, scan_diags) = parse_with_scan_diags(source);
    assert!(listener.errors.is_empty(), "{:?}", listener.errors);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
}

/// The `bracket_mismatch_01.dart` class (two constructor forms; the second
/// mismatches `D(d:` against a later `])`): whatever rows the argument and
/// list drains carry, the stranded region behind the primary error mints
/// no `unexpected_token` rows — pre-fix this file produced six of them.
/// Under the dual-option law the second form also drops (upstream
/// `bracket_mismatch_01.dart.expect`: no scanner problem), so the `D(d:`
/// argument list sees the junk `]` after its three elements and
/// `ensureCloseParen` (:9580-9587) reports once and jumps to the real `)`
/// right behind it; the enclosing list and call drains then close on their
/// real closers.
#[test]
fn constructor_form_spill_mints_no_unexpected_rows() {
    let source = "class C {\n  C({c});\n  C m() {\n    return C(\n      c: [\n        C(c: [\n          C(),\n          C(),\n          C(),\n        ]),\n        C(),\n        C(),\n      ],\n    );\n  }\n}\n\nclass D {\n  D({d});\n  D m() {\n    return D(\n      d: [\n        D(d:\n          D(),\n          D(),\n          D(),\n        ]),\n        D(),\n        D(),\n      ],\n    );\n  }\n}\n";
    let (listener, _) = parse_with_scan_diags(source);
    let codes = listener.error_codes();
    assert!(
        !codes.iter().any(|c| c == "unexpected_token"),
        "spill rows leaked into the statement loop: {codes:?}"
    );
}

// —— boundary: the spill run ends at EOF ———————————————————————————————————

/// Truncating the fixture after the stranded closers drains to EOF: the
/// `)` is consumed by the drain, and the two unmatched pairs (`(` against
/// its synthetic parked at the `]`, the function `{` against its eof
/// synthetic) surface post-parse. The in-parse semicolon row anchors past
/// the relocation point — on the token the parse resumed at (the `)` at
/// 26..27, the #2024 recovery-anchor rule; the official `ensureSemicolon`
/// walk-back starts from the construct's last token, which after a
/// relocation is the resume token) — so the pair row keeps the `]` anchor
/// to itself. One `]` pair plus the length-1 eof row for `{`
/// (translateErrorToken's unconditional `length: 1`).
#[test]
fn spill_run_drains_to_eof() {
    let source = "void f() {\n  [\n    (\n    ])";
    let (listener, scan_diags) = parse_with_scan_diags(source);
    let got: Vec<(String, u32, u32)> = listener.errors.clone();
    assert_eq!(
        got,
        vec![
            ("expected_token".to_string(), 26, 27),
            ("expected_token".to_string(), 25, 26),
            ("expected_token".to_string(), 27, 28),
        ],
        "spill run at EOF: parser stream diverged"
    );
    assert_eq!(
        listener.texts,
        [
            "Expected to find ';'.",
            "Expected to find ')'.",
            "Expected to find '}'.",
        ]
    );
    assert!(
        scan_diags.is_empty(),
        "the scanner records the pairs without rows: {scan_diags:?}"
    );
}

// —— boundary: a construct opener re-enters parsing ———————————————————————

/// A statement head after the spill run re-enters parsing (official
/// `looksLikeExpressionStart` fork :9588-9600 re-enters instead of
/// draining): `x = 1;` after the stranded `)` parses as a normal
/// expression statement — two structural statement events (the list
/// statement and the assignment), and the drain stops at the `x`.
#[test]
fn spill_run_stops_at_construct_opener() {
    // The unclosed `(` never meets its synthetic in the parse (the drain
    // consumes the stranded closers), so its pair surfaces at parse end at
    // the parked position; the in-parse semicolon row anchors past the
    // relocation point — on the token the parse resumed at (the `)` at
    // 26..27, the #2024 recovery-anchor rule).
    let listener = assert_stream(
        "void f() {\n  [\n    (\n    ])\n  x = 1;\n}\n",
        &[("expected_token", 26, 27), ("expected_token", 25, 26)],
        "spill run stops at the opener",
    );
    let statements = listener
        .log
        .iter()
        .filter(|e| **e == ("handle", "expression_statement"))
        .count();
    assert_eq!(statements, 2, "the opener must re-enter statement parsing");
}

// —— boundary: junk outside a spill region keeps the stall shape ——————————

/// A lone `,` statement has no primary bracket error behind it, so the
/// official no-progress guard shape stands: `missing_identifier` at the
/// comma (the expression template's excluded-punctuation path,
/// `identifier_context_impl.dart` :436-458), the `ensureSemicolon` row
/// anchored on the block's `{`, and the guard's `unexpected_token`.
#[test]
fn lone_junk_outside_spill_keeps_stall_row() {
    let listener = assert_stream(
        "void f() { , }\n",
        &[
            ("missing_identifier", 11, 12),
            ("expected_token", 9, 10),
            ("unexpected_token", 11, 12),
        ],
        "lone comma keeps the stall row",
    );
    assert_eq!(
        listener.texts,
        [
            "Expected an identifier.",
            "Expected to find ';'.",
            "Unexpected text ','.",
        ],
        "message templates diverged"
    );
}

// —— the #564-era keep-set shapes stay pinned —————————————————————————————

/// `void f(int 5) {}` (the #564 formal-name oracle probe): exactly one row,
/// the parameter list's tail `ensureCloseParen` anchored on the junk `5` —
/// the keep-set substrate recovery.rs pins must not move.
#[test]
fn formal_name_junk_shape_stays_pinned() {
    let listener = assert_stream(
        "void f(int 5) {}",
        &[("expected_token", 11, 12)],
        "#564 formal-name shape",
    );
    assert_eq!(listener.texts, ["Expected to find ')'."]);
}
