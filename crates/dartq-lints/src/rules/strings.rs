//! The string-literal family: `unnecessary_brace_in_string_interps`,
//! `unnecessary_string_escapes`, `eol_at_end_of_file`,
//! `lines_longer_than_80_chars`, `prefer_single_quotes`,
//! `unnecessary_raw_strings`, `use_raw_strings`.
//!
//! Dartq models string literals as one span-only [`NodeKind::StringLiteral`]
//! node over the segment tokens; interpolation is a token-layer fact
//! (collapse map §3.5 `LiteralStringEnd`: interpolationCount 裁 — token-level
//! interpolation discrimination). Both rules therefore walk the literal's
//! token range: `String` segment tokens, `StringInterpolationExpression`
//! (`${`) begin-groups closed by [`TokenTable::matching_end`], and
//! `StringInterpolationIdentifier` (`$id`) markers.
//!
//! Offsets are `u32` throughout the substrate and bounded by the source
//! length; the `usize` to `u32` casts below mirror the engine's own
//! convention.

#![allow(clippy::cast_possible_truncation)]

use dartq_ast::NodeKind;
use dartq_scanner::TokenKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[
    &eol_at_end_of_file::SPEC,
    &lines_longer_than_80_chars::SPEC,
    &prefer_single_quotes::SPEC,
    &unnecessary_raw_strings::SPEC,
    &use_raw_strings::SPEC,
];

pub(crate) mod unnecessary_brace_in_string_interps {
    use super::{Check, Group, NodeKind, RuleSpec, TokenKind};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_brace_in_string_interps.dart:41-83`
    /// — an interpolation `${e}` whose expression is a bare identifier
    /// (without `$`), `this`, or a plain type literal, and whose closing
    /// brace is not followed by an identifier-part character, reports at the
    /// interpolation. Message: `diagnostic.g.dart`
    /// `unnecessaryBraceInStringInterps`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_brace_in_string_interps",
        group: Group::Recommended,
        problem: "Unnecessary braces in a string interpolation.",
        correction: "Try removing the braces.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let tokens = c.unit.tokens;
        let range = c.unit.ast.token_range(node);
        let mut i = range.start as usize;
        while i < range.end as usize {
            let Some(token) = tokens.get(i) else {
                break;
            };
            if token.kind != TokenKind::StringInterpolationExpression {
                i += 1;
                continue;
            }
            // Jump past the whole `${...}` group: nested strings' own
            // interpolations belong to their StringLiteral nodes.
            let Some(close) = tokens.matching_end(i) else {
                break;
            };
            // The expression must be exactly one token: an identifier
            // (without `$`), `this`, or a plain type literal
            // (`unnecessary_brace...dart:56-69` — one-token forms only).
            let simple = (close - i == 2)
                .then(|| tokens.get(i + 1))
                .flatten()
                .is_some_and(|inner| match inner.kind {
                    TokenKind::Identifier => {
                        !inner.is_synthetic() && !c.lexeme((i + 1) as u32).contains('$')
                    }
                    // `this` (keyword tokens lexeme-match).
                    TokenKind::Keyword(_) => c.lexeme((i + 1) as u32) == "this",
                    _ => false,
                });
            if simple {
                // `isIdentifierPart(bracket.next)` (:20-22, 79-82): the first
                // character of what follows the `}` decides. In the token
                // stream that is the next `String` segment (its lexeme starts
                // with the literal text after the brace); any other token
                // (`$` of a following interpolation, EOF) counts as
                // not-an-identifier-part.
                let needed = tokens.get(close + 1).is_some_and(|next| match next.kind {
                    TokenKind::String => {
                        let b = next.byte_range();
                        let source = c.unit.source;
                        b.start < source.len()
                            && matches!(
                                source.as_bytes()[b.start],
                                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'
                            )
                    }
                    _ => false,
                });
                if !needed {
                    let start = token.utf16_offset;
                    let close_end = tokens.get(close).map_or(start, |t| {
                        t.utf16_offset + c.lexeme(close as u32).encode_utf16().count() as u32
                    });
                    c.report(start..close_end, &[]);
                }
            }
            i = close + 1;
        }
    }
}

pub(crate) mod unnecessary_string_escapes {
    use super::{Check, Group, NodeKind, RuleSpec, TokenKind};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_string_escapes.dart:24-117`
    /// — an escape of a character that needs no escaping reports at the
    /// backslash. Raw strings skip; multiline strings keep escaped quotes
    /// only where unescaping would merge the closing quotes. Message:
    /// `diagnostic.g.dart` `unnecessaryStringEscapes`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_string_escapes",
        group: Group::Recommended,
        problem: "Unnecessary escape in string literal.",
        correction: "Remove the '\\' escape.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let tokens = c.unit.tokens;
        let range = c.unit.ast.token_range(node);
        let segments: Vec<usize> = (range.start..range.end)
            .filter(|&t| {
                tokens
                    .get(t as usize)
                    .is_some_and(|tok| tok.kind == TokenKind::String)
            })
            .map(|t| t as usize)
            .collect();
        // A raw literal is one `r'...'` token (never interpolated) — skip.
        if segments.len() == 1 && c.lexeme(segments[0] as u32).starts_with('r') {
            return;
        }
        let last = segments.len().saturating_sub(1);
        for (nth, &segment) in segments.iter().enumerate() {
            let lexeme = c.lexeme(segment as u32);
            let bytes = lexeme.as_bytes();
            let is_single = bytes.first() == Some(&b'\'');
            let leading = if nth == 0 { quote_run(bytes, false) } else { 0 };
            let trailing = if nth == last {
                quote_run(bytes, true)
            } else {
                0
            };
            let is_multiline = leading == 3;
            visit_lexeme(c, segment, leading, trailing, is_single, is_multiline);
        }
    }

    /// The run length of leading (or trailing) quote characters — 1 or 3 for
    /// well-formed literals.
    fn quote_run(bytes: &[u8], trailing: bool) -> usize {
        let quote = if bytes.first() == Some(&b'r') {
            bytes.get(1).copied().unwrap_or(b'\'')
        } else {
            bytes.first().copied().unwrap_or(b'\'')
        };
        if trailing {
            bytes.iter().rev().take_while(|&&b| b == quote).count()
        } else {
            bytes.iter().take_while(|&&b| b == quote).count()
        }
    }

    /// The official `visitLexeme` (unnecessary_string_escapes.dart:44-91)
    /// over one segment's contents. Byte offsets inside the segment convert
    /// to UTF-16 through the segment token's `utf16_offset` plus the prefix
    /// count.
    fn visit_lexeme(
        c: &mut Check<'_>,
        segment: usize,
        leading: usize,
        trailing: usize,
        is_single: bool,
        is_multiline: bool,
    ) {
        let lexeme = c.lexeme(segment as u32);
        let bytes = lexeme.as_bytes();
        // Empty contents (`''`, `""`): both quotes book into the leading
        // and trailing runs, so the runs overlap across the whole lexeme
        // and the contents slice degenerates. The official `visitLexeme`
        // yields no candidates for empty contents (:44-91), so return
        // before slicing.
        if bytes.len() <= leading + trailing {
            return;
        }
        let contents = core::str::from_utf8(&bytes[leading..bytes.len() - trailing])
            .expect("contents of a valid lexeme");
        let contents_end = bytes.len() - trailing; // lexeme-local
        let token = c
            .unit
            .tokens
            .get(segment)
            .expect("segment outside the token table");
        // The multiline keep-exception compares the contents end against the
        // token end; both are lexeme-local here (`:56-60`).
        let continues_past_contents = contents_end != bytes.len();
        let utf16_base = token.utf16_offset;

        // Consecutive same-kind quotes pending the multiline decision:
        // lexeme-local byte index of the quote (or of its backslash when
        // escaped) -> escaped.
        let mut pending: Vec<(usize, bool)> = Vec::new();
        let mut reports: Vec<u32> = Vec::new();
        {
            let check_pending = |pending: &[(usize, bool)], reports: &mut Vec<u32>| {
                if is_multiline && pending.len() < 3 {
                    for &(index, escaped) in pending {
                        if !escaped {
                            continue;
                        }
                        // `'''___\'''`: without the last backslash the
                        // closing quotes merge into a parse error (:56-60).
                        if continues_past_contents && index + 2 == contents_end {
                            continue;
                        }
                        let at = utf16_base + lexeme[..index].encode_utf16().count() as u32;
                        reports.push(at);
                    }
                }
            };

            // The official loop advances past the escaped char inside the
            // body (`i += 1`, :66), so `\\` escapes exactly one char — a
            // `char_indices` for-loop would re-read the second backslash.
            let mut rel = 0;
            while rel < contents.len() {
                let mut current = contents[rel..].chars().next().expect("char boundary");
                let mut escaped = false;
                if current == '\\' && rel + 1 < contents.len() {
                    escaped = true;
                    rel += current.len_utf8();
                    current = contents[rel..].chars().next().expect("checked above");
                    if (is_single && current == '"')
                        || (!is_single && current == '\'')
                        || !ALLOWED_ESCAPED_CHARS.contains(&current)
                    {
                        // The report sits on the backslash (official
                        // `contentsOffset + i - 1`, :69): one unit before
                        // the escaped char.
                        let backslash = leading + rel - 1;
                        let at = utf16_base + lexeme[..backslash].encode_utf16().count() as u32;
                        reports.push(at);
                    }
                }
                if (is_single && current == '\'') || (!is_single && current == '"') {
                    pending.push((leading + rel - usize::from(escaped), escaped));
                } else {
                    check_pending(&pending, &mut reports);
                    pending.clear();
                }
                rel += current.len_utf8();
            }
            check_pending(&pending, &mut reports);
        }

        for at in reports {
            // The official report is length 1 on the backslash (:69).
            c.report(at..at + 1, &[]);
        }
    }

    /// The language-spec escaped chars (unnecessary_string_escapes.dart:37-50).
    const ALLOWED_ESCAPED_CHARS: &[char] =
        &['"', '\'', '$', '\\', 'n', 'r', 'f', 'b', 't', 'v', 'x', 'u'];
}

pub(crate) mod unnecessary_string_interpolations {
    use dartq_ast::NodeKind;
    use dartq_scanner::TokenKind;

    use super::{Check, Group, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_string_interpolations.dart:31-56`
    /// — a string interpolation of exactly three elements (empty leading and
    /// trailing string segments around one expression) whose expression has
    /// static type `String` (non-nullable) reports at the whole literal; the
    /// interpolated form of an adjacent-strings chain never reports
    /// (`node.parent is AdjacentStrings`, :42).
    ///
    /// Approximation (documented): the static-type question is answered for
    /// the one face that is decidable from syntax — the sole interpolation
    /// expression is itself a complete plain string literal (a single `String`
    /// token inside `${...}`), which is always non-nullable `String`. The
    /// `$identifier` face and multi-token expressions (`'$name'` over a
    /// `String` variable, `'${expr.path}'` — the official corpus's main
    /// family) need the element model and under-report. `this` is excluded
    /// even in its one-token form: its type is the enclosing class, never
    /// `String`. The adjacency guard reads the token before the literal's
    /// opening quote (a `String` segment or `CloseCurly` — the two ways a
    /// preceding literal can end) exactly where the official reads the
    /// `AdjacentStrings` parent. Message: `diagnostic.g.dart`
    /// `unnecessaryStringInterpolations`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_string_interpolations",
        group: Group::Recommended,
        problem: "Unnecessary use of string interpolation.",
        correction: "Try replacing the string literal with the variable name.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    #[expect(
        clippy::match_same_arms,
        reason = "the interpolation-identifier arm is a documented no-op face \
                  (:50-53 keeps `'$x'` unflagged); folding it into the wildcard \
                  would erase that pinned under-report note"
    )]
    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let tokens = c.unit.tokens;
        let range = c.unit.ast.token_range(node);
        if range.len() < 2 {
            return; // a plain (non-interpolated) literal is one token
        }
        let start = range.start;
        // Empty leading segment (:48): the opening `String` token is the bare
        // quote run — `'''` and friends, no raw prefix, no escaped content.
        if !tokens
            .get(start as usize)
            .is_some_and(|t| t.kind == TokenKind::String && is_quote_run(c.lexeme(start)))
        {
            return;
        }
        // Adjacent strings (:42): a preceding literal ends in one of two
        // token shapes; either makes this interpolation the interpolated
        // element of an `AdjacentStrings` chain, which never reports.
        if start > 0
            && tokens
                .get(start as usize - 1)
                .is_some_and(|t| matches!(t.kind, TokenKind::String | TokenKind::CloseCurly))
        {
            return;
        }
        match tokens.get(start as usize + 1).map(|t| t.kind) {
            // `'$id'`: the sole expression is one identifier token; its type
            // is unknown to syntax, so the face under-reports (:50-53 keeps
            // `'$x'` over a `String` variable unflagged here).
            Some(TokenKind::StringInterpolationIdentifier) => {}
            // `'${...}'`: the sole expression must be exactly one complete
            // string literal — a single `String` token filling the `${...}`
            // group, then the bare closing quote run ending the literal
            // (:44-47, three elements).
            Some(TokenKind::StringInterpolationExpression) => {
                let Some(close) = tokens.matching_end(start as usize + 1) else {
                    return;
                };
                let close = close as u32;
                let sole_string = close == start + 3
                    && tokens
                        .get(close as usize + 1)
                        .is_some_and(|t| t.kind == TokenKind::String)
                    && is_quote_run(c.lexeme(close + 1))
                    && close + 2 == range.end;
                if sole_string {
                    let span = c.utf16_span(node);
                    c.report(span, &[]);
                }
            }
            _ => {}
        }
    }

    /// Whether `lexeme` is a bare quote run: `'` / `"` or their triple forms
    /// (the empty leading/trailing segment value, :47-48). Raw strings start
    /// with `r` and never interpolate.
    fn is_quote_run(lexeme: &str) -> bool {
        let bytes = lexeme.as_bytes();
        (bytes.len() == 1 || bytes.len() == 3) && bytes.iter().all(|&b| b == bytes[0])
    }
}

pub(crate) mod avoid_escaping_inner_quotes {
    use super::{Check, Group, NodeKind, RuleSpec, TokenKind};

    /// Official: `pkg/linter/lib/src/rules/avoid_escaping_inner_quotes.dart:30-67`
    /// — a non-raw, non-multiline string (plain literal or interpolation)
    /// whose value contains the outer quote character but not the other
    /// reports at the literal, with both quote characters as message
    /// arguments (:58-66). The value face: the escaped-quote characters of
    /// the literal text (`node.value`, the interpolation's concatenated
    /// `InterpolationString` segments, :47-56).
    ///
    /// dartq faces: production [`NodeKind::StringLiteral`] rows anchor at
    /// the head token (bridge `end_literal_string`), so the literal's
    /// segments are walked from the head — `String` segments carry the
    /// escape faces, `$id` markers contribute nothing, `${...}` groups hold
    /// their own literals. In the value terms above this reads the
    /// escaped-quote face exactly; a unicode escape (`\u{27}`) that
    /// produces a quote character in the official unescape stays silent
    /// here (documented under-report).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_escaping_inner_quotes",
        group: Group::Standalone,
        problem: "Unnecessary escape of '{0}'.",
        correction: "Try changing the outer quotes to '{1}'.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let head = c.head_token(node);
        let bytes = c.lexeme(head).as_bytes();
        // Raw literals never report (:42/:52 — `node.isRaw`).
        let quote_at = usize::from(bytes.first() == Some(&b'r'));
        let Some(&quote) = bytes.get(quote_at) else {
            return;
        };
        if quote != b'\'' && quote != b'"' {
            return;
        }
        // Multiline literals never report (:42/:53 — `node.isMultiline`):
        // the leading quote run is three.
        let leading = bytes[quote_at..]
            .iter()
            .take_while(|&&b| b == quote)
            .count();
        if leading != 1 {
            return;
        }
        let other = if quote == b'\'' { b'"' } else { b'\'' };

        // Walk the literal's segments from the head token.
        let tokens = c.unit.tokens;
        let mut i = head;
        let mut end = token_end(c, head);
        let mut has_quote = false;
        let mut has_other = false;
        loop {
            let Some(token) = tokens.get(i as usize) else {
                break;
            };
            match token.kind {
                TokenKind::String => {
                    let lexeme = c.lexeme(i);
                    let closing = closes_literal(lexeme, quote);
                    scan_segment(
                        lexeme,
                        i == head,
                        closing,
                        quote,
                        other,
                        &mut has_quote,
                        &mut has_other,
                    );
                    end = token_end(c, i);
                    if closing {
                        break;
                    }
                    i += 1;
                }
                // `$identifier` interpolations contribute no literal text.
                TokenKind::StringInterpolationIdentifier => i += 1,
                // `${...}` groups hold their own literal rows; skip whole.
                TokenKind::StringInterpolationExpression => {
                    let Some(close) = tokens.matching_end(i as usize) else {
                        break;
                    };
                    i = close as u32 + 1;
                }
                _ => break,
            }
        }
        // `_isChangeable` (:63-66): the value carries the outer quote but
        // not the other.
        if has_quote && !has_other {
            let start = tokens
                .get(head as usize)
                .expect("head token in table")
                .utf16_offset;
            let quote_char = (quote as char).to_string();
            let other_char = (other as char).to_string();
            c.report(start..end, &[&quote_char, &other_char]);
        }
    }

    fn token_end(c: &Check<'_>, token: u32) -> u32 {
        let t = c.unit.tokens.get(token as usize).expect("token in table");
        t.utf16_offset + c.lexeme(token).encode_utf16().count() as u32
    }

    /// Whether the segment ends the literal: its last byte is the quote and
    /// an even number of backslashes precedes it (an escaped final quote
    /// cannot be the closer).
    fn closes_literal(lexeme: &str, quote: u8) -> bool {
        let bytes = lexeme.as_bytes();
        if bytes.last() != Some(&quote) {
            return false;
        }
        let mut backslashes = 0usize;
        let mut k = bytes.len() - 1;
        while k > 0 && bytes[k - 1] == b'\\' {
            backslashes += 1;
            k -= 1;
        }
        backslashes % 2 == 0
    }

    /// The segment's escaped-quote faces over its contents (the opening
    /// quote of the head segment and the closing quote of the closing
    /// segment are stripped — both single quotes, non-multiline). A raw
    /// other-quote character counts too (`'a"b'` carries `"` in its value).
    fn scan_segment(
        lexeme: &str,
        is_first: bool,
        closing: bool,
        quote: u8,
        other: u8,
        has_quote: &mut bool,
        has_other: &mut bool,
    ) {
        let bytes = lexeme.as_bytes();
        let start = if is_first { quote_at(bytes) + 1 } else { 0 };
        let mut end = bytes.len();
        if closing {
            end -= 1;
        }
        if start >= end {
            return;
        }
        let content = &bytes[start..end];
        let mut k = 0;
        while k < content.len() {
            match content[k] {
                b'\\' if k + 1 < content.len() => {
                    match content[k + 1] {
                        q if q == quote => *has_quote = true,
                        o if o == other => *has_other = true,
                        _ => {}
                    }
                    k += 2;
                }
                o if o == other => {
                    *has_other = true;
                    k += 1;
                }
                _ => k += 1,
            }
        }
    }

    /// The quote character's offset (past an `r` raw prefix).
    fn quote_at(bytes: &[u8]) -> usize {
        usize::from(bytes.first() == Some(&b'r'))
    }
}

/// The shared adjacent-concatenation face (collapse map §3.5
/// `AdjacentStringLiteralsHandle` D1: adjacent strings = consecutive
/// `StringLiteral` siblings). Two literals are adjacent iff no token sits
/// between them — comments are not tokens and never break a run; the flat
/// tiling's marker rows (`NoTypeArguments`, `ElseControlFlow`, …) tile
/// token-adjacently but yield no string face, and a pattern-context run's
/// last member tiles INSIDE a `ConstantPattern` row.
///
/// Returns the maximal run's string rows containing the current node, in
/// source order (the singleton `[node]` when neither neighbor is adjacent).
fn adjacent_run(c: &Check<'_>) -> Vec<u32> {
    let node = c.node();
    let Some(parent) = c.unit.ast.parent(node) else {
        return vec![node];
    };
    let siblings = c.collect_direct(parent);
    // The string face of a sibling: the literal row itself, or the single
    // literal a pattern-context `ConstantPattern` wraps.
    let face = |row: u32| -> Option<u32> {
        match c.kind(row) {
            NodeKind::StringLiteral => Some(row),
            NodeKind::ConstantPattern => c
                .collect_direct(row)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::StringLiteral),
            _ => None,
        }
    };
    let faces: Vec<u32> = siblings.iter().copied().filter_map(face).collect();
    let Some(pos) = faces.iter().position(|&kid| kid == node) else {
        return vec![node];
    };
    let adjacent = |left: u32, right: u32| {
        c.unit.ast.token_range(left).end == c.unit.ast.token_range(right).start
    };
    let mut start = pos;
    while start > 0 && adjacent(faces[start - 1], faces[start]) {
        start -= 1;
    }
    let mut end = pos;
    while end + 1 < faces.len() && adjacent(faces[end], faces[end + 1]) {
        end += 1;
    }
    faces[start..=end].to_vec()
}

/// The token extent of one string literal: `[head, tail]` over the whole
/// spelling. A production `StringLiteral` row's own token range covers only
/// its first segment; the interpolation markers continue the spelling —
/// `StringInterpolationIdentifier` consumes the `$` plus one identifier
/// token, `StringInterpolationExpression` runs to the matching `}` (the
/// nested-string-safe jump, `tokens.matching_end`), and each `String`
/// token at depth zero extends the literal.
fn literal_token_extent(c: &Check<'_>, node: u32) -> core::ops::Range<u32> {
    let range = c.unit.ast.token_range(node);
    let mut last = range.end - 1;
    let mut i = range.end - 1;
    loop {
        i += 1;
        let Some(token) = c.unit.tokens.get(i as usize) else {
            break;
        };
        match token.kind {
            TokenKind::String => last = i,
            TokenKind::StringInterpolationIdentifier => {
                // `$name` — the marker plus its one identifier token.
                i += 1;
            }
            TokenKind::StringInterpolationExpression => {
                let Some(close) = c.unit.tokens.matching_end(i as usize) else {
                    break;
                };
                i = close as u32;
            }
            _ => break,
        }
    }
    range.start..last + 1
}

pub(crate) mod leading_newlines_in_multiline_strings {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/leading_newlines_in_multiline_strings.dart:61-72`
    /// — a multiline string (triple-quoted, the `isMultiline` lexeme face)
    /// that spans a line boundary but does not start with a newline reports
    /// at the literal. The offset past the delimiters is 3, or 4 after an
    /// `r` raw prefix (:65-67); the leading-character face is the raw
    /// lexeme (an escape never counts). Interpolated literals read their
    /// first segment (:54-59 — the opening `'''…` token). Message:
    /// `diagnostic.g.dart` `leadingNewlinesInMultilineStrings`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "leading_newlines_in_multiline_strings",
        group: Group::Standalone,
        problem: "Missing a newline at the beginning of a multiline string.",
        correction: "Try adding a newline at the beginning of the string.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The whole spelling: a production row's token range covers only the
        // first segment, so the extent walk crosses the interpolation markers.
        let range = super::literal_token_extent(c, node);
        let lexeme = c.lexeme(range.start);
        let content_at = usize::from(lexeme.starts_with('r'))
            + if lexeme.contains("'''") || lexeme.contains("\"\"\"") {
                3
            } else {
                return;
            };
        // The line-boundary face: the literal's first and last characters
        // sit on different lines (:63-64 — the empty and single-line
        // multiline spellings never report).
        let Some(first) = c.unit.tokens.get(range.start as usize) else {
            return;
        };
        let Some(last) = c.unit.tokens.get(range.end as usize - 1) else {
            return;
        };
        let source = c.unit.source;
        let (start_line, _) = c
            .unit
            .lines
            .line_col_utf16(source, first.utf16_offset);
        let (end_line, _) = c.unit.lines.line_col_utf16(
            source,
            last.utf16_offset + c.lexeme(range.end - 1).encode_utf16().count() as u32,
        );
        if start_line == end_line {
            return;
        }
        // The first content character must be a newline (:65-67).
        match lexeme.as_bytes().get(content_at) {
            Some(b'\n' | b'\r') => {}
            _ => c.report(c.utf16_span(node), &[]),
        }
    }
}

pub(crate) mod missing_whitespace_between_adjacent_strings {
    use super::{Check, Group, NodeKind, RuleSpec, TokenKind};

    /// Official: `pkg/linter/lib/src/rules/missing_whitespace_between_adjacent_strings.dart:40-69`
    /// — for each adjacent pair, when the left part carries whitespace but
    /// neither ends nor the right begins with whitespace, the left part
    /// reports (:56-66). Whitespace is the `[' ', '\n', '\r', '\t']` set
    /// over the cooked values (:128-134); an interpolation at a string's
    /// start or end counts as whitespace (:83-125, the synthesized empty
    /// segments), and a left part without any whitespace skips (:62-64).
    /// Strings passed to a bare `RegExp(…)`/`matches(…)` call skip
    /// (:43-53). Message: `diagnostic.g.dart`
    /// `missingWhitespaceBetweenAdjacentStrings`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "missing_whitespace_between_adjacent_strings",
        group: Group::Standalone,
        problem: "Missing whitespace between adjacent strings.",
        correction: "Try adding whitespace between the strings.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The pair face: each left member reports its own pair (:56-66).
        let Some(next) = adjacent_next(c, node) else {
            return;
        };
        if regexp_or_matches_context(c, node, next) {
            return;
        }
        let (Some(current), Some(next)) =
            (whitespace_face(c, node), whitespace_face(c, next))
        else {
            return;
        };
        if current.1 || next.0 || !current.2 {
            return;
        }
        c.report(c.utf16_span(node), &[]);
    }

    /// The run member after the current node, when one follows (the pair
    /// face: each left member reports its own pair).
    fn adjacent_next(c: &Check<'_>, node: u32) -> Option<u32> {
        let run = super::adjacent_run(c);
        let pos = run.iter().position(|&kid| kid == node)?;
        run.get(pos + 1).copied()
    }

    /// The cooked-value whitespace face of one literal: `(starts_with,
    /// ends_with, has)` over `[' ', '\n', '\r', '\t']`. An interpolated
    /// literal reads its String segments (the official InterpolationString
    /// values): an empty first/last segment — an interpolation at the
    /// very start or end — counts as whitespace (:92-93, :118-119).
    fn whitespace_face(c: &Check<'_>, node: u32) -> Option<(bool, bool, bool)> {
        const WHITESPACES: [char; 4] = [' ', '\n', '\r', '\t'];
        // The whole spelling: a production row's token range covers only the
        // first segment, so the extent walk crosses the interpolation markers.
        let range = super::literal_token_extent(c, node);
        let kind_is = |token: u32, kind: TokenKind| {
            c.unit
                .tokens
                .get(token as usize)
                .is_some_and(|t| t.kind == kind)
        };
        let interpolated = range.clone().any(|token| {
            kind_is(token, TokenKind::StringInterpolationExpression)
                || kind_is(token, TokenKind::StringInterpolationIdentifier)
        });
        if !interpolated {
            let value = crate::rules::directives::string_value(c, node)?;
            return Some((
                value.starts_with(WHITESPACES),
                value.ends_with(WHITESPACES),
                value.contains(WHITESPACES),
            ));
        }
        // The segment contents: the opening segment drops its `r`/quote
        // run, the closing segment its quote run, the middles stand.
        let string_tokens: Vec<u32> = range
            .filter(|&token| kind_is(token, TokenKind::String))
            .collect();
        let first = *string_tokens.first()?;
        let last = *string_tokens.last()?;
        fn strip_leading(text: &str) -> &str {
            text.strip_prefix('r')
                .unwrap_or(text)
                .trim_start_matches(['\'', '"'])
        }
        fn strip_trailing(text: &str) -> &str {
            text.trim_end_matches(['\'', '"'])
        }
        let segment_text = |token: u32| -> &str {
            let text = c.lexeme(token);
            if token == first {
                strip_leading(text)
            } else if token == last {
                strip_trailing(text)
            } else {
                text
            }
        };
        let first_value = crate::rules::directives::cook_string_body(segment_text(first))?;
        let last_value = crate::rules::directives::cook_string_body(segment_text(last))?;
        let has = string_tokens.iter().any(|&token| {
            crate::rules::directives::cook_string_body(segment_text(token))
                .is_some_and(|value| value.contains(WHITESPACES))
        });
        Some((
            first_value.is_empty() || first_value.starts_with(WHITESPACES),
            last_value.is_empty() || last_value.ends_with(WHITESPACES),
            has,
        ))
    }

    /// The `RegExp()` / bare `matches()` exemption (:43-53): the argument
    /// list covering the pair belongs to a bare send (or creation) named
    /// `RegExp` or `matches`. Both official faces are name-only — the
    /// instance-creation arm keys on the constructor's enclosing class
    /// NAME (`RegExp` whenever the class resolves, a same-file `RegExp`
    /// class included), the bare-send arm on the method name
    /// (`realTarget == null`).
    fn regexp_or_matches_context(c: &Check<'_>, left: u32, right: u32) -> bool {
        let group = c.byte_span(left).start..c.byte_span(right).end;
        let covers = |row: u32| {
            let span = c.byte_span(row);
            span.start <= group.start && group.end <= span.end
        };
        // The innermost ancestor covering the group: the argument list.
        let mut arguments = left;
        loop {
            match c.unit.ast.parent(arguments) {
                Some(parent) if covers(parent) => {
                    arguments = parent;
                    if c.kind(arguments) == NodeKind::Arguments {
                        break;
                    }
                }
                _ => return false,
            }
        }
        // The invocation: the argument list's covering parent (the send
        // row spans the whole call; a member access carries a trailing
        // same-span marker).
        let Some(call) = c.unit.ast.parent(arguments) else {
            return false;
        };
        let name = match c.kind(call) {
            NodeKind::Send => {
                let Some(parent) = c.unit.ast.parent(call) else {
                    return false;
                };
                let kids = c.collect_direct(parent);
                let Some(pos) = kids.iter().position(|&kid| kid == call) else {
                    return false;
                };
                let Some((head, _)) = crate::rules::slice::send_parts(c, &kids, pos) else {
                    return false;
                };
                if c.kind(kids[head]) != NodeKind::Identifier
                    || crate::rules::slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some()
                {
                    return false;
                }
                c.lexeme(c.head_token(kids[head])).to_owned()
            }
            // The creation faces (`new RegExp(…)`, `const RegExp(…)`) name
            // the class through the constructor-reference child.
            NodeKind::NewExpression | NodeKind::ImplicitCreation => c
                .collect_direct(call)
                .into_iter()
                .filter_map(|kid| {
                    c.collect_direct(kid)
                        .into_iter()
                        .find(|&piece| c.kind(piece) == NodeKind::Identifier)
                })
                .next()
                .map_or(String::new(), |name| c.lexeme(c.head_token(name)).to_owned()),
            // The flat bare-send tiling: the whole-call `Send` row tiles
            // BESIDE its argument list under the enclosing row
            // (`matches('a b' 'cd')` — Send 13..18, Arguments 14..18, both
            // under the initializer).
            _ => {
                let kids = c.collect_direct(call);
                let pos = if c.kind(call) == NodeKind::Send {
                    kids.iter().position(|&kid| kid == call)
                } else {
                    kids.iter().position(|&kid| {
                        c.kind(kid) == NodeKind::Send && {
                            let send = c.byte_span(kid);
                            let args = c.byte_span(arguments);
                            send.start <= args.start && args.end <= send.end
                        }
                    })
                };
                let Some(pos) = pos else {
                    return false;
                };
                let Some((head, _)) = crate::rules::slice::send_parts(c, &kids, pos) else {
                    return false;
                };
                if c.kind(kids[head]) != NodeKind::Identifier
                    || crate::rules::slice::access_marker(c, &kids, pos, NodeKind::DotAccess)
                        .is_some()
                {
                    return false;
                }
                c.lexeme(c.head_token(kids[head])).to_owned()
            }
        };
        if name == "matches" {
            return true;
        }
        name == "RegExp"
    }
}

pub(crate) mod no_adjacent_strings_in_list {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/no_adjacent_strings_in_list.dart:38-82`
    /// — an adjacent-strings group in a list literal, a set literal (never
    /// a map, :67-70), a for-element body (:46-50), an if-element's then
    /// arm when no `else` follows or its else arm (:52-59), or a list
    /// pattern's constant elements (:73-81) reports at the whole group.
    /// Message: `diagnostic.g.dart` `noAdjacentStringsInList`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_adjacent_strings_in_list",
        group: Group::Standalone,
        problem: "Don't use adjacent strings in a list literal.",
        correction: "Try adding a comma between the strings.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let run = super::adjacent_run(c);
        if run.len() < 2 || run[0] != node {
            return;
        }
        // The group span: first literal's head to last literal's tail.
        let span = c.utf16_span(run[0]).start..c.utf16_span(run[run.len() - 1]).end;
        let group_bytes = c.byte_span(run[0]).start..c.byte_span(run[run.len() - 1]).end;
        let head_token = c.head_token(node);
        let covers = |row: u32| {
            let s = c.byte_span(row);
            s.start <= group_bytes.start && group_bytes.end <= s.end
        };
        // The container adjudication over the production tiling: an
        // element's rows tile BESIDE their literal row (a list literal's
        // strings are siblings of the childless `ListLiteral` row), so each
        // level decides on the ancestor row first — the control-flow rows
        // keep their element runs as children — and then on the level's
        // covering sibling rows. Anything else walks on; the official only
        // visits the five element containers, so a walk past them all is
        // silent.
        let mut covering_list_patterns = 0u8;
        let mut row = node;
        loop {
            let Some(parent) = c.unit.ast.parent(row) else {
                return;
            };
            if !covers(parent) {
                return;
            }
            row = parent;
            match c.kind(row) {
                // The for-element body is the row's tail (:46-50).
                NodeKind::ForControlFlow => {
                    if c.byte_span(row).end >= group_bytes.end {
                        c.report(span.clone(), &[]);
                    }
                    return;
                }
                // The if-element faces (:52-59): a then arm reports
                // only without `else`; an else arm always reports.
                NodeKind::IfControlFlow => {
                    let range = c.unit.ast.token_range(row);
                    match range.clone().find(|&token| c.lexeme(token) == "else") {
                        None => {
                            if c.byte_span(row).end >= group_bytes.end {
                                c.report(span.clone(), &[]);
                            }
                        }
                        Some(else_token) => {
                            if else_token < head_token
                                && c.byte_span(row).end >= group_bytes.end
                            {
                                c.report(span.clone(), &[]);
                            }
                        }
                    }
                    return;
                }
                // The switch face (:73-81): the group sits under the case's
                // covering list pattern(s). The officially-tested shapes
                // both report — the direct element (`case ['a' 'b']`) and
                // the one-level-nested constant element
                // (`test_switchPattern`: `case [['one' 'two'], var name]`
                // reports at the inner group) — so any covering
                // `ListPattern` between the group and the case reports.
                NodeKind::SwitchCase => {
                    if covering_list_patterns >= 1 {
                        c.report(span.clone(), &[]);
                    }
                    return;
                }
                _ => {}
            }
            // The level's covering sibling rows adjudicate the literal faces.
            for sib in c.collect_direct(row) {
                if !covers(sib) {
                    continue;
                }
                match c.kind(sib) {
                    NodeKind::ListLiteral => {
                        c.report(span.clone(), &[]);
                        return;
                    }
                    NodeKind::SetOrMapLiteral => {
                        // `node.isMap` (:67-70): a `MapEntry` row tiling
                        // beside the literal whose span reaches into the
                        // group is the map face.
                        let map = c.collect_direct(row).into_iter().any(|piece| {
                            c.kind(piece) == NodeKind::MapEntry && {
                                let entry = c.byte_span(piece);
                                entry.start < group_bytes.end && group_bytes.start < entry.end
                            }
                        });
                        if !map {
                            c.report(span.clone(), &[]);
                        }
                        return;
                    }
                    NodeKind::ListPattern => {
                        covering_list_patterns += 1;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The UTF-16 span of a literal's whole token extent (the report anchor for
/// the rules that face the full spelling, not just the head segment).
fn extent_utf16_span(c: &Check<'_>, extent: core::ops::Range<u32>) -> core::ops::Range<u32> {
    let start = c
        .unit
        .tokens
        .get(extent.start as usize)
        .map_or(0, |t| t.utf16_offset);
    let last = extent.end - 1;
    let end = c.unit.tokens.get(last as usize).map_or(start, |t| {
        t.utf16_offset + c.lexeme(last).encode_utf16().count() as u32
    });
    start..end
}

/// Whether the lexeme opens a multiline literal (`'''` / `"""` after the
/// optional `r` raw prefix — the `StringLexemeHelper` face).
fn is_multiline_lexeme(lexeme: &str) -> bool {
    let body = lexeme.strip_prefix('r').unwrap_or(lexeme);
    body.starts_with("'''") || body.starts_with("\"\"\"")
}

/// Whether the lexeme is single-quoted (`'` / `'''` after the optional `r`).
fn is_single_quoted_lexeme(lexeme: &str) -> bool {
    lexeme
        .strip_prefix('r')
        .unwrap_or(lexeme)
        .starts_with('\'')
}

/// The contents of a simple (one-token) literal lexeme: the raw text between
/// the opening and closing quote runs (`r` prefix stripped, escapes intact).
fn string_contents(lexeme: &str) -> Option<&str> {
    let body = lexeme.strip_prefix('r').unwrap_or(lexeme);
    let quote = *body.as_bytes().first()?;
    let leading = body.bytes().take_while(|&b| b == quote).count();
    let rest = &body[leading..];
    let trailing = rest.bytes().rev().take_while(|&b| b == quote).count();
    Some(&rest[..rest.len() - trailing.min(rest.len())])
}

/// Whether `head`'s literal sits inside another literal's `${...}` group —
/// the official `isNestedString` guard (`parent.thisOrAncestorOfType<
/// StringInterpolation>()`). The production rows carry no nesting links (a
/// string row anchors at its head token), so the face is a bounded backward
/// group scan: any `${` group open whose close lies at or after `head` (the
/// depth-0 test over `(`/`[`/`{`/`${` pairs) contains the literal; a depth-0
/// `;` can never occur inside an interpolation and stops the scan.
fn nested_in_interpolation(c: &Check<'_>, head: u32) -> bool {
    let tokens = c.unit.tokens;
    let mut depth = 0i32;
    let mut t = head;
    while t > 0 {
        t -= 1;
        let Some(token) = tokens.get(t as usize) else {
            return false;
        };
        match token.kind {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth += 1;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                if depth == 0 {
                    // The group spans the literal; the `${` carrying it may
                    // still sit further back.
                    continue;
                }
                depth -= 1;
            }
            TokenKind::StringInterpolationExpression => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            TokenKind::Semicolon if depth == 0 => return false,
            _ => {}
        }
    }
    false
}

/// Whether any string token sits strictly inside a `${...}` group of the
/// literal's extent — the official `containsString` face (any nested string
/// literal among the interpolation's elements).
fn contains_nested_string_token(c: &Check<'_>, extent: core::ops::Range<u32>) -> bool {
    let tokens = c.unit.tokens;
    let mut t = extent.start;
    while t < extent.end {
        let kind = tokens.get(t as usize).map_or(TokenKind::Eof, |tok| tok.kind);
        match kind {
            TokenKind::StringInterpolationExpression => {
                let Some(close) = tokens.matching_end(t as usize) else {
                    return false;
                };
                if ((t + 1) as usize..close).any(|k| {
                    tokens.get(k).is_some_and(|tok| tok.kind == TokenKind::String)
                }) {
                    return true;
                }
                t = close as u32 + 1;
            }
            TokenKind::StringInterpolationIdentifier => t += 2,
            _ => t += 1,
        }
    }
    false
}

pub(crate) mod eol_at_end_of_file {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/eol_at_end_of_file.dart:36-58` —
    /// a non-empty file ends in exactly one newline: several trailing
    /// newlines report the too-many code at the trimmed end, none reports
    /// the missing code. The two codes share the machine name; both message
    /// pairs are carried verbatim (`diagnostic.g.dart`
    /// `eolAtEndOfFileMissing` / `eolAtEndOfFileTooMany`), so
    /// [`Check::report_texts`] selects per shape.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "eol_at_end_of_file",
        group: Group::Standalone,
        problem: "Missing a newline at the end of the file.",
        correction: "Try adding a newline at the end of the file.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    const TOO_MANY_PROBLEM: &str = "Too many newlines at the end of the file.";
    const TOO_MANY_CORRECTION: &str = "Try removing the extra newlines at the end of the file.";

    fn check(c: &mut Check<'_>) {
        let content = c.unit.source;
        if content.is_empty() {
            return;
        }
        // `content.trimRight().length` (:48, :55) — the trimmed end, in code
        // units, where the length-1 report sits.
        let trimmed = dart_trim_right_len(content);
        if ends_with_multiple_newlines(content) {
            c.report_texts(
                trimmed..trimmed + 1,
                TOO_MANY_PROBLEM,
                TOO_MANY_CORRECTION,
                &[],
            );
        } else if !(content.ends_with('\n') || content.ends_with('\r')) {
            c.report(trimmed..trimmed + 1, &[]);
        }
    }

    /// The `endsWithMultipleNewlines` extension (:66-72): `\n\n`, `\r\r`, or
    /// `\r\n\r\n`.
    fn ends_with_multiple_newlines(content: &str) -> bool {
        ["\n\n", "\r\r", "\r\n\r\n"].iter().any(|e| content.ends_with(e))
    }

    /// `String.trimRight` over the Dart whitespace set, answering the
    /// trimmed length in code units.
    fn dart_trim_right_len(content: &str) -> u32 {
        content
            .trim_end_matches(is_dart_whitespace)
            .encode_utf16()
            .count() as u32
    }

    fn is_dart_whitespace(ch: char) -> bool {
        matches!(
            ch,
            '\u{09}'..='\u{0D}'
                | '\u{20}'
                | '\u{85}'
                | '\u{A0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
        )
    }
}

pub(crate) mod lines_longer_than_80_chars {
    use super::{Check, Group, NodeKind, RuleSpec, TokenKind};

    /// Official: `pkg/linter/lib/src/rules/lines_longer_than_80_chars.dart:139-186`
    /// — every line longer than 80 code units (terminators excluded, a
    /// `\r\n` tail additionally dropped only past the limit) reports from
    /// column 81 to the line end, unless the line is exempted: a multiline
    /// string covers all its lines, a single-line string or interpolation
    /// whose composed value contains `/` or `\` covers its own line, and a
    /// comment covers a line for every one of its content lines containing
    /// `/` or `\` (a `// ignore:` comment's own line is always covered,
    /// :33-40). Message: `diagnostic.g.dart` `linesLongerThan80Chars`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "lines_longer_than_80_chars",
        group: Group::Standalone,
        problem: "The line length exceeds the 80-character limit.",
        correction: "Try breaking the line across multiple lines.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let lines = LineTable::new(c.unit.source);
        let mut allowed: Vec<u32> = Vec::new();
        allowed_string_lines(c, &lines, &mut allowed);
        allowed_comment_lines(c, &lines, &mut allowed);
        allowed.sort_unstable();
        for i in 0..lines.count() {
            let start = lines.start(i);
            let mut end = if i + 1 == lines.count() {
                lines.utf16_len
            } else {
                lines.start(i + 1) - 1
            };
            // The `\r\n` tail drops only once the line already measures long
            // (:160-164) — a line whose 81st unit is the `\r` falls back to
            // exactly 80 and passes.
            let mut len = end - start;
            if len > 80 && lines.crlf_terminated(i) {
                end -= 1;
                len -= 1;
            }
            if len <= 80 || allowed.binary_search(&(i as u32)).is_ok() {
                continue;
            }
            // Column 81 anchors the caret where the line needs to wrap
            // (:177-178).
            c.report(start + 80..end, &[]);
        }
    }

    /// The `/` or `\` probe (`_looksLikeUriOrPath`, :29-38).
    fn looks_like_uri_or_path(value: &str) -> bool {
        value.bytes().any(|b| b == b'/' || b == b'\\')
    }

    /// The string-literal exemptions (`_AllowedLongLineVisitor`, :66-127):
    /// the whole arena's `StringLiteral` rows, extent walk included.
    fn allowed_string_lines(c: &Check<'_>, lines: &LineTable, allowed: &mut Vec<u32>) {
        for row in 0..c.unit.ast.len() as u32 {
            if c.kind(row) != NodeKind::StringLiteral {
                continue;
            }
            let extent = super::literal_token_extent(c, row);
            let span = super::extent_utf16_span(c, extent.clone());
            if super::is_multiline_lexeme(c.lexeme(extent.start)) {
                // `_handleMultilines` (:113-120): every line the literal
                // touches is exempt.
                let first = lines.line_of(span.start);
                let last = lines.line_of(span.end);
                allowed.extend(first..=last);
            } else if let Some(value) = literal_value(c, row, &extent) {
                // `_handleSingleLine` (:122-127): a URI-or-path value covers
                // its own line.
                if looks_like_uri_or_path(&value) {
                    allowed.push(lines.line_of(span.start));
                }
            }
        }
    }

    /// The composed value of one literal: the cooked segment texts with one
    /// space per interpolation-expression code unit (`visitStringInterpolation`,
    /// :82-98 — `' ' * e.length`), or [`crate::rules::directives::string_value`]
    /// for a simple literal. `None` when a segment is malformed.
    fn literal_value(
        c: &Check<'_>,
        row: u32,
        extent: &core::ops::Range<u32>,
    ) -> Option<String> {
        if extent.len() == 1 {
            return crate::rules::directives::string_value(c, row);
        }
        let mut value = String::new();
        let mut prev_end = c
            .unit
            .tokens
            .get(extent.start as usize)?
            .utf16_offset;
        let segment_indices = extent
            .clone()
            .filter(|&t| {
                c.unit
                    .tokens
                    .get(t as usize)
                    .is_some_and(|tok| tok.kind == TokenKind::String)
            })
            .collect::<Vec<u32>>();
        let (first, last) = (*segment_indices.first()?, *segment_indices.last()?);
        for position in 0..segment_indices.len() {
            let t = segment_indices[position];
            let token = c.unit.tokens.get(t as usize)?;
            let spaces = token.utf16_offset.saturating_sub(prev_end);
            value.extend(core::iter::repeat_n(' ', spaces as usize));
            let lexeme = c.lexeme(t);
            let text = if t == first {
                strip_quote_prefix(lexeme)
            } else {
                lexeme
            };
            let text = if t == last {
                strip_quote_suffix(text)
            } else {
                text
            };
            value.push_str(&crate::rules::directives::cook_string_body(text)?);
            prev_end = token.utf16_offset + lexeme.encode_utf16().count() as u32;
        }
        let end = super::extent_utf16_span(c, extent.clone()).end;
        value.extend(core::iter::repeat_n(' ', (end - prev_end) as usize));
        Some(value)
    }

    /// The `r`-prefixed opening quote run of the literal's first segment.
    fn strip_quote_prefix(lexeme: &str) -> &str {
        let body = lexeme.strip_prefix('r').unwrap_or(lexeme);
        let quote = body.as_bytes().first().copied().unwrap_or(b'\'');
        let run = body.bytes().take_while(|&b| b == quote).count();
        &body[run..]
    }

    /// The closing quote run of the literal's last segment.
    fn strip_quote_suffix(lexeme: &str) -> &str {
        let quote = lexeme.as_bytes().last().copied().unwrap_or(b'\'');
        let run = lexeme.bytes().rev().take_while(|&b| b == quote).count();
        &lexeme[..lexeme.len() - run]
    }

    /// The comment exemptions (`_AllowedCommentVisitor`, :42-63): each
    /// comment content line containing `/` or `\` covers its line; a
    /// `// ignore:` comment's own line is always covered.
    fn allowed_comment_lines(c: &Check<'_>, lines: &LineTable, allowed: &mut Vec<u32>) {
        for comment in c.comments.all() {
            let lexeme = &c.unit.source[comment.range.start as usize..comment.range.end as usize];
            let base = lines.line_of(
                c.unit
                    .lines
                    .byte_to_utf16(c.unit.source, comment.range.start),
            );
            match comment.kind {
                crate::comments::CommentKind::Line => {
                    if let Some(rest) = lexeme.strip_prefix("///") {
                        cover(rest, base, allowed);
                    } else if let Some(content) = lexeme.strip_prefix("//") {
                        if content.trim_start().starts_with("ignore:") {
                            allowed.push(base);
                        } else {
                            cover(content, base, allowed);
                        }
                    }
                }
                crate::comments::CommentKind::Block => {
                    if lexeme.len() >= 4 {
                        // `content.substring(2, length - 2)` split on every
                        // terminator shape (:55-61).
                        let body = &lexeme[2..lexeme.len() - 2];
                        for (nth, part) in body
                            .split("\r\n")
                            .flat_map(|s| s.split('\r'))
                            .flat_map(|s| s.split('\n'))
                            .enumerate()
                        {
                            cover(part, base + nth as u32, allowed);
                        }
                    }
                }
            }
        }
    }

    fn cover(value: &str, line: u32, allowed: &mut Vec<u32>) {
        if looks_like_uri_or_path(value) {
            allowed.push(line);
        }
    }

    /// The unit's line table in code-unit offsets — the `LineInfo` face:
    /// lines terminate at `\r\n`, `\r`, or `\n`, and a trailing terminator
    /// opens a final empty line.
    struct LineTable {
        starts: Vec<u32>,
        crlf: Vec<bool>,
        utf16_len: u32,
    }

    impl LineTable {
        fn new(source: &str) -> Self {
            let chars: Vec<char> = source.chars().collect();
            let mut starts = vec![0u32];
            let mut crlf = Vec::new();
            let mut utf16 = 0u32;
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                utf16 += ch.len_utf16() as u32;
                if ch == '\r' {
                    if chars.get(i + 1) == Some(&'\n') {
                        utf16 += 1;
                        i += 1;
                        crlf.push(true);
                    } else {
                        crlf.push(false);
                    }
                    starts.push(utf16);
                } else if ch == '\n' {
                    crlf.push(false);
                    starts.push(utf16);
                }
                i += 1;
            }
            Self {
                starts,
                crlf,
                utf16_len: utf16,
            }
        }

        fn count(&self) -> usize {
            self.starts.len()
        }

        fn start(&self, line: usize) -> u32 {
            self.starts[line]
        }

        fn crlf_terminated(&self, line: usize) -> bool {
            self.crlf.get(line).copied().unwrap_or(false)
        }

        /// The 0-based line containing `offset` (the `getLocation` face; an
        /// offset at the very end belongs to the last line).
        fn line_of(&self, offset: u32) -> u32 {
            self.starts.partition_point(|&s| s <= offset) as u32 - 1
        }
    }
}

pub(crate) mod prefer_single_quotes {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_single_quotes.dart:73-106`
    /// — a double-quoted literal (simple or interpolated, `r` included)
    /// reports at the literal unless the value carries a single quote (the
    /// cooked segments' face for interpolations, :95-100), unless a nested
    /// string sits inside one of its `${...}` groups (`containsString`,
    /// :75-83), or unless the literal is itself nested inside an
    /// interpolation (`isNestedString`, :85-88). Single-quoted literals
    /// never report; the `prefer_double_quotes` counterpart is a later
    /// wave and shares these three guards through this module's helpers.
    /// Message: `diagnostic.g.dart` `preferSingleQuotes`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_single_quotes",
        group: Group::Standalone,
        problem: "Unnecessary use of double quotes.",
        correction: "Try using single quotes unless the string contains single quotes.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let extent = super::literal_token_extent(c, node);
        if super::is_single_quoted_lexeme(c.lexeme(extent.start)) {
            return;
        }
        if super::nested_in_interpolation(c, extent.start) {
            return;
        }
        if extent.len() == 1 {
            // The simple face (:91-99): the cooked value decides.
            if crate::rules::directives::string_value(c, node).is_some_and(|v| v.contains('\'')) {
                return;
            }
        } else {
            // The interpolation face (:101-116): the literal segments'
            // cooked values decide, then the nested-string bail.
            if segments_contain_single_quote(c, &extent) {
                return;
            }
            if super::contains_nested_string_token(c, extent.clone()) {
                return;
            }
        }
        c.report(super::extent_utf16_span(c, extent), &[]);
    }

    /// Whether any interpolation segment's cooked value carries a `'` — the
    /// official `elements.any((e) => e is InterpolationString &&
    /// e.value.contains("'"))` (:105-110).
    fn segments_contain_single_quote(c: &Check<'_>, extent: &core::ops::Range<u32>) -> bool {
        let segments: Vec<u32> = extent
            .clone()
            .filter(|&t| {
                c.unit
                    .tokens
                    .get(t as usize)
                    .is_some_and(|tok| tok.kind == dartq_scanner::TokenKind::String)
            })
            .collect();
        let Some(&first) = segments.first() else {
            return false;
        };
        let last = *segments.last().expect("first checked above");
        for &t in &segments {
            let lexeme = c.lexeme(t);
            let text = if t == first {
                strip_segment_prefix(lexeme)
            } else {
                lexeme
            };
            let text = if t == last {
                strip_segment_suffix(text)
            } else {
                text
            };
            if crate::rules::directives::cook_string_body(text).is_some_and(|v| v.contains('\'')) {
                return true;
            }
        }
        false
    }

    fn strip_segment_prefix(lexeme: &str) -> &str {
        let body = lexeme.strip_prefix('r').unwrap_or(lexeme);
        let quote = body.as_bytes().first().copied().unwrap_or(b'\'');
        let run = body.bytes().take_while(|&b| b == quote).count();
        &body[run..]
    }

    fn strip_segment_suffix(lexeme: &str) -> &str {
        let quote = lexeme.as_bytes().last().copied().unwrap_or(b'\'');
        let run = lexeme.bytes().rev().take_while(|&b| b == quote).count();
        &lexeme[..lexeme.len() - run]
    }
}

pub(crate) mod unnecessary_raw_strings {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_raw_strings.dart:29-34`
    /// — a raw simple string whose lexeme carries neither `\` nor `$`
    /// reports at the literal. Only simple literals enter (`registry
    /// .addSimpleStringLiteral`): a raw string never interpolates, so the
    /// raw head lexeme is the whole literal — an interpolated literal's
    /// middle segment can still open with an `r` (`"${x}rest"`) and must
    /// stay silent. Message: `diagnostic.g.dart` `unnecessaryRawStrings`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_raw_strings",
        group: Group::Standalone,
        problem: "Unnecessary use of a raw string.",
        correction: "Try using a normal string.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let extent = super::literal_token_extent(c, c.node());
        if extent.len() != 1 {
            return;
        }
        let lexeme = c.lexeme(extent.start);
        if !lexeme.starts_with('r') {
            return;
        }
        if lexeme.contains('\\') || lexeme.contains('$') {
            return;
        }
        c.report(c.utf16_span(c.node()), &[]);
    }
}

pub(crate) mod use_raw_strings {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/use_raw_strings.dart:29-56` — a
    /// non-raw simple string whose every escape is `\\` or `\$` reports at
    /// the literal (those are exactly the escapes a raw string preserves;
    /// any other escape bails). Interpolated literals are not simple and
    /// never enter. Message: `diagnostic.g.dart` `useRawStrings`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_raw_strings",
        group: Group::Standalone,
        problem: "Use a raw string to avoid using escapes.",
        correction: "Try making the string a raw string and removing the escapes.",
        kinds: &[NodeKind::StringLiteral],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let extent = super::literal_token_extent(c, c.node());
        // Only simple literals enter (`registry.addSimpleStringLiteral`):
        // an interpolated literal's segments carry `\\`/`\$` spellings of
        // their own that a raw string could not preserve.
        if extent.len() != 1 {
            return;
        }
        let lexeme = c.lexeme(extent.start);
        if lexeme.starts_with('r') {
            return;
        }
        let Some(contents) = super::string_contents(lexeme) else {
            return;
        };
        // The official scan (:40-52): a backslash enters an escape; only
        // `\\` and `\$` may follow; the final code unit is never examined.
        let bytes = contents.as_bytes();
        let mut has_escape = false;
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'\\' {
                has_escape = true;
                i += 1;
                let current = bytes[i];
                if current != b'\\' && current != b'$' {
                    return;
                }
            }
            i += 1;
        }
        if has_escape {
            c.report(c.utf16_span(c.node()), &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, one_rule};

    /// The AST side of a string literal is one span-only node over the whole
    /// token range (the scanner owns the segmentation).
    fn fixture(source: &str) -> Fixture {
        Fixture::build(source, |b, t| {
            let len = t.len() as u32;
            b.begin(NodeKind::CompilationUnit, 0);
            b.leaf_span(NodeKind::StringLiteral, 0, len - 1);
            b.end(NodeKind::CompilationUnit, len);
        })
    }

    // —— unnecessary_brace_in_string_interps ————————————————————————————

    /// The plain `${name}` form reports (:doc example family).
    #[test]
    fn brace_in_interp_reports() {
        let f = fixture(r"var s = 'Hello ${name}!';");
        let lints = f.run(&one_rule("unnecessary_brace_in_string_interps"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unnecessary braces in a string interpolation."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing the braces.".to_owned())
        );
    }

    /// `_` after the brace is an identifier part — braces stay needed.
    #[test]
    fn brace_needed_before_identifier_part_passes() {
        let f = fixture(r"var s = 'Hello ${name}_world';");
        assert!(
            f.run(&one_rule("unnecessary_brace_in_string_interps"))
                .is_empty()
        );
    }

    /// `this` interpolates braceless too (:60-62).
    #[test]
    fn brace_around_this_reports() {
        let f = fixture(r"var s = 'Hello ${this}!';");
        let lints = f.run(&one_rule("unnecessary_brace_in_string_interps"));
        assert_eq!(lints.len(), 1);
    }

    /// Multi-token expressions (`a.b`) keep their braces (:63-71).
    #[test]
    fn brace_around_member_access_passes() {
        let f = fixture(r"var s = 'Hello ${a.b}!';");
        assert!(
            f.run(&one_rule("unnecessary_brace_in_string_interps"))
                .is_empty()
        );
    }

    /// Identifiers containing `$` stay braced (:57-59).
    #[test]
    fn brace_around_dollar_identifier_passes() {
        let f = fixture(r"var s = 'Hello ${a$b}!';");
        assert!(
            f.run(&one_rule("unnecessary_brace_in_string_interps"))
                .is_empty()
        );
    }

    /// A following interpolation `$` is not an identifier part — the braces
    /// are still unnecessary (`'${a}$b'`).
    #[test]
    fn brace_before_dollar_interp_reports() {
        let f = fixture(r"var s = 'x${a}$b';");
        let lints = f.run(&one_rule("unnecessary_brace_in_string_interps"));
        assert_eq!(lints.len(), 1);
    }

    // —— unnecessary_string_escapes ——————————————————————————————————————

    /// `\'` inside a double-quoted string is unnecessary (:85-88).
    #[test]
    fn escaped_quote_in_double_quoted_reports() {
        let f = fixture(r#"var s = "a\'b";"#);
        let lints = f.run(&one_rule("unnecessary_string_escapes"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary escape in string literal.");
        // Token `"a\'b"` starts at offset 8; the backslash sits 2 units in.
        assert_eq!(lints[0].range.start, 10);
        assert_eq!(lints[0].range.end, 11);
    }

    /// `\'` inside a single-quoted string is required.
    #[test]
    fn escaped_quote_in_single_quoted_passes() {
        let f = fixture(r"var s = 'a\'b';");
        assert!(f.run(&one_rule("unnecessary_string_escapes")).is_empty());
    }

    /// Raw strings never report (:79-82).
    #[test]
    fn raw_string_passes() {
        let f = fixture(r#"var s = r"a\'b";"#);
        assert!(f.run(&one_rule("unnecessary_string_escapes")).is_empty());
    }

    /// `\q` is not a language escape (:37-50).
    #[test]
    fn unknown_escape_reports() {
        let f = fixture(r#"var s = "a\qb";"#);
        let lints = f.run(&one_rule("unnecessary_string_escapes"));
        assert_eq!(lints.len(), 1);
    }

    /// Allowed escapes pass (`\n`, `\$`, `\\`).
    #[test]
    fn allowed_escapes_pass() {
        let f = fixture(r#"var s = "a\nb\$c\\\\d";"#);
        assert!(f.run(&one_rule("unnecessary_string_escapes")).is_empty());
    }

    /// Multiline: an escaped quote at the very contents end before the
    /// closing quotes is kept (`'''\''''` — unescaping would merge quotes).
    #[test]
    fn multiline_escaped_closing_quote_passes() {
        let f = fixture("var s = '''\\'''';");
        assert!(f.run(&one_rule("unnecessary_string_escapes")).is_empty());
    }

    /// Multiline: an escaped quote NOT adjacent to the end is unnecessary.
    #[test]
    fn multiline_interior_escaped_quote_reports() {
        let f = fixture("var s = '''a\\'b''';");
        let lints = f.run(&one_rule("unnecessary_string_escapes"));
        assert_eq!(lints.len(), 1);
    }

    /// Interpolated strings walk their String segments (:108-116).
    #[test]
    fn interpolation_segments_are_walked() {
        // Segments: `"a\'` | `${name}` code | `b\'"`.
        let f = fixture(r#"var s = "a\'${name}b\'";"#);
        let lints = f.run(&one_rule("unnecessary_string_escapes"));
        assert_eq!(lints.len(), 2);
    }

    /// Empty contents (`''`, `""`, raw `r''`) visit nothing: both quotes
    /// book into the leading and trailing runs, so the runs overlap the
    /// whole lexeme — the official `visitLexeme` yields no candidates for
    /// empty contents (:44-91), and the rule must not slice degenerately.
    #[test]
    fn empty_literals_yield_no_candidates() {
        let f = fixture("var a = '';\nvar b = \"\";\nvar c = r'';\n");
        assert!(f.run(&one_rule("unnecessary_string_escapes")).is_empty());
    }

    // —— unnecessary_string_interpolations ————————————————————————————————
    // The rule reads the literal's flat token range; the fixture mounts one
    // `StringLiteral` span per literal (the collapse-map §3.5 contract) over
    // a scanned source.

    /// One whole-literal `StringLiteral` span over tokens `[first, last]`.
    fn interp_fixture(source: &str, first: u32, last: u32) -> Fixture {
        Fixture::build(source, |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.leaf_span(NodeKind::StringLiteral, first, last + 1);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        })
    }

    fn interp_hits(source: &str, first: u32, last: u32) -> Vec<crate::engine::Lint> {
        interp_fixture(source, first, last).run(&one_rule("unnecessary_string_interpolations"))
    }

    /// `'${'x'}'` — the sole interpolation expression is a plain string
    /// literal, so the whole literal reports (:54-56, the literal face of
    /// `test_unnecessaryInterpolation_single`).
    #[test]
    fn interp_of_a_string_literal_reports() {
        let source = "var s = '${'x'}';";
        // 0:var 1:s 2:= 3:' 4:${ 5:'x' 6:} 7:' 8:; — five literal tokens.
        let lints = interp_hits(source, 3, 7);
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary use of string interpolation.");
        assert_eq!(
            lints[0].correction,
            Some("Try replacing the string literal with the variable name.".to_owned())
        );
        // The report covers the whole literal, not the inner expression.
        assert_eq!(lints[0].range.start, 8);
        assert_eq!(lints[0].range.end, 16);
    }

    /// `'$name'` over an unknown-typed identifier under-reports — the
    /// static-type face is resolution-bound (:50-53).
    #[test]
    fn interp_of_an_identifier_passes() {
        // 3:' 4:$ 5:name 6:'
        assert!(interp_hits("var s = '$name';", 3, 6).is_empty());
    }

    /// A non-empty leading segment (`'x$a'`) keeps its interpolation
    /// (:54-56, `test_necessaryInterpolation_single`).
    #[test]
    fn interp_with_leading_text_passes() {
        // 3:'x 4:$ 5:a 6:'
        assert!(interp_hits("var s = 'x$a';", 3, 6).is_empty());
    }

    /// Multi-token expressions stay unflagged (the property face is
    /// resolution-bound; syntax alone only sees a composite expression).
    #[test]
    fn interp_of_a_composite_expression_passes() {
        // 3:' 4:${ 5:a 6:. 7:length 8:} 9:'
        assert!(interp_hits("var s = '${a.length}';", 3, 9).is_empty());
    }

    /// A nested interpolated literal inside `${...}` is multi-token — the
    /// documented under-report face.
    #[test]
    fn interp_of_an_interpolated_literal_passes() {
        // 3:' 4:${ 5:'a 6:$ 7:b 8:' 9:} 10:'
        assert!(interp_hits("var s = '${'a$b'}';", 3, 10).is_empty());
    }

    /// The interpolated element of an adjacent-strings chain never reports
    /// (:42, `test_necessaryInterpolation_adjacentStrings`): the token
    /// before the second literal is the first literal's closing quote.
    #[test]
    fn interp_inside_adjacent_strings_passes() {
        let source = "var b = 'x' '${'y'}';";
        // 0:var 1:b 2:= 3:'x' | 4:' 5:${ 6:'y' 7:} 8:' 9:;
        let f = Fixture::build(source, |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.leaf_span(NodeKind::StringLiteral, 3, 4);
            b.leaf_span(NodeKind::StringLiteral, 4, 9);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(
            f.run(&one_rule("unnecessary_string_interpolations"))
                .is_empty()
        );
    }

    /// `'$this'` interpolates the enclosing object — never `String` (:50-53).
    #[test]
    fn interp_of_this_passes() {
        // 3:' 4:$ 5:this 6:'
        assert!(interp_hits("var s = '$this';", 3, 6).is_empty());
    }

    /// Multiline and double-quoted forms report on the same shape.
    #[test]
    fn interp_of_a_literal_multiline_and_double_quoted_report() {
        // 0:''' 1:${ 2:'y' 3:} 4:'''
        assert_eq!(interp_hits("'''${'y'}'''", 0, 4).len(), 1);
        // 0:" 1:${ 2:'y' 3:} 4:"
        assert_eq!(interp_hits(r#""${'y'}""#, 0, 4).len(), 1);
    }

    /// Raw strings never interpolate — nothing to report.
    #[test]
    fn raw_braces_pass() {
        // The raw literal is one token.
        assert!(interp_hits(r"r'${'y'}'", 0, 0).is_empty());
    }

    // —— avoid_escaping_inner_quotes ————————————————————————————————————

    /// `test/rules/avoid_escaping_inner_quotes_test.dart` — a literal
    /// escaping its own quote with no other escape reports over the whole
    /// literal, offering the swapped outer quotes.
    #[test]
    fn escaping_inner_quotes_reports_a_single_quote_escape() {
        let f = Fixture::parse(r#"var a = 'a\'b';"#);
        let lints = f.run(&one_rule("avoid_escaping_inner_quotes"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary escape of '''.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try changing the outer quotes to '\"'.")
        );
    }

    /// The double-quoted face mirrors the message variants.
    #[test]
    fn escaping_inner_quotes_reports_a_double_quote_escape() {
        let f = Fixture::parse(r#"var a = "a\"b";"#);
        let lints = f.run(&one_rule("avoid_escaping_inner_quotes"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary escape of '\"'.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try changing the outer quotes to '''.")
        );
    }

    /// Both quote kinds in one value silence the rule (no better outer
    /// quote), and swapping the outer quotes removes the need to escape.
    #[test]
    fn escaping_inner_quotes_both_quotes_pass() {
        let f = Fixture::parse(r#"var a = 'a"b\'c';"#);
        let f2 = Fixture::parse(r#"var b = "a'b";"#);
        assert!(f.run(&one_rule("avoid_escaping_inner_quotes")).is_empty());
        assert!(f2.run(&one_rule("avoid_escaping_inner_quotes")).is_empty());
    }

    /// Raw, multiline, and empty literals skip.
    #[test]
    fn escaping_inner_quotes_raw_multiline_empty_pass() {
        let f = Fixture::parse("var a = r'a\"b';\nvar b = '''a\\'b''';\nvar c = '';");
        assert!(f.run(&one_rule("avoid_escaping_inner_quotes")).is_empty());
    }

    /// Only the other quote character silences the rule: an unrelated
    /// escape (`\n`) leaves the escaped quote changeable, and escaping
    /// inside an interpolated literal reports the same way.
    #[test]
    fn escaping_inner_quotes_other_escape_and_interpolation_faces() {
        let f = Fixture::parse("var a = 'a\\nb\\'c';\nvar b = 'a\\'b $a c';");
        assert_eq!(f.run(&one_rule("avoid_escaping_inner_quotes")).len(), 2);
    }
}

/// The W3 (#992) production-bridge pins: the string-wave rules over real
/// parser trees.
#[cfg(test)]
mod w3_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    // —— leading_newlines_in_multiline_strings ————————————————————————

    /// A multiline literal without a leading newline reports at the
    /// literal (`leading_newlines_in_multiline_strings.dart` doc example).
    #[test]
    fn leading_newlines_without_leading_newline_reports() {
        let f = Fixture::parse("var a = '''this\n is multiline''';\n");
        let lints = f.run(&one_rule("leading_newlines_in_multiline_strings"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing a newline at the beginning of a multiline string."
        );
        let literal = tok(&f.source, &f.tokens, "'''this\n is multiline'''", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, literal));
    }

    /// A leading newline, a single-line multiline spelling and a raw
    /// literal with a leading newline pass; the interpolated face reads
    /// its first segment (:54-67).
    #[test]
    fn leading_newlines_compliant_spellings_pass() {
        let f = Fixture::parse("var a = '''\nstarts right''';\n");
        assert!(
            f.run(&one_rule("leading_newlines_in_multiline_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var b = '''single line''';\n");
        assert!(
            f.run(&one_rule("leading_newlines_in_multiline_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var c = r'''\nraw starts right''';\n");
        assert!(
            f.run(&one_rule("leading_newlines_in_multiline_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var x = 1;\nvar d = '''more $x\nthan one''';\n");
        assert_eq!(
            f.run(&one_rule("leading_newlines_in_multiline_strings"))
                .len(),
            1
        );
    }

    // —— missing_whitespace_between_adjacent_strings ——————————————————

    /// A whitespace-carrying left part at a bare seam reports at the left
    /// part (`missing_whitespace_between_adjacent_strings.dart` doc
    /// example family).
    #[test]
    fn missing_whitespace_reports_the_left_part() {
        let f = Fixture::parse("var a = 'a long line' 'continues';\n");
        let lints = f.run(&one_rule("missing_whitespace_between_adjacent_strings"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Missing whitespace between adjacent strings."
        );
        let left = tok(&f.source, &f.tokens, "'a long line'", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, left));
    }

    /// A three-part run reports once per pair (each left member).
    #[test]
    fn missing_whitespace_three_part_run_reports_each_left() {
        let f = Fixture::parse("var b = 'pair one' 'pair two' 'pair three';\n");
        assert_eq!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .len(),
            2
        );
    }

    /// Whitespace at the seam, a whitespace-free left part, an
    /// interpolation ending in whitespace, and the RegExp/matches
    /// exemptions all pass (:62-64, :83-125, :43-53).
    #[test]
    fn missing_whitespace_compliant_and_exempt_faces_pass() {
        let f = Fixture::parse("var a = 'one ' 'two';\n");
        assert!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var b = 'ab' 'cd';\n");
        assert!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var c = 'one' ' two';\n");
        assert!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var y = 1;\nvar d = 'x $y ' 'tail';\n");
        assert!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .is_empty()
        );

        let f = Fixture::parse("var e = new RegExp('a b' 'cd');\nvar g = matches('a b' 'cd');\n");
        assert!(
            f.run(&one_rule("missing_whitespace_between_adjacent_strings"))
                .is_empty()
        );
    }

    // —— no_adjacent_strings_in_list ———————————————————————————————————

    /// The list, set and if-element faces report the group at its head
    /// (no_adjacent_strings_in_list.dart:42-59, :67-70).
    #[test]
    fn no_adjacent_strings_list_set_and_if_faces() {
        let f = Fixture::parse(
            "var a = ['l one' 'l two'];\nvar b = <String>{'s one' 's two'};\nvar x = 0;\nvar d = [if (x == 0) 'i one' 'i two'];\n",
        );
        let lints = f.run(&one_rule("no_adjacent_strings_in_list"));
        assert_eq!(lints.len(), 3, "{lints:?}");
        assert_eq!(lints[0].message, "Don't use adjacent strings in a list literal.");
        let head = tok(&f.source, &f.tokens, "'l one'", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, head));
    }

    /// The for-element body and the if-element else arm report; the then
    /// arm with an else following stays silent (:46-59).
    #[test]
    fn no_adjacent_strings_for_and_else_faces() {
        let f = Fixture::parse(
            "var c = [for (var i in [1]) 'f one' 'f two'];\nvar x = 0;\nvar e = [if (x == 0) 't' 'u' else 'e one' 'e two'];\n",
        );
        assert_eq!(
            f.run(&one_rule("no_adjacent_strings_in_list")).len(),
            2,
            "{:?}",
            f.run(&one_rule("no_adjacent_strings_in_list"))
        );
    }

    /// A map entry is never a list face (:67-70); the officially-tested
    /// pattern shapes both report (:73-81) — the direct element and the
    /// one-level-nested constant element (`test_switchPattern` verbatim:
    /// `case [['one' 'two'], var name]` reports at the inner group).
    #[test]
    fn no_adjacent_strings_map_and_pattern_boundaries() {
        let f = Fixture::parse("var m = <String, String>{'k': 'v one' 'v two'};\n");
        assert!(f.run(&one_rule("no_adjacent_strings_in_list")).is_empty());

        let f = Fixture::parse(
            "void h(Object v) {\n  switch (v) {\n    case ['p one' 'p two']:\n      break;\n  }\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("no_adjacent_strings_in_list")).len(),
            1,
            "{:?}",
            f.run(&one_rule("no_adjacent_strings_in_list"))
        );

        let f = Fixture::parse(
            "void h(List<String?> row) {\n  switch (row) {\n    case [['one' 'two'], var name]:\n  }\n}\n",
        );
        assert_eq!(
            f.run(&one_rule("no_adjacent_strings_in_list")).len(),
            1,
            "{:?}",
            f.run(&one_rule("no_adjacent_strings_in_list"))
        );
    }
}

/// The W5 (#1026) production-bridge pins: the string-wave rules over real
/// parser trees, against the upstream corpora
/// (`pkg/linter/test/rules/<name>_test.dart`).
#[cfg(test)]
mod w5_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    // —— eol_at_end_of_file ————————————————————————————————————————————

    /// `test_hasEol` — a file without a trailing newline reports one unit
    /// at the trimmed end.
    #[test]
    fn eol_missing_reports_at_the_trimmed_end() {
        let f = Fixture::parse("class A {}");
        let lints = f.run(&one_rule("eol_at_end_of_file"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Missing a newline at the end of the file.");
        assert_eq!(
            lints[0].correction,
            Some("Try adding a newline at the end of the file.".to_owned())
        );
        assert_eq!(lints[0].range, 10..11);
    }

    /// `test_hasNoEol` — a single trailing newline passes.
    #[test]
    fn eol_single_trailing_newline_passes() {
        let f = Fixture::parse("class A {}\n");
        assert!(f.run(&one_rule("eol_at_end_of_file")).is_empty());
    }

    /// `test_hasMultipleNewlines_message` — several trailing newlines
    /// report the too-many texts at the trimmed end.
    #[test]
    fn eol_too_many_newlines_report() {
        let f = Fixture::parse("class A {}\n\n");
        let lints = f.run(&one_rule("eol_at_end_of_file"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Too many newlines at the end of the file.");
        assert_eq!(
            lints[0].correction,
            Some("Try removing the extra newlines at the end of the file.".to_owned())
        );
        assert_eq!(lints[0].range, 10..11);
    }

    // —— lines_longer_than_80_chars —————————————————————————————————————

    /// `test_longerThan80Characters` — the report anchors at column 81 and
    /// runs to the line end (`start + 80..end`).
    #[test]
    fn lines_over_80_report_from_column_81() {
        let source = format!("var p =\n    '{}';\n", " ".repeat(81));
        let f = Fixture::parse(&source);
        let lints = f.run(&one_rule("lines_longer_than_80_chars"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "The line length exceeds the 80-character limit.");
        // Line 2 starts at 8 and is 88 units long (`    '` + 81 + `';`).
        assert_eq!(lints[0].range, 88..96);
    }

    /// `test_exactly80Characters` — an 80-unit line passes.
    #[test]
    fn lines_exactly_80_pass() {
        let source = format!("var p =\n    '{}';\n", " ".repeat(73));
        let f = Fixture::parse(&source);
        assert!(f.run(&one_rule("lines_longer_than_80_chars")).is_empty());
    }

    /// `test_multilineString` and `test_stringContainsForwardSlash` — a
    /// multiline literal covers all its lines and a URI-or-path value
    /// covers its own.
    #[test]
    fn lines_multiline_and_uri_exemptions() {
        let source = "var p = \"\"\"\nThis line is a long, very long, very very long, \
                      very very very long, very very very very long\n\"\"\";\n";
        let f = Fixture::parse(source);
        assert!(f.run(&one_rule("lines_longer_than_80_chars")).is_empty());

        let source = format!("var p =\n    '/{}';\n", "x".repeat(90));
        let f = Fixture::parse(&source);
        assert!(f.run(&one_rule("lines_longer_than_80_chars")).is_empty());
    }

    // —— prefer_single_quotes ———————————————————————————————————————————

    /// `test_doubleQuotes` — a double-quoted literal reports at itself.
    #[test]
    fn prefer_single_quotes_double_quotes_report() {
        let f = Fixture::parse("var x = \"no quote\";");
        let lints = f.run(&one_rule("prefer_single_quotes"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary use of double quotes.");
        assert_eq!(
            lints[0].correction,
            Some(
                "Try using single quotes unless the string contains single quotes."
                    .to_owned()
            )
        );
        let literal = tok(&f.source, &f.tokens, "\"no quote\"", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, literal));
    }

    /// `test_singleQuote` and `test_doubleQuotes_innerSingleQuote` —
    /// single quotes and a contained `'` pass.
    #[test]
    fn prefer_single_quotes_compliant_values_pass() {
        let f = Fixture::parse("var x = 'uses single';");
        assert!(f.run(&one_rule("prefer_single_quotes")).is_empty());

        let f = Fixture::parse("var x = \"has quote '\";");
        assert!(f.run(&one_rule("prefer_single_quotes")).is_empty());
    }

    /// `test_doubleQuotes_interpolationWithSingleQuote` — a nested string
    /// inside `${...}` silences the rule; the interpolation face itself
    /// reports (`test_doubleQuotes_interpolation`).
    #[test]
    fn prefer_single_quotes_nested_string_and_interpolation_faces() {
        let f = Fixture::parse("var x = \"foo ${1 == 'x'} bar\";");
        assert!(f.run(&one_rule("prefer_single_quotes")).is_empty());

        let f = Fixture::parse("void f(String p) {\n  \"no quote $p\";\n}\n");
        assert_eq!(f.run(&one_rule("prefer_single_quotes")).len(), 1);
    }

    // —— unnecessary_raw_strings ————————————————————————————————————————

    /// `test_singleQuote_raw` — a raw string without `\` or `$` reports.
    #[test]
    fn unnecessary_raw_strings_reports_a_plain_raw_string() {
        let f = Fixture::parse("var s = r'a b c d';");
        let lints = f.run(&one_rule("unnecessary_raw_strings"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Unnecessary use of a raw string.");
        let literal = tok(&f.source, &f.tokens, "r'a b c d'", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, literal));
    }

    /// `test_singleQuote_raw_containsBackslash` and `test_singleQuote` — a
    /// `\`/`$`-carrying raw string and a non-raw string pass.
    #[test]
    fn unnecessary_raw_strings_escapes_and_plain_pass() {
        let f = Fixture::parse("var s = r'a b c\\d';");
        assert!(f.run(&one_rule("unnecessary_raw_strings")).is_empty());

        let f = Fixture::parse("var s = 'a b c d';");
        assert!(f.run(&one_rule("unnecessary_raw_strings")).is_empty());
    }

    /// Only simple literals enter (`registry.addSimpleStringLiteral`): an
    /// interpolated literal's middle segment opens with `r` in
    /// `"${x}rest"` and must stay silent.
    #[test]
    fn unnecessary_raw_strings_interpolated_literal_is_inert() {
        let f = Fixture::parse("var x = 1;\nvar s = \"${x}rest\";");
        assert!(f.run(&one_rule("unnecessary_raw_strings")).is_empty());
    }

    // —— use_raw_strings ————————————————————————————————————————————————

    /// `test_escapedBackslash` — a literal whose every escape is raw-safe
    /// reports.
    #[test]
    fn use_raw_strings_reports_raw_safe_escapes() {
        let f = Fixture::parse(r"var f = '\\';");
        let lints = f.run(&one_rule("use_raw_strings"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Use a raw string to avoid using escapes.");
    }

    /// `test_escapedMultiple_withNewline` and `test_raw_escapedBackslash`
    /// — any other escape bails; raw strings never enter.
    #[test]
    fn use_raw_strings_other_escapes_and_raw_pass() {
        let f = Fixture::parse(r"var f = '\$ and \\ and \n';");
        assert!(f.run(&one_rule("use_raw_strings")).is_empty());

        let f = Fixture::parse(r"var f = r'\\';");
        assert!(f.run(&one_rule("use_raw_strings")).is_empty());
    }

    /// `test_escapedMultiple_withInterpolation` — an interpolated literal
    /// is not a simple string and never enters (`registry
    /// .addSimpleStringLiteral`), even with only raw-safe escapes.
    #[test]
    fn use_raw_strings_interpolated_literal_is_inert() {
        let f = Fixture::parse("var x = 1;\nvar f = '\\$ and \\\\ and $x';");
        assert!(f.run(&one_rule("use_raw_strings")).is_empty());
    }
}
