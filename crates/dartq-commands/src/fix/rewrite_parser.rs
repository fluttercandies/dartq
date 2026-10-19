use super::*;


/// `expected_token`: the official trio told apart by the quoted token the
/// message names — `InsertSemicolon` (`;` lands after the report, never
/// after an `await` identifier used outside `async`), `ReplaceWithArrow`
/// (a switch-expression case's `:` and the gap before it become ` =>`),
/// `InsertOnKeyword` (`on ` before the extended type, or before the
/// extension's name when the type is missing).
pub(crate) fn expected_token_fix(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    if input.message.contains("';'") {
        if slice16(text, &input.range)? == "await" {
            return None;
        }
        return Some((input.range.end, 0, ";".to_owned()));
    }
    let tokens = input.tokens?;
    let hit = token_starting_at(tokens, input.range.start)?;
    if input.message.contains("'=>'") {
        if kind_of(tokens, hit)? != TokenKind::Colon {
            return None;
        }
        let start = utf16_end(tokens, text, hit.checked_sub(1)?)?;
        let end = utf16_end(tokens, text, hit)?;
        return Some((start, end - start, " =>".to_owned()));
    }
    if input.message.contains("'on'") {
        if matches!(
            kind_of(tokens, hit)?,
            TokenKind::Identifier | TokenKind::Keyword(Keyword::Void | Keyword::Dynamic)
        ) {
            return Some((tokens.get(hit)?.utf16_offset, 0, "on ".to_owned()));
        }
        // The extended type is synthetic: the keyword lands before the
        // name, behind its type parameters.
        let mut name = hit.checked_sub(1)?;
        if matches!(
            kind_of(tokens, name)?,
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
        ) {
            name = angle_group_start(tokens, name)?.checked_sub(1)?;
        }
        if kind_of(tokens, name)? != TokenKind::Identifier
            || kind_of(tokens, name.checked_sub(1)?)? != TokenKind::Keyword(Keyword::Extension)
        {
            return None;
        }
        return Some((tokens.get(name)?.utf16_offset, 0, "on ".to_owned()));
    }
    None
}

/// `extension_augmentation_has_on_clause`: the `on` clause goes from the
/// keyword through the body's opener (official `RemoveOnClause`:
/// `startStart(onClause, leftBracket-or-semicolon)`).
pub(crate) fn remove_on_clause(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let on = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, on)? != TokenKind::Keyword(Keyword::On) {
        return None;
    }
    let body = depth0_scan(tokens, on + 1, |kind| {
        matches!(kind, TokenKind::OpenCurly | TokenKind::Semicolon)
    })?;
    let start = tokens.get(on)?.utf16_offset;
    let end = tokens.get(body)?.utf16_offset;
    Some((start, end.checked_sub(start)?, String::new()))
}

/// `getter_with_parameters`: the reported `(` through its `)` goes whole
/// (official `RemoveParametersInGetterDeclaration` over the kept
/// `FormalParameterList` node).
pub(crate) fn remove_getter_parameters(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let open = token_starting_at(tokens, input.range.start)?;
    let close = skip_group(tokens, open, TokenKind::OpenParen)?;
    let (start, length) = span_over(input.text, tokens, open, close)?;
    Some((start, length, String::new()))
}

/// `record_literal_one_positional_no_trailing_comma` /
/// `record_type_one_positional_no_trailing_comma`: `,` lands at the end
/// of the single field — the token before the reported `)` (official
/// `AddTrailingComma`: `fields.last.end`).
pub(crate) fn add_trailing_comma(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let close = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, close)? != TokenKind::CloseParen {
        return None;
    }
    let field_end = close.checked_sub(1)?;
    if kind_of(tokens, field_end)? == TokenKind::OpenParen {
        return None;
    }
    Some((utf16_end(tokens, input.text, field_end)?, 0, ",".to_owned()))
}

/// `missing_typedef_parameters`: `()` lands at the end of the token before
/// the alias's `;` (official `AddEmptyArgumentList`: `endToken.previous.end`).
pub(crate) fn add_typedef_parameters(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let hit = token_starting_at(tokens, input.range.start)?;
    let semicolon = depth0_scan(tokens, hit, |kind| kind == TokenKind::Semicolon)?;
    let before = semicolon.checked_sub(1)?;
    if kind_of(tokens, before)? == TokenKind::Keyword(Keyword::Typedef) {
        return None;
    }
    Some((utf16_end(tokens, input.text, before)?, 0, "()".to_owned()))
}

/// `pattern_assignment_declares_variable`: the declared pattern's `var` /
/// `final` keyword plus one unit goes — the keyword stands before the
/// reported name, behind an optional type (official `RemoveVarKeyword`;
/// a typed pattern without a keyword has no edit).
pub(crate) fn remove_pattern_var_keyword(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let name = token_starting_at(tokens, input.range.start)?;
    let mut cursor = name.checked_sub(1)?;
    let keyword = |index: usize| {
        matches!(
            kind_of(tokens, index),
            Some(TokenKind::Keyword(Keyword::Var | Keyword::Final))
        )
    };
    if !keyword(cursor) {
        // Walk back over the type: `?`, the type arguments, the dotted name.
        if kind_of(tokens, cursor)? == TokenKind::Question {
            cursor = cursor.checked_sub(1)?;
        }
        if matches!(
            kind_of(tokens, cursor)?,
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
        ) {
            cursor = angle_group_start(tokens, cursor)?.checked_sub(1)?;
        }
        if kind_of(tokens, cursor)? != TokenKind::Identifier {
            return None;
        }
        while cursor >= 2
            && kind_of(tokens, cursor - 1)? == TokenKind::Period
            && kind_of(tokens, cursor - 2)? == TokenKind::Identifier
        {
            cursor -= 2;
        }
        cursor = cursor.checked_sub(1)?;
        if !keyword(cursor) {
            return None;
        }
    }
    let token = tokens.get(cursor)?;
    remove_lexeme(text, token.utf16_offset, tokens.lexeme(text, cursor))
}

/// `unexpected_separator_in_number`: the literal is rebuilt without the
/// underscore runs that do not stand between two digits (official
/// `RemoveUnexpectedUnderscores`: a run survives only when a digit
/// precedes and follows it; hex digits count in a hex literal). The
/// scanner reports every illegal run at the literal's start, so the one
/// replacement covers them all.
pub(crate) fn remove_number_separators(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let literal = token_starting_at(tokens, input.range.start)?;
    let hex = match kind_of(tokens, literal)? {
        TokenKind::Int | TokenKind::Double => false,
        TokenKind::Hexadecimal => true,
        _ => return None,
    };
    let lexeme = tokens.lexeme(text, literal);
    let mut rebuilt = String::with_capacity(lexeme.len());
    let mut run = 0usize;
    let mut previous_is_digit = false;
    for byte in lexeme.bytes() {
        let is_digit = byte.is_ascii_digit() || (hex && byte.is_ascii_hexdigit());
        if byte == b'_' {
            run += 1;
            continue;
        }
        if run > 0 && is_digit && previous_is_digit {
            rebuilt.extend(std::iter::repeat_n('_', run));
        }
        run = 0;
        previous_is_digit = is_digit;
        rebuilt.push(char::from(byte));
    }
    if rebuilt == lexeme {
        return None;
    }
    let token = tokens.get(literal)?;
    Some((token.utf16_offset, utf16_count(lexeme), rebuilt))
}

/// `missing_function_body` / `static_getter_without_body` /
/// `static_setter_without_body`: the reported `;` body becomes the
/// official block — ` {`, a `// TODO: implement <name>` line, a
/// `throw UnimplementedError();` line unless the member's written return
/// type is `void` or the member is a setter, and the closing brace at
/// the declaration line's indentation (official
/// `ConvertIntoBlockBody.missingBody` over an `EmptyFunctionBody`: the
/// replacement runs from the token before the `;` through it; the
/// official reads the element's return type, which for an unwritten
/// return type is `dynamic` — an override-inferred `void` is the one
/// shape this written-type read does not see).
pub(crate) fn missing_body_block(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let semicolon = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, semicolon)? != TokenKind::Semicolon {
        return None;
    }
    let before = semicolon.checked_sub(1)?;
    let (name, is_setter) = match kind_of(tokens, before)? {
        TokenKind::CloseParen => {
            let open = matching_begin(tokens, before)?;
            let mut name = open.checked_sub(1)?;
            if matches!(
                kind_of(tokens, name)?,
                TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
            ) {
                name = angle_group_start(tokens, name)?.checked_sub(1)?;
            }
            let is_setter = name.checked_sub(1).is_some_and(|index| {
                kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Set))
            });
            (name, is_setter)
        }
        TokenKind::Identifier => {
            if kind_of(tokens, before.checked_sub(1)?)? != TokenKind::Keyword(Keyword::Get) {
                return None;
            }
            (before, false)
        }
        _ => return None,
    };
    let mut display = tokens.lexeme(text, name).to_owned();
    if display.is_empty() {
        return None;
    }
    let head = name.checked_sub(1);
    let after_operator = head
        .is_some_and(|index| kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Operator)));
    if after_operator && display == "-" && kind_of(tokens, name + 2)? == TokenKind::CloseParen {
        display = "unary-".to_owned();
    }
    // The written return type: the token before the name (behind a
    // getter's `get`); `void` alone reads as void.
    let type_token = match head {
        Some(index)
            if matches!(
                kind_of(tokens, index),
                Some(TokenKind::Keyword(Keyword::Get))
            ) =>
        {
            index.checked_sub(1)
        }
        Some(index) if after_operator => index.checked_sub(1),
        other => other,
    };
    let is_void = is_setter
        || type_token
            .is_some_and(|index| kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Void)));
    // The declaration's first token: back over the head to the previous
    // member's end (or the file start) — its line carries the prefix.
    let mut first = name;
    while let Some(prev) = first.checked_sub(1) {
        match kind_of(tokens, prev)? {
            TokenKind::Semicolon | TokenKind::CloseCurly | TokenKind::OpenCurly => break,
            TokenKind::CloseParen | TokenKind::CloseSquare => first = matching_begin(tokens, prev)?,
            _ => first = prev,
        }
    }
    let prefix = line_indent(text, tokens.get(first)?.byte_offset as usize);
    let mut block = format!(" {{\n{prefix}  // TODO: implement {display}\n");
    if !is_void {
        block.push_str(&format!("{prefix}  throw UnimplementedError();\n"));
    }
    block.push_str(&format!("{prefix}}}"));
    let start = utf16_end(tokens, text, before)?;
    let end = utf16_end(tokens, text, semicolon)?;
    Some((start, end - start, block))
}

/// `extension_declares_constructor` / `mixin_declares_constructor`: the
/// constructor member goes from the end of the previous member (or the
/// body's `{`) through its own end token — `;` or the body's `}`
/// (official `RemoveConstructor`: `endEnd(previous.endToken ??
/// leftBracket, constructor.endToken)`); a `(` report is a primary
/// constructor, whose `(…)` goes alone.
pub(crate) fn remove_constructor(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let hit = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, hit)? == TokenKind::OpenParen {
        let close = tokens.matching_end(hit)?;
        let (start, length) = span_over(text, tokens, hit, close)?;
        return Some((start, length, String::new()));
    }
    let open = depth0_scan(tokens, hit, |kind| kind == TokenKind::OpenParen)?;
    let close = tokens.matching_end(open)?;
    let end = match kind_of(tokens, close + 1)? {
        TokenKind::Semicolon => close + 1,
        TokenKind::OpenCurly => tokens.matching_end(close + 1)?,
        TokenKind::Function | TokenKind::Eq => {
            depth0_scan(tokens, close + 2, |kind| kind == TokenKind::Semicolon)?
        }
        TokenKind::Colon => initializer_list_end(tokens, close + 2)?,
        _ => return None,
    };
    let mut first = hit;
    loop {
        let prev = first.checked_sub(1)?;
        match kind_of(tokens, prev)? {
            TokenKind::Keyword(
                Keyword::Const | Keyword::Factory | Keyword::External | Keyword::Augment,
            )
            | TokenKind::Identifier
            | TokenKind::Period
            | TokenKind::At => first = prev,
            TokenKind::CloseParen => first = matching_begin(tokens, prev)?,
            TokenKind::Semicolon | TokenKind::CloseCurly | TokenKind::OpenCurly => break,
            _ => return None,
        }
    }
    let start = utf16_end(tokens, text, first.checked_sub(1)?)?;
    let end = utf16_end(tokens, text, end)?;
    Some((start, end.checked_sub(start)?, String::new()))
}

/// The end token of a constructor whose initializer list starts at
/// `from`: its `;`, or the `}` of the block body — a depth-0 `{` opens
/// the body unless it follows a token that can only precede a set/map
/// literal.
pub(crate) fn initializer_list_end(tokens: &TokenTable, from: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut cursor = from;
    loop {
        match kind_of(tokens, cursor)? {
            TokenKind::Eof => return None,
            TokenKind::OpenParen | TokenKind::OpenSquare => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare => depth = depth.checked_sub(1)?,
            TokenKind::Semicolon if depth == 0 => return Some(cursor),
            TokenKind::OpenCurly => {
                let literal = depth > 0
                    || matches!(
                        kind_of(tokens, cursor.checked_sub(1)?)?,
                        TokenKind::Eq
                            | TokenKind::Comma
                            | TokenKind::Colon
                            | TokenKind::Question
                            | TokenKind::QuestionQuestion
                            | TokenKind::Gt
                            | TokenKind::GtGt
                            | TokenKind::Keyword(Keyword::Const)
                    );
                let close = tokens.matching_end(cursor)?;
                if !literal {
                    return Some(close);
                }
                cursor = close;
            }
            _ => {}
        }
        cursor += 1;
    }
}

/// Whether `kind` is one of the binary operators a constant pattern's
/// expression parse consumes (bitwise-or precedence and tighter) — the
/// operators `invalid_constant_pattern_binary` reports.
pub(crate) fn is_constant_pattern_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Bar
            | TokenKind::Caret
            | TokenKind::Ampersand
            | TokenKind::LtLt
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::TildeSlash
    )
}

/// A single-token constant operand: a literal or a bare name.
pub(crate) fn is_constant_operand(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String
            | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Null)
    )
}

/// The first token of the dotted-name or literal operand ending at `at`
/// (a numeric literal takes a unary `-` that no operand precedes).
pub(crate) fn operand_start(tokens: &TokenTable, at: usize) -> Option<usize> {
    if !is_constant_operand(kind_of(tokens, at)?) {
        return None;
    }
    let mut start = at;
    if kind_of(tokens, at)? == TokenKind::Identifier {
        while start >= 2
            && kind_of(tokens, start - 1)? == TokenKind::Period
            && kind_of(tokens, start - 2)? == TokenKind::Identifier
        {
            start -= 2;
        }
    } else if matches!(
        kind_of(tokens, at)?,
        TokenKind::Int | TokenKind::Double | TokenKind::Hexadecimal
    ) && start >= 1
        && kind_of(tokens, start - 1)? == TokenKind::Minus
        && !start
            .checked_sub(2)
            .is_some_and(|index| kind_of(tokens, index).is_some_and(is_constant_operand))
    {
        start -= 1;
    }
    Some(start)
}

/// The last token of the dotted-name or literal operand starting at `at`
/// (a unary `-` takes its numeric literal).
pub(crate) fn operand_end(tokens: &TokenTable, at: usize) -> Option<usize> {
    let mut end = at;
    if kind_of(tokens, at)? == TokenKind::Minus {
        end += 1;
        if !matches!(
            kind_of(tokens, end)?,
            TokenKind::Int | TokenKind::Double | TokenKind::Hexadecimal
        ) {
            return None;
        }
        return Some(end);
    }
    if !is_constant_operand(kind_of(tokens, end)?) {
        return None;
    }
    if kind_of(tokens, end)? == TokenKind::Identifier {
        while kind_of(tokens, end + 1)? == TokenKind::Period
            && kind_of(tokens, end + 2)? == TokenKind::Identifier
        {
            end += 2;
        }
    }
    Some(end)
}

/// The `const` wrap of the pattern expression over tokens `first..=last`:
/// `const ` before an enclosing parenthesized pattern, else the span
/// replaced by `const (…)` (the official `AddConst` insertions folded
/// into one edit).
pub(crate) fn const_wrap(
    text: &str,
    tokens: &TokenTable,
    first: usize,
    last: usize,
) -> Option<(u32, u32, String)> {
    let parenthesized = first
        .checked_sub(1)
        .is_some_and(|index| kind_of(tokens, index) == Some(TokenKind::OpenParen))
        && kind_of(tokens, last + 1) == Some(TokenKind::CloseParen);
    if parenthesized {
        return Some((tokens.get(first - 1)?.utf16_offset, 0, "const ".to_owned()));
    }
    let (start, length) = span_over(text, tokens, first, last)?;
    let inner = slice16(text, &(start..start + length))?;
    Some((start, length, format!("const ({inner})")))
}

/// `invalid_constant_pattern_binary`: the reported operator's whole
/// operand chain — literals and dotted names joined by the operators the
/// constant parse consumes — wraps in `const (…)`, or `const ` lands
/// before an enclosing parenthesized pattern (official `AddConst`
/// binary-expression leg; an operand shape outside the chain grammar
/// drops).
pub(crate) fn const_pattern_binary(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let operator = token_starting_at(tokens, input.range.start)?;
    if !is_constant_pattern_operator(kind_of(tokens, operator)?) {
        return None;
    }
    let mut first = operand_start(tokens, operator.checked_sub(1)?)?;
    while first >= 2 && is_constant_pattern_operator(kind_of(tokens, first - 1)?) {
        first = operand_start(tokens, first - 2)?;
    }
    let mut last = operand_end(tokens, operator + 1)?;
    while is_constant_pattern_operator(kind_of(tokens, last + 1)?) {
        last = operand_end(tokens, last + 2)?;
    }
    const_wrap(input.text, tokens, first, last)
}

/// `invalid_constant_pattern_negation`: the `-` and its reported
/// non-numeric operand wrap in `const (…)` (official `AddConst`
/// prefix-expression leg).
pub(crate) fn const_pattern_negation(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let hit = token_starting_at(tokens, input.range.start)?;
    let anchor = if kind_of(tokens, hit)? == TokenKind::Period {
        hit.checked_sub(1)?
    } else {
        hit
    };
    let start = operand_start(tokens, anchor)?;
    let last = operand_end(tokens, start)?;
    let minus = start.checked_sub(1)?;
    if kind_of(tokens, minus)? != TokenKind::Minus {
        return None;
    }
    const_wrap(input.text, tokens, minus, last)
}

/// `invalid_constant_pattern_generic`: the type literal with its type
/// arguments wraps in `const (…)` when it is the whole case pattern
/// (`:` or `=>` follows), or `const ` lands before an enclosing
/// parenthesized pattern (official `AddConst` type-literal leg); a
/// guarded or nested pattern — where the official wraps the enclosing
/// node whole — gets no edit.
pub(crate) fn const_pattern_generic(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let lt = token_starting_at(tokens, input.range.start)?;
    let gt = angle_group_end(tokens, lt)?;
    let first = operand_start(tokens, lt.checked_sub(1)?)?;
    if kind_of(tokens, first)? != TokenKind::Identifier {
        return None;
    }
    match kind_of(tokens, gt + 1)? {
        TokenKind::Colon | TokenKind::Function => {}
        TokenKind::CloseParen
            if first
                .checked_sub(1)
                .is_some_and(|index| kind_of(tokens, index) == Some(TokenKind::OpenParen)) => {}
        _ => return None,
    }
    const_wrap(input.text, tokens, first, gt)
}

/// `use_named_constants`: the reported creation folds into the constant the
/// message names — the official `ReplaceWithNamedConstant` reads the
/// correction's quoted `{0}`; the lint's problem text carries the same
/// argument (`Use the constant 'C.zero' rather than …`), so the first
/// single-quoted run of the message is the replacement.
pub(crate) fn named_constant_reference(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let message = input.message;
    let open = message.find('\'')?;
    let close = message.get(open + 1..)?.find('\'')? + open + 1;
    let name = message.get(open + 1..close)?;
    if name.is_empty() {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        name.to_owned(),
    ))
}
