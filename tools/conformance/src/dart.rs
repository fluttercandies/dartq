//! A minimal Dart source lexer, purpose-built for structural scanning of test
//! files: it classifies comments, string literals (raw or not, single/double,
//! triple-quoted, adjacent runs) and brace/paren/bracket nesting, and treats
//! every other byte as a plain token boundary.
//!
//! This exists so the extractor never misreads code inside string literals or
//! comments, and so string literals can be decoded exactly like Dart's own
//! scanner (escape processing, `$`-interpolation detection). It is
//! deliberately NOT the dartq parser: the tool must not share bugs with the
//! thing it tests.

/// A scanned Dart string literal: its decoded value and shape.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DartString {
    /// The decoded value (escapes resolved; interpolation preserved verbatim
    /// inside the value).
    pub value: String,
    /// Whether the literal contains a `$` interpolation (or a raw string
    /// whose content would otherwise be one — raw strings never interpolate).
    pub interpolated: bool,
}

/// One unit of an adjacent string-literal run: a single literal's decoded
/// value.
struct LiteralPart {
    value: String,
    interpolated: bool,
}

/// Parses a string-literal expression (a single literal or an adjacent run of
/// literals, possibly separated by whitespace) starting at `src[*pos]`.
/// On success advances `pos` past the expression. `None` when the expression
/// does not start with a string literal.
pub fn parse_string_expression(src: &str, pos: &mut usize) -> Option<DartString> {
    let mut parts: Vec<LiteralPart> = Vec::new();
    let mut cursor = *pos;
    loop {
        cursor += skip_ws_and_comments(&src[cursor..]);
        let Some(part) = parse_single_string(&src[cursor..]) else {
            break;
        };
        cursor += part.consumed;
        parts.push(LiteralPart { value: part.value, interpolated: part.interpolated });
        // Adjacent literals may be separated only by whitespace/comments,
        // which the loop head skips; anything else ends the run.
        let rest = &src[cursor..];
        let after = cursor + skip_ws_and_comments(rest);
        let rest = &src[after..];
        let raw = rest.starts_with('r')
            && matches!(rest.as_bytes().get(1), Some(b'\'') | Some(b'"'));
        if !raw && !rest.starts_with('\'') && !rest.starts_with('"') {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    let interpolated = parts.iter().any(|p| p.interpolated);
    let value = parts.into_iter().map(|p| p.value).collect::<String>();
    *pos = cursor;
    Some(DartString { value, interpolated })
}

struct SingleString {
    value: String,
    interpolated: bool,
    consumed: usize,
}

/// Parses one string literal at the head of `src`.
fn parse_single_string(src: &str) -> Option<SingleString> {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let raw = bytes.first() == Some(&b'r');
    if raw {
        i += 1;
    }
    let quote = *bytes.get(i)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let triple = bytes.get(i + 1) == Some(&quote) && bytes.get(i + 2) == Some(&quote);
    let (open_len, closer): (usize, &str) = if triple {
        (3, if quote == b'\'' { "'''" } else { "\"\"\"" })
    } else {
        (1, if quote == b'\'' { "'" } else { "\"" })
    };
    i += open_len;
    let mut value = String::new();
    let mut interpolated = false;
    loop {
        let rest = src.get(i..)?;
        if rest.starts_with(closer) {
            // A triple-quoted closer cannot share a prefix with a shorter
            // one; single-quote closer is unambiguous here.
            let consumed = i + closer.len();
            return Some(SingleString { value, interpolated, consumed });
        }
        let b = *bytes.get(i)?;
        match b {
            b'\\' if !raw => {
                let (ch, len) = decode_escape(src.get(i..)?, triple)?;
                value.push_str(&ch);
                i += len;
            }
            b'\'' | b'"' if !triple => {
                // Inside a single-quoted string the other quote is plain
                // content; the own quote would have matched `closer` above.
                let ch = src[i..].chars().next()?;
                value.push_str(ch.encode_utf8(&mut [0u8; 4]));
                i += ch.len_utf8();
            }
            _ => {
                let ch = src[i..].chars().next()?;
                if ch == '$' && !raw {
                    interpolated = true;
                }
                value.push_str(ch.encode_utf8(&mut [0u8; 4]));
                i += ch.len_utf8();
            }
        }
    }
}

/// Decodes one escape sequence at the head of `rest` (which starts with `\`).
/// Returns the decoded text and the consumed byte length. Line-continuation
/// and simple escapes are supported; unknown escapes keep the character as-is
/// (Dart treats unknown escapes as the character itself).
fn decode_escape(rest: &str, _triple: bool) -> Option<(String, usize)> {
    let bytes = rest.as_bytes();
    debug_assert_eq!(bytes[0], b'\\');
    let Some(&b) = bytes.get(1) else { return None };
    let simple = match b {
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        b'b' => Some('\u{0008}'),
        b'f' => Some('\u{000C}'),
        b'v' => Some('\u{000B}'),
        b'x' => None,
        b'u' => None,
        b'0'..=b'7' => None,
        b'\n' => return Some((String::new(), 2)), // line continuation
        _ => {
            let ch = rest[1..].chars().next()?;
            return Some((ch.to_string(), 1 + ch.len_utf8()));
        }
    };
    if let Some(ch) = simple {
        return Some((ch.to_string(), 2));
    }
    match b {
        b'x' => {
            let hex = rest.get(2..4)?;
            let v = u32::from_str_radix(hex, 16).ok()?;
            Some((char::from_u32(v)?.to_string(), 4))
        }
        b'u' => {
            if bytes.get(2) == Some(&b'{') {
                let end = rest[3..].find('}')? + 3;
                let v = u32::from_str_radix(&rest[3..end], 16).ok()?;
                Some((char::from_u32(v)?.to_string(), end + 1))
            } else {
                let hex = rest.get(2..6)?;
                let v = u32::from_str_radix(hex, 16).ok()?;
                Some((char::from_u32(v)?.to_string(), 6))
            }
        }
        _ => {
            // 1-3 digit octal escapes (web-only); keep the digit verbatim.
            let ch = rest[1..].chars().next()?;
            Some((ch.to_string(), 1 + ch.len_utf8()))
        }
    }
}

/// Skips whitespace and comments, returning the number of bytes skipped.
pub fn skip_ws_and_comments(src: &str) -> usize {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    loop {
        match bytes.get(i) {
            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => i += 1,
            Some(b'/') if bytes.get(i + 1) == Some(&b'/') => {
                i = src[i..].find('\n').map_or(src.len(), |e| i + e);
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                let mut j = i + 2;
                while j < src.len() && depth > 0 {
                    if src[j..].starts_with("/*") {
                        depth += 1;
                        j += 2;
                    } else if src[j..].starts_with("*/") {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                i = j;
            }
            _ => return i,
        }
    }
}

/// Scans `src` from `pos`, returning the byte offset of the next
/// `identifier(` call head at any nesting level, skipping strings, comments
/// and any other content. The identifier must not be followed by another
/// identifier character. Returns `(identifier, open_paren_offset,
/// after_paren_offset)`.
pub fn find_call(
    src: &str,
    mut pos: usize,
    names: &dyn Fn(&str) -> bool,
) -> Option<(String, usize, usize)> {
    let bytes = src.as_bytes();
    while pos < src.len() {
        let rest = &src[pos..];
        let raw_string = rest.starts_with('r')
            && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"'));
        if raw_string || rest.starts_with('\'') || rest.starts_with('"') {
            parse_single_string_at(src, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            let mut depth = 1usize;
            let mut j = pos + 2;
            while j < src.len() && depth > 0 {
                if src[j..].starts_with("/*") {
                    depth += 1;
                    j += 2;
                } else if src[j..].starts_with("*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            pos = j;
            continue;
        }
        if is_ident_start(bytes[pos]) {
            let end = pos
                + src[pos..]
                    .find(|c: char| !is_ident_char(c))
                    .unwrap_or(src.len() - pos);
            let word = &src[pos..end];
            let mut after = end + skip_ws_and_comments(&src[end..]);
            if src[after..].starts_with('(') {
                if names(word) {
                    return Some((word.to_string(), after, after + 1));
                }
                after += 1;
                pos = after;
                continue;
            }
            pos = end;
            continue;
        }
        pos += 1;
    }
    None
}

fn parse_single_string_at(src: &str, pos: &mut usize) {
    if let Some(s) = parse_single_string(&src[*pos..]) {
        *pos += s.consumed;
    } else {
        *pos += 1;
    }
}

/// Matches balanced parens in `src` starting at an open paren at `open`
/// (exclusive), returning the offset of the matching close paren.
pub fn match_paren(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0i64;
    let mut pos = open;
    let bytes = src.as_bytes();
    while pos < src.len() {
        let rest = &src[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            parse_single_string_at(src, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            parse_single_string_at(src, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            let mut depth2 = 1usize;
            let mut j = pos + 2;
            while j < src.len() && depth2 > 0 {
                if src[j..].starts_with("/*") {
                    depth2 += 1;
                    j += 2;
                } else if src[j..].starts_with("*/") {
                    depth2 -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            pos = j;
            continue;
        }
        match bytes[pos] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            _ => {}
        }
        pos += 1;
    }
    None
}

/// Splits an argument list body (text between the parens) into top-level
/// comma-separated argument texts. Handles nesting, strings and comments.
pub fn split_args(body: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0i64;
    let mut start = 0usize;
    let mut pos = 0usize;
    let bytes = body.as_bytes();
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.starts_with('\'') || rest.starts_with('"') {
            parse_single_string_at(body, &mut pos);
            continue;
        }
        if rest.starts_with('r') && matches!(bytes.get(pos + 1), Some(b'\'') | Some(b'"')) {
            parse_single_string_at(body, &mut pos);
            continue;
        }
        if rest.starts_with("//") {
            pos += rest.find('\n').map_or(rest.len(), |e| e);
            continue;
        }
        if rest.starts_with("/*") {
            pos += 2;
            while pos < body.len() && !body[pos..].starts_with("*/") {
                pos += 1;
            }
            pos = (pos + 2).min(body.len());
            continue;
        }
        match bytes[pos] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            // `<`/`>` are counted as generic-argument nesting, clamped at zero
            // so comparison operators (`a < b`) cannot suppress splitting.
            b'<' => depth += 1,
            b'>' if depth > 0 => depth -= 1,
            b',' if depth == 0 => {
                args.push(body[start..pos].trim().to_string());
                start = pos + 1;
            }
            _ => {}
        }
        pos += 1;
    }
    if start < body.len() {
        let last = body[start..].trim();
        if !last.is_empty() {
            args.push(last.to_string());
        }
    }
    args
}

/// Parses a named argument `label: expr` into `(label, expr)`; `None` for a
/// positional argument.
pub fn named_arg(arg: &str) -> Option<(String, &str)> {
    let bytes = arg.as_bytes();
    let mut pos = 0usize;
    if !is_ident_start(*bytes.first()?) {
        return None;
    }
    while pos < arg.len() && is_ident_char(arg[pos..].chars().next()?) {
        pos += arg[pos..].chars().next()?.len_utf8();
    }
    let rest = arg[pos..].trim_start();
    rest.strip_prefix(':').map(|expr| (arg[..pos].to_string(), expr.trim_start()))
}

/// One parsed call argument: its optional label and the expression text.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Arg {
    pub label: Option<String>,
    pub expr: String,
}

/// Splits an argument list body into labeled/positional arguments.
pub fn parse_args(body: &str) -> Vec<Arg> {
    split_args(body)
        .into_iter()
        .map(|text| match named_arg(&text) {
            Some((label, expr)) => Arg { label: Some(label), expr: expr.to_string() },
            None => Arg { label: None, expr: text },
        })
        .collect()
}

pub fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

pub fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Returns the offset just past an identifier (or keyword) starting at `pos`.
pub fn ident_end(src: &str, pos: usize) -> usize {
    pos + src[pos..]
        .find(|c: char| !is_ident_char(c))
        .unwrap_or(src.len() - pos)
}

/// Reads the identifier at `pos` (empty when `pos` is not at one).
pub fn ident_at(src: &str, pos: usize) -> &str {
    let end = ident_end(src, pos);
    &src[pos..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(src: &str) -> DartString {
        let mut pos = 0;
        parse_string_expression(src, &mut pos).expect("parses")
    }

    #[test]
    fn simple_and_raw_strings() {
        assert_eq!(s("'abc'").value, "abc");
        assert!(!s("'abc'").interpolated);
        assert_eq!(s(r"r'a\nb'").value, r"a\nb");
        assert!(!s(r"r'a\nb'").interpolated);
    }

    #[test]
    fn escapes_decode() {
        assert_eq!(s(r"'a\nb\x41\u{1F600}'").value, "a\nbA\u{1F600}");
        assert_eq!(s(r"'\'quoted\''").value, "'quoted'");
    }

    #[test]
    fn triple_quotes_multiline() {
        let src = "'''\nline1\nline2\n'''";
        assert_eq!(s(src).value, "\nline1\nline2\n");
    }

    #[test]
    fn interpolation_detected() {
        assert!(s("'$x and ${y.z}'").interpolated);
        assert!(!s(r"r'$x'").interpolated);
    }

    #[test]
    fn adjacent_strings_concatenate() {
        let d = s("'ab' 'cd'\n  r'ef'");
        assert_eq!(d.value, "abcdef");
        assert!(!d.interpolated);
    }

    #[test]
    fn adjacent_with_interpolation_flag() {
        let d = s("'ab' '$cd'");
        assert_eq!(d.value, "ab$cd");
        assert!(d.interpolated);
    }

    #[test]
    fn mixed_quotes_are_plain_content() {
        assert_eq!(s(r#""it's""#).value, "it's");
        assert_eq!(s(r#"'say \"hi\"'"#).value, "say \"hi\"");
    }

    #[test]
    fn string_with_comment_like_content() {
        let d = s(r"'// not a comment // ^^^ [diag.x]'");
        assert_eq!(d.value, "// not a comment // ^^^ [diag.x]");
    }

    #[test]
    fn find_call_skips_strings_and_comments() {
        let src = r"
        // findCall( inside comment
        var x = 'notCall(';
        await resolveTestCodeWithDiagnostics(r'''code''');
        ";
        let hit = find_call(src, 0, &|n: &str| n == "resolveTestCodeWithDiagnostics");
        let (name, open, _) = hit.expect("finds the call");
        assert_eq!(name, "resolveTestCodeWithDiagnostics");
        assert_eq!(&src[open..open + 1], "(");
    }

    #[test]
    fn find_call_does_not_match_prefix_of_longer_ident() {
        let src = "resolveTestCodeWithDiagnosticsFoo(x);";
        assert!(find_call(src, 0, &|n: &str| n == "resolveTestCode").is_none());
    }

    #[test]
    fn split_args_respects_nesting() {
        let args = split_args(r#"PackageConfigFileBuilder()..add(name: 'a', rootFolder: getFolder(x)), meta: true, r'''b(), c'''"#);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "PackageConfigFileBuilder()..add(name: 'a', rootFolder: getFolder(x))");
        assert_eq!(args[2], "r'''b(), c'''");
    }

    #[test]
    fn named_arg_detection() {
        assert_eq!(named_arg("meta: true"), Some(("meta".into(), "true")));
        assert_eq!(named_arg("'positional'"), None);
    }
}
