use super::*;


/// A width-0 keyword hit (`new`, `late`): the rewrite drops the keyword
/// plus its trailing horizontal whitespace (a newline stays — joining
/// lines is a formatting decision, not a fix). The keyword is ASCII, so
/// its lexeme length is its UTF-16 length.
pub(crate) fn drop_keyword(text: &str, offset16: u32, keyword: &str) -> Option<(u32, u32, String)> {
    debug_assert_eq!(
        keyword.len(),
        keyword.chars().map(char::len_utf16).sum::<usize>()
    );
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(keyword)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let length = u32::try_from(keyword.len() + ws).ok()?;
    Some((offset16, length, String::new()))
}

/// A span-exact rewrite: the hit span must slice to exactly `expect`.
pub(crate) fn replace_exact(
    text: &str,
    range: &core::ops::Range<u32>,
    expect: &str,
    replacement: &str,
) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    text.get(start..end).filter(|slice| *slice == expect)?;
    Some((range.start, range.end - range.start, replacement.to_owned()))
}

/// `${id}` → `$id`: the hit span is the whole interpolation group (the
/// rule reports it exactly); the `${…}` shape and a bare ASCII identifier
/// body are verified before rewriting.
pub(crate) fn unbrace(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    let slice = text.get(start..end)?;
    let inner = slice.strip_prefix("${")?.strip_suffix('}')?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((range.start, range.end - range.start, format!("${inner}")))
}

/// A width-0 `this` hit: the rewrite drops the `this.` qualifier — the
/// keyword plus horizontal whitespace and the dot (the official
/// `RemoveThisExpression` deletes from the keyword through the operator;
/// a newline never joins).
pub(crate) fn drop_this_qualifier(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix("this")?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if !rest[ws..].starts_with('.') {
        return None;
    }
    Some((offset16, 4 + u32::try_from(ws).ok()? + 1, String::new()))
}

/// The whole-literal hit of an interpolation of one string literal
/// (`''${'x'}''`): the rewrite keeps the inner literal verbatim (the
/// official producer deletes the wrapper ranges around it). The empty
/// quote-run wrappers and the `${…}` group are verified first.
pub(crate) fn unwrap_string_interpolation(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let bytes = slice16(text, range)?.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let lead = quote_run(bytes, quote, false);
    let trail = quote_run(bytes, quote, true);
    // 1/3 quotes: the plain and triple-quote wrappers; 2: the empty
    // adjacent-string face (`''${'x'}''`). The runs must match.
    if lead != trail || !matches!(lead, 1..=3) {
        return None;
    }
    let body = bytes.get(lead..bytes.len() - trail)?.strip_prefix(b"${")?;
    if body.last() != Some(&b'}') {
        return None;
    }
    // The kept expression is the string literal between `${` and the
    // final `}`: it opens with its own quote (optionally raw) and closes
    // with the matching one.
    let inner = &body[..body.len() - 1];
    let open = match inner.first()? {
        b'\'' | b'"' => inner.first()?,
        b'r' => inner.get(1)?,
        _ => return None,
    };
    if *open != b'\'' && *open != b'"' {
        return None;
    }
    if inner.last() != Some(open) || inner.len() < 2 {
        return None;
    }
    let start = utf16_offset_to_byte(text, range.start);
    let first = start + lead + 2;
    let last = start + bytes.len() - trail - 1;
    Some((
        range.start,
        range.end - range.start,
        text[first..last].to_owned(),
    ))
}

/// A `null`-operand hit under `??`: the rewrite drops the operator with
/// the operand (`a ?? null` → `a`, `null ?? a` → `a`; the official
/// deletion spans operator and operand together). The intervening
/// whitespace is horizontal only — a newline stays.
pub(crate) fn drop_if_null_null(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    replace_exact(text, range, "null", "")?;
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    // Right operand (`a ?? null`): the deletion starts at the whitespace
    // before the operator — the official span runs from the left operand's
    // end through the null.
    if let Some(head) = text.get(..start) {
        let op_ws = head.len() - head.trim_end_matches([' ', '\t']).len();
        if head[..head.len() - op_ws].ends_with("??") {
            let before = &head[..head.len() - op_ws - 2];
            let lead = before.len() - before.trim_end_matches([' ', '\t']).len();
            let at = u32::try_from(start - op_ws - 2 - lead).ok()?;
            return Some((at, range.end - at, String::new()));
        }
    }
    // Left operand (`null ?? a`): the text after opens with the operator.
    let rest = text.get(end..)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[ws..];
    if let Some(tail) = after.strip_prefix("??") {
        let tail_ws = tail.len() - tail.trim_start_matches([' ', '\t']).len();
        return Some((
            range.start,
            4 + u32::try_from(ws + 2 + tail_ws).ok()?,
            String::new(),
        ));
    }
    None
}

/// The whole-assignment hit of `x ??= null`: a lone expression-statement
/// line loses the whole line (the official `getLinesRange` face); any
/// other context loses `??= null`.
pub(crate) fn remove_null_aware_assignment(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let slice = slice16(text, range)?;
    let eq = u32::try_from(slice.find("??=")?).ok()?;
    let rhs = slice.get(eq as usize + 3..)?;
    if rhs.trim() != "null" || slice[..eq as usize].trim().is_empty() {
        return None;
    }
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    // The statement-line face: nothing but whitespace before the hit on
    // its line, and `;` plus horizontal whitespace to the line end after.
    // The whole line goes — the official `getLinesRange` includes the
    // line's own indentation — through the line terminator.
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    if text[line_start..start].trim().is_empty()
        && let Some(rest) = text.get(end..)
    {
        let after = rest.trim_start_matches([' ', '\t']);
        if let Some(after_semi) = after.strip_prefix(';') {
            let trailing = after_semi.len() - after_semi.trim_start_matches([' ', '\t']).len();
            let tail = &after_semi[trailing..];
            let eol = if tail.starts_with("\r\n") {
                2
            } else if tail.starts_with('\n') {
                1
            } else {
                tail.is_empty().then_some(0)?
            };
            let deleted = utf16_count(&text[line_start..end + 1 + trailing + eol]);
            return Some((utf16_count(&text[..line_start]), deleted, String::new()));
        }
    }
    // The expression face: the official deletion runs from the left-hand
    // side's end through the value's — the whitespace before the operator
    // goes with it.
    let head = &slice[..eq as usize];
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    let at = range.start + eq - u32::try_from(ws).ok()?;
    Some((at, range.end - at, String::new()))
}

pub(crate) fn drop_null_initializer(
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let slice = slice16(text, range)?;
    let eq = u32::try_from(slice.rfind('=')?).ok()?;
    // The initializer is exactly `null` and nothing before the `=` reads
    // as an assignment of its own. The deletion carries the whitespace
    // before the `=` (the official deletion runs from the name's end).
    if slice.get(eq as usize + 1..)?.trim() != "null" || slice[..eq as usize].contains('=') {
        return None;
    }
    let head = &slice[..eq as usize];
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    let at = range.start + eq - u32::try_from(ws).ok()?;
    Some((at, range.end - at, String::new()))
}

/// A width-0 variable-name hit under a final/const declaration with a
/// nullable written type: the rewrite deletes the type's trailing `?`
/// (the official `RemoveQuestionMark` deletes the question token of the
/// written type, never a type argument's).
pub(crate) fn drop_type_question_mark(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let head = text.get(..start)?;
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    if !head[..head.len() - ws].ends_with('?') {
        return None;
    }
    Some((offset16 - u32::try_from(ws).ok()? - 1, 1, String::new()))
}

/// A width-0 `new` hit: the rewrite drops the `.new` (the official
/// `RemoveConstructorName` deletes from the period through the name).
pub(crate) fn drop_dot_new(text: &str, offset16: u32) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix("new")?;
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let head = text.get(..start)?;
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    if !head[..head.len() - ws].ends_with('.') {
        return None;
    }
    let at = u32::try_from(start - ws - 1).ok()?;
    Some((at, u32::try_from(ws + 4).ok()?, String::new()))
}

/// The first content offset on `[at, bound)`: the first byte that is
/// neither horizontal whitespace nor a comment. A `//` run owns the rest
/// of the line and an unclosed `/*` never reaches content — both answer
/// `None` (nothing but comments remains on the line). Block comments nest
/// and must close inside the bound to keep the walk going.
fn line_content_at(text: &str, mut at: usize, bound: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    while at < bound {
        match bytes[at] {
            b' ' | b'\t' | b'\r' => at += 1,
            b'/' if bytes.get(at + 1) == Some(&b'/') => return None,
            b'/' if bytes.get(at + 1) == Some(&b'*') => match skip_block_comment(text, at, bound) {
                Some(next) => at = next,
                None => return None,
            },
            _ => return Some(at),
        }
    }
    None
}

/// The offset just past a `/* … */` comment opening at `at` (nesting
/// counted), or `None` when it does not close inside `bound`.
fn skip_block_comment(text: &str, at: usize, bound: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 1u32;
    let mut i = at + 2;
    while i < bound && depth > 0 {
        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    (depth == 0).then_some(i)
}

/// The offset just past the last `;` outside comments and string literals
/// on `[at, bound)` — a previous same-line directive's end, where the
/// separator run towards the next directive begins. A `//` run ends the
/// walk (it owns the line tail); quotes skip to their pair on the line.
fn last_semicolon_before(text: &str, at: usize, bound: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut prev_end = None;
    let mut i = at;
    while i < bound {
        match bytes[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b';' => {
                prev_end = Some(i + 1);
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => return prev_end,
            b'/' if bytes.get(i + 1) == Some(&b'*') => match skip_block_comment(text, i, bound) {
                Some(next) => i = next,
                None => return prev_end,
            },
            quote @ (b'\'' | b'"') => match text[i + 1..bound].find(quote as char) {
                Some(rel) => i = i + rel + 2,
                None => return prev_end,
            },
            _ => i += 1,
        }
    }
    prev_end
}

/// `unused_import` (and its `duplicate_import`/`unnecessary_import`
/// siblings): the URI literal's span expands to its directive's deletion
/// range, the official `RemoveUnusedImport` posture over the analyzer's
/// line arithmetic (upstream Dart SDK source, pkg/analysis_server,
/// remove_unused_import.dart — `_deletionRange` over `getLinesRange`):
///
/// * a directive alone on its line drops the whole line, terminator with
///   it (CRLF aware; the last line of the file carries none);
/// * a same-line sibling that follows owns the separator: the cut runs
///   from this keyword to that directive's keyword — separator whitespace
///   and comments between the two go with the removal;
/// * otherwise only the directive's text goes — keyword through `;`. A
///   sibling that precedes cedes its separator (the cut starts at its
///   `;`); a leading comment keeps its line, the cut starting at the
///   keyword's leading whitespace run; a trailing comment keeps its line
///   and the terminator, the cut ending past the `;`'s trailing
///   whitespace run.
///
/// A directive whose `;` leaves the URI's line (a clause crossing lines)
/// never cuts — the report anchors the URI and only a same-line read of
/// the directive stays verifiable.
pub(crate) fn drop_import_directive(text: &str, range: SourceRange) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.offset);
    let end = utf16_offset_to_byte(text, range.offset + range.length);
    let (line_start, line_end) = line_bounds(text, start, end)?;
    // The directive's `import` keyword sits right before the URI — only
    // horizontal whitespace between them, and the keyword is a word of its
    // own, not an identifier's tail.
    let head = text[line_start..start].trim_end_matches([' ', '\t']);
    let keyword = head.strip_suffix("import")?;
    if keyword.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$') {
        return None;
    }
    let keyword_start = line_start + keyword.len();
    // The directive's own `;` — the first on the line past the URI (the
    // combinator and config clauses carry none of their own).
    let directive_end = text[end..line_end].find(';')? + end + 1;
    // A same-line directive that follows: the cut runs keyword → keyword,
    // the separator between the two riding the removal.
    if let Some(next) = line_content_at(text, directive_end, line_end)
        && ["import", "export", "part", "library"].iter().any(|word| {
            text[next..line_end].starts_with(word)
                && !text[next + word.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_identifier_char)
        })
    {
        return Some(cut16(text, keyword_start, next));
    }
    // The directive owns its line: the whole line goes, terminator with it.
    let before = &text[line_start..keyword_start];
    let after = &text[directive_end..line_end];
    let bare = before.bytes().all(|b| b == b' ' || b == b'\t');
    if bare && after.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r')) {
        let (offset, length) = cut_line(text, line_start, line_end);
        return Some((offset, length, String::new()));
    }
    // Nothing before it on the line but indentation: the cut runs from the
    // line start through the `;`'s trailing whitespace run — the official
    // `getLineContentEnd` stop just short of the trailing comment.
    if bare {
        let ws = after.len() - after.trim_start_matches([' ', '\t', '\r']).len();
        return Some(cut16(text, line_start, directive_end + ws));
    }
    // Other content leads the line: a preceding directive cedes its
    // separator (the cut opens just past its `;`); otherwise the keyword's
    // leading whitespace run opens the cut — `getLineContentStart`'s walk,
    // a leading comment staying whole.
    let cut_start = last_semicolon_before(text, line_start, keyword_start).unwrap_or_else(|| {
        keyword_start - (before.len() - before.trim_end_matches([' ', '\t']).len())
    });
    Some(cut16(text, cut_start, directive_end))
}

/// The `[from, to)` deletion in UTF-16 with an empty replacement.
fn cut16(text: &str, from: usize, to: usize) -> (u32, u32, String) {
    (
        utf16_count(&text[..from]),
        utf16_count(&text[from..to]),
        String::new(),
    )
}

/// `unused_local_variable`: the name renames to `_{name}` — the wildcard
/// posture silencing the row. The whole declaration line drops only when
/// it is a bare single declarator whose initializer is trivially pure
/// (literal-shaped tokens, [`pure_single_declarator`]); anything else
/// keeps the rename.
pub(crate) fn rewrite_unused_local(
    tokens: Option<&TokenTable>,
    text: &str,
    range: SourceRange,
) -> Option<(u32, u32, String)> {
    let (name, start, end) = word_slice(text, range)?;
    if name.starts_with('_') {
        return None;
    }
    if let Some(tokens) = tokens
        && let Some((first, _)) = hit_tokens(tokens, &(range.offset..range.offset + range.length))
        && let Some(semicolon_end) = pure_single_declarator(tokens, first)
        && let Some((line_start, line_end)) = line_bounds(text, start, end)
        && let Some(owns_head) = declaration_owns_line_head(tokens, text, first, line_start)
        && owns_head
        && text[semicolon_end..line_end]
            .trim_matches([' ', '\t', '\r'])
            .is_empty()
    {
        let (offset, length) = cut_line(text, line_start, line_end);
        return Some((offset, length, String::new()));
    }
    Some((range.offset, range.length, format!("_{name}")))
}

/// Whether the declaration holding token `first` owns its line's head:
/// the declaration's first token — the one after the previous `;` / `{` /
/// `}` walking back from `first` — has only horizontal whitespace between
/// the line start and itself. A statement or sibling sharing the line
/// keeps the rename, and a declaration opened on an earlier line (`int`
/// over `int\nx = 1;`) never owns this line's head.
pub(crate) fn declaration_owns_line_head(
    tokens: &TokenTable,
    text: &str,
    first: usize,
    line_start: usize,
) -> Option<bool> {
    let mut head = 0;
    for index in (0..first).rev() {
        if matches!(
            kind_of(tokens, index)?,
            TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly
        ) {
            head = index + 1;
            break;
        }
    }
    let decl_start = utf16_offset_to_byte(text, tokens.get(head)?.utf16_offset);
    Some(
        text.get(line_start..decl_start)
            .is_some_and(|lead| lead.trim_matches([' ', '\t']).is_empty()),
    )
}

/// Whether the declaration holding token `name_index` is one declarator
/// with a trivially-pure initializer — every token between the name and
/// the statement's depth-0 `;` is literal-shaped (numbers, plain strings,
/// `true`/`false`/`null`/`const`, the collection punctuation) — and the
/// `;`'s byte end when it is. A call, a closure, a name interpolation, or
/// a second declarator refuses.
pub(crate) fn pure_single_declarator(tokens: &TokenTable, name_index: usize) -> Option<usize> {
    let mut depth: u32 = 0;
    let mut index = name_index + 1;
    loop {
        let token = tokens.get(index)?;
        match token.kind {
            TokenKind::Semicolon if depth == 0 => return Some(token_end(token)),
            TokenKind::Comma if depth == 0 => return None,
            TokenKind::Eq if depth == 0 => {}
            TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String
            | TokenKind::Minus
            | TokenKind::Index
            | TokenKind::Colon
            | TokenKind::Comma
            | TokenKind::Keyword(Keyword::Null | Keyword::True | Keyword::False | Keyword::Const) =>
                {}
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => return None,
        }
        index += 1;
    }
}

/// `unused_shown_name`: the name leaves its `show` combinator — with its
/// following comma when it is not last, the preceding comma when it is,
/// and the whole `show` clause (leading whitespace with it) when it is
/// the only name.
pub(crate) fn drop_shown_name(text: &str, range: SourceRange) -> Option<(u32, u32, String)> {
    let (_, start, end) = word_slice(text, range)?;
    let (line_start, line_end) = line_bounds(text, start, end)?;
    let head = &text[line_start..start];
    let tail = &text[end..line_end];
    let head_t = head.trim_end_matches([' ', '\t']);
    let tail_t = tail.trim_start_matches([' ', '\t']);
    // Not the last name: `show A, B` loses `A, `.
    if let Some(rest) = tail_t.strip_prefix(',') {
        let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let cut = (tail.len() - tail_t.len()) + 1 + ws;
        return Some((
            range.offset,
            range.length + u32::try_from(cut).ok()?,
            String::new(),
        ));
    }
    // The last name but not the only one: `show B, A` loses `, A`.
    if head_t.ends_with(',') {
        let drop = (head.len() - head_t.len()) + 1;
        return Some((
            range.offset - u32::try_from(drop).ok()?,
            range.length + u32::try_from(drop).ok()?,
            String::new(),
        ));
    }
    // The only name: the whole `show` clause goes,
    // `import 'u' show A;` → `import 'u';`.
    if head_t.len() >= 4
        && head_t.ends_with("show")
        && !head_t[..head_t.len() - 4]
            .ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
    {
        let lead = (head.len() - head_t.len()) + 4;
        let before = &text[line_start..start - lead];
        let drop = lead + (before.len() - before.trim_end_matches([' ', '\t']).len());
        return Some((
            range.offset - u32::try_from(drop).ok()?,
            range.length + u32::try_from(drop).ok()?,
            String::new(),
        ));
    }
    None
}

/// `unused_catch_clause`: the `catch (e)` part of an `on T catch (e)`
/// leaves — the whitespace before the keyword, the keyword, its
/// parentheses, and the flanking whitespace (the code only fires without
/// a stack sibling, so nothing trails the name but `)`). An `on` type
/// must sit before the keyword — the shape the code reports.
pub(crate) fn drop_catch_clause(text: &str, range: SourceRange) -> Option<(u32, u32, String)> {
    let (_, start, end) = word_slice(text, range)?;
    let (line_start, line_end) = line_bounds(text, start, end)?;
    let head = &text[line_start..start];
    let head_t = head.trim_end_matches([' ', '\t']);
    let keyword = head.strip_suffix('(')?.trim_end_matches([' ', '\t']);
    if !keyword.ends_with("catch")
        || keyword[..keyword.len() - 5]
            .ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
        || !text[end..line_end]
            .trim_start_matches([' ', '\t'])
            .starts_with(')')
    {
        return None;
    }
    // The code only fires on `on T catch (e)`: an identifier tail — the
    // `on` type's last word — must sit before the keyword (a bare
    // `catch (e)` has none and is auto-used upstream anyway).
    let before = keyword[..keyword.len() - 5].trim_end();
    if !before.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
        return None;
    }
    let catch_start = line_start + head.rfind("catch")?;
    let lead = text[line_start..catch_start].len()
        - text[line_start..catch_start]
            .trim_end_matches([' ', '\t'])
            .len();
    let ws = text[end..line_end].len() - text[end..line_end].trim_start_matches([' ', '\t']).len();
    // The drop reaches from the whitespace before `catch` through `)`:
    // lead + `catch` + the gap to `(` + `(` + the gap to the name.
    let trailing = head.len() - head_t.len();
    let paren_gap = (head_t.len() - 1) - keyword.len();
    let drop = lead + 5 + paren_gap + 1 + trailing;
    Some((
        range.offset - u32::try_from(drop).ok()?,
        range.length + u32::try_from(drop + ws + 1).ok()?,
        String::new(),
    ))
}

/// `unused_catch_stack`: the stack-trace parameter leaves with its
/// preceding comma (`catch (e, s)` → `catch (e)`).
pub(crate) fn drop_catch_stack(text: &str, range: SourceRange) -> Option<(u32, u32, String)> {
    let (_, start, end) = word_slice(text, range)?;
    let (line_start, line_end) = line_bounds(text, start, end)?;
    let head = &text[line_start..start];
    let head_t = head.trim_end_matches([' ', '\t']);
    if !head_t.ends_with(',')
        || !text[end..line_end]
            .trim_start_matches([' ', '\t'])
            .starts_with(')')
    {
        return None;
    }
    let drop = (head.len() - head_t.len()) + 1;
    Some((
        range.offset - u32::try_from(drop).ok()?,
        range.length + u32::try_from(drop).ok()?,
        String::new(),
    ))
}

/// `unused_label`: the label declaration leaves with its colon and the
/// whitespace run after it — the code's own proof that no `break` or
/// `continue` targets the name.
pub(crate) fn drop_label(text: &str, range: SourceRange) -> Option<(u32, u32, String)> {
    let (_, start, end) = word_slice(text, range)?;
    let (line_start, line_end) = line_bounds(text, start, end)?;
    let head_t = text[line_start..start].trim_end_matches([' ', '\t']);
    if !head_t.is_empty() && !head_t.ends_with(':') {
        return None;
    }
    let tail = &text[end..line_end];
    let tail_t = tail.trim_start_matches([' ', '\t']);
    let rest = tail_t.strip_prefix(':')?;
    let cut =
        (tail.len() - tail_t.len()) + 1 + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
    Some((
        range.offset,
        range.length + u32::try_from(cut).ok()?,
        String::new(),
    ))
}
