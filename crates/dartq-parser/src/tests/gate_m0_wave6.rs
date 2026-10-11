//! M0-gate wave-6 regression pins (#606): the spill run of a primary
//! bracket error drains silently instead of minting one `Unexpected text`
//! row per stranded token (the `bracket_mismatch_*.dart` m0-syntax fixture
//! class, where the oracle emits zero `unexpectedToken` rows). The official
//! decision order (`parser_impl.dart`, Dart 3.15 main):
//!
//! * The scanner reports an unmatched begin at the first mismatching token
//!   and parks a synthetic closer there (scanner `discardBeginGroupUntil` /
//!   `insertSyntheticClosers`); the real closers behind the mismatch are
//!   the spill the official enclosing group drains consume.
//! * `ensureCloseParen` (:5028-5050): a missing `)` is reported once
//!   (`expectedButGot`, surfaced as `EXPECTED_TOKEN` per the 3.13.1 oracle)
//!   and the pointer jumps to the opener's scanner-guaranteed closer —
//!   `moveSynthetic` silently when that closer is the scanner's synthetic
//!   (:5034-5037, "the scanner has already reported"). The junk region
//!   between is consumed by the jump, never row-per-token.
//! * The collection drains share the shape: the list literal jumps to the
//!   opener's endGroup for a non-entry follower (:8569-8575), the argument
//!   list routes through `ensureCloseParen` (:9588-9600), and both re-enter
//!   parsing instead when the follower looks like the next construct
//!   (`looksLikeLiteralEntry` :8563 / `looksLikeExpressionStart` :9588).
//! * A stranded token that official's chain DOES hand to the expression
//!   engine reports `expectedIdentifier` there and nothing else beyond the
//!   single jump row (`ExpressionIdentifierContext.ensureIdentifier`,
//!   `identifier_context_impl.dart`:427-458 — excluded punctuation reports
//!   and inserts without consuming).
//! * The block statement loop's no-progress guard (:6618-6630) therefore
//!   never fires on this class in the official chain; our chain leaves the
//!   mismatch's spill to the statement loop, which drains it at the loop
//!   head — before a stranded token can enter `parse_statement` and mint
//!   the expression/semicolon rows the official jumps never emit.

use super::collector::{Collector, latest_features};
use crate::Parser;
use dartq_scanner::{ScanConfig, scan};

/// Scans `source` keeping scanner diagnostics visible (the bracket-mismatch
/// class carries the scanner's unmatched-begin row) and parses the table.
/// Returns the collector and the scanner diagnostics.
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
        let mut p = Parser::new(source, table, latest_features(), listener);
        p.parse_unit();
        listener = p.listener;
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

/// The `bracket_mismatch_00.dart` shape: the `(` opens inside a list
/// literal and the `]` mismatches it, stranding `)` and `]` behind the
/// primary error. The scanner reports the unmatched `(` once at the `]`
/// and the parser adopts its synthetic closer silently; the stranded `)`
/// `]` never reach `parse_statement` (drained at the loop head), so the
/// only parser row is the `ensureSemicolon` report anchored on the last
/// real token before the synthetic `;` (offsets: `]`@67, `)`@68). Official
/// — whose chain leaves the `]` unpaired so the paren's inner expression
/// sees it — mints the pair at the same junk head (`missing_identifier`
/// via `ensureIdentifier`, `identifier_context_impl.dart`:427-458, plus
/// the `ensureCloseParen` `expectedButGot` :5043-5046); our scanner parks
/// the synthetic closer inside the paren instead, making it an empty
/// record, so the inner-expression row does not arise. Pre-fix this face
/// minted `unexpected_token` at both stranded closers plus a stranded
/// `missing_identifier` and a duplicated `expected_token`.
#[test]
fn bracket_mismatch_spill_drains_without_per_token_rows() {
    let source = "void good() {\n  [\n    ([\n    ])\n  ];\n}\n\nvoid bad() {\n  [\n    (\n    ])\n  ];\n}\n";
    let (listener, scan_diags) = parse_with_scan_diags(source);
    let got: Vec<(String, u32, u32)> = listener.errors.clone();
    assert_eq!(
        got,
        vec![("expected_token".to_string(), 67, 68)],
        "bracket mismatch spill drains: parser stream diverged"
    );
    assert_eq!(
        listener.texts,
        ["Expected to find ';'."],
        "message templates diverged"
    );
    assert_eq!(
        scan_diags.len(),
        1,
        "scanner reports the unmatched `(` once"
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
/// The argument list adopts the scanner's parked `)` at the `]` through
/// its direct closer consumption (`parseArgumentsRest` :9534-9537 head /
/// :9580-9587 tail — no `ensureCloseParen`), so the loop head locates the
/// mark by the backward scan over the consumed block prefix.
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
/// `)` is consumed by the drain, the scanner's unclosed-`}` row stands, and
/// the parse terminates at EOF with the single semicolon row.
#[test]
fn spill_run_drains_to_eof() {
    let source = "void f() {\n  [\n    (\n    ])";
    let (listener, scan_diags) = parse_with_scan_diags(source);
    let got: Vec<(String, u32, u32)> = listener.errors.clone();
    assert_eq!(
        got,
        vec![("expected_token".to_string(), 25, 26)],
        "spill run at EOF: parser stream diverged"
    );
    assert_eq!(listener.texts, ["Expected to find ';'."]);
    assert_eq!(scan_diags.len(), 2, "unmatched `(` and `{{` rows");
}

// —— boundary: a construct opener re-enters parsing ———————————————————————

/// A statement head after the spill run re-enters parsing (official
/// `looksLikeExpressionStart` fork :9588-9600 re-enters instead of
/// draining): `x = 1;` after the stranded `)` parses as a normal
/// expression statement — two structural statement events (the list
/// statement and the assignment), and the drain stops at the `x`.
#[test]
fn spill_run_stops_at_construct_opener() {
    let listener = assert_stream(
        "void f() {\n  [\n    (\n    ])\n  x = 1;\n}\n",
        &[("expected_token", 25, 26)],
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
            "Expected an identifier, but got ','.",
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
