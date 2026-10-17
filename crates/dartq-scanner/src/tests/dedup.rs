//! Dedup set-semantics pins (finish pass, `diag.rs::finish`).
//!
//! The oracle's recorded row set is a `Set<Diagnostic>` — `Diagnostic ==`
//! compares code, offset, length, message, so a row surfaces once no matter
//! how many error tokens translate into it or where they land in emission
//! order. The finish pass implements this with an ADJACENT dedup after the
//! stable sort by byte start, which matches the oracle if and only if
//! identical rows are always emitted consecutively by the lexer (the
//! structural invariant below); the pins here hold that from the observable
//! side: if any input ever makes identical rows non-adjacent, the adjacent
//! dedup leaks the surplus row and the duplicate-check fails.

use super::scan_src;

/// The emission invariant that makes adjacent dedup equal set semantics
/// (verified by a full read of every `Lexer::diag` / `diag_pin_last_unit`
/// call site; this test's battery exercises each multi-emission shape):
///
/// 1. Identical rows can only come from a single scanner EVENT emitting
///    several rows — there is no re-visit path: `self.pos` is monotonic
///    within the one real pass (recovery-option runs discard their
///    diagnostics, curly recovery re-scans in a fresh lexer whose result
///    replaces the first pass wholesale), so each byte position is claimed
///    by at most one event.
/// 2. The multi-row events — the `unexpected_separator_in_number` run inside
///    one number and the EOF unterminated-string cascade (`while suspended
///    pop → unterminated`) — emit nothing but identical rows between their
///    iterations.
/// 3. Every other site emits at most one row per event at its position, and
///    same-code pairs at one position are exclusive branches of one event
///    (the astral illegal character emits two pins at DIFFERENT stops with
///    different messages; an escape reports at most one error per segment;
///    a missing-brace `\u{` row is followed only by a different-code
///    code-point row at the same backslash).
///
/// Therefore identical rows are always consecutive pre-sort, share their
/// byte span (hence their sort key), and stay adjacent through the stable
/// sort — adjacent dedup removes all of them, exactly like the oracle set.
#[test]
fn dedup_matches_oracle_set_semantics_across_multi_emission_shapes() {
    // Each input exercises at least one multi-emission or same-start-group
    // shape: nested unterminated-interpolation EOF cascades, a body-local
    // EOF break adjacent to the cascade run, an unterminated-comment row
    // parked at the same eof offset before the cascade (the audit's S1
    // interleave candidate), separator-diag runs, `$`-identifier failures
    // sharing the newline/eof start with an unterminated pin, escape
    // errors, astral illegal characters, and unsupported operators.
    let inputs: &[&str] = &[
        // EOF cascades (one identical row per suspended body).
        "f('${f('${')\n",
        "f('${f('${f('${')",
        "'''${'''${'''",
        "'${",
        "f('${\"b${\"c",
        // Body-local EOF break + cascade over the same eof pin.
        "f('${'a",
        "f('${'''a",
        // Pure cascade collapse: both suspended bodies pin the identical row
        // at eof - 1 (the reconciled `nested_unterminated_diagnostic_order`
        // face — one row, not two).
        "'a${\"b${c",
        // Unterminated comment at eof BEFORE the cascade (S1 candidate):
        // would expose A,B,A if the cascade ever interleaved with it.
        "f('${/*",
        "f('${f('${/*",
        "'''${'''${/*",
        "/*",
        "/* /* /*",
        // Separator-diag runs inside one number (identical rows, number start).
        "1_e_2_e",
        "0x_1_2",
        "1__2__3",
        "1.1_e+_",
        "0x_",
        "1_",
        "1e_e",
        // missing_identifier sharing a start with an unterminated pin.
        "\"x$\ny\"",
        "f('$",
        "f('${'$",
        "'$$",
        // Escape errors (at most one per segment, backslash start).
        "'\\x1'",
        "'\\x",
        "'\\u",
        "'\\u{",
        "'\\u{ffffffff}'",
        "'\\u{1fffff}'",
        "'\\u{}'",
        "'''a\\",
        // Illegal characters: astral (two pins, distinct stops), controls,
        // non-ASCII whitespace; adjacent to other rows at eof.
        "var x = 1;\n😀",
        "😀\u{0}\u{1}",
        "\"😀",
        "\u{00A0}\u{3000}",
        // Unsupported operators (one row per token start).
        "x === y !== z",
        "a===b===c",
        // Kitchen sink: several shapes in one file.
        "f('${/*1_e_2_😀",
    ];
    for src in inputs {
        let r = scan_src(src);
        let mut seen = std::collections::HashSet::new();
        for d in &r.diagnostics {
            let key = (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str(),
            );
            assert!(
                seen.insert(key),
                "duplicate row survived dedup (oracle set semantics broken) for {src:?}: \
                 {} @{}+{}",
                d.code.name,
                d.range.offset,
                d.range.length
            );
        }
    }
}
