//! Curly-bracket two-pass recovery decision (official
//! `getOffsetForCurlyBracketRecoveryStart`, abstract_scanner.dart:506-527).
//!
//! When the first pass ends with errors and exactly one unclosed `{` on the
//! grouping stack, the token stream is walked from that `{` forward: the last
//! `{`/`${` whose closer sits on a different line at a different indentation
//! is blamed as the brace the user is about to type (the "user is typing"
//! shape — the corpus `with_outline/typing_*` files). A second pass re-scans
//! with that offset and closes the blamed brace with a synthetic closer at
//! the `}` that would otherwise have consumed it
//! (`tokenizeCloseBrace` :1139-1153, `tokenizeInterpolatedExpression`
//! :2258-2266).
//!
//! Everything here runs on the error path only; the clean-file path never
//! reaches this module. Offsets and indents are byte-based: line membership
//! and indentation comparisons are relative and agree with the official
//! UTF-16 numbers for space-indented source.

use crate::keyword::Keyword;
use crate::token::{TokenKind, TokenTable};

/// The recovery start: the byte offset of the last misaligned `{`/`${`
/// between the unclosed `{` at `begin_idx` and EOF, or `None` when the walk
/// finds none (no re-scan — mirrors the official null return).
pub(crate) fn recovery_offset(src: &str, table: &TokenTable, begin_idx: u32) -> Option<usize> {
    let tokens = table.as_slice();
    let line_starts = line_starts(src);
    let mut last_mismatch = None;
    for idx in begin_idx as usize + 1..tokens.len() - 1 {
        let kind = tokens[idx].kind;
        if !matches!(
            kind,
            TokenKind::OpenCurly | TokenKind::StringInterpolationExpression
        ) {
            continue;
        }
        // Every `{`/`${` below the stack head has a closer at this point
        // (real close, mismatch synthetic, or EOF synthetic); the head itself
        // is `begin_idx`, excluded above. Skip `None` defensively instead of
        // mirroring the official `endGroup!` crash.
        let Some(end_idx) = table.matching_end(idx) else {
            continue;
        };
        if line_of(&line_starts, tokens[idx].byte_offset)
            == line_of(&line_starts, tokens[end_idx].byte_offset)
        {
            continue;
        }
        let indent = spaces_at_start_of_logical_line(table, &line_starts, idx);
        if indent >= 0 && indent != spaces_at_start_of_logical_line(table, &line_starts, end_idx) {
            last_mismatch = Some(idx);
        }
    }
    last_mismatch.map(|idx| tokens[idx].byte_offset as usize)
}

/// Byte offsets of the line starts (`\n`, and a lone `\r` — the same line
/// model as `LineIndex`). Error-path only: one linear byte scan.
fn line_starts(src: &str) -> Vec<u32> {
    let bytes = src.as_bytes();
    let mut starts = vec![0u32];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                starts.push(i as u32 + 1);
                i += 1;
            }
            b'\r' => {
                i += 1;
                if bytes.get(i) != Some(&b'\n') {
                    starts.push(i as u32);
                }
            }
            _ => i += 1,
        }
    }
    starts
}

/// 0-based line of a byte offset (greatest start `<= offset`; index 0 always
/// qualifies — official `_getLineOf` binary search).
fn line_of(line_starts: &[u32], offset: u32) -> usize {
    line_starts.partition_point(|&start| start <= offset) - 1
}

/// Official `_spacesAtStartOfLogicalLineOf` (:416-493): the indentation of
/// the logical line of `idx`, or `-1` when the official heuristic declines
/// (the `{` follows none of the recognized shapes).
///
/// For a `{` the heuristic first walks to the token that "owns" it —
/// `if`/`for`/`while`/`switch`/`catch` through the `(...)` before the brace,
/// `else`/`try`/`finally` directly, or `x = {` / `x = const {` — and declines
/// for anything else (plain statement/block braces return `-1`, so a `-1`
/// never mismatches and never blames).
fn spaces_at_start_of_logical_line(table: &TokenTable, line_starts: &[u32], mut idx: usize) -> i64 {
    let tokens = table.as_slice();
    if tokens[idx].kind == TokenKind::OpenCurly {
        if idx == 0 {
            return -1;
        }
        let prev = idx - 1;
        let mut found_wanted = false;
        if tokens[prev].kind == TokenKind::CloseParen {
            // Walk back to the `(` matching this `)`; the token before it
            // must be one of the control-flow keywords (official :434-454).
            let close_paren = prev;
            let mut matched = None;
            let mut c = close_paren;
            while c > 0 {
                c -= 1;
                if table.matching_end(c) == Some(close_paren) {
                    matched = Some(c);
                    break;
                }
                // A stale end group pointing past this paren ends the search
                // (official :440-442); running below index 0 is the sentinel
                // equivalent of `candidate.isEof`.
                if let Some(end) = table.matching_end(c)
                    && tokens[end].byte_offset > tokens[close_paren].byte_offset
                {
                    break;
                }
            }
            if let Some(open_paren) = matched
                && open_paren > 0
            {
                idx = open_paren - 1;
                found_wanted = matches!(
                    tokens[idx].kind,
                    TokenKind::Keyword(
                        Keyword::If
                            | Keyword::For
                            | Keyword::While
                            | Keyword::Switch
                            | Keyword::Catch
                    )
                );
            }
        } else if matches!(
            tokens[prev].kind,
            TokenKind::Keyword(Keyword::Else | Keyword::Try | Keyword::Finally)
        ) {
            found_wanted = true;
        } else if tokens[prev].kind == TokenKind::Eq
            && prev >= 1
            && tokens[prev - 1].kind == TokenKind::Identifier
        {
            // `x = {` (official :459-462).
            found_wanted = true;
        } else if tokens[prev].kind == TokenKind::Keyword(Keyword::Const)
            && prev >= 2
            && tokens[prev - 1].kind == TokenKind::Eq
            && tokens[prev - 2].kind == TokenKind::Identifier
        {
            // `x = const {` (official :463-468).
            found_wanted = true;
        }
        if !found_wanted {
            return -1;
        }
    }
    let line = line_of(line_starts, tokens[idx].byte_offset);
    if line == 0 {
        // Official :477: the first token's offset is its own indentation.
        return i64::from(tokens[0].byte_offset);
    }
    let line_start = line_starts[line];
    let mut first = idx;
    while first > 0 && tokens[first - 1].byte_offset >= line_start {
        first -= 1;
    }
    // `line >= 1` puts the first token of the file above `line_start`, so the
    // walk always stops on the line's first token (official
    // `candidate.next.offset - lineStart`).
    i64::from(tokens[first].byte_offset - line_start)
}
