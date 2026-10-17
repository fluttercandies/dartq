//! Span derivation: nodes store token-index pairs; byte and UTF-16 offsets
//! derive from the token table at access time. Zero-width
//! synthetic-only nodes derive a zero-length span at their insertion point.
//!
//! Sequencing: these functions require the *unified* table — the parser must
//! first fold its inserted-token side table into the scanner's table via
//! `TokenTable::append_inserted` (Eof re-seated last, dartq-ast `TokenRef`
//! doc). Before that, an inserted id (`table_len + i`) is outside the table;
//! after it, every id resolves. (Distinct from the builder's lazy
//! payload-slot materialization.) A read against an unfolded or stale table
//! is the parser-bug class Family T shares with the cursor: debug builds
//! catch it at `assert_read_boundary`, release builds degrade to a
//! zero-width span pinned at the EOF token — under-report, never a panic
//! (valid parses never read it; oracle face unchanged).

use dartq_scanner::{Token, TokenTable};
use dartq_source::utf16_len_of;

use crate::kind::NodeKind;
use crate::table::SyntaxTable;

fn token_at(tokens: &TokenTable, index: u32) -> Option<&Token> {
    tokens.get(index as usize)
}

/// Read-boundary contract, debug builds: node token ranges
/// stay inside the token table, and only a `CompilationUnit` may span the Eof
/// sentinel — official End extents exclude Eof except the unit root, so a
/// node sealed with `end_token == tokens.len()` would silently derive a span
/// across trailing trivia to source end. A violation is a parser contract
/// break, caught here at the read boundary (release trusts the parser).
#[cfg(debug_assertions)]
fn assert_read_boundary(tokens: &TokenTable, ast: &SyntaxTable, id: u32) {
    let range = ast.token_range(id);
    debug_assert!(
        range.end as usize <= tokens.len(),
        "node {id} token range {range:?} runs past the token table"
    );
    // The end bound alone lets an EMPTY range anchored at `tokens.len()`
    // through — the Eof check below skips empty ranges, and `token_at` would
    // then die on `range.start` with a misleading message. An EOF-synthetic
    // node anchors at `len - 1`.
    debug_assert!(
        (range.start as usize) < tokens.len(),
        "node {id} token range {range:?} starts past the token table"
    );
    if ast.kind(id) != NodeKind::CompilationUnit && !range.is_empty() {
        if let Some(last) = token_at(tokens, range.end - 1) {
            debug_assert!(
                last.kind != TokenKind::Eof,
                "node {id} spans the Eof sentinel (only a CompilationUnit may)"
            );
        }
    }
}

/// The node's byte span `[start, end)` in the source text.
///
/// Exception kept on purpose: a [`NodeKind::CompilationUnit`]
/// spans `[0, eof.end]` regardless of leading trivia, mirroring the official
/// hardcoded `CompilationUnitImpl.offset => 0` (analyzer `ast.dart:9230-9233`)
/// — do not "fix" this by derivation.
#[must_use]
pub fn byte_span(tokens: &TokenTable, ast: &SyntaxTable, id: u32) -> core::ops::Range<usize> {
    #[cfg(debug_assertions)]
    assert_read_boundary(tokens, ast, id);
    let range = ast.token_range(id);
    if ast.kind(id) == NodeKind::CompilationUnit {
        let eof = tokens.last();
        return 0..(eof.byte_offset + eof.byte_len) as usize;
    }
    // Read boundary: an id outside the table is the pre-fold/stale-id
    // parser-bug class — degrade to a zero-width span pinned at the EOF
    // token (under-report) instead of panicking. Debug builds already
    // tripped `assert_read_boundary` above.
    let eof_offset = tokens.last().byte_offset as usize;
    let Some(start_token) = token_at(tokens, range.start) else {
        debug_assert!(false, "node token id outside the token table");
        return eof_offset..eof_offset;
    };
    let start = start_token.byte_offset as usize;
    if range.is_empty() {
        return start..start;
    }
    match token_at(tokens, range.end - 1) {
        Some(last) => start..(last.byte_offset + last.byte_len) as usize,
        None => {
            debug_assert!(false, "node token id outside the token table");
            eof_offset..eof_offset
        }
    }
}

/// The node's span as UTF-16 code-unit offsets (the engine's external
/// position convention). Same derivation and same compilation-unit exception
/// as [`byte_span`].
#[must_use]
pub fn utf16_span(
    source: &str,
    tokens: &TokenTable,
    ast: &SyntaxTable,
    id: u32,
) -> core::ops::Range<u32> {
    #[cfg(debug_assertions)]
    assert_read_boundary(tokens, ast, id);
    let range = ast.token_range(id);
    if ast.kind(id) == NodeKind::CompilationUnit {
        let eof = tokens.last();
        return 0..eof.utf16_offset + utf16_len_of(&source[eof.byte_range()]);
    }
    // Same read boundary and degrade as [`byte_span`].
    let eof_offset = tokens.last().utf16_offset;
    let Some(start_token) = token_at(tokens, range.start) else {
        debug_assert!(false, "node token id outside the token table");
        return eof_offset..eof_offset;
    };
    let start = start_token.utf16_offset;
    if range.is_empty() {
        return start..start;
    }
    match token_at(tokens, range.end - 1) {
        Some(last) => start..last.utf16_offset + utf16_len_of(&source[last.byte_range()]),
        None => {
            debug_assert!(false, "node token id outside the token table");
            eof_offset..eof_offset
        }
    }
}
