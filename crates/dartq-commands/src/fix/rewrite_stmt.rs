use super::*;


// ---------------------------------------------------------------------------
// Semantic source: the cross-file create-file family
//
// The one face whose official change targets a file other than the hit
// file: the missing URI target is created, not patched in place. The
// producer rebuilds the official `CreateFile` shape (upstream Dart SDK
// source, pkg/analysis_server/lib/src/services/correction/dart/
// create_file.dart) from the row's own evidence and drops where the
// evidence cannot prove it — an unprovable hit never invents a file.
// ---------------------------------------------------------------------------

/// `uri_does_not_exist`'s created-file edit: the missing target of an
/// `import`/`export` is born with the upstream TODO stub, the missing
/// target of a `part` with the `part of '<library>';` head — inserted at
/// offset 0 of the target's empty text. The evidence must prove the face:
/// the span slices to the directive's single plain string literal whose
/// inner text the message quotes (`node is SimpleStringLiteral`), the
/// directive keyword stands directly before it (`NamespaceDirective` /
/// `PartDirective` are the official parent dispatch's two arms), and the
/// URI is a relative `.dart` path the hit file's directory can name. The
/// resolver's-world faces drop: `package:`/`dart:` and every other scheme
/// URI, `part of` (`PartOfDirective`), conditional-import arms, escaped
/// and adjacent-string literals.
pub(crate) fn create_missing_uri_target(input: &FixInput<'_>) -> Option<(String, String)> {
    let tokens = input.tokens?;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    // One plain literal: a concatenation or interpolation span is never
    // the `SimpleStringLiteral` the official producer guards on.
    if first != last {
        return None;
    }
    // The directive keyword stands directly before the URI — trivia never
    // reaches the token stream, and directives carry no tokens between
    // keyword and URI.
    match kind_of(tokens, first.checked_sub(1)?)? {
        TokenKind::Keyword(Keyword::Import)
        | TokenKind::Keyword(Keyword::Export)
        | TokenKind::Keyword(Keyword::Part) => {}
        _ => return None,
    }
    // The quoted URI must be the literal's inner text — the plain-literal
    // proof, and the decoded-URI read in one comparison (an escaped or
    // interpolated literal decodes to something else and drops).
    let uri = quoted_uri(input.text, input.range.clone(), input.message)?;
    // A relative `.dart` path only: scheme URIs resolve through the
    // resolver's package and SDK world the row's evidence does not carry
    // (the known-diff face), and `isDart` is the official gate on the
    // target the change would create.
    if uri.contains(':') || !uri.ends_with(".dart") {
        return None;
    }
    let dir = input.path.parent()?;
    let target = dartq_pub::normalize(&dir.join(uri));
    let content = match kind_of(tokens, first - 1)? {
        // The official stub, verbatim — no trailing newline.
        TokenKind::Keyword(Keyword::Import | Keyword::Export) => {
            "// TODO Implement this library.".to_owned()
        }
        // `part of '<library-relative-to-the-part-file's-dir>';` + the
        // builder's blank-line tail — URIs carry `/` on every platform.
        TokenKind::Keyword(Keyword::Part) => {
            let library = dartq_pub::normalize(input.path);
            let from = target.parent()?;
            format!("part of '{}';\n\n", relative_uri(from, &library)?)
        }
        _ => return None,
    };
    Some((target.display().to_string(), content))
}

/// The URI the `uri_does_not_exist` message quotes —
/// `Target of URI doesn't exist: '<uri>'.` — accepted only when it equals
/// the hit span's inner text (a single quoted literal, quotes stripped).
pub(crate) fn quoted_uri<'a>(text: &'a str, range: core::ops::Range<u32>, message: &str) -> Option<&'a str> {
    let rest = message.strip_suffix("'.")?;
    let quoted = &rest[rest.rfind('\'')? + 1..];
    let literal = &text[utf16_offset_to_byte(text, range.start)..utf16_offset_to_byte(text, range.end)];
    let quote = literal.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = literal.strip_prefix(quote)?.strip_suffix(quote)?;
    (inner == quoted && !inner.is_empty()).then_some(inner)
}

/// `to_file` relative to `from_dir`, `/`-separated — the official
/// `pathContext.relative(...)` + `split/join('/')` shape. Both sides
/// share the run's path spelling, so the component walk is exact.
pub(crate) fn relative_uri(from_dir: &Path, to_file: &Path) -> Option<String> {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to_file.components().collect();
    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let (ups, downs) = (&from[shared..], &to[shared..]);
    let mut out = String::new();
    for _ in ups {
        out.push_str("../");
    }
    for (index, component) in downs.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        out.push_str(component.as_os_str().to_str()?);
    }
    (!out.is_empty()).then_some(out)
}



/// Whether the slice is an identifier shape: non-empty, not digit-led, all
/// Dart word characters (the drop-family rewrites' name proof).
pub(crate) fn word_shape(slice: &str) -> bool {
    let Some(first) = slice.chars().next() else {
        return false;
    };
    !first.is_ascii_digit()
        && slice
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `empty_constructor_bodies`: the reported empty block is replaced with
/// `;` (the official `RemoveEmptyConstructorBody` replaces the body's
/// lines range with the semicolon, so `;` lands next to the parameter
/// list). The horizontal whitespace before `{` goes with it; a `{` that
/// opens its own line keeps the rest of that line (a trailing comment
/// survives where the official lines-range face would take it — a
/// cosmetic divergence left to the formatter). The `{…}` wrapper over an
/// empty interior is verified first — a comment inside is not this
/// rule's hit.
pub(crate) fn semicolon_body(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let range = &input.range;
    let inner = slice16(input.text, range)?
        .strip_prefix('{')?
        .strip_suffix('}')?;
    if !inner.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
        return None;
    }
    let start = utf16_offset_to_byte(input.text, range.start);
    let head = input.text.get(..start)?;
    let ws = head.len() - head.trim_end_matches([' ', '\t']).len();
    let lead = u32::try_from(ws).ok()?;
    Some((
        range.start - lead,
        range.end - range.start + lead,
        ";".to_owned(),
    ))
}

/// `empty_statements`: two official faces. Alone on its line (the textual
/// shape of the official `RemoveEmptyStatement`'s `getLinesRange`
/// deletion) the whole line with its terminator goes; sharing a line with
/// other code, only the reported `;` goes — the lines range would take
/// the neighbors with it. A dangling construct body (`while (x) ;` — the
/// official `ReplaceWithBrackets` face, `parent is! Block`) becomes ` {}`
/// folded over the gap from the previous token, re-targeting the
/// construct at an empty block instead of the next statement.
pub(crate) fn drop_empty_statement(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    replace_exact(input.text, &input.range, ";", "")?;
    let tokens = input.tokens?;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last {
        return None;
    }
    let start = utf16_offset_to_byte(input.text, input.range.start);
    let end = utf16_offset_to_byte(input.text, input.range.end);
    let line_start = input.text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_rest = input.text[end..].split(['\n', '\r']).next()?;
    if input.text[line_start..start]
        .trim_matches([' ', '\t'])
        .is_empty()
        && line_rest.trim_matches([' ', '\t']).is_empty()
    {
        // The cut carries the line's own terminator (the lines range'
        // end sits past the `\r\n` pair).
        let mut cut_end = end + line_rest.len();
        let rest_bytes = input.text.as_bytes();
        if rest_bytes.get(cut_end) == Some(&b'\r') {
            cut_end += 1;
        }
        if rest_bytes.get(cut_end) == Some(&b'\n') {
            cut_end += 1;
        }
        return Some((
            utf16_count(&input.text[..line_start]),
            utf16_count(&input.text[line_start..cut_end]),
            String::new(),
        ));
    }
    let head = input
        .text
        .get(..start)?
        .trim_end_matches([' ', '\t', '\r', '\n']);
    if head.ends_with(['{', '}', ';']) {
        return Some((
            input.range.start,
            input.range.end - input.range.start,
            String::new(),
        ));
    }
    // The brackets face: ` {}` replaces the gap plus the semicolon.
    let prev_end = token_end(tokens.get(first.checked_sub(1)?)?);
    Some((
        utf16_count(&input.text[..prev_end]),
        utf16_count(&input.text[prev_end..end]),
        " {}".to_owned(),
    ))
}

/// `use_rethrow_when_possible`: the reported `throw e` span is replaced
/// with `rethrow` (the official `UseRethrow` replaces the throw
/// expression). The `throw`-plus-one-identifier shape is verified first.
pub(crate) fn rethrow_span(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let range = &input.range;
    let rest = slice16(input.text, range)?.strip_prefix("throw")?;
    if !rest.starts_with([' ', '\t']) || !word_shape(rest.trim_matches([' ', '\t'])) {
        return None;
    }
    Some((range.start, range.end - range.start, "rethrow".to_owned()))
}

/// `prefer_is_not_empty`: the negated emptiness test loses its `!` and the
/// member gains the `Not` — `!x.isEmpty` → `x.isNotEmpty` (the official
/// `UseIsNotEmpty` deletes the operator and renames the property). The
/// `!…isEmpty` shape with the receiver's `.` before the name is verified
/// first.
pub(crate) fn is_not_empty_access(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let range = &input.range;
    let receiver = slice16(input.text, range)?
        .strip_prefix('!')?
        .strip_suffix("isEmpty")?;
    if !receiver.ends_with('.') {
        return None;
    }
    Some((
        range.start,
        range.end - range.start,
        format!("{receiver}isNotEmpty"),
    ))
}

/// `avoid_single_cascade_in_expression_statements`: the single section's
/// cascade operator becomes its `.`-equivalent (`..` → `.`, `?..` → `?.`;
/// the official `ReplaceCascadeWithDot` replaces the section's operator
/// token) — an index section keeps only the operator's null-aware prefix
/// (`..[0]` → `[0]`, `?..[0]` → `?[0]`, the official `_indexReplacement`).
/// The span's depth-0 cascade operator is the rewrite target — a nested
/// cascade inside the target expression never matches.
pub(crate) fn cascade_with_dot(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let mut depth = 0u32;
    let mut operator = None;
    for index in first..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod if depth == 0 => {
                operator = Some(index);
                break;
            }
            _ => {}
        }
    }
    let operator = operator?;
    let index_section = kind_of(tokens, operator + 1) == Some(TokenKind::OpenSquare);
    let dot = match (kind_of(tokens, operator)?, index_section) {
        (TokenKind::QuestionPeriodPeriod, true) => "?",
        (TokenKind::QuestionPeriodPeriod, false) => "?.",
        (_, true) => "",
        (_, false) => ".",
    };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    let start = utf16_offset_to_byte(text, start16);
    let end = utf16_offset_to_byte(text, start16 + length16);
    let op_start = utf16_offset_to_byte(text, tokens.get(operator)?.utf16_offset);
    let op_end = token_end(tokens.get(operator)?);
    let mut replacement = String::with_capacity(end - start);
    replacement.push_str(text.get(start..op_start)?);
    replacement.push_str(dot);
    replacement.push_str(text.get(op_end..end)?);
    Some((start16, length16, replacement))
}

/// `unnecessary_to_list_in_spreads`: the reported `toList` name expands
/// back to its invocation and the whole `.toList(…)` leaves the spread
/// (the official `RemoveToList` deletes operator..argumentList). The
/// name, its `.` operator, and the argument list are verified against the
/// token stream; a null-aware `?.toList()` carries no `.` operator and
/// drops.
pub(crate) fn remove_to_list(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last || !word_at(tokens, text, first, "toList") {
        return None;
    }
    let period = first.checked_sub(1)?;
    if kind_of(tokens, period)? != TokenKind::Period {
        return None;
    }
    let mut cursor = first + 1;
    if kind_of(tokens, cursor)? == TokenKind::Lt {
        cursor = tokens.matching_end(cursor)? + 1;
    }
    if kind_of(tokens, cursor)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(cursor)?;
    let (start16, length16) = span_over(text, tokens, period, rparen)?;
    Some((start16, length16, String::new()))
}

/// `prefer_is_empty`: the `length` comparison rewrites to the emptiness
/// getter (the official `ReplaceWithIsEmpty` matrix). The int literal
/// sits on one side of the depth-0 comparison, the `length` access on the
/// other; the replacement keeps everything through the access's period
/// (the official face keeps the receiver verbatim). The always-true/false
/// judgments the lint reports without a correction get no edit, exactly
/// as the official producer returns without one.
pub(crate) fn is_empty_call(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let op = depth0_find(tokens, text, first, last + 1, |kind, _| {
        matches!(
            kind,
            TokenKind::EqEq
                | TokenKind::BangEq
                | TokenKind::Lt
                | TokenKind::LtEq
                | TokenKind::Gt
                | TokenKind::GtEq
        )
    })?;
    // The literal run is exactly the literal (its leading `-` included);
    // the right-literal layout reads first, the official producer's order.
    let literal_run = |run: core::ops::Range<usize>| -> Option<i64> {
        let negative = kind_of(tokens, run.start) == Some(TokenKind::Minus);
        if run.len() != usize::from(negative) + 1 {
            return None;
        }
        int_literal_value(tokens, text, run.start)
    };
    let (value, access, literal_on_right) = if let Some(value) = literal_run(op + 1..last + 1) {
        (value, first..op, true)
    } else if let Some(value) = literal_run(first..op) {
        (value, op + 1..last + 1, false)
    } else {
        return None;
    };
    // The access ends `… . length` — a null-aware `?.` access has no
    // provable emptiness getter and drops.
    let length = access.end - 1;
    if length == access.start
        || !word_at(tokens, text, length, "length")
        || kind_of(tokens, length.checked_sub(1)?)? != TokenKind::Period
    {
        return None;
    }
    let getter = match (value, literal_on_right, tokens.lexeme(text, op)) {
        (0, true, "==" | "<=") | (1, true, "<") => "isEmpty",
        (0, true, ">" | "!=") | (1, true, ">=") => "isNotEmpty",
        (0, false, "==" | ">=") | (1, false, ">") => "isEmpty",
        (0, false, "<" | "!=") | (1, false, "<=") => "isNotEmpty",
        _ => return None,
    };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    let access_start = utf16_offset_to_byte(text, tokens.get(access.start)?.utf16_offset);
    let length_start = utf16_offset_to_byte(text, tokens.get(length)?.utf16_offset);
    let receiver = text.get(access_start..length_start)?;
    Some((start16, length16, format!("{receiver}{getter}")))
}

/// Whether the token run is an identifier chain — `a`, `a.b`, `a.b.c` —
/// the assignable-target shape the conditional-assignment rewrite keeps
/// (`this`/`super` receivers, index and invocation targets are refused).
pub(crate) fn is_identifier_chain(tokens: &TokenTable, run: core::ops::Range<usize>) -> bool {
    !run.is_empty()
        && run.len() % 2 == 1
        && (0..run.len()).all(|offset| {
            let kind = kind_of(tokens, run.start + offset);
            if offset % 2 == 0 {
                matches!(kind, Some(TokenKind::Identifier))
            } else {
                kind == Some(TokenKind::Period)
            }
        })
}

/// `prefer_conditional_assignment`: the null-check `if` becomes the
/// null-aware assignment (`if (a == null) a = b;` → `a ??= b;` — the
/// official `ReplaceWithConditionalAssignment` replaces the whole
/// statement). The condition's depth-0 `==` splits the checked operand
/// from the `null` literal (either order), and the then arm is one
/// `operand = value` statement, bare or single-statement block; the
/// operand must be an identifier chain matching the assignment's target.
/// A dangling `else` refuses the bare arm — the rewrite never absorbs it.
pub(crate) fn conditional_assignment(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if !word_at(tokens, text, first, "if") {
        return None;
    }
    let lparen = first + 1;
    if kind_of(tokens, lparen)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(lparen)?;
    let eq = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::EqEq
    })?;
    let operand = if is_null_token(tokens, eq + 1) && eq + 2 == rparen {
        lparen + 1..eq
    } else if eq == lparen + 2 && is_null_token(tokens, eq - 1) {
        eq + 1..rparen
    } else {
        return None;
    };
    if !is_identifier_chain(tokens, operand.clone()) {
        return None;
    }
    // The then arm: a bare expression statement ending at the span's `;`
    // (the first depth-0 `;` from the arm on is the final token — no
    // `else` precedes), or a single-statement block: the `}` ends the
    // span, the statement's `;` sits right behind it, and no other `;`
    // separates the braces — exactly one statement.
    let statement = match kind_of(tokens, rparen + 1)? {
        TokenKind::OpenCurly => {
            let close = tokens.matching_end(rparen + 1)?;
            if close != last || kind_of(tokens, close - 1)? != TokenKind::Semicolon {
                return None;
            }
            if depth0_find(tokens, text, rparen + 2, close - 1, |kind, _| {
                kind == TokenKind::Semicolon
            })
            .is_some()
            {
                return None;
            }
            rparen + 2..close - 1
        }
        _ if kind_of(tokens, last)? == TokenKind::Semicolon => {
            if depth0_find(tokens, text, rparen + 1, last + 1, |kind, _| {
                kind == TokenKind::Semicolon
            }) != Some(last)
            {
                return None;
            }
            rparen + 1..last
        }
        _ => return None,
    };
    let eq_assign = depth0_find(tokens, text, statement.start, statement.end, |kind, _| {
        kind == TokenKind::Eq
    })?;
    let rhs = eq_assign + 1..statement.end;
    if rhs.is_empty() || !same_tokens(tokens, text, operand.clone(), statement.start..eq_assign) {
        return None;
    }
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{} ??= {};",
            token_text(tokens, text, operand)?,
            token_text(tokens, text, rhs)?,
        ),
    ))
}

/// The shared deletion face of a construct reported over its whole span
/// (`unnecessary_breaks`, `unnecessary_library_directive`): the official
/// producers delete the construct's lines range — alone on its line (or
/// lines), the cut takes the line(s) through the terminator (the
/// [`remove_null_aware_assignment`]-shaped whole-line face); sharing a
/// line with a sibling, the cut takes exactly the reported span plus its
/// leading horizontal whitespace, the same-line deletion face of
/// [`drop_import_directive`].
pub(crate) fn drop_span_statement(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    if let Some((line_start, line_end)) = line_bounds(text, start, end)
        && text[line_start..start]
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
        && text[end..line_end]
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
    {
        let (offset, length) = cut_line(text, line_start, line_end);
        return Some((offset, length, String::new()));
    }
    let ws = text[..start].len() - text[..start].trim_end_matches([' ', '\t']).len();
    let lead = u32::try_from(ws).ok()?;
    Some((
        range.start - lead,
        range.end - range.start + lead,
        String::new(),
    ))
}

/// `unnecessary_breaks`: the trailing `break;` of a multi-statement case
/// leaves (the official `RemoveBreak` deletes the statement's lines
/// range, [`drop_span_statement`]'s shape). The rule reports the `break`
/// KEYWORD token alone, so the keyword face extends the span through the
/// statement's `;` (the trivia run between them included) before the
/// cut; a span already slicing `break;` verifies as-is.
pub(crate) fn drop_break_statement(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    let body = slice.trim_matches([' ', '\t']);
    if body == "break" {
        // The keyword report: the `;` terminating the statement joins
        // the cut (the official deletes the whole statement's lines).
        // The trivia run counts bytes; the edit is UTF-16 — a comment
        // between `break` and `;` may carry multi-byte characters, so
        // the run converts before it adds.
        let start = utf16_offset_to_byte(text, input.range.end);
        let rest = text.get(start..)?;
        let run = trivia_run(rest);
        let after_run = rest.get(run..)?;
        if !after_run.starts_with(';') {
            return None;
        }
        let run16 = utf16_count(rest.get(..run)?);
        let end16 = input.range.end + run16 + 1;
        return drop_span_statement(
            text,
            &core::ops::Range {
                start: input.range.start,
                end: end16,
            },
        );
    }
    if body != "break;" {
        return None;
    }
    drop_span_statement(text, &input.range)
}

/// `unnecessary_library_directive`: the bare `library` directive's line
/// leaves (the official `RemoveUnnecessaryLibraryDirective` deletes the
/// directive's lines range, [`drop_span_statement`]'s shape). The span
/// must slice to the `library …;` directive before anything is cut.
pub(crate) fn drop_library_directive(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let trimmed = slice.trim_matches([' ', '\t', '\r', '\n']);
    if !trimmed.starts_with("library")
        || trimmed[7..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        || !trimmed.ends_with(';')
    {
        return None;
    }
    drop_span_statement(input.text, &input.range)
}

/// `unnecessary_parenthesis`: the two parentheses leave and the inner
/// expression keeps verbatim (the official `RemoveUnnecessaryParentheses`
/// deletes the paren tokens). The span's first and last tokens must be
/// the matching `(`/`)` pair — a drifted hit over anything else drops.
pub(crate) fn drop_parentheses(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::OpenParen || tokens.matching_end(first)? != last {
        return None;
    }
    let inner = text.get(
        utf16_offset_to_byte(text, range.start + 1)..utf16_offset_to_byte(text, range.end - 1),
    )?;
    // The whole span goes to the inner text: one edit carries both
    // deletions (the official `RemoveParentheses` deletes the two paren
    // tokens).
    Some((range.start, range.end - range.start, inner.to_owned()))
}

/// `unnecessary_null_checks`: the `!` leaves (the official
/// `RemoveNonNullAssertion` deletes the bang token). The reported span is
/// either the bare `!` operator or the whole postfix spelling ending in
/// it — both verify before the deletion.
pub(crate) fn drop_non_null_assertion(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let range = &input.range;
    if slice16(input.text, range)? == "!" {
        return Some((range.start, 1, String::new()));
    }
    if !slice16(input.text, range)?.ends_with('!') {
        return None;
    }
    Some((range.end - 1, 1, String::new()))
}

/// `eol_at_end_of_file`: a missing newline is inserted at the text end
/// and a trailing newline run collapses to one (the official
/// `AddEolAtEndOfFile` insertion / replacement — the replacement always
/// spells one `\n`, never a `\r\n` pair). The report anchors the trimmed
/// end; the fix reads the tail itself.
pub(crate) fn add_eol(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    if text.ends_with("\n\n") || text.ends_with("\r\r") || text.ends_with("\r\n\r\n") {
        let run_start = text.trim_end_matches(['\n', '\r']).len();
        return Some((
            utf16_count(&text[..run_start]),
            utf16_count(&text[run_start..]),
            "\n".to_owned(),
        ));
    }
    if text.ends_with('\n') || text.ends_with('\r') {
        return None; // exactly one newline — the shape the lint never reports
    }
    Some((utf16_count(text), 0, "\n".to_owned()))
}

/// `noop_primitive_operations`: the typed-invocation face — `.name(…)`,
/// `?..name(…)` leaves its receiver (the official `RemoveInvocation`
/// deletes operator..argumentList; the adjacent-strings face reports a
/// literal and gets no edit there, mirrored by the name-shape refusal).
/// The reported span is the method-name token; the operator run and the
/// argument list are verified against the token stream.
pub(crate) fn remove_invocation(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if first != last
        || !matches!(
            kind_of(tokens, first)?,
            TokenKind::Identifier | TokenKind::Keyword(_)
        )
    {
        return None;
    }
    let op = first.checked_sub(1)?;
    if !matches!(
        kind_of(tokens, op)?,
        TokenKind::Period
            | TokenKind::QuestionPeriod
            | TokenKind::PeriodPeriod
            | TokenKind::QuestionPeriodPeriod
    ) {
        return None;
    }
    let mut cursor = first + 1;
    if kind_of(tokens, cursor)? == TokenKind::Lt {
        cursor = tokens.matching_end(cursor)? + 1;
    }
    if kind_of(tokens, cursor)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(cursor)?;
    let (start16, length16) = span_over(text, tokens, op, rparen)?;
    Some((start16, length16, String::new()))
}

/// `unnecessary_unawaited`: the bare `unawaited(…)` wrapper leaves its
/// argument verbatim (the official `RemoveUnawaited` deletes the name
/// through the left parenthesis and the right parenthesis — the argument
/// text is never touched). The rule only reports bare invocations, so
/// the name is the span head and no operator precedes it.
pub(crate) fn remove_unawaited(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if first != last || tokens.lexeme(text, first) != "unawaited" {
        return None;
    }
    let mut cursor = first + 1;
    if kind_of(tokens, cursor)? == TokenKind::Lt {
        cursor = tokens.matching_end(cursor)? + 1;
    }
    if kind_of(tokens, cursor)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(cursor)?;
    let inner =
        text.get(token_end(tokens.get(cursor)?)..tokens.get(rparen)?.byte_offset as usize)?;
    let (start16, length16) = span_over(text, tokens, first, rparen)?;
    Some((start16, length16, inner.to_owned()))
}

/// `unnecessary_final`: the reported `final` keyword either leaves (a
/// written type follows — the official `ReplaceFinalWithVar`'s
/// `RemoveUnnecessaryFinal` face) or becomes `var` (a pattern
/// declaration or a typeless variable — the `ReplaceFinalWithVar` face).
/// The head walk after the keyword reads which shape follows: an
/// identifier-chain head (`int`, `core.int`, `List<int>`, `int?`,
/// `int Function(int)`, a record type group) followed by another
/// identifier word is the type; anything ending at `=`, `,`, `;`, `in`,
/// or a pattern opener (`(`-group, `[`, `{`) is the name/pattern. A
/// verdict the walk cannot read drops the fix.
pub(crate) fn final_to_var(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if first != last || kind_of(tokens, first)? != TokenKind::Keyword(Keyword::Final) {
        return None;
    }
    let mut cursor = first + 1;
    // The type head: identifier chains with type arguments, nullability,
    // dotted names, `Function` suffixes, and record groups.
    if matches!(kind_of(tokens, cursor)?, TokenKind::Identifier)
        || word_at(tokens, text, cursor, "Function")
    {
        cursor += 1;
        loop {
            match kind_of(tokens, cursor)? {
                TokenKind::Period => cursor += 1,
                TokenKind::Lt => cursor = tokens.matching_end(cursor)? + 1,
                TokenKind::Question => cursor += 1,
                TokenKind::Keyword(Keyword::Function) => {
                    cursor += 1;
                    if kind_of(tokens, cursor)? == TokenKind::OpenParen {
                        cursor = tokens.matching_end(cursor)? + 1;
                    }
                }
                _ => break,
            }
            if matches!(kind_of(tokens, cursor)?, TokenKind::Identifier) {
                cursor += 1;
            } else if word_at(tokens, text, cursor, "Function") {
                cursor += 1;
            } else {
                break;
            }
        }
    } else if kind_of(tokens, cursor)? == TokenKind::OpenParen {
        cursor = tokens.matching_end(cursor)? + 1;
        if kind_of(tokens, cursor)? == TokenKind::Question {
            cursor += 1;
        }
    }
    // The verdict token: another identifier word means the walked head was
    // a written type (`final int x`); a name terminator or a pattern
    // opener means the head was the name itself (`final x`, `final (a, b)
    // = pair`).
    let typed = match kind_of(tokens, cursor)? {
        TokenKind::Identifier => tokens.lexeme(text, cursor) != "in",
        TokenKind::Keyword(Keyword::Function) => true,
        _ => false,
    };
    let replacement = if typed { "" } else { "var" };
    // The typed face deletes the keyword with its trailing horizontal
    // whitespace (the official `range.startStart` deletion, line-bounded);
    // the var face replaces the keyword token alone (the official
    // `range.token` replacement — the following whitespace stays).
    let (start, end16) = if typed {
        let keyword_end = token_end(tokens.get(first)?);
        let tail = text.get(keyword_end..)?;
        let ws = tail.len() - tail.trim_start_matches([' ', '\t']).len();
        (range.start, utf16_count(&text[..keyword_end + ws]))
    } else {
        (range.start, range.end)
    };
    Some((start, end16 - start, replacement.to_owned()))
}

/// `prefer_const_declarations`: the `final` of a constant-initialized
/// declaration becomes `const`, and each initializer-head `const`
/// keyword deletes (the official `ReplaceFinalWithConst`'s keyword
/// replacement plus the per-initializer const deletions folded into one
/// whole-span replacement: `final x = const [1];` → `const x = [1];`).
/// The span must open with the `final` keyword.
///
/// A `const` deletes only where the official producer serves the
/// initializer shape — a `const`-headed typed literal (`[`, `{`, `<`),
/// dot shorthand (`.x(...)`), or instance creation (`A(...)`) that IS the
/// declarator's whole initializer (a `const (` record literal, a
/// `const [1] as T` cast, or a `const` under a nested `=` keeps its
/// keyword — upstream reads `initializer.constKeyword` off those three
/// node kinds alone; upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/replace_final_with_const.dart).
/// The deletion is `startStart(const, const.next)`: the trivia run —
/// whitespace AND comments — to the next real token.
pub(crate) fn final_with_const(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::Keyword(Keyword::Final) {
        return None;
    }
    let end = utf16_offset_to_byte(text, range.end);
    let final_end = token_end(tokens.get(first)?);
    let mut out = String::with_capacity(end - final_end);
    out.push_str("const");
    let mut cursor = final_end;
    let mut depth = 0u32;
    // `name_pos`: a declarator name is expected — true at the list head
    // and after each depth-0 `,`; a depth-0 `=` in name position opens an
    // initializer and arms `after_eq` for the `const` check (an `=`
    // inside the initializer — a nested assignment — never arms it).
    let mut name_pos = true;
    let mut after_eq = false;
    let mut index = first + 1;
    while index <= last {
        let token = tokens.get(index)?;
        match token.kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Keyword(Keyword::Const) if depth == 0 && after_eq => {
                if !const_head_is_served(tokens, index + 1, last)? {
                    after_eq = false;
                    index += 1;
                    continue;
                }
                // `startStart(constToken, constToken.next!)` — the next
                // real token's own start; comments and newlines in the
                // trivia run go with the keyword.
                let at = token.byte_offset as usize;
                out.push_str(text.get(cursor..at)?);
                cursor = tokens.get(index + 1)?.byte_offset as usize;
                after_eq = false;
                index += 1;
                continue;
            }
            TokenKind::Comma if depth == 0 => {
                name_pos = true;
                after_eq = false;
            }
            TokenKind::Eq if depth == 0 => {
                after_eq = name_pos;
                name_pos = false;
            }
            _ if depth == 0 => after_eq = false,
            _ => {}
        }
        index += 1;
    }
    out.push_str(text.get(cursor..end)?);
    Some((range.start, range.end - range.start, out))
}

/// Whether the `const` at `const_index` heads an initializer the official
/// `ReplaceFinalWithConst` serves — a typed literal (`[`, `{`, `<`), a
/// dot shorthand or an instance creation (a name chain landing on `(`),
/// ending the declarator (the next depth-0 token is `,` or the span
/// closes). Anything else — a record literal `const (…)`, a `const`
/// prefix of a wider expression, a name reference — keeps the keyword.
pub(crate) fn const_head_is_served(tokens: &TokenTable, head: usize, last: usize) -> Option<bool> {
    let mut cursor = head;
    // A typed literal's bracket heads the initializer outright; the
    // type-argument form `const <T>[`/`{` resolves its `<>` group first
    // and still needs the bracket (a `const <T>(…)` is a record literal —
    // not served).
    if matches!(
        kind_of(tokens, cursor)?,
        TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::Lt
    ) {
        if kind_of(tokens, cursor)? == TokenKind::Lt {
            cursor = tokens.matching_end(cursor)? + 1;
        }
        if !matches!(
            kind_of(tokens, cursor)?,
            TokenKind::OpenSquare | TokenKind::OpenCurly
        ) {
            return Some(false);
        }
        cursor = tokens.matching_end(cursor)? + 1;
    } else {
        // A creation's name chain — `A`, `A.b`, `A<int>.b`, `.b` — must
        // land on the argument list's `(` (a bare `(` opens a record
        // literal instead, which the producer never touches).
        let mut named = false;
        loop {
            match kind_of(tokens, cursor)? {
                TokenKind::Identifier
                | TokenKind::Keyword(_)
                | TokenKind::Period
                | TokenKind::Question => {
                    named = true;
                    cursor += 1;
                }
                TokenKind::Lt => {
                    named = true;
                    cursor = tokens.matching_end(cursor)? + 1;
                }
                _ => break,
            }
        }
        if !named || kind_of(tokens, cursor)? != TokenKind::OpenParen {
            return Some(false);
        }
        cursor = tokens.matching_end(cursor)? + 1;
    }
    // The initializer must close the declarator: the next depth-0 token
    // is `,`, `;`, or the span's own end (a wider expression — `as`,
    // `?.`, a binary arm — keeps the keyword).
    if cursor > last {
        return Some(true);
    }
    Some(matches!(
        kind_of(tokens, cursor)?,
        TokenKind::Comma | TokenKind::Semicolon
    ))
}

/// `curly_braces_in_flow_control_structures`: the reported non-block body
/// statement is wrapped in a brace block — `{\n<indent>` opens before it,
/// `\n<prefix}>` closes after it (the official `UseCurlyBraces`
/// insertions folded into one whole-body replacement; the body text and
/// any trailing same-line comment keep verbatim inside).
pub(crate) fn curly_braces_body(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    wrap_control_body(input, false)
}

/// `always_put_control_body_on_new_line`: the reported span is the body's
/// head token — the statement end is rebuilt first, then the same wrap
/// the official `UseCurlyBraces` lands (its `nonBulk` face for this
/// lint).
pub(crate) fn curly_braces_head(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    wrap_control_body(input, true)
}

/// The shared control-body wrap. The body must follow `)`, `else`, or
/// `do` (the control head the rules report at), must not already be a
/// block, and its line prefix — the control keyword's line prefix — must
/// be whitespace only; anything else drops.
pub(crate) fn wrap_control_body(input: &FixInput<'_>, expand_head: bool) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first)? == TokenKind::OpenCurly {
        return None; // block bodies are never wrapped
    }
    let last = if expand_head {
        statement_end(tokens, text, first)?
    } else {
        last
    };
    let anchor = first.checked_sub(1)?;
    let control_head = match kind_of(tokens, anchor)? {
        TokenKind::CloseParen => {
            let lparen = matching_begin(tokens, anchor)?;
            lparen.checked_sub(1)?
        }
        TokenKind::Keyword(Keyword::Else | Keyword::Do) => anchor,
        _ => return None,
    };
    let head_byte = tokens.get(control_head)?.byte_offset as usize;
    let line_start = text[..head_byte].rfind('\n').map_or(0, |i| i + 1);
    let prefix = text.get(line_start..head_byte)?;
    if !prefix.bytes().all(|b| matches!(b, b' ' | b'\t')) {
        return None;
    }
    let indent = format!("{prefix}  ");
    // A comment sharing the body's line rides inside the block (the
    // official `_endAfterComments` face); a block comment that does not
    // close on the line drops the fix.
    let mut body_end = token_end(tokens.get(last)?);
    let line_end = text[body_end..]
        .find('\n')
        .map_or(text.len(), |i| body_end + i);
    let tail = text[body_end..line_end].trim_matches([' ', '\t', '\r']);
    if tail.starts_with("//") {
        body_end = line_end;
    } else if tail.starts_with("/*") {
        body_end = text[body_end..line_end]
            .find("*/")
            .map(|i| body_end + i + 2)?;
    }
    let start16 = tokens.get(first)?.utf16_offset;
    let end16 = utf16_count(&text[..body_end]);
    let inner = text.get(utf16_offset_to_byte(text, start16)..body_end)?;
    Some((
        start16,
        end16 - start16,
        format!("{{\n{indent}{inner}\n{prefix}}}"),
    ))
}

/// `empty_catches`: the reported empty block's whole catch clause leaves
/// (the official `RemoveEmptyCatch` deletes the catch clause's lines
/// range) — but only where the official offers the fix at all: the try
/// must keep a handler after the cut (another catch clause or a
/// `finally`). The clause is rebuilt backwards from the reported block:
/// the `(…)` parameter group with its `catch` keyword, or the `on` type
/// face; the try gate reads the balanced block before the clause and the
/// clauses after it.
pub(crate) fn remove_empty_catch(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::OpenCurly || tokens.matching_end(first)? != last {
        return None;
    }
    // The clause start: `catch (…)` — the parameter group and its
    // keyword — or `on Type` — the type's identifier run with its dots
    // and type arguments, over the `on` keyword; an `on Type catch (…)`
    // face opens at the `on` (the official deletes the whole CatchClause
    // node).
    let cursor = first.checked_sub(1)?;
    let clause_start = match kind_of(tokens, cursor)? {
        TokenKind::CloseParen => {
            let lparen = matching_begin(tokens, cursor)?;
            let keyword = lparen.checked_sub(1)?;
            if !word_at(tokens, text, keyword, "catch") {
                return None;
            }
            match keyword.checked_sub(1) {
                Some(back)
                    if matches!(
                        kind_of(tokens, back),
                        Some(TokenKind::Identifier)
                            | Some(TokenKind::Gt)
                            | Some(TokenKind::GtGt)
                            | Some(TokenKind::GtGtGt)
                    ) =>
                {
                    on_type_start(tokens, text, back)?
                }
                _ => keyword,
            }
        }
        _ => on_type_start(tokens, text, cursor)?,
    };
    // The try gate: a balanced `}` block directly before the clause means
    // a sibling catch clause keeps the try covered; at the first clause
    // (the walk lands on the `try` keyword) a sibling (`catch`/`on`/
    // `finally`) must follow this one.
    let mut before = clause_start.checked_sub(1)?;
    while kind_of(tokens, before)? == TokenKind::CloseCurly {
        before = matching_begin(tokens, before)?.checked_sub(1)?;
    }
    let has_sibling = if word_at(tokens, text, before, "try") {
        let next = last + 1;
        word_at(tokens, text, next, "catch")
            || word_at(tokens, text, next, "on")
            || kind_of(tokens, next)? == TokenKind::Keyword(Keyword::Finally)
    } else {
        true
    };
    if !has_sibling {
        return None;
    }
    // The deletion: the clause alone on its line(s) takes them through
    // the terminator; otherwise exactly the clause plus its leading
    // horizontal whitespace.
    let clause_start16 = tokens.get(clause_start)?.utf16_offset;
    let end16 = utf16_count(&text[..token_end(tokens.get(last)?)]);
    if let Some((line_start, line_end)) = line_bounds(
        text,
        utf16_offset_to_byte(text, clause_start16),
        token_end(tokens.get(last)?),
    ) {
        if text[line_start..utf16_offset_to_byte(text, clause_start16)]
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
            && text[token_end(tokens.get(last)?)..line_end]
                .bytes()
                .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
        {
            let (cut_start, _) = cut_line(text, line_start, line_end);
            let eol = if text[line_end..].starts_with("\r\n") {
                2
            } else {
                usize::from(text[line_end..].starts_with('\n'))
            };
            return Some((
                cut_start,
                utf16_count(&text[line_start..token_end(tokens.get(last)?) + eol]),
                String::new(),
            ));
        }
    }
    let head_ws = text[..utf16_offset_to_byte(text, clause_start16)].len()
        - text[..utf16_offset_to_byte(text, clause_start16)]
            .trim_end_matches([' ', '\t'])
            .len();
    let lead = u32::try_from(head_ws).ok()?;
    Some((
        clause_start16 - lead,
        end16 - clause_start16 + lead,
        String::new(),
    ))
}

/// `prefer_single_quotes`: the double-quoted literal's quote runs become
/// single (the official `ConvertToSingleQuotes` — the same machinery as
/// [`convert_quotes`], direction pinned).
pub(crate) fn convert_to_single_quotes(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    convert_quotes(tokens, text, range, Some(true))
}

/// `prefer_double_quotes`: the single-quoted literal's quote runs become
/// double (the official `ConvertToDoubleQuotes`).
pub(crate) fn convert_to_double_quotes(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    convert_quotes(tokens, text, range, Some(false))
}

/// `avoid_escaping_inner_quotes`: the outer quote moves to the character
/// the value escaped (the official `ConvertQuotes` — the direction reads
/// off the literal: `'a\'b'` → `"a'b"`, `"a\"b"` → `'a"b'`).
pub(crate) fn convert_inner_quotes(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    convert_quotes(input.tokens?, input.text, &input.range, None)
}

/// The quote-run swap behind the three quote rules: the outer quote runs
/// (single or triple, matched head and tail) re-spell to the target quote
/// and every string segment's escapes re-balance — an unescaped target
/// quote gains its backslash, an escaped old quote loses it (the official
/// `_fixBackslashesForQuotes` walk); interpolation groups ride verbatim.
/// A raw simple literal keeps its prefix only when the content cannot
/// collide with the new quote; any other raw shape drops.
pub(crate) fn convert_quotes(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
    to_single: Option<bool>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::String || kind_of(tokens, last)? != TokenKind::String {
        return None;
    }
    let head = tokens.lexeme(text, first);
    let raw = head.starts_with('r');
    let raw_skip = usize::from(raw);
    let quote = *head.as_bytes().get(raw_skip)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let lead = head[raw_skip..].bytes().take_while(|&b| b == quote).count();
    let tail = tokens.lexeme(text, last);
    let trail = tail.bytes().rev().take_while(|&b| b == quote).count();
    if lead != trail || !matches!(lead, 1..=3) {
        return None;
    }
    let target = match to_single {
        Some(true) => b'\'',
        Some(false) => b'"',
        None if quote == b'\'' => b'"',
        None => b'\'',
    };
    if target == quote {
        return None;
    }
    if raw {
        // A raw literal cannot interpolate, so first == last here; the
        // prefix keeps only when the contents stay unambiguous under the
        // new quote (the official `_canKeepAsRaw`) — otherwise the fix
        // drops rather than re-spelling a non-raw escape set.
        let content = head.get(raw_skip + lead..head.len() - trail)?;
        let triple: String = std::iter::repeat(target as char).take(3).collect();
        if content.ends_with(target as char) || content.contains(&triple) {
            return None;
        }
        let mut out = String::with_capacity(head.len() + 2);
        out.push('r');
        for _ in 0..lead {
            out.push(target as char);
        }
        out.push_str(content);
        for _ in 0..lead {
            out.push(target as char);
        }
        return Some((range.start, range.end - range.start, out));
    }
    let multiline = lead == 3;
    let mut out = String::with_capacity(tail.len() + head.len());
    for _ in 0..lead {
        out.push(target as char);
    }
    let mut index = first;
    while index <= last {
        match kind_of(tokens, index)? {
            TokenKind::String => {
                if index != first && index != last {
                    return None; // one literal's extent holds two segments at most
                }
                let segment = tokens.lexeme(text, index);
                let lead_cut = if index == first { lead } else { 0 };
                let trail_cut = if index == last { trail } else { 0 };
                let content = segment.get(lead_cut..segment.len() - trail_cut)?;
                push_converted(&mut out, content, quote, target, multiline, index == last);
                index += 1;
            }
            TokenKind::StringInterpolationIdentifier => {
                out.push_str(tokens.lexeme(text, index));
                index += 1;
            }
            TokenKind::StringInterpolationExpression => {
                let close = tokens.matching_end(index)?;
                let start = tokens.get(index)?.byte_offset as usize;
                out.push_str(text.get(start..token_end(tokens.get(close)?))?);
                index = close + 1;
            }
            _ => return None,
        }
    }
    for _ in 0..trail {
        out.push(target as char);
    }
    Some((range.start, range.end - range.start, out))
}

/// One segment's contents under the quote swap — the official
/// `_fixBackslashesForQuotes` walk: an unescaped target quote gains its
/// backslash (multiline literals only every third of a run, and always
/// the final character of the closing segment), an escaped old quote
/// loses its backslash, every other spelling rides verbatim.
pub(crate) fn push_converted(
    out: &mut String,
    content: &str,
    old: u8,
    new: u8,
    multiline: bool,
    string_end: bool,
) {
    let chars: Vec<char> = content.chars().collect();
    let mut escaping = false;
    let mut run = 0u32;
    for (index, &ch) in chars.iter().enumerate() {
        if ch as u32 == new as u32 {
            if escaping {
                escaping = false;
            } else if multiline {
                run += 1;
                if run == 3 {
                    run = 0;
                    out.push('\\');
                } else if string_end && index + 1 == chars.len() {
                    out.push('\\');
                }
            } else {
                out.push('\\');
            }
        } else {
            run = 0;
            if ch as u32 == old as u32 && escaping {
                out.pop();
                escaping = false;
            }
            escaping = ch == '\\' && !escaping;
        }
        out.push(ch);
    }
}

// ---------------------------------------------------------------------------
// Lint-face rewriters (the `FixSource::Lint` breadth)
//
// The F4 wave's rows: each mirrors one official quick-fix producer's edit
// shape (upstream Dart SDK source,
// pkg/analysis_server/lib/src/services/correction/dart/*, keyed through the
// lint registry of fix_internal.dart); the official producers read the
// resolved AST node under the report, these rebuild the same shape from the
// report span and the token stream, and drop where the shape does not
// verify. Producers whose edits need element, type, or import facts have no
// row (the fix_table module docs carry the ledger).
// ---------------------------------------------------------------------------

/// Whether the token range carries any depth-0 operator whose precedence
/// sits below the position a rewrite binds at (`!`, `||`/`&&`, `~/`,
/// `.call`) — the textual read of the official producers' `needsParens`
/// gates: comparisons, arithmetic, logic, null-coalesce, conditional,
/// assignment, cascade, and the `is`/`as` tests all qualify; a pure
/// postfix chain never does.
pub(crate) fn binds_loose(tokens: &TokenTable, range: core::ops::Range<usize>) -> bool {
    let mut depth = 0u32;
    for index in range {
        let Some(kind) = kind_of(tokens, index) else {
            return true;
        };
        match kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1).unwrap_or(0);
            }
            _ if depth == 0 => {
                if matches!(
                    kind,
                    TokenKind::Eq
                        | TokenKind::EqEq
                        | TokenKind::BangEq
                        | TokenKind::Lt
                        | TokenKind::LtEq
                        | TokenKind::Gt
                        | TokenKind::GtEq
                        | TokenKind::Plus
                        | TokenKind::Minus
                        | TokenKind::Star
                        | TokenKind::Slash
                        | TokenKind::Percent
                        | TokenKind::TildeSlash
                        | TokenKind::Ampersand
                        | TokenKind::Bar
                        | TokenKind::Caret
                        | TokenKind::LtLt
                        | TokenKind::GtGt
                        | TokenKind::GtGtGt
                        | TokenKind::AmpersandAmpersand
                        | TokenKind::BarBar
                        | TokenKind::QuestionQuestion
                        | TokenKind::Question
                        | TokenKind::PlusEq
                        | TokenKind::MinusEq
                        | TokenKind::StarEq
                        | TokenKind::SlashEq
                        | TokenKind::PercentEq
                        | TokenKind::TildeSlashEq
                        | TokenKind::AmpersandEq
                        | TokenKind::BarEq
                        | TokenKind::CaretEq
                        | TokenKind::LtLtEq
                        | TokenKind::GtGtEq
                        | TokenKind::GtGtGtEq
                        | TokenKind::QuestionQuestionEq
                        | TokenKind::PeriodPeriod
                        | TokenKind::QuestionPeriodPeriod
                        | TokenKind::Keyword(Keyword::Is)
                        | TokenKind::Keyword(Keyword::As)
                ) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// The `(question, colon)` split of a conditional's token range — the
/// depth-0 `?` and its matching `:` (each deeper `?` opens a nested
/// conditional whose `:` closes it first).
pub(crate) fn ternary_split(
    tokens: &TokenTable,
    text: &str,
    first: usize,
    last: usize,
) -> Option<(usize, usize)> {
    let question = depth0_find(tokens, text, first, last + 1, |kind, _| {
        kind == TokenKind::Question
    })?;
    let mut depth = 0u32;
    let mut pending = 0u32;
    for index in question + 1..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Question if depth == 0 => pending += 1,
            TokenKind::Colon if depth == 0 => {
                if pending == 0 {
                    return Some((question, index));
                }
                pending -= 1;
            }
            _ => {}
        }
    }
    None
}

/// A single-token `true`/`false` literal and its value, or `None`.
pub(crate) fn bool_literal(tokens: &TokenTable, text: &str, index: usize) -> Option<bool> {
    if index != 0 && !matches!(kind_of(tokens, index), Some(TokenKind::Keyword(_))) {
        return None;
    }
    match tokens.lexeme(text, index) {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// `!` + optional parentheses around the text of a token range — the
/// negation face the boolean rewrites share (`needsParens` read by
/// [`binds_loose`]).
pub(crate) fn bang_wrapped(tokens: &TokenTable, text: &str, range: core::ops::Range<usize>) -> Option<String> {
    let body = token_text(tokens, text, range.clone())?;
    Some(if binds_loose(tokens, range) {
        format!("!({body})")
    } else {
        format!("!{body}")
    })
}
