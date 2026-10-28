use super::*;


/// The UTF-16 length of a string slice (the edits' currency).
pub(crate) fn utf16_count(slice: &str) -> u32 {
    u32::try_from(slice.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX)
}

/// The byte slice of `text` under the UTF-16 `range`.
pub(crate) fn slice16<'s>(text: &'s str, range: &core::ops::Range<u32>) -> Option<&'s str> {
    text.get(utf16_offset_to_byte(text, range.start)..utf16_offset_to_byte(text, range.end))
}

/// The leading (or trailing) run of `quote` bytes.
pub(crate) fn quote_run(bytes: &[u8], quote: u8, trailing: bool) -> usize {
    if trailing {
        bytes.iter().rev().take_while(|&&b| b == quote).count()
    } else {
        bytes.iter().take_while(|&&b| b == quote).count()
    }
}

// ---------------------------------------------------------------------------
// Semantic source: the engine's drop-family diagnostics
//
// Each rewriter expands the diagnostic's span into the mechanical edit
// and re-verifies the shape the code reports — an unverifiable hit drops,
// never patches on faith. Every drop stays within one line, and the
// whitespace arithmetic runs over ASCII-only regions (byte counts equal
// UTF-16 counts there).
// ---------------------------------------------------------------------------

/// The identifier shape the drop-family rewrites verify: a non-empty,
/// non-digit-led slice of Dart word characters. Returns the slice with its
/// byte span.
pub(crate) fn word_slice(text: &str, range: SourceRange) -> Option<(&str, usize, usize)> {
    let start = utf16_offset_to_byte(text, range.offset);
    let end = utf16_offset_to_byte(text, range.offset + range.length);
    let slice = text.get(start..end)?;
    word_shape(slice).then_some((slice, start, end))
}

/// The one-line bounds of the byte span `[start, end)`: `None` when the
/// span crosses a newline, else `(line_start, line_end)` with `line_end`
/// excluding the terminator.
pub(crate) fn line_bounds(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    if text.get(start..end)?.contains('\n') {
        return None;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    Some((line_start, line_end))
}

/// Cuts `[line_start, line_end)` plus its terminator (CRLF aware; the
/// last line has none) — the whole-line drop shape, in UTF-16.
pub(crate) fn cut_line(text: &str, line_start: usize, line_end: usize) -> (u32, u32) {
    let eol = if text[line_end..].starts_with("\r\n") {
        2
    } else {
        usize::from(text[line_end..].starts_with('\n'))
    };
    (
        utf16_count(&text[..line_start]),
        utf16_count(&text[line_start..line_end + eol]),
    )
}

// ---------------------------------------------------------------------------
// Token-verified rewrites
//
// The structural rewrites rebuild their replacement from the token
// stream: keyword and operator positions are quote-safe only there (a
// naive text scan would rewrite `'is'` or split on the `+` inside a
// string). Every rewriter re-verifies the shape its rule reports before
// composing — an unverifiable hit drops, never patches on faith.
// ---------------------------------------------------------------------------

/// The `[first, last]` token-index window of a hit range. Only real
/// tokens count — the scanner-recovery tail (the phantom bracket pair
/// behind the `[]` index token, `Eof`) is zero-width and unsorted, so a
/// binary search over the stream cannot close the window.
pub(crate) fn hit_tokens(tokens: &TokenTable, range: &core::ops::Range<u32>) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for index in 0..tokens.as_slice().len() {
        let token = tokens.get(index)?;
        if token.byte_len == 0 || token.utf16_offset < range.start {
            continue;
        }
        if token.utf16_offset >= range.end {
            break;
        }
        if first.is_none() {
            first = Some(index);
        }
        last = Some(index);
    }
    Some((first?, last?))
}

/// The UTF-16 `[offset, length)` covering tokens `first..=last`.
pub(crate) fn span_over(text: &str, tokens: &TokenTable, first: usize, last: usize) -> Option<(u32, u32)> {
    let start = tokens.get(first)?.utf16_offset;
    let end = tokens.get(last)?.utf16_offset
        + u32::try_from(tokens.lexeme(text, last).encode_utf16().count()).ok()?;
    Some((start, end.checked_sub(start)?))
}

pub(crate) fn kind_of(tokens: &TokenTable, index: usize) -> Option<TokenKind> {
    tokens.get(index).map(|token| token.kind)
}

/// The token's byte-end position.
pub(crate) fn token_end(token: &dartq_scanner::Token) -> usize {
    token.byte_offset as usize + token.byte_len as usize
}

/// A word-shaped token with the exact lexeme (plain identifiers and
/// pseudo keywords both qualify — the scanner's split between
/// `Identifier` and contextual `Keyword` is contextual).
pub(crate) fn word_at(tokens: &TokenTable, text: &str, index: usize, word: &str) -> bool {
    matches!(
        kind_of(tokens, index),
        Some(TokenKind::Identifier | TokenKind::Keyword(_))
    ) && tokens.lexeme(text, index) == word
}

pub(crate) fn is_null_token(tokens: &TokenTable, index: usize) -> bool {
    kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Null))
}

/// The first depth-0 token in `from..to` matching `pred`, over the `()`,
/// `[]`, `{}`, and `${}` nesting (`<` is not a group here — comparisons
/// and type arguments must not disturb the balance).
pub(crate) fn depth0_find(
    tokens: &TokenTable,
    text: &str,
    from: usize,
    to: usize,
    pred: impl Fn(TokenKind, &str) -> bool,
) -> Option<usize> {
    let mut depth = 0u32;
    for index in from..to {
        let token = tokens.get(index)?;
        match token.kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        if depth == 0 && pred(token.kind, tokens.lexeme(text, index)) {
            return Some(index);
        }
    }
    None
}

/// Whether the token before `!(…)` lets the inner parentheses go: the
/// official face drops them when the `!` expression's parent binds looser
/// than relational — delimiters and statement heads, the assignment and
/// conditional families, the logical and equality operators, `is`/`as` —
/// and keeps them behind tighter-binding parents (selectors, prefix
/// operators, the relational and multiplicative arms).
pub(crate) fn paren_removable(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Semicolon
        | TokenKind::OpenCurly
        | TokenKind::CloseCurly
        | TokenKind::OpenParen
        | TokenKind::OpenSquare
        | TokenKind::Comma
        | TokenKind::Colon
        | TokenKind::Question
        | TokenKind::Eq
        | TokenKind::PlusEq
        | TokenKind::MinusEq
        | TokenKind::StarEq
        | TokenKind::SlashEq
        | TokenKind::PercentEq
        | TokenKind::TildeSlashEq
        | TokenKind::LtLtEq
        | TokenKind::GtGtEq
        | TokenKind::GtGtGtEq
        | TokenKind::AmpersandEq
        | TokenKind::BarEq
        | TokenKind::CaretEq
        | TokenKind::QuestionQuestionEq
        | TokenKind::Function
        | TokenKind::AmpersandAmpersand
        | TokenKind::BarBar
        | TokenKind::QuestionQuestion
        | TokenKind::EqEq
        | TokenKind::BangEq => true,
        TokenKind::Keyword(kw) => matches!(
            kw,
            Keyword::Return
                | Keyword::Throw
                | Keyword::Yield
                | Keyword::Case
                | Keyword::In
                | Keyword::Else
                | Keyword::Do
                | Keyword::Is
                | Keyword::As
        ),
        _ => false,
    }
}

/// The trimmed source text of a token range (an empty range is the empty
/// string).
pub(crate) fn token_text(tokens: &TokenTable, text: &str, range: core::ops::Range<usize>) -> Option<String> {
    if range.is_empty() {
        return Some(String::new());
    }
    let (start, length) = span_over(text, tokens, range.start, range.end - 1)?;
    text.get(utf16_offset_to_byte(text, start)..utf16_offset_to_byte(text, start + length))
        .map(str::trim)
        .map(str::to_owned)
}

/// Whether the token ranges hold the same lexeme sequence.
pub(crate) fn same_tokens(
    tokens: &TokenTable,
    text: &str,
    a: core::ops::Range<usize>,
    b: core::ops::Range<usize>,
) -> bool {
    a.len() == b.len()
        && !a.is_empty()
        && a.enumerate()
            .all(|(i, index)| tokens.lexeme(text, index) == tokens.lexeme(text, b.start + i))
}

/// The value of the int literal at `index` (optionally negated by the
/// preceding `-`): decimal/hex with digit separators, the lint's own
/// `getIntValue` face.
pub(crate) fn int_literal_value(tokens: &TokenTable, text: &str, index: usize) -> Option<i64> {
    let (negative, at) = if kind_of(tokens, index) == Some(TokenKind::Minus) {
        (true, index + 1)
    } else {
        (false, index)
    };
    if kind_of(tokens, at) != Some(TokenKind::Int) {
        return None;
    }
    let lexeme = tokens.lexeme(text, at);
    let hex = lexeme.starts_with("0x") || lexeme.starts_with("0X");
    let digits: String = lexeme
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|&ch| ch != '_')
        .collect();
    i64::from_str_radix(&digits, if hex { 16 } else { 10 })
        .ok()
        .map(|value| if negative { -value } else { value })
}

/// The comparison operator read with its operands swapped (`a < b` ≡
/// `b > a`): the left-literal face canonicalizes to the matrix's layout.
pub(crate) fn mirror_op(op: &str) -> &str {
    match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        other => other,
    }
}

/// The `[` or `(` opener of the closing `]`/`)` at `close`, walking back
/// over the balanced interior — the reverse of [`TokenTable::matching_end`].
pub(crate) fn matching_begin(tokens: &TokenTable, close: usize) -> Option<usize> {
    let open = match kind_of(tokens, close)? {
        TokenKind::CloseParen => TokenKind::OpenParen,
        TokenKind::CloseSquare => TokenKind::OpenSquare,
        TokenKind::CloseCurly => TokenKind::OpenCurly,
        _ => return None,
    };
    let mut depth = 0u32;
    let mut cursor = close;
    loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            kind if kind == open && depth == 0 => return Some(cursor),
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
    }
}

/// The end token index of the statement starting at `start` — the
/// structural walk behind the control-body wrap (the official producers
/// read the AST statement node; the fix row carries the head token, so
/// the statement shape is rebuilt from the token stream). Structured
/// heads recurse into their arms; everything else ends at its first
/// depth-0 `;`. Every step consumes tokens, so the walk is bounded by the
/// stream; a shape it cannot read drops (`None`).
pub(crate) fn statement_end(tokens: &TokenTable, text: &str, start: usize) -> Option<usize> {
    match kind_of(tokens, start)? {
        TokenKind::Keyword(Keyword::If) => {
            let rparen = skip_group(tokens, start + 1, TokenKind::OpenParen)?;
            let then_end = statement_end(tokens, text, rparen + 1)?;
            if word_at(tokens, text, then_end + 1, "else") {
                statement_end(tokens, text, then_end + 2)
            } else {
                Some(then_end)
            }
        }
        TokenKind::Keyword(Keyword::While | Keyword::For) => {
            let rparen = skip_group(tokens, start + 1, TokenKind::OpenParen)?;
            statement_end(tokens, text, rparen + 1)
        }
        TokenKind::Keyword(Keyword::Do) => {
            let body_end = statement_end(tokens, text, start + 1)?;
            if !word_at(tokens, text, body_end + 1, "while") {
                return None;
            }
            let rparen = skip_group(tokens, body_end + 2, TokenKind::OpenParen)?;
            match kind_of(tokens, rparen + 1)? {
                TokenKind::Semicolon => Some(rparen + 1),
                _ => None,
            }
        }
        TokenKind::Keyword(Keyword::Switch) => {
            let rparen = skip_group(tokens, start + 1, TokenKind::OpenParen)?;
            skip_group(tokens, rparen + 1, TokenKind::OpenCurly)
        }
        TokenKind::Keyword(Keyword::Try) => {
            let mut end = skip_group(tokens, start + 1, TokenKind::OpenCurly)?;
            loop {
                let next = end + 1;
                if word_at(tokens, text, next, "on") || word_at(tokens, text, next, "catch") {
                    let mut cursor = next + 1;
                    if kind_of(tokens, cursor)? == TokenKind::OpenParen {
                        cursor = tokens.matching_end(cursor)? + 1;
                    }
                    end = skip_group(tokens, cursor, TokenKind::OpenCurly)?;
                } else if kind_of(tokens, next)? == TokenKind::Keyword(Keyword::Finally) {
                    end = skip_group(tokens, next + 1, TokenKind::OpenCurly)?;
                } else {
                    return Some(end);
                }
            }
        }
        TokenKind::OpenCurly => tokens.matching_end(start),
        // Expression, declaration, return, break, label — the statement
        // ends at its first depth-0 `;`.
        _ => {
            let mut depth = 0u32;
            let mut cursor = start;
            loop {
                match kind_of(tokens, cursor)? {
                    TokenKind::OpenParen
                    | TokenKind::OpenSquare
                    | TokenKind::OpenCurly
                    | TokenKind::StringInterpolationExpression => depth += 1,
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth = depth.checked_sub(1)?;
                    }
                    TokenKind::Semicolon if depth == 0 => return Some(cursor),
                    _ => {}
                }
                cursor += 1;
            }
        }
    }
}

/// The matching closer of the group opener at `at` (the opener kind is
/// verified, not assumed).
pub(crate) fn skip_group(tokens: &TokenTable, at: usize, open: TokenKind) -> Option<usize> {
    if kind_of(tokens, at)? != open {
        return None;
    }
    tokens.matching_end(at)
}

/// The `on` keyword ending an `on Type` face whose type run closes just
/// before `cursor` — the generics and the dotted identifier name walk
/// back; `None` when the run does not open at `on`.
pub(crate) fn on_type_start(tokens: &TokenTable, text: &str, mut cursor: usize) -> Option<usize> {
    if matches!(
        kind_of(tokens, cursor)?,
        TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
    ) {
        let mut depth = 0u32;
        loop {
            match kind_of(tokens, cursor)? {
                TokenKind::Gt => depth += 1,
                TokenKind::GtGt => depth += 2,
                TokenKind::GtGtGt => depth += 3,
                TokenKind::Lt => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            cursor = cursor.checked_sub(1)?;
        }
    }
    loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::Identifier | TokenKind::Period => {}
            _ => break,
        }
    }
    word_at(tokens, text, cursor, "on").then_some(cursor)
}

/// `annotate_overrides` / `annotate_redeclares`: `@override` / `@redeclare`
/// lands on its own line before the member's first attached line — the
/// official `AddOverride` / `AddRedeclare` replacement at the member's
/// begin token, doc comments included. The member block is walked up over
/// its annotation and doc-comment lines (plain `//` trailing comments
/// belong to the previous member and stop the walk).
/// The start of the line before `byte`, read past the byte's own line
/// terminator — the walk-back read the member-head helpers share (`byte`
/// sits just past a line's `\n`, and searching `text[..byte]` unguarded
/// would find that terminator and strand the walk on an empty line).
pub(crate) fn line_start_before(text: &str, byte: usize) -> usize {
    let mut bounded = byte;
    if bounded > 0 && text.as_bytes()[bounded - 1] == b'\n' {
        bounded -= 1;
        if bounded > 0 && text.as_bytes()[bounded - 1] == b'\r' {
            bounded -= 1;
        }
    }
    text[..bounded].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line's leading whitespace at `byte`.
pub(crate) fn line_indent_end(text: &str, byte: usize) -> usize {
    byte + text[byte..].len() - text[byte..].trim_start_matches([' ', '\t']).len()
}

/// The whole-line cut of `[start, end)` — every line the span touches
/// loses its content and terminator (the official `getLinesRange`
/// deletion face for multi-line constructs).
pub(crate) fn drop_lines_range(text: &str, start16: u32, end16: u32) -> Option<(u32, u32)> {
    let start = utf16_offset_to_byte(text, start16);
    let end = utf16_offset_to_byte(text, end16);
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    Some(cut_line(text, line_start, line_end))
}

/// The start of the member's attached line block: the reported span's line
/// walked up over annotation and doc-comment lines (the official lines
/// range covers the declaration's begin token, docs included). Lines
/// inside a `/* … */` block comment ride too — a continuation line needn't
/// open with `*` (the walk tracks an unclosed `/*` region so a
/// `/*\n  text\n*/` comment moves whole).
pub(crate) fn member_block_start(text: &str, start16: u32) -> usize {
    let start = utf16_offset_to_byte(text, start16);
    let mut block_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    // `in_block` marks accepted lines already inside a `/*` region — the
    // next failure must be the comment's `/*` head line.
    let mut in_block = false;
    loop {
        let head_end = text[..block_start]
            .trim_end_matches([' ', '\t', '\r'])
            .len();
        let line_start = line_start_before(text, head_end);
        let trimmed = text[line_start..head_end].trim_start();
        if trimmed.starts_with('@')
            || trimmed.starts_with("///")
            || trimmed.starts_with("/**")
            || trimmed.starts_with('*')
        {
            block_start = line_start;
            continue;
        }
        if in_block {
            // Mid-comment walk: the `/*` head line ends the region; lines
            // above it (`///` docs, `@` annotations) keep walking.
            if trimmed.starts_with("/*") {
                block_start = line_start;
                in_block = false;
                continue;
            }
            return block_start;
        }
        // A line inside an unclosed `/*` region rides the comment — the
        // source before it holds a `/*` with no matching `*/` yet.
        let before = &text[..line_start];
        let opens = before.rfind("/*");
        let closes = before.rfind("*/");
        if opens.is_some_and(|o| closes.is_none_or(|c| c < o)) {
            block_start = line_start;
            in_block = true;
            continue;
        }
        return block_start;
    }
}

/// The first token at or after the UTF-16 `offset` — the width-0 report
/// positions (`start..start` at a name) resolve through here.
pub(crate) fn token_at(tokens: &TokenTable, offset: u32) -> Option<usize> {
    let mut lo = 0usize;
    let mut hi = tokens.as_slice().len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if tokens.get(mid)?.utf16_offset < offset {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    (lo < tokens.as_slice().len()).then_some(lo)
}

/// Whether the token's lexeme opens like an identifier (words, including
/// contextual keywords and type names).
pub(crate) fn wordish(tokens: &TokenTable, text: &str, index: usize) -> bool {
    tokens
        .lexeme(text, index)
        .chars()
        .next()
        .is_some_and(is_identifier_char)
}

/// Whether the parameter segment at `[start, end)` reads as a typed one —
/// more than one token (type + name), or a function-typed suffix the
/// official `_allParametersHaveTypes` exempts. A single bare word refuses
/// (the old-form name-only parameter the conversion cannot express).
pub(crate) fn parameter_typed(tokens: &TokenTable, text: &str, start: usize, end: usize) -> Option<bool> {
    if start >= end {
        return None;
    }
    let mut depth = 0u32;
    let mut group = false;
    let mut count = 0usize;
    for index in start..end {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                depth += 1;
                if depth == 1 && kind_of(tokens, index) == Some(TokenKind::OpenParen) {
                    group = true;
                }
            }
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ if depth == 0 => count += 1,
            _ => {}
        }
    }
    Some(group || count > 1 || count == 1 && !wordish(tokens, text, start))
}

/// The group depth reached at `index`, scanning forward from `from` —
/// the comma classifier the parameter gate splits on.
pub(crate) fn depth_of(tokens: &TokenTable, from: usize, index: usize) -> Option<u32> {
    let mut depth = 0u32;
    for at in from..index {
        match kind_of(tokens, at)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
    }
    Some(depth)
}

// ---------------------------------------------------------------------------
// Fix source: the engine's parser diagnostics
//
// The rewriters behind the table's `FixSource::Parser` rows (module docs).
// Each mirrors one official producer's edit shape (upstream Dart SDK
// source, pkg/analysis_server/lib/src/services/correction/dart/*); the
// official producers read the resolved AST node under the report, these
// rebuild the same shape from the report span and the token stream, and
// drop where the shape does not verify. Parser reports are token spans:
// the hit token is the token starting at the report offset.
// ---------------------------------------------------------------------------

/// The official `RemoveLexeme` deletion: the whole word `word` at the hit
/// start plus exactly one following UTF-16 unit — `[offset, offset +
/// word.length + 1)`, whatever the extra unit is (the producer deletes it
/// blind; at the end of the file only the word goes).
pub(crate) fn remove_lexeme(text: &str, offset16: u32, word: &str) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(word)?;
    if rest.starts_with(is_identifier_char) {
        return None;
    }
    let extra = rest.chars().next().map_or(0, char::len_utf16);
    let length = u32::try_from(word.len() + extra).ok()?;
    Some((offset16, length, String::new()))
}

/// The official `RemoveKeyword` deletion: the whole word `word` at the hit
/// start plus the whitespace run to the next token (newlines included); a
/// comment ends the run, as the producer stops at the next token's
/// preceding comment.
pub(crate) fn remove_keyword(text: &str, offset16: u32, word: &str) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(word)?;
    if rest.starts_with(is_identifier_char) {
        return None;
    }
    let ws = rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len();
    let length = u32::try_from(word.len() + ws).ok()?;
    Some((offset16, length, String::new()))
}

/// The `startStart(keyword, keyword.next!)` deletion of the
/// `RemoveConst`/`RemoveLate`/`RemoveUnnecessaryConst` producers: the
/// whole word `word` at the hit start plus the trivia run to the next
/// real token — whitespace AND comments (comments ride the next token's
/// `precedingComments` edge, inside the span).
pub(crate) fn remove_keyword_across_comments(
    text: &str,
    offset16: u32,
    word: &str,
) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(word)?;
    if rest.starts_with(is_identifier_char) {
        return None;
    }
    let run = trivia_run(rest);
    // The run counts bytes; the edit is UTF-16 — a comment in the run
    // may carry multi-byte characters, so the run converts before it
    // adds (the keyword itself is ASCII).
    let run16 = utf16_count(rest.get(..run)?);
    // The keyword itself is ASCII — its bytes are its UTF-16 units; the
    // checked add keeps the pair in the u32 edit-length domain.
    let length = u32::try_from(word.len()).ok()?.checked_add(run16)?;
    Some((offset16, length, String::new()))
}

/// The byte length of the trivia run heading `rest` — whitespace and
/// comments alternating to the next real token (a `//` comment ends at
/// its newline; `/* */` comments nest; an unterminated comment runs to
/// the end of the file).
pub(crate) fn trivia_run(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let mut at = 0usize;
    loop {
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t' | b'\r' | b'\n') {
            at += 1;
        }
        if bytes[at..].starts_with(b"//") {
            match bytes[at..].iter().position(|&b| b == b'\n') {
                Some(i) => at += i + 1,
                None => return bytes.len(),
            }
        } else if bytes[at..].starts_with(b"/*") {
            let mut depth = 1u32;
            let mut i = at + 2;
            while depth > 0 && i + 1 < bytes.len() {
                if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return bytes.len();
            }
            at = i;
        } else {
            return at;
        }
    }
}

/// An ASCII identifier character — the boundary test behind the
/// whole-word checks.
pub(crate) fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// The real token starting exactly at the UTF-16 `offset` — the parser
/// report's hit token (the zero-width recovery tail never qualifies).
pub(crate) fn token_starting_at(tokens: &TokenTable, offset: u32) -> Option<usize> {
    tokens
        .as_slice()
        .iter()
        .position(|token| token.byte_len > 0 && token.utf16_offset == offset)
}

/// The UTF-16 end offset of token `index`.
pub(crate) fn utf16_end(tokens: &TokenTable, text: &str, index: usize) -> Option<u32> {
    let token = tokens.get(index)?;
    Some(token.utf16_offset + utf16_count(tokens.lexeme(text, index)))
}

/// The `<` opening the type-argument group whose closer is at `close`
/// (`>`, `>>`, `>>>` all close; the nesting is counted by `>` units).
pub(crate) fn angle_group_start(tokens: &TokenTable, close: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut cursor = close;
    loop {
        match kind_of(tokens, cursor)? {
            TokenKind::Gt => depth += 1,
            TokenKind::GtGt => depth += 2,
            TokenKind::GtGtGt => depth += 3,
            TokenKind::Lt => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            TokenKind::Identifier | TokenKind::Period | TokenKind::Comma | TokenKind::Question => {}
            TokenKind::Keyword(Keyword::Void | Keyword::Dynamic) => {}
            _ => return None,
        }
        cursor = cursor.checked_sub(1)?;
    }
}

/// The `>`-family closer of the type-argument group opening at `open`
/// (`<`); `None` when the group does not close as plain type arguments.
pub(crate) fn angle_group_end(tokens: &TokenTable, open: usize) -> Option<usize> {
    if kind_of(tokens, open)? != TokenKind::Lt {
        return None;
    }
    let mut depth = 0i32;
    let mut cursor = open;
    loop {
        match kind_of(tokens, cursor)? {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt => depth -= 1,
            TokenKind::GtGt => depth -= 2,
            TokenKind::GtGtGt => depth -= 3,
            TokenKind::Identifier | TokenKind::Period | TokenKind::Comma | TokenKind::Question => {}
            TokenKind::Keyword(Keyword::Void | Keyword::Dynamic) => {}
            _ => return None,
        }
        if depth <= 0 {
            return (depth == 0).then_some(cursor);
        }
        cursor += 1;
    }
}

/// The first depth-0 token at or after `from` for which `pred` holds, over
/// the `()`, `[]`, `{}` nesting — unlike [`depth0_find`], an opener itself
/// is offered to `pred` at the depth it stands on, so a body `{` is found
/// rather than entered.
pub(crate) fn depth0_scan(
    tokens: &TokenTable,
    from: usize,
    pred: impl Fn(TokenKind) -> bool,
) -> Option<usize> {
    let mut depth = 0u32;
    let mut cursor = from;
    loop {
        let kind = kind_of(tokens, cursor)?;
        if kind == TokenKind::Eof {
            return None;
        }
        if depth == 0 && pred(kind) {
            return Some(cursor);
        }
        match kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        cursor += 1;
    }
}

/// The 0-based line of a byte offset.
pub(crate) fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&byte| byte == b'\n').count()
}

/// The comment ride of the official `SortChildPropertyLast`: comments
/// starting on `line` between `from_byte` and `to_byte` carry forward past
/// their end; the first comment on a later line stops the walk. Returns the
/// carried byte end.
pub(crate) fn ride_line_comments(text: &str, from_byte: usize, to_byte: usize, line: usize) -> usize {
    let bytes = text.as_bytes();
    let mut at = from_byte;
    let mut carried = from_byte;
    while at < to_byte {
        while at < to_byte && matches!(bytes[at], b' ' | b'\t' | b'\r') {
            at += 1;
        }
        if at >= to_byte {
            break;
        }
        let comment_end = if bytes[at..].starts_with(b"//") {
            text[at..to_byte].find('\n').map_or(to_byte, |i| at + i)
        } else if bytes[at..].starts_with(b"/*") {
            match text[at..to_byte].find("*/") {
                Some(i) => at + i + 2,
                None => break,
            }
        } else {
            break;
        };
        // A comment starting on a later line stops the ride.
        if line_of(text, at) != line {
            break;
        }
        carried = comment_end;
        at = comment_end;
    }
    carried
}
