use super::*;

/// `x == null ? a : x` → `x ?? a` (the official `ConvertToIfNull`
/// replacement text: nullable operand, `??`, default arm — parenthesized
/// when the arm is itself a bare conditional).
pub(crate) fn if_null_operators(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    let (op, operand, then_arm, else_arm) = null_check_tokens(tokens, text, first, last)?;
    let (nullable, default) = match op {
        TokenKind::EqEq => (else_arm, then_arm),
        _ => (then_arm, else_arm),
    };
    if !same_tokens(tokens, text, operand, nullable.clone()) {
        return None;
    }
    let default_text = token_text(tokens, text, default.clone())?;
    // A default whose precedence sits below `??` (a bare conditional —
    // the only such shape) needs its parentheses.
    let wrap = depth0_find(tokens, text, default.start, default.end, |kind, _| {
        kind == TokenKind::Question
    })
    .is_some();
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{} ?? {}{}{}",
            token_text(tokens, text, nullable)?,
            if wrap { "(" } else { "" },
            default_text,
            if wrap { ")" } else { "" },
        ),
    ))
}

/// `x == null ? null : x.y` → `x?.y`: the arm's access chain peels back
/// to the null-check operand (the rule's own test, re-verified), the
/// peeled access goes null-aware — `.` becomes `?.`, a trailing `!`
/// becomes `?` (or vanishes when bare), an existing `?.` stays.
pub(crate) fn null_aware_operators(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    let (op, operand, then_arm, else_arm) = null_check_tokens(tokens, text, first, last)?;
    let result = match op {
        TokenKind::EqEq if is_null_token(tokens, then_arm.start) => else_arm,
        TokenKind::BangEq if is_null_token(tokens, else_arm.start) => then_arm,
        _ => return None,
    };
    let n = operand.len();
    if result.len() <= n
        || !same_tokens(
            tokens,
            text,
            operand.clone(),
            result.start..result.start + n,
        )
    {
        return None;
    }
    let receiver = token_text(tokens, text, operand)?;
    // The access right after the peeled receiver decides the rewrite.
    let after = result.start + n;
    let access = tokens.get(after)?;
    let arm_end = token_end(tokens.get(result.end - 1)?);
    let tail = &text[token_end(access)..arm_end];
    let rewritten = match access.kind {
        // The access token itself is excluded from `tail`: `.` and `?.`
        // both grow to `?.`, a trailing `!` becomes `?` (or vanishes when
        // nothing follows it).
        TokenKind::Period | TokenKind::QuestionPeriod => format!("{receiver}?.{tail}"),
        TokenKind::Bang => {
            if after + 1 == result.end {
                receiver
            } else {
                format!("{receiver}?{tail}")
            }
        }
        _ => return None,
    };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, rewritten))
}

/// The `indexOf` comparison rewrite (`x.indexOf(y) == -1` →
/// `!x.contains(y)`, the official `ConvertToContains`): the rewrite
/// matrix alone produces an edit — the always-true/false judgments get
/// none, exactly as the official producer returns without one.
pub(crate) fn contains_call(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The method-name `indexOf` is the last word followed by `(` (a
    // receiver's own `indexOf` call sits before it).
    let call = (first..last)
        .filter(|&index| word_at(tokens, text, index, "indexOf"))
        .rfind(|&index| kind_of(tokens, index + 1) == Some(TokenKind::OpenParen))?;
    let lparen = call + 1;
    let rparen = tokens.matching_end(lparen)?;
    // The start-index argument must be the literal 0 (the lint's own
    // gate) and is dropped from the rewrite.
    let comma = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    });
    let argument = match comma {
        Some(comma) => {
            let second = comma + 1..rparen;
            if second.len() != 1 || tokens.lexeme(text, second.start) != "0" {
                return None;
            }
            lparen + 1..comma
        }
        None => lparen + 1..rparen,
    };
    // The int literal sits on one side of the comparison operator; the
    // left-literal layout reads the operator with its operands swapped to
    // reach the matrix's `indexOf OP literal` layout.
    let left_literal = kind_of(tokens, first) == Some(TokenKind::Minus)
        || kind_of(tokens, first) == Some(TokenKind::Int);
    let is_comparison = |kind: Option<TokenKind>| {
        matches!(
            kind,
            Some(
                TokenKind::EqEq
                    | TokenKind::BangEq
                    | TokenKind::Lt
                    | TokenKind::LtEq
                    | TokenKind::Gt
                    | TokenKind::GtEq
            )
        )
    };
    let (value, effective, receiver_start) = if left_literal {
        // `-1 != x.indexOf(y)`: the operator sits after the literal, the
        // hit ends at the invocation.
        let op_index = if kind_of(tokens, first) == Some(TokenKind::Minus) {
            first + 2
        } else {
            first + 1
        };
        if !is_comparison(kind_of(tokens, op_index)) || rparen != last {
            return None;
        }
        let after_op = utf16_offset_to_byte(text, tokens.get(op_index + 1)?.utf16_offset);
        (
            int_literal_value(tokens, text, first)?,
            mirror_op(tokens.lexeme(text, op_index)).to_owned(),
            after_op,
        )
    } else {
        let op_index = rparen + 1;
        if op_index >= last || !is_comparison(kind_of(tokens, op_index)) {
            return None;
        }
        (
            int_literal_value(tokens, text, op_index + 1)?,
            tokens.lexeme(text, op_index).to_owned(),
            tokens.get(first)?.byte_offset as usize,
        )
    };
    let negated = match (value, effective.as_str()) {
        (-1, "==" | "<=") | (0, "<") => true,
        (-1, "!=" | ">") | (0, ">=") => false,
        _ => return None,
    };
    let argument_text = token_text(tokens, text, argument)?;
    let receiver = &text[receiver_start..tokens.get(call)?.byte_offset as usize];
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{}{receiver}contains({argument_text})",
            if negated { "!" } else { "" }
        ),
    ))
}

/// One merged interpolation component: a string literal's contents, a
/// bare identifier, or an arbitrary expression's source.
pub(crate) struct Component {
    pub(crate) text: String,
    pub(crate) identifier: bool,
}

pub(crate) fn compose_interpolation(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The chain's depth-0 `+` splits the operands.
    let mut operands: Vec<core::ops::Range<usize>> = Vec::new();
    let mut depth = 0u32;
    let mut start = first;
    for index in first..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Plus if depth == 0 => {
                operands.push(start..index);
                start = index + 1;
            }
            _ => {}
        }
    }
    operands.push(start..last + 1);
    if operands.len() < 2 {
        return None;
    }
    let mut components = Vec::new();
    let mut style: Option<(u8, bool)> = None;
    for operand in operands {
        let (component, literal_style) = flatten_operand(tokens, text, operand.clone())?;
        match (style, literal_style) {
            (None, _) => style = literal_style,
            (Some(existing), Some(new)) if existing != new => return None,
            _ => {}
        }
        components.push(component);
    }
    let (quote, triple) = style?;
    let open = char::from(quote)
        .to_string()
        .repeat(if triple { 3 } else { 1 });
    // A bare identifier absorbs a following identifier-part head, so it
    // takes the braced form when the next component would continue it.
    let mut out = open.clone();
    for (index, component) in components.iter().enumerate() {
        let next_continues = components
            .get(index + 1)
            .and_then(|next| next.text.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
        if component.identifier {
            if next_continues {
                out.push_str("${");
                out.push_str(&component.text);
                out.push('}');
            } else {
                out.push('$');
                out.push_str(&component.text);
            }
        } else {
            out.push_str(&component.text);
        }
    }
    out.push_str(&open);
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, out))
}

/// One operand of the `+` chain → its component (with the string style
/// when the operand is a literal). Adjacent strings flatten piece-wise,
/// `x.toString()` unwraps to `x`, parentheses unwrap — the official
/// `_extractComponentsInto` walk. Raw strings drop the whole fix.
pub(crate) fn flatten_operand(
    tokens: &TokenTable,
    text: &str,
    operand: core::ops::Range<usize>,
) -> Option<(Component, Option<(u8, bool)>)> {
    let first = operand.start;
    let last = operand.end - 1;
    match kind_of(tokens, first)? {
        TokenKind::String => {
            let lexeme = tokens.lexeme(text, first);
            if lexeme.starts_with('r') {
                return None;
            }
            let quote = *lexeme.as_bytes().first()?;
            // Adjacent strings: consecutive bare String tokens, each its
            // own literal piece — all sharing one quote style.
            let mut split = first;
            while split <= last && kind_of(tokens, split) == Some(TokenKind::String) {
                split += 1;
            }
            if split > last {
                let mut contents = String::new();
                for index in first..=last {
                    if string_style(tokens, text, index)?
                        != (quote, quote_is_triple(tokens, text, first)?)
                    {
                        return None;
                    }
                    contents.push_str(&segment_contents(tokens, text, index)?);
                }
                return Some((
                    Component {
                        text: contents,
                        identifier: false,
                    },
                    Some((quote, quote_is_triple(tokens, text, first)?)),
                ));
            }
            // An interpolated literal: `${…}` groups and segments to the
            // end, contents verbatim between the outer quote runs.
            let mut cursor = split;
            while cursor <= last {
                match kind_of(tokens, cursor)? {
                    TokenKind::String => cursor += 1,
                    TokenKind::StringInterpolationExpression => {
                        cursor = tokens.matching_end(cursor)? + 1;
                    }
                    _ => return None,
                }
            }
            let head = tokens.get(first)?.byte_offset as usize
                + quote_run(lexeme.as_bytes(), quote, false);
            let tail_lexeme = tokens.lexeme(text, last);
            let tail = tokens.get(last)?.byte_offset as usize + tail_lexeme.len()
                - quote_run(
                    tail_lexeme.as_bytes(),
                    *tail_lexeme.as_bytes().last()?,
                    true,
                );
            Some((
                Component {
                    text: text[head..tail].to_owned(),
                    identifier: false,
                },
                Some((quote, quote_run(lexeme.as_bytes(), quote, false) > 1)),
            ))
        }
        TokenKind::Identifier | TokenKind::Keyword(_) if first == last => Some((
            Component {
                text: tokens.lexeme(text, first).to_owned(),
                identifier: true,
            },
            None,
        )),
        _ => {
            // `x.toString()` unwraps to `x`; parentheses unwrap.
            if last >= first + 4
                && kind_of(tokens, last - 3) == Some(TokenKind::Period)
                && word_at(tokens, text, last - 2, "toString")
                && kind_of(tokens, last - 1) == Some(TokenKind::OpenParen)
                && kind_of(tokens, last) == Some(TokenKind::CloseParen)
            {
                return flatten_operand(tokens, text, first..last - 3);
            }
            if kind_of(tokens, first) == Some(TokenKind::OpenParen)
                && tokens.matching_end(first)? == last
            {
                return flatten_operand(tokens, text, first + 1..last);
            }
            let (start16, length16) = span_over(text, tokens, first, last)?;
            let slice = text.get(
                utf16_offset_to_byte(text, start16)..utf16_offset_to_byte(text, start16 + length16),
            )?;
            Some((
                Component {
                    text: slice.to_owned(),
                    identifier: false,
                },
                None,
            ))
        }
    }
}

/// The `(quote byte, is-triple)` style of a bare string segment.
pub(crate) fn string_style(tokens: &TokenTable, text: &str, index: usize) -> Option<(u8, bool)> {
    let _ = text;
    let lexeme = tokens.lexeme(text, index);
    let quote = if lexeme.starts_with('r') {
        *lexeme.as_bytes().get(1)?
    } else {
        *lexeme.as_bytes().first()?
    };
    Some((quote, quote_run(lexeme.as_bytes(), quote, false) > 1))
}

/// The triple-quote flag of the operand's head segment.
pub(crate) fn quote_is_triple(tokens: &TokenTable, text: &str, index: usize) -> Option<bool> {
    string_style(tokens, text, index).map(|(_, triple)| triple)
}

/// The contents of one bare string segment: the lexeme minus its outer
/// quote runs (escapes stay as written).
pub(crate) fn segment_contents(tokens: &TokenTable, text: &str, index: usize) -> Option<String> {
    let lexeme = tokens.lexeme(text, index);
    let quote = if lexeme.starts_with('r') {
        *lexeme.as_bytes().get(1)?
    } else {
        *lexeme.as_bytes().first()?
    };
    let lead = quote_run(lexeme.as_bytes(), quote, false);
    let trail = quote_run(lexeme.as_bytes(), quote, true);
    // The scanner's unterminated-string recovery emits a lexeme of pure
    // quote bytes (the synthetic closer is excluded) — the quote runs then
    // overlap and the slice would invert. Same posture as the raw-string
    // bail: the segment drops.
    if lead + trail > lexeme.len() {
        return None;
    }
    let start = tokens.get(index)?.byte_offset as usize;
    Some(text[start + lead..start + lexeme.len() - trail].to_owned())
}

/// `Map()` → `{}` (written type arguments ride along), `Set()` →
/// `<dynamic>{}` unless the context declares `Set`, `Set.from([…])` /
/// `Set.of([…])` and `[…].toSet()` → the set literal — the official
/// `ConvertToMapLiteral` / `ConvertToSetLiteral` replacements.
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
pub(crate) fn collection_literal(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    // The `.toSet()` face: an optional type argument group, the list
    // literal, then `.toSet()`.
    let mut cursor = first;
    let mut head_args = String::new();
    if kind_of(tokens, cursor) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(cursor)?;
        head_args = token_text(tokens, text, cursor..close + 1)?;
        cursor = close + 1;
    }
    // The `.toSet()` face: the target list literal — a `[…]` group, or
    // the scanner's single `[]` token for the empty literal — then
    // `.toSet()`.
    if matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::OpenSquare | TokenKind::Index)
    ) {
        let (close, contents) = if kind_of(tokens, cursor) == Some(TokenKind::Index) {
            (cursor, String::new())
        } else {
            let close = tokens.matching_end(cursor)?;
            (close, token_text(tokens, text, cursor + 1..close)?)
        };
        if close + 4 == last
            && kind_of(tokens, close + 1) == Some(TokenKind::Period)
            && word_at(tokens, text, close + 2, "toSet")
            && kind_of(tokens, close + 3) == Some(TokenKind::OpenParen)
            && kind_of(tokens, last) == Some(TokenKind::CloseParen)
        {
            let unambiguous = !contents.is_empty() && !contents.starts_with("...");
            let prefix = if head_args.is_empty() && !unambiguous {
                "<dynamic>"
            } else {
                ""
            };
            let (start16, length16) = span_over(text, tokens, first, last)?;
            return Some((
                start16,
                length16,
                format!("{head_args}{prefix}{{{contents}}}"),
            ));
        }
        return None;
    }
    // The creation faces: the callee identifier (after an optional
    // `new`), its type arguments, then the constructor shape.
    let mut cursor = cursor.max(first);
    if kind_of(tokens, cursor) == Some(TokenKind::Keyword(Keyword::New)) {
        cursor += 1;
    }
    let name = tokens.lexeme(text, cursor).to_owned();
    if !matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::Identifier | TokenKind::Keyword(_))
    ) || !matches!(
        name.as_str(),
        "Map" | "LinkedHashMap" | "Set" | "LinkedHashSet"
    ) {
        return None;
    }
    cursor += 1;
    // The written type arguments ride into the replacement, then the
    // constructor shape: `.from`/`.of` (the named face) or `(`.
    let mut ctor_args = String::new();
    if kind_of(tokens, cursor) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(cursor)?;
        ctor_args = token_text(tokens, text, cursor..close + 1)?;
        cursor = close + 1;
    }
    let named = kind_of(tokens, cursor) == Some(TokenKind::Period)
        && (word_at(tokens, text, cursor + 1, "from") || word_at(tokens, text, cursor + 1, "of"));
    if named {
        cursor += 2;
    }
    if kind_of(tokens, cursor) != Some(TokenKind::OpenParen) {
        return None;
    }
    let close = tokens.matching_end(cursor)?;
    if !named {
        // The unnamed face: empty parentheses close the creation.
        if close != last || close != cursor + 1 {
            return None;
        }
        let prefix = match name.as_str() {
            "Set" | "LinkedHashSet"
                if !declares_set_context(tokens, text, first).unwrap_or(false) =>
            {
                "<dynamic>"
            }
            _ => "",
        };
        let (start16, length16) = span_over(text, tokens, first, last)?;
        return Some((start16, length16, format!("{ctor_args}{prefix}{{}}")));
    }
    // The single argument is a list literal — a `[…]` group or the
    // scanner's single `[]` token — and the argument list closes right
    // after it.
    let mut arg = cursor + 1;
    let mut list_args = String::new();
    if kind_of(tokens, arg) == Some(TokenKind::Lt) {
        let close = tokens.matching_end(arg)?;
        list_args = token_text(tokens, text, arg..close + 1)?;
        arg = close + 1;
    }
    let (close, elements) = match kind_of(tokens, arg) {
        Some(TokenKind::OpenSquare) => {
            let close = tokens.matching_end(arg)?;
            (close, token_text(tokens, text, arg + 1..close)?)
        }
        Some(TokenKind::Index) => (arg, String::new()),
        _ => return None,
    };
    if close + 1 != last || tokens.matching_end(cursor)? != last {
        return None;
    }
    let unambiguous = !elements.is_empty() && !elements.starts_with("...");
    let inferred = !ctor_args.is_empty()
        || !list_args.is_empty()
        || unambiguous
        || declares_set_context(tokens, text, first).unwrap_or(false);
    let prefix = if inferred { "" } else { "<dynamic>" };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!("{ctor_args}{prefix}{{{elements}}}"),
    ))
}

/// Whether the creation sits in a declaration whose written type is
/// `Set` (`Set s = Set()`): the official inference face, read backwards
/// over the declaration shape `= name [type]`.
pub(crate) fn declares_set_context(tokens: &TokenTable, text: &str, first: usize) -> Option<bool> {
    let Some(at) = first
        .checked_sub(1)
        .filter(|&i| kind_of(tokens, i) == Some(TokenKind::Eq))
    else {
        return Some(false);
    };
    let Some(name) = at
        .checked_sub(1)
        .filter(|&i| kind_of(tokens, i) == Some(TokenKind::Identifier))
    else {
        return Some(false);
    };
    let mut cursor = name - 1;
    // Skip a type-argument group, then expect the type identifier.
    if matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt)
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
        cursor = cursor.checked_sub(1)?;
    }
    Some(
        kind_of(tokens, cursor) == Some(TokenKind::Identifier)
            && tokens.lexeme(text, cursor) == "Set",
    )
}

/// `['a']..add(x)` → `['a', x]` (and the `addAll` literal form): the
/// cascade sections after the target compose into the literal — one
/// whole-tail replacement per cascade (the official insert-and-delete
/// pair folded; a section that is not `add`/`addAll` ends the tail and
/// stays put).
pub(crate) fn inlined_adds(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (name_index, _) = hit_tokens(tokens, range)?;
    let name = tokens.lexeme(text, name_index);
    if !matches!(name, "add" | "addAll")
        || kind_of(tokens, name_index + 1) != Some(TokenKind::OpenParen)
    {
        return None;
    }
    let lparen = name_index + 1;
    let rparen = tokens.matching_end(lparen)?;
    if depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    })
    .is_some()
    {
        return None; // the official face takes exactly one argument
    }
    // Backwards: the cascade marker, then the target literal's `]`.
    match kind_of(tokens, name_index.checked_sub(1)?) {
        Some(TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod) => {}
        _ => return None,
    }
    let close_square = name_index - 2;
    // The target literal's `]`: a `[…]` group — or the scanner's single
    // `[]` token for the empty literal, its own opener and closer.
    let open_square = match kind_of(tokens, close_square) {
        Some(TokenKind::Index) => close_square,
        Some(TokenKind::CloseSquare) => {
            let mut depth = 0u32;
            let mut cursor = close_square;
            loop {
                cursor = cursor.checked_sub(1)?;
                match kind_of(tokens, cursor)? {
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth += 1;
                    }
                    TokenKind::OpenParen
                    | TokenKind::OpenCurly
                    | TokenKind::StringInterpolationExpression => depth = depth.checked_sub(1)?,
                    TokenKind::OpenSquare => {
                        if depth == 0 {
                            break cursor;
                        }
                        depth = depth.checked_sub(1)?;
                    }
                    _ => {}
                }
            }
        }
        _ => return None,
    };
    // The hit's own section opens the composition; the sections after it
    // follow while they are `add`/`addAll`.
    let target_nonempty = close_square > open_square + 1;
    let mut additions = vec![section_addition(tokens, text, name, lparen, rparen)?];
    let mut cursor = rparen;
    loop {
        let next = cursor + 1;
        if !matches!(
            kind_of(tokens, next),
            Some(TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod)
        ) {
            break;
        }
        let section = next + 1;
        let section_name = tokens.lexeme(text, section);
        if !matches!(section_name, "add" | "addAll")
            || kind_of(tokens, section + 1) != Some(TokenKind::OpenParen)
        {
            break;
        }
        let section_rparen = tokens.matching_end(section + 1)?;
        match section_addition(tokens, text, section_name, section + 1, section_rparen) {
            Some(addition) => {
                additions.push(addition);
                cursor = section_rparen;
            }
            None => break,
        }
    }
    let (start16, length16) = span_over(text, tokens, close_square, cursor)?;
    let joined = additions.join(", ");
    // The empty literal is one `[]` token: the whole literal is rewritten.
    let replacement = if close_square == open_square {
        format!("[{joined}]")
    } else {
        format!("{}{}]", if target_nonempty { ", " } else { "" }, joined)
    };
    Some((start16, length16, replacement))
}

/// The composed element source of one `add`/`addAll` section: the single
/// argument, or the list literal's elements for `addAll` (`None` when the
/// section is not the official face — multiple arguments, a non-literal
/// `addAll` argument, an empty one).
pub(crate) fn section_addition(
    tokens: &TokenTable,
    text: &str,
    name: &str,
    lparen: usize,
    rparen: usize,
) -> Option<String> {
    if depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Comma
    })
    .is_some()
    {
        return None; // the official face takes exactly one argument
    }
    if name == "addAll" {
        if kind_of(tokens, lparen + 1) != Some(TokenKind::OpenSquare) {
            return None; // the spread rule's face, not ours
        }
        let sq = tokens.matching_end(lparen + 1)?;
        if sq + 1 != rparen {
            return None;
        }
        let elements = token_text(tokens, text, lparen + 2..sq)?;
        if elements.is_empty() {
            return None; // the official face returns without a fix here
        }
        Some(elements)
    } else {
        let argument = token_text(tokens, text, lparen + 1..rparen)?;
        if argument.is_empty() {
            return None;
        }
        Some(argument)
    }
}

/// `var f = () {};` → `void f() {}` (the official
/// `ConvertToFunctionDeclaration`, its resolution-free face): the
/// declaration prefix goes, the name keeps, the closure's parameters and
/// body follow the inferred-`void` return type (`Future<void>` under
/// `async`). Value-returning closures and expression bodies drop — their
/// return type is not derivable here.
pub(crate) fn function_declaration(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first) != Some(TokenKind::Identifier)
        || kind_of(tokens, first + 1) != Some(TokenKind::Eq)
        || kind_of(tokens, last + 1) != Some(TokenKind::Semicolon)
    {
        return None;
    }
    let name = tokens.lexeme(text, first);
    // The declaration prefix, walked backwards: `late`/`final`/`const`/
    // `var`, then the optional written type (dotted, nullable, with type
    // arguments) — a boundary that is not a statement edge drops the fix
    // (a sibling variable, a bare expression context).
    let mut cursor = first;
    while let Some(prev) = cursor
        .checked_sub(1)
        .and_then(|index| kind_of(tokens, index))
    {
        match prev {
            TokenKind::Question
            | TokenKind::Identifier
            | TokenKind::Keyword(
                Keyword::Final | Keyword::Const | Keyword::Var | Keyword::Late | Keyword::Function,
            ) => {}
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                let mut depth = 0u32;
                loop {
                    cursor -= 1;
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
                }
            }
            _ => break,
        }
        cursor -= 1;
    }
    if let Some(boundary) = cursor.checked_sub(1)
        && !matches!(
            kind_of(tokens, boundary),
            Some(TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly)
        )
    {
        return None;
    }
    // The closure: the parameter list, an optional async marker after it,
    // then a block body without a value `return`.
    let scan = first + 2;
    if kind_of(tokens, scan) != Some(TokenKind::OpenParen) {
        return None;
    }
    let params_end = tokens.matching_end(scan)?;
    let mut return_type = "void ";
    let mut marker = params_end + 1;
    match kind_of(tokens, marker) {
        Some(TokenKind::Keyword(Keyword::Async)) => {
            return_type = "Future<void> ";
            marker += 1;
            if kind_of(tokens, marker) == Some(TokenKind::Star) {
                return None;
            }
        }
        Some(TokenKind::Keyword(Keyword::Sync)) => return None,
        _ => {}
    }
    if kind_of(tokens, marker) != Some(TokenKind::OpenCurly) {
        return None;
    }
    for index in first + 2..=last {
        if kind_of(tokens, index) == Some(TokenKind::Keyword(Keyword::Return))
            && kind_of(tokens, index + 1) != Some(TokenKind::Semicolon)
        {
            return None;
        }
    }
    let (start16, length16) = span_over(text, tokens, cursor, last + 1)?;
    let closure = &text
        [tokens.get(first + 2)?.byte_offset as usize..tokens.get(last + 1)?.byte_offset as usize];
    Some((start16, length16, format!("{return_type}{name}{closure}")))
}
