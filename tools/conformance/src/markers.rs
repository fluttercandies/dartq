//! Inline diagnostic expectation markers: the byte-exact port of the official
//! harness algorithms (`removeDiagnosticExpectations` and the marker
//! reader of `pkg/analyzer_testing/lib/src/expected_diagnostics.dart`,
//! upstream Dart SDK source).
//!
//! Grammar:
//! - caret lines (`^[ \t]*//[ \t]*\^+[ \t]*$`) give the position of the
//!   following expectation line;
//! - expectation lines start with `//` then `[diag.camelName]` or
//!   `[context N]`, optionally followed by explicit `[column N][length M]`
//!   metadata.
//!
//! Removal drops caret and expectation lines, keeping each retained line's
//! own terminator, with terminators only between retained lines.
//!
//! The oracle/runner lanes strip cases with the same functions before
//! analysis, so until then these surfaces are exercised by the tests only.
#![allow(dead_code)]

/// A caret marker line: whitespace, `//`, whitespace, a `^` run, whitespace.
pub fn is_caret_line(text: &str) -> bool {
    let rest = text.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix("//") else { return false };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(caret_end) = rest.find(|c| c != '^') else {
        return !rest.is_empty();
    };
    rest[caret_end..].trim().is_empty()
}

/// An expectation marker line: whitespace, `//`, whitespace, then
/// `[diag.Name]` or `[context N]`.
pub fn is_expectation_line(text: &str) -> bool {
    let rest = text.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix("//") else { return false };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix('[') else { return false };
    if let Some(rest) = rest.strip_prefix("diag.") {
        let mut chars = rest.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
            return false;
        }
        let name_len = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(rest.len() - 1, |e| e + 1);
        return rest[name_len..].starts_with(']');
    }
    if let Some(rest) = rest.strip_prefix("context") {
        let digits = rest.trim_start_matches([' ', '\t']);
        let num_len = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
        if num_len == 0 {
            return false;
        }
        let after = &digits[num_len..];
        return after.starts_with(']');
    }
    false
}

/// One source line with its terminator, split per the official `_Line.parse`.
pub struct Line<'a> {
    pub text: &'a str,
    pub terminator: &'a str,
}

/// Splits content into lines, preserving each line's terminator (`\r\n`,
/// `\n`, `\r`); the final line has an empty terminator.
pub fn parse_lines(content: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let bytes = content.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                let mut end = i + 1;
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    end += 1;
                }
                lines.push(Line { text: &content[start..i], terminator: &content[i..end] });
                start = end;
                i = end;
            }
            _ => i += 1,
        }
    }
    lines.push(Line { text: &content[start..], terminator: "" });
    lines
}

fn is_marker_line(text: &str) -> bool {
    is_caret_line(text) || is_expectation_line(text)
}

/// Removes diagnostic expectation marker lines (the official
/// `removeDiagnosticExpectations`).
pub fn remove_expectations(content: &str) -> String {
    let lines = parse_lines(content);
    let kept: Vec<&Line<'_>> = lines.iter().filter(|l| !is_marker_line(l.text)).collect();
    let mut out = String::with_capacity(content.len());
    for (i, line) in kept.iter().enumerate() {
        out.push_str(line.text);
        if i + 1 < kept.len() {
            out.push_str(line.terminator);
        }
    }
    out
}

/// One extracted expectation: the marker name (already converted to the
/// snake_case code), and the 0-based line / column / UTF-16 length it refers
/// to in the stripped content.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Expectation {
    pub code: String,
    pub line: i64,
    pub column: usize,
    pub length: usize,
}

/// Extracts expectations from marker lines (the official marker reader).
///
/// The position comes from explicit `[column N][length M]` metadata when
/// present (1-based column), else from the nearest preceding caret line.
/// The line is the last retained (non-marker) line before the expectation.
pub fn extract_expectations(content: &str) -> Vec<Expectation> {
    let mut result = Vec::new();
    let mut kept_count: usize = 0;
    let mut caret: Option<(usize, usize)> = None;
    for line in parse_lines(content) {
        if is_caret_line(line.text) {
            let first = line.text.find('^').expect("caret line contains ^");
            let length = line.text.bytes().filter(|b| *b == b'^').count();
            caret = Some((first, length));
            continue;
        }
        if let Some(code_start) = diag_name(line.text) {
            let (column, length) = match explicit_location(line.text) {
                Some(loc) => loc,
                None => match caret {
                    Some((column, length)) => (column, length),
                    None => (0, 0),
                },
            };
            result.push(Expectation {
                code: camel_to_snake(code_start),
                line: kept_count as i64 - 1,
                column,
                length,
            });
            continue;
        }
        if is_expectation_line(line.text) {
            continue; // context-message marker
        }
        kept_count += 1;
        caret = None;
    }
    result
}

/// The camelCase name inside a `[diag.Name]` marker line: a leading letter or
/// `_` followed by letters, digits and `_`, then `]` (the official
/// `_diagLinePattern`).
fn diag_name(text: &str) -> Option<&str> {
    let rest = text.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("//")?.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("[diag.")?;
    let first = rest.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let end = rest[1..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(rest.len(), |e| e + 1);
    if !rest[end..].starts_with(']') {
        return None;
    }
    Some(&rest[..end])
}

/// The explicit `[column N][length M]` metadata of a marker line: the
/// 1-based column converted to 0-based.
fn explicit_location(text: &str) -> Option<(usize, usize)> {
    let col = text.find("[column ")?;
    let rest = &text[col + "[column ".len()..];
    let col_end = rest.find(']')?;
    let column: usize = rest[..col_end].trim().parse().ok()?;
    let len = text[col..].find("[length ")?;
    let lrest = &text[col + len + "[length ".len()..];
    let len_end = lrest.find(']')?;
    let length: usize = lrest[..len_end].trim().parse().ok()?;
    Some((column.saturating_sub(1), length))
}

/// Converts a `camelCase` marker name to the analyzer's lowercase snake_case
/// code name (the inverse of the official `toCamelCase`).
pub fn camel_to_snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, c) in camel.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Converts a snake_case (or already-lowercase) code name to `camelCase` (the
/// official `toCamelCase`).
pub fn snake_to_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    for (i, part) in snake.split('_').enumerate() {
        if i == 0 {
            out.push_str(part);
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-exact removal cases over the official algorithm (hand-computed):
    /// marker lines vanish, terminators only between retained lines.
    #[test]
    fn removal_byte_exact() {
        // 1: plain removal between lines.
        assert_eq!(remove_expectations("a\n//  ^^^\nb\n"), "a\nb\n");
        // 2: no trailing terminator for a removed final line.
        assert_eq!(remove_expectations("a\n//  ^^^"), "a");
        // 3: caret line is the only line removed; \r\n preserved verbatim.
        assert_eq!(remove_expectations("a\r\n//\t^\r\n// [diag.x] m\r\nb"), "a\r\nb");
        // 4: expectation lines without preceding caret.
        assert_eq!(remove_expectations("a\n// [diag.fooBar] msg\nb"), "a\nb");
        // 5: context markers are also expectation lines.
        assert_eq!(remove_expectations("a\n// [context 1] msg\nb\n"), "a\nb\n");
        // 6: non-marker comments survive.
        assert_eq!(
            remove_expectations("a\n// plain\n// ^^^^ not alone? no\n"),
            "a\n// plain\n// ^^^^ not alone? no\n"
        );
        // 7: an empty final line survives as an empty retained line.
        assert_eq!(remove_expectations("a\n\n//  ^\nb"), "a\n\nb");
        // 8: consecutive marker lines all removed.
        assert_eq!(
            remove_expectations("a\n//  ^^^\n// [diag.x] m\n// [context 2] c\nb"),
            "a\nb"
        );
        // 9: caret-like content inside code is kept.
        assert_eq!(remove_expectations("var x = '^^^';\n"), "var x = '^^^';\n");
        // 10: `//`-only lines are not caret lines.
        assert_eq!(remove_expectations("a\n//\nb\n"), "a\n//\nb\n");
    }

    #[test]
    fn expectation_extraction_positions() {
        // The caret run starts at the 0-based column of the target: `//` plus
        // (column - 3) spaces precede the carets (the official `_caretLine`).
        let code = "class A {\n  int x = y;\n//        ^^^\n// [diag.undefinedIdentifier] y\n}\n";
        let e = extract_expectations(code);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].code, "undefined_identifier");
        assert_eq!(e[0].line, 1);
        assert_eq!(e[0].column, 10);
        assert_eq!(e[0].length, 3);
    }

    #[test]
    fn explicit_location_overrides_caret() {
        let code = "var x = 1;\n// [diag.foo][column 5][length 2] m\n";
        let e = extract_expectations(code);
        assert_eq!(e[0].column, 4);
        assert_eq!(e[0].length, 2);
    }

    #[test]
    fn zero_length_metadata_without_caret() {
        let code = "x;\n// [diag.a.b?no][column 1][length 0] m\n";
        // `a.b?no` is not a valid marker name (dot) — not an expectation.
        assert!(extract_expectations(code).is_empty());
    }

    #[test]
    fn marker_without_position_records_origin() {
        let code = "var x = 1;\n// [diag.someThing] m\n";
        let e = extract_expectations(code);
        assert_eq!(e[0].column, 0);
        assert_eq!(e[0].length, 0);
        assert_eq!(e[0].line, 0);
    }

    #[test]
    fn caret_resets_after_each_kept_line() {
        let code = "a\n//  ^^\n// [diag.x] m\nb\nc\n";
        let e = extract_expectations(code);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].line, 0);
    }

    #[test]
    fn camel_snake_mapping() {
        assert_eq!(camel_to_snake("abstractFieldInitializer"), "abstract_field_initializer");
        assert_eq!(camel_to_snake("uriDoesNotExist"), "uri_does_not_exist");
        assert_eq!(camel_to_snake("x"), "x");
        assert_eq!(snake_to_camel("abstract_field_initializer"), "abstractFieldInitializer");
        assert_eq!(snake_to_camel("already_snake"), "alreadySnake");
        for name in ["abstractFieldInitializer", "uriDoesNotExist", "a"] {
            assert_eq!(snake_to_camel(&camel_to_snake(name)), name, "{name}");
        }
    }
}
