//! `@docImport` directive extraction from documentation comments.
//!
//! Doc imports are `@docImport` lines in the documentation comment attached
//! to the `library` directive (library files) or the `part of` directive
//! (part files); the analyzer resolves their URIs like real imports and
//! reports `uri_does_not_exist_in_doc_import` for missing targets. The
//! attachment decision needs directive positions, so it stays with the
//! caller: this module consumes ONE documentation comment and mirrors the
//! extraction decision order of the comment parser —
//!
//! 1. the comment is split into content lines (the `///` marker plus one
//!    space, or the `/**`/`*`/`*/` block decorations, are stripped; offsets
//!    track the stripped content),
//! 2. lines inside fenced ``` code blocks and inside indented code blocks
//!    (≥ 4 leading spaces after an empty line) are never doc imports,
//! 3. a doc import line starts (after whitespace) with the literal
//!    `@docImport ` — trailing space included, case sensitive; the rest of
//!    the line is an import directive text whose URI string literal (any
//!    simple-string form: raw, triple-quoted, adjacent strings) becomes the
//!    resolved URI, and an interpolated URI leaves no string
//!    (`uri_with_interpolation` face, same as real imports).
//!
//! `@docImport` on any other declaration's comment is not validated by the
//! oracle; callers therefore extract only from the one directive comment.

use dartq_source::SourceRange;

/// The literal that opens a doc import line (trailing space included).
const DOC_IMPORT_PREFIX: &str = "@docImport ";

/// One `///` lexeme of a line documentation comment.
#[derive(Clone, Copy, Debug)]
pub struct CommentLexeme<'a> {
    /// UTF-16 offset of the lexeme start in the file.
    pub utf16: u32,
    /// The lexeme text, without the line terminator.
    pub text: &'a str,
}

/// The documentation comment one directive carries.
#[derive(Clone, Copy, Debug)]
pub enum DocComment<'a> {
    /// The consecutive `///` lexemes in source order. Lexemes that do not
    /// start with `///` are skipped, as the oracle's token walk does.
    Lines(&'a [CommentLexeme<'a>]),
    /// One `/** … */` lexeme.
    Block {
        /// UTF-16 offset of the lexeme start in the file.
        utf16: u32,
        /// The lexeme text.
        text: &'a str,
    },
}

/// One `@docImport` directive extracted from a doc comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocImportDirective {
    /// The URI string the oracle reports: the string literal's value
    /// (quotes stripped, escapes processed, adjacent strings concatenated).
    /// `None` when the directive's URI is an interpolation — resolved as no
    /// string, the `uri_with_interpolation` face.
    pub uri: Option<Box<str>>,
    /// UTF-16 span of the URI string literal — the raw text including the
    /// `r` prefix and quotes — the diagnostic anchor.
    pub span: SourceRange,
}

/// A content line of a documentation comment: the text left after the
/// comment decoration, and the UTF-16 offset that text starts at.
struct Line<'a> {
    utf16: u32,
    text: &'a str,
}

/// Extracts the `@docImport` directives of one documentation comment, in
/// source order.
#[must_use]
pub fn doc_imports(comment: DocComment<'_>) -> Vec<DocImportDirective> {
    let lines = match comment {
        DocComment::Lines(lexemes) => line_comment_lines(lexemes),
        DocComment::Block { utf16, text } => block_comment_lines(text, utf16),
    };
    scan(&lines)
}

/// The content lines of a `///` comment: the marker plus one following space
/// is stripped per line (ASCII, so the UTF-16 delta equals the byte count).
/// Non-`///` lexemes are skipped, mirroring the oracle's token walk.
fn line_comment_lines<'a>(lexemes: &[CommentLexeme<'a>]) -> Vec<Line<'a>> {
    lexemes
        .iter()
        .filter(|lexeme| lexeme.text.starts_with("///"))
        .map(|lexeme| {
            let strip = 3 + usize::from(lexeme.text.starts_with("/// "));
            Line {
                utf16: lexeme.utf16 + strip as u32,
                text: &lexeme.text[strip..],
            }
        })
        .collect()
}

/// The content lines of one `/** … */` lexeme, mirroring the oracle's
/// multi-line comment sequence: the first line keeps the `/**` in its
/// content; later lines drop leading whitespace and one `* ` or lone `*`
/// decoration; a trailing `*/` never enters content; a line holding only
/// the end marker is skipped entirely.
fn block_comment_lines(text: &str, utf16: u32) -> Vec<Line<'_>> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut end = line_end(bytes, 0);
    let single_line = end == text.len();
    if single_line && text.ends_with("*/") {
        end -= 2;
    }
    lines.push(line_at(text, utf16, 0, end));
    while !single_line {
        // The remaining text is exactly the end marker: no content line.
        if text.ends_with("*/") && end == text.len() - 2 {
            return lines;
        }
        let mut start = end + 1;
        if start >= text.len() {
            return lines;
        }
        while is_whitespace(bytes[start]) {
            start += 1;
            if start >= text.len() {
                return lines;
            }
        }
        end = line_end(bytes, start);
        let ends_comment = end == text.len() && text.ends_with("*/");
        if ends_comment {
            end -= 2;
        }
        if text[start..].starts_with("* ") {
            start += 2;
        } else if end == start + 1 && bytes[start] == b'*' {
            start += 1;
        }
        if ends_comment && start == end {
            return lines;
        }
        lines.push(line_at(text, utf16, start, end));
    }
    lines
}

/// One content line over `text[start..end]`, with the UTF-16 offset of
/// `start`.
fn line_at<'a>(text: &'a str, utf16: u32, start: usize, end: usize) -> Line<'a> {
    Line {
        utf16: utf16 + utf16_len(&text[..start]),
        text: &text[start..end],
    }
}

/// The byte index where the line starting at `from` ends: at the `\n`, or
/// before a `\r` that precedes one, or at the text end.
fn line_end(bytes: &[u8], from: usize) -> usize {
    match bytes[from..].iter().position(|&b| b == b'\n') {
        Some(index) if index > from && bytes[index - 1] == b'\r' => from + index - 1,
        Some(index) => from + index,
        None => bytes.len(),
    }
}

/// The whitespace set of the comment parser (space, tab, LF, CR).
fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// The byte index of the first non-whitespace character.
fn read_whitespace(text: &str) -> usize {
    text.bytes()
        .position(|byte| !is_whitespace(byte))
        .unwrap_or(text.len())
}

/// UTF-16 length of `text`.
fn utf16_len(text: &str) -> u32 {
    text.chars().map(|ch| ch.len_utf16() as u32).sum()
}

/// The doc-import scan over one comment's content lines, in the parser's
/// decision order: fenced and indented code blocks swallow their lines;
/// otherwise a line whose content starts (after whitespace) with
/// `@docImport ` hosts a directive.
fn scan(lines: &[Line<'_>]) -> Vec<DocImportDirective> {
    let mut found = Vec::new();
    let mut previous_empty = true;
    let mut fence: Option<usize> = None;
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        // Inside a fenced block only the closing delimiter matters.
        if let Some(ticks) = fence {
            if fence_delimiter(line.text, ticks).is_some() {
                fence = None;
            }
            index += 1;
            continue;
        }
        let whitespace = read_whitespace(line.text);
        // An indented code block opens on a ≥ 4-space line after an empty
        // line and swallows every ≥ 4-space line; the first shallower line
        // re-enters the decision chain with its own emptiness state.
        if previous_empty && whitespace >= 4 {
            index += 1;
            while index < lines.len() && read_whitespace(lines[index].text) >= 4 {
                index += 1;
            }
            if index == lines.len() {
                break;
            }
            previous_empty = lines[index].text.is_empty();
            continue;
        }
        if let Some(ticks) = fence_delimiter(line.text, 3) {
            fence = Some(ticks);
            previous_empty = false;
        } else if line.text[whitespace..].starts_with(DOC_IMPORT_PREFIX) {
            if let Some(directive) = parse_doc_import(line, whitespace) {
                found.push(directive);
            }
            previous_empty = false;
        } else {
            // The `/**` opener line counts as empty (it carries no prose),
            // which decides whether the next line opens an indented block.
            previous_empty = if line.text == "/**" {
                true
            } else {
                line.text.is_empty()
            };
        }
        index += 1;
    }
    found
}

/// A fenced-code delimiter on `text`: at least `minimum` backticks after
/// leading whitespace; the tick run may be longer, and anything may follow.
fn fence_delimiter(text: &str, minimum: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let start = read_whitespace(text);
    if start + minimum > bytes.len() || !bytes[start..start + minimum].iter().all(|&b| b == b'`') {
        return None;
    }
    Some(bytes[start..].iter().take_while(|&&b| b == b'`').count())
}

/// Parses one `@docImport ` line's import text: the URI string literal after
/// the prefix and whitespace, adjacent strings concatenated, with the raw
/// span of the literal inside the comment line.
fn parse_doc_import(line: &Line<'_>, whitespace: usize) -> Option<DocImportDirective> {
    let rest = &line.text[whitespace + DOC_IMPORT_PREFIX.len()..];
    let mut scan = StringScan {
        text: rest,
        pos: read_whitespace(rest),
    };
    let mut value = String::new();
    let mut literal = scan.next_literal()?;
    let mut span = literal_span(line, whitespace, literal.start, literal.end);
    if literal.interpolated {
        // No stringValue: the directive resolves no URI.
        return Some(DocImportDirective { uri: None, span });
    }
    value.push_str(&literal.value);
    let start = literal.start;
    let mut end = literal.end;
    // Adjacent strings: whitespace, then another literal start.
    while let Some(skip) = adjacent_start(&rest[end..]) {
        scan.pos = end + skip;
        literal = scan.next_literal()?;
        span = literal_span(line, whitespace, literal.start, literal.end);
        if literal.interpolated {
            return Some(DocImportDirective { uri: None, span });
        }
        value.push_str(&literal.value);
        end = literal.end;
    }
    Some(DocImportDirective {
        uri: Some(value.into_boxed_str()),
        span: literal_span(line, whitespace, start, end),
    })
}

/// The whitespace skip to a following string literal, if the text after one
/// literal restarts with a quote (or raw-quote).
fn adjacent_start(text: &str) -> Option<usize> {
    let index = read_whitespace(text);
    let bytes = text.as_bytes();
    let quote = matches!(bytes.get(index), Some(b'\'' | b'"'))
        || (matches!(bytes.get(index), Some(b'r' | b'R'))
            && matches!(bytes.get(index + 1), Some(b'\'' | b'"')));
    quote.then_some(index)
}

/// One scanned string literal of the import text.
struct Literal {
    /// Byte span inside the import text: raw prefix and quotes included;
    /// an unterminated literal ends at the text end (the scanner
    /// synthesizes the missing close).
    start: usize,
    end: usize,
    /// The processed value; empty when interpolated.
    value: String,
    /// The literal carries a `$` interpolation: no string value exists.
    interpolated: bool,
}

/// The scan state over one import text; positions are byte offsets into the
/// original text so spans survive the whitespace skips.
struct StringScan<'a> {
    text: &'a str,
    pos: usize,
}

impl StringScan<'_> {
    /// Scans one string literal at the cursor. Returns `None` when no
    /// literal is here — no directive on the line.
    fn next_literal(&mut self) -> Option<Literal> {
        let bytes = self.text.as_bytes();
        let raw = matches!(bytes.get(self.pos), Some(b'r' | b'R'))
            && matches!(bytes.get(self.pos + 1), Some(b'\'' | b'"'));
        if raw {
            self.pos += 1;
        }
        let Some(&quote) = bytes.get(self.pos) else {
            return None;
        };
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let triple =
            bytes.get(self.pos + 1) == Some(&quote) && bytes.get(self.pos + 2) == Some(&quote);
        let width = if triple { 3 } else { 1 };
        let start = self.pos;
        self.pos += width;
        let content_start = self.pos;
        let mut value = String::new();
        let mut interpolated = false;
        let mut closed = false;
        while self.pos < bytes.len() {
            let byte = bytes[self.pos];
            if !raw && byte == b'\\' {
                // Skipped here; decoded from the raw text afterwards.
                self.pos = (self.pos + 2).min(bytes.len());
                continue;
            }
            if byte == quote {
                let run = bytes[self.pos..]
                    .iter()
                    .take_while(|&&b| b == quote)
                    .count();
                if run >= width {
                    self.pos += width;
                    closed = true;
                    break;
                }
            }
            if !raw && byte == b'$' && interpolation_start(bytes, self.pos) {
                interpolated = true;
                self.pos += 1;
                if bytes.get(self.pos) == Some(&b'{') {
                    self.pos += 1;
                    self.skip_expression(bytes);
                } else {
                    while self.pos < bytes.len() && is_ident_part(bytes[self.pos]) {
                        self.pos += 1;
                    }
                }
                continue;
            }
            self.pos += 1;
        }
        let content_end = if closed { self.pos - width } else { self.pos };
        let raw_text = &self.text[content_start..content_end];
        if raw {
            value.push_str(raw_text);
        } else if !interpolated {
            unescape(raw_text, &mut value);
        }
        if !closed {
            self.pos = bytes.len();
        }
        Some(Literal {
            start,
            end: self.pos,
            value,
            interpolated,
        })
    }

    /// Skips one `${ … }` expression body, honoring nested quotes so a
    /// nested string cannot close the outer literal.
    fn skip_expression(&mut self, bytes: &[u8]) {
        let mut depth = 1usize;
        while self.pos < bytes.len() && depth > 0 {
            match bytes[self.pos] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                open @ (b'\'' | b'"') => {
                    self.pos += 1;
                    while self.pos < bytes.len() && bytes[self.pos] != open {
                        self.pos += usize::from(bytes[self.pos] == b'\\');
                        self.pos += 1;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
    }
}

/// Whether the `$` at `pos` opens an interpolation: an identifier character
/// (letter, `_`, `$`, non-ASCII) follows, or `{`.
fn interpolation_start(bytes: &[u8], pos: usize) -> bool {
    match bytes.get(pos + 1) {
        Some(b'{') => true,
        Some(&b) => is_ident_start(b),
        None => false,
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte == b'$' || byte.is_ascii_alphabetic() || byte >= 0x80
}

fn is_ident_part(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

/// Decodes a cooked string's escapes into `value` (the scanner reported no
/// interpolation). Unknown escapes decode to the escaped character; an
/// unparsable numeric escape contributes its letter literally.
fn unescape(text: &str, value: &mut String) {
    let mut chars = text.char_indices();
    while let Some((_, ch)) = chars.next() {
        if ch != '\\' {
            value.push(ch);
            continue;
        }
        let Some((letter_at, escaped)) = chars.next() else {
            value.push('\\');
            break;
        };
        match escaped {
            'n' => value.push('\n'),
            'r' => value.push('\r'),
            'f' => value.push('\x0C'),
            'b' => value.push('\x08'),
            't' => value.push('\t'),
            'v' => value.push('\x0B'),
            'x' | 'u' => {
                let (parsed, consumed) = escape_code(text, letter_at + 1, escaped);
                match parsed.and_then(char::from_u32) {
                    Some(decoded) => value.push(decoded),
                    // A missing digit run leaves the letter literal.
                    None => value.push(escaped),
                }
                for _ in 0..consumed {
                    chars.next();
                }
            }
            other => value.push(other),
        }
    }
}

/// The numeric part of a `\x`/`\u` escape, `start` being the first
/// character after the letter: `(code point, consumed chars)`. `\u{…}`
/// takes a braced run, `\x` up to two and `\u` up to four plain digits;
/// an out-of-range braced run decodes to U+FFFD.
fn escape_code(text: &str, start: usize, letter: char) -> (Option<u32>, usize) {
    let bytes = text.as_bytes();
    if letter == 'u' && bytes.get(start) == Some(&b'{') {
        let Some(close) = bytes[start + 1..].iter().position(|&b| b == b'}') else {
            return (None, 0);
        };
        let digits = &text[start + 1..start + 1 + close];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return (None, 0);
        }
        // A run too large for a code point decodes to the replacement char.
        let code = Some(match u32::from_str_radix(digits, 16) {
            Ok(code) if code <= 0x10FFFF => code,
            _ => 0xFFFD,
        });
        return (code, close + 1);
    }
    let max = if letter == 'u' { 4 } else { 2 };
    let mut value = 0u32;
    let mut width = 0;
    while width < max {
        let Some(digit) = bytes
            .get(start + width)
            .and_then(|&b| (b as char).to_digit(16))
        else {
            break;
        };
        value = value * 16 + digit;
        width += 1;
    }
    ((width > 0).then_some(value), width)
}

/// The UTF-16 span of a byte range of the import text, relative to the
/// comment line.
fn literal_span(line: &Line<'_>, whitespace: usize, start: usize, end: usize) -> SourceRange {
    let prefix = whitespace + DOC_IMPORT_PREFIX.len();
    let from = prefix + start;
    let to = prefix + end;
    SourceRange {
        offset: line.utf16 + utf16_len(&line.text[..from]),
        length: utf16_len(&line.text[from..to]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::ResolutionCode;
    use crate::uri::{DirectiveKind, PackageTable, UriResolver};
    use dartq_pub::PackageConfig;
    use dartq_sdk::SdkSnapshot;
    use dartq_source::FileTable;
    use rustc_hash::FxHashMap;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A `///` doc comment over the given lines, starting at the file start.
    fn line_comment(lines: &[&str]) -> Vec<DocImportDirective> {
        let lexemes: Vec<CommentLexeme<'_>> = lines
            .iter()
            .scan(0u32, |offset, line| {
                let utf16 = *offset;
                *offset += line.chars().map(|ch| ch.len_utf16() as u32).sum::<u32>() + 1;
                Some(CommentLexeme { utf16, text: line })
            })
            .collect();
        doc_imports(DocComment::Lines(&lexemes))
    }

    #[test]
    fn extracts_corpus_shapes_with_oracle_spans() {
        // The exact corpus shape (e.g. lib/analysis_rule/rule_state.dart):
        // `/// @docImport '<uri>';` on line 1. The span anchors on the URI
        // string literal including quotes: column 16 (0-based offset 15),
        // length = uri + 2 quotes.
        let source = "/// @docImport 'package:analyzer/analysis_rule/analysis_rule.dart';";
        let found = line_comment(&[source, "library;"]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0],
            DocImportDirective {
                uri: Some("package:analyzer/analysis_rule/analysis_rule.dart".into()),
                span: SourceRange {
                    offset: 15,
                    length: 51
                },
            }
        );

        // Two directives on consecutive lines (the lib/src/dart/error/
        // lint_codes.dart shape, oracle rows 5|16|51 and 6|16|39): line 2's
        // lexeme starts at 67 + 1 newline, so its span offset is
        // 68 + 4 (`/// `) + 11 (`@docImport `) = 83.
        let found = line_comment(&[
            "/// @docImport 'package:analyzer/analysis_rule/analysis_rule.dart';",
            "/// @docImport 'package:analyzer/src/error/codes.dart';",
            "library;",
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[1].uri.as_deref(),
            Some("package:analyzer/src/error/codes.dart")
        );
        assert_eq!(
            found[1].span,
            SourceRange {
                offset: 83,
                length: 39
            }
        );
    }

    #[test]
    fn comment_without_directive_yields_nothing() {
        assert!(line_comment(&["/// Some prose about `@docImport` usage.", "library;"]).is_empty());
        // The no-space spelling is not a doc import (the literal carries a
        // trailing space in the parser).
        assert!(line_comment(&["/// @docImport'x.dart';", "library;"]).is_empty());
        // Mid-line occurrences are not line starts.
        assert!(line_comment(&["/// see also @docImport 'x.dart';", "library;"]).is_empty());
        // A `///` inside the content does not restart a directive.
        assert!(line_comment(&["/// /// @docImport 'x.dart';", "library;"]).is_empty());
    }

    #[test]
    fn indented_lines_and_code_blocks_follow_the_parser() {
        // A comment lexeme never carries indentation (whitespace tokens are
        // separate), but content after the `/// ` strip may start with
        // spaces, and the parser skips them before the prefix test.
        let found = line_comment(&["///   @docImport 'a.dart';", "library;"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uri.as_deref(), Some("a.dart"));

        // Four-plus leading content spaces after an empty line open an
        // indented code block, which swallows its lines.
        let found = line_comment(&["///", "///     @docImport 'a.dart';", "///", "library;"]);
        assert!(found.is_empty());
        // Three spaces are short of the code-block threshold.
        let found = line_comment(&["///", "///    @docImport 'a.dart';", "library;"]);
        assert_eq!(found.len(), 1);

        // Fenced code blocks swallow directives until the closing fence.
        let found = line_comment(&[
            "/// @docImport 'a.dart';",
            "/// ```dart",
            "/// @docImport 'b.dart';",
            "/// ```",
            "/// @docImport 'c.dart';",
            "library;",
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].uri.as_deref(), Some("a.dart"));
        assert_eq!(found[1].uri.as_deref(), Some("c.dart"));
    }

    #[test]
    fn block_comments_strip_decorations() {
        // First line keeps the `/**`; later lines drop the `* ` decoration;
        // the trailing `*/` never enters content.
        let text = "/**\n * @docImport 'a.dart';\n */";
        let found = doc_imports(DocComment::Block { utf16: 100, text });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uri.as_deref(), Some("a.dart"));
        // Span: lexeme offset 100, `\n * ` to the content start (7 bytes),
        // then `@docImport ` (11).
        assert_eq!(
            found[0].span,
            SourceRange {
                offset: 118,
                length: 8
            }
        );

        // The block sequence strips leading whitespace itself, so plain
        // indentation never reaches the 4-space code-block test — but the
        // star-decoration residue can: `*` + 5 spaces strips to 4 spaces,
        // and the `/**` opener counts as an empty line, so this opens an
        // indented code block and the directive is swallowed.
        let text = "/**\n *     @docImport 'a.dart';\n */";
        assert!(doc_imports(DocComment::Block { utf16: 0, text }).is_empty());

        // A directive on the `/**` line itself is not a line start.
        let text = "/** @docImport 'a.dart'; */";
        assert!(doc_imports(DocComment::Block { utf16: 0, text }).is_empty());
    }

    #[test]
    fn string_forms_cover_the_simple_literal_grammar() {
        let cases: [(&str, Option<&str>); 8] = [
            ("@docImport 'a.dart';", Some("a.dart")),
            ("@docImport \"a.dart\";", Some("a.dart")),
            ("@docImport r'raw.dart';", Some("raw.dart")),
            ("@docImport '''t.dart''';", Some("t.dart")),
            ("@docImport 'a.dart' as p;", Some("a.dart")),
            ("@docImport 'a.dart' show a;", Some("a.dart")),
            // Adjacent strings concatenate (stringValue).
            ("@docImport 'a.dart' 'more';", Some("a.dartmore")),
            // Interpolation: no stringValue at all.
            ("@docImport 'a${1}b';", None),
        ];
        for (line, uri) in cases {
            let found = line_comment(&[&format!("/// {line}"), "library;"]);
            assert_eq!(found.len(), 1, "one directive for {line}");
            assert_eq!(found[0].uri.as_deref(), uri, "uri for {line}");
        }

        // No directive at all: nothing string-like after the prefix, or
        // nothing after it.
        for line in ["@docImport ;", "@docImport", "@docImport as p;"] {
            assert!(
                line_comment(&[&format!("/// {line}"), "library;"]).is_empty(),
                "{line}"
            );
        }

        // Escapes decode into the value; the span stays raw (19 units:
        // quote + a + \u0041 + \x62 + c.dart + quote).
        let found = line_comment(&["/// @docImport 'a\\u0041\\x62c.dart';", "library;"]);
        assert_eq!(found[0].uri.as_deref(), Some("aAbc.dart"));
        assert_eq!(found[0].span.length, 19);

        // An unterminated literal spans to the line end (the scanner
        // synthesizes the close): `'open.dart` is 10 units.
        let found = line_comment(&["/// @docImport 'open.dart", "library;"]);
        assert_eq!(found[0].uri.as_deref(), Some("open.dart"));
        assert_eq!(
            found[0].span,
            SourceRange {
                offset: 15,
                length: 10
            }
        );

        // `$` without an identifier character is literal text.
        let found = line_comment(&["/// @docImport 'a$.dart';", "library;"]);
        assert_eq!(found[0].uri.as_deref(), Some("a$.dart"));
    }

    #[test]
    fn utf16_spans_count_astral_content_correctly() {
        // An astral character inside the literal costs two UTF-16 units but
        // four bytes: `'a🎉b.dart'` spans 11 units over 12 bytes.
        let found = line_comment(&["/// @docImport 'a🎉b.dart';", "library;"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uri.as_deref(), Some("a🎉b.dart"));
        assert_eq!(
            found[0].span,
            SourceRange {
                offset: 15,
                length: 11
            }
        );
    }

    /// A throwaway workspace: `pkg_a` is the analyzed package, `app` holds
    /// the package config.
    struct Workspace {
        root: std::path::PathBuf,
    }

    impl Workspace {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-resolution-doc-import-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("root dir");
            // Pin to the canonical root: temp dirs may sit under a symlinked
            // ancestor and the resolver composes from canonical spellings.
            let root = root.canonicalize().expect("canonical root");
            let write = |relative: &str, content: &str| {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                fs::write(&path, content).expect("file");
            };
            write("pkg_a/lib/a.dart", "// fixture\n");
            write("pkg_a/lib/main.dart", "// fixture\n");
            write(
                "app/.dart_tool/package_config.json",
                r#"{
                  "configVersion": 2,
                  "packages": [
                    {"name": "pkg_a", "rootUri": "../../pkg_a", "packageUri": "lib/"}
                  ]
                }"#,
            );
            Self { root }
        }

        fn packages(&self) -> PackageTable {
            let config_dir = self.root.join("app/.dart_tool");
            let json = fs::read_to_string(config_dir.join("package_config.json")).expect("config");
            let config = PackageConfig::parse(&json).expect("valid config");
            PackageTable::new(&config, &config_dir).expect("indexable config")
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A synthetic dart SDK exposing `dart:core` on segment `vm`.
    struct SdkFixture {
        root: std::path::PathBuf,
    }

    impl SdkFixture {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-resolution-doc-import-sdk-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            let write = |relative: &str, content: &str| {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().expect("parent")).expect("dir");
                fs::write(&path, content).expect("file");
            };
            write("version", "3.13.1\n");
            write(
                "lib/libraries.json",
                r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
            );
            write(
                "lib/_internal/allowed_experiments.json",
                r#"{
                  "version": 1,
                  "experimentSets": {"sdkExperiments": []},
                  "sdk": {"default": {"experimentSet": "sdkExperiments"}},
                  "packages": {}
                }"#,
            );
            write("lib/core/core.dart", "library core;");
            Self { root }
        }

        fn load(&self) -> SdkSnapshot {
            SdkSnapshot::load(&self.root, None).expect("synthetic SDK loads")
        }
    }

    impl Drop for SdkFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Resolves every `@docImport` of a one-comment file: the library doc
    /// comment extract, each URI resolved through the doc-import face.
    fn resolve_doc_imports(source: &str) -> Vec<(Option<ResolutionCode>, SourceRange)> {
        let workspace = Workspace::new();
        let sdk = SdkFixture::new();
        let packages = workspace.packages();
        let snapshot = sdk.load();
        let mut files = FileTable::new();
        let declared = FxHashMap::default();
        let container = files.intern(&workspace.root.join("pkg_a/lib/main.dart"));
        let mut resolver = UriResolver::new(
            &packages,
            &snapshot,
            &declared,
            &mut files,
            &|_| false, // disk-only existence: no residency in this fixture
        );
        doc_imports(DocComment::Lines(&[CommentLexeme {
            utf16: 0,
            text: source,
        }]))
        .into_iter()
        .map(|directive| {
            let resolution = resolver.resolve(
                container,
                directive.uri.as_deref(),
                DirectiveKind::DocImport,
            );
            (resolution.code, directive.span)
        })
        .collect()
    }

    #[test]
    fn existing_targets_stay_clean() {
        // Relative hit through the package base, absolute package hit, and a
        // known dart: library: no row for any of them.
        let outcomes = resolve_doc_imports("/// @docImport 'a.dart';");
        assert!(
            matches!(outcomes.as_slice(), &[(None, _)]),
            "relative: {outcomes:?}"
        );
        let outcomes = resolve_doc_imports("/// @docImport 'package:pkg_a/a.dart';");
        assert!(
            matches!(outcomes.as_slice(), &[(None, _)]),
            "package: {outcomes:?}"
        );
        let outcomes = resolve_doc_imports("/// @docImport 'dart:core';");
        assert!(
            matches!(outcomes.as_slice(), &[(None, _)]),
            "dart:core: {outcomes:?}"
        );
    }

    #[test]
    fn missing_relative_target_reports_the_oracle_warning() {
        let outcomes = resolve_doc_imports("/// @docImport 'missing.dart';");
        let Some((Some(code), span)) = outcomes.first().copied() else {
            panic!("expected one row, got {outcomes:?}");
        };
        assert_eq!(code, ResolutionCode::UriDoesNotExistInDocImport);
        // Oracle-exact message shape, rendered from the frozen descriptor.
        assert_eq!(
            code.info().message.replace("{0}", "missing.dart"),
            "Target of URI doesn't exist: 'missing.dart'."
        );
        assert_eq!(
            span,
            SourceRange {
                offset: 15,
                length: 14
            }
        );
    }

    #[test]
    fn missing_package_targets_report_the_oracle_warning() {
        // Known package, missing file; and a package outside the config:
        // both unresolvable targets, one warning each.
        for uri in ["package:pkg_a/gone.dart", "package:nope/a.dart"] {
            let source = format!("/// @docImport '{uri}';");
            let outcomes = resolve_doc_imports(&source);
            assert!(
                matches!(
                    outcomes.as_slice(),
                    &[(Some(ResolutionCode::UriDoesNotExistInDocImport), _)]
                ),
                "{uri}"
            );
        }
        // An unknown dart: library takes the same face.
        let outcomes = resolve_doc_imports("/// @docImport 'dart:boom';");
        assert!(matches!(
            outcomes.as_slice(),
            &[(Some(ResolutionCode::UriDoesNotExistInDocImport), _)]
        ));
    }

    #[test]
    fn no_directive_resolves_nothing() {
        // No directive, no URI, no row: the extraction is empty, so the
        // resolver never runs — the pin for the silent face.
        let found = line_comment(&["/// prose only", "library;"]);
        assert!(found.is_empty());
    }
}
