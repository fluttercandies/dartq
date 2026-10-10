//! The string-literal family: `unnecessary_brace_in_string_interps`,
//! `unnecessary_string_escapes`.
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
        assert!(f.run(&one_rule("unnecessary_string_interpolations")).is_empty());
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
}
