use super::*;


// ---------------------------------------------------------------------------
// Semantic source: the F6 written-surface family
//
// The semantic codes whose official fix is a single-span mechanical rewrite
// provable from the diagnostic's span, the file text, and the token stream —
// upstream Dart SDK source, `pkg/analysis_server/.../dart/`, producers named
// per row. The spans the engine emits anchor the rewrite: a keyword row
// carries the keyword's own range, an operator row the operator's, a
// declaration row the declared name's (the declarator/statement is rebuilt
// from the token stream). A hit whose slice does not verify its shape drops
// rather than patches.
// ---------------------------------------------------------------------------

/// `RemoveQuestionMark` over the reported type's `?` — the
/// `unnecessary_question_mark` row spans the `?` itself (the `Null?` /
/// `dynamic?` head), the `unnecessary_null_check_pattern` row the pattern
/// operator, and the `nullable_type_in_catch_clause` row the `name?` pair
/// (upstream `range.token(questionMark)` deletes the `?` alone in every
/// face; the span ends at it).
pub(crate) fn drop_type_question(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if slice == "?" {
        return Some((input.range.start, 1, String::new()));
    }
    if slice.ends_with('?') && !slice[..slice.len() - 1].contains('?') {
        return Some((input.range.end - 1, 1, String::new()));
    }
    None
}

/// `RemoveNonNullAssertion` over the reported `!` — the expression face's
/// `range.entity(bangToken)` (the `!` alone) and the pattern face's
/// `startStart(bang, bang.next)` (the `!` plus its trivia run). The
/// `pattern` flag picks the producer's shape per row.
pub(crate) fn drop_bang(input: &FixInput<'_>, pattern: bool) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if slice != "!" {
        return None;
    }
    if !pattern {
        return Some((input.range.start, 1, String::new()));
    }
    // The pattern face deletes the token plus the trivia run to the next
    // real token (comments ride `next`'s precedingComments edge).
    let start = utf16_offset_to_byte(text, input.range.start);
    let run = trivia_run(text.get(start + 1..)?);
    Some((
        input.range.start,
        1 + u32::try_from(run).ok()?,
        String::new(),
    ))
}

pub(crate) fn drop_bang_expression(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    drop_bang(input, false)
}

pub(crate) fn drop_bang_pattern(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    drop_bang(input, true)
}

/// `ReplaceWithNotNullAware`: the reported null-aware operator becomes its
/// non-aware form — `?.` → `.`, `?..` → `..`, and on the index face the
/// `?[` span's `?` alone goes (upstream `range.token(question)` deletes the
/// question, the `[` stays — `range.token(operator)` replaces the others).
/// The span slices to the operator exactly; any other text refuses.
pub(crate) fn not_null_aware_operator(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let replacement = match slice {
        "?." => ".",
        "?.." => "..",
        "?[" => return Some((input.range.start, 1, String::new())),
        "...?" => "...",
        _ => return None,
    };
    Some((
        input.range.start,
        u32::try_from(slice.len()).ok()?,
        replacement.to_owned(),
    ))
}

/// `type_check_with_null`: the reported `x is Null` / `x is! Null` becomes
/// `x == null` / `x != null` — the official `UseEqEqNull`/`UseNotEqNull`
/// producers append the comparison after the operand; the span here covers
/// the whole check, so the rewrite replaces the ` is Null` / ` is! Null`
/// tail in place (the operand's own text survives untouched).
pub(crate) fn is_null_to_eq_null(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let trimmed = slice.trim_end();
    let (head, eq) = if let Some(head) = trimmed.strip_suffix("is! Null") {
        (head, "!=")
    } else if let Some(head) = trimmed.strip_suffix("is Null") {
        (head, "==")
    } else {
        return None;
    };
    let head = head.trim_end();
    if head.is_empty() {
        return None;
    }
    let cut = input.range.start + utf16_count(head);
    Some((
        cut,
        input.range.start + u32::try_from(trimmed.len()).ok()? - cut,
        format!(" {eq} null"),
    ))
}

/// `ReplaceNewWithConst`: the reported `new` becomes `const` — the
/// `non_const_call_to_literal_constructor` row spans the whole `new`-led
/// creation; the keyword rides its head.
pub(crate) fn new_to_const(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if !slice.starts_with("new ") && slice != "new" {
        return None;
    }
    Some((input.range.start, 3, "const".to_owned()))
}

/// `RemoveConst` over the report's enclosing `const` — the
/// `non_constant_list_element` / `non_constant_map_element` /
/// `non_constant_set_element` / `const_eval_method_invocation` rows report
/// inside the `const` literal or creation the keyword heads (upstream
/// `_RemoveConst`'s `ExpressionImpl` arm resolves the `constantContext`'s
/// keyword). The token stream walks back from the hit to the depth-0
/// `const` heading the construct; the deletion runs `startStart(const,
/// const.next)` — trivia included (the family shares the
/// `RemoveUnnecessaryConst` deletion shape).
pub(crate) fn drop_enclosing_const(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _last) = hit_tokens(tokens, &input.range)?;
    // The literal's (or argument list's) own opener is the first
    // `[`/`{`/`(` reached walking back at depth 0 — inner groups jump
    // over their matching closer, so a `const` inside the literal never
    // surfaces.
    let mut cursor = first;
    let open = loop {
        let index = cursor.checked_sub(1)?;
        match kind_of(tokens, index)? {
            // The `const` heading the reported construct itself
            // (`const m()` — the invocation span follows the keyword).
            TokenKind::Keyword(Keyword::Const) => {
                let start16 = tokens.get(index)?.utf16_offset;
                return remove_keyword_across_comments(text, start16, "const");
            }
            TokenKind::CloseCurly | TokenKind::CloseSquare | TokenKind::CloseParen => {
                cursor = matching_begin(tokens, index)?;
            }
            TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::OpenParen => break index,
            TokenKind::Semicolon | TokenKind::Eof => return None,
            _ => cursor = index,
        }
    };
    // The `const` heads the opener — `const [`, `const <int>[`, or the
    // creation's `const C(` where the walk crossed the name. A `<…>`
    // group or a name chain may sit between.
    let mut cursor = open;
    loop {
        let index = cursor.checked_sub(1)?;
        match kind_of(tokens, index)? {
            TokenKind::Keyword(Keyword::Const) => {
                let start16 = tokens.get(index)?.utf16_offset;
                return remove_keyword_across_comments(text, start16, "const");
            }
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                cursor = angle_group_start(tokens, index)?;
            }
            TokenKind::Identifier | TokenKind::Period => cursor = index,
            _ => return None,
        }
    }
}

/// `unnecessary_cast`: the ` as T` tail drops — the official
/// `RemoveUnnecessaryCast` deletes `endEnd(operand, asExpression)`; the
/// report spans the whole cast expression, so the cut is the ` as` run at
/// its tail (the deepest `as` at depth 0 binds last).
pub(crate) fn drop_as_cast(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    // The last depth-0 ` as ` in the span is the cast the code reports.
    // String literals and comments cannot carry the two-letter word at a
    // byte boundary we accept here — the span comes from the semantic
    // layer, so the `as` inside it is real.
    let mut cut = None;
    let bytes = slice.as_bytes();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'a' && bytes[i + 1] == b's' {
            let before_ok = i == 0 || !is_identifier_char(bytes[i - 1] as char);
            let after_ok = !is_identifier_char(bytes[i + 2] as char);
            if before_ok && after_ok {
                cut = Some(i);
            }
        }
        i += 1;
    }
    let at = cut?;
    // The deletion runs from the whitespace before `as` through the type's
    // end (the span's own end).
    let mut lead = at;
    while lead > 0 && matches!(bytes[lead - 1], b' ' | b'\t') {
        lead -= 1;
    }
    let offset = input.range.start + utf16_count(&slice[..lead]);
    Some((offset, input.range.end - offset, String::new()))
}

/// `unnecessary_wildcard_pattern`: the `_` operand of a `&&` pattern goes
/// with its operator — a left operand cuts `startStart(parent,
/// rightOperand)` (the `_` through the `&&`), a right operand cuts
/// `endEnd(leftOperand, parent)` (the `&&` through the `_`; official
/// `RemoveUnnecessaryWildcardPattern`). The report spans the `_` itself;
/// parenthesized wraps ride inside it.
pub(crate) fn drop_wildcard_and_operand(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    let mut inner = slice.trim();
    while inner.starts_with('(') && inner.ends_with(')') {
        inner = inner[1..inner.len() - 1].trim();
    }
    if inner != "_" {
        return None;
    }
    let start = utf16_offset_to_byte(text, input.range.start);
    let end = utf16_offset_to_byte(text, input.range.end);
    if let Some(op) = text[..start].trim_end().strip_suffix("&&") {
        // Right operand: `endEnd(leftOperand, parent)` — the cut starts at
        // the left operand's own end, before the `&&` gap.
        let op16 = utf16_count(text[..op.len()].trim_end());
        return Some((op16, input.range.end - op16, String::new()));
    }
    let after = text[end..].trim_start();
    if let Some(with_op) = after.strip_prefix("&&") {
        // Left operand: cut the span through the `&&` and the run to the
        // next operand (`startStart(node, nextOperand)`).
        let gap = text[end..].len() - after.len();
        let trail = with_op.len() - with_op.trim_start().len();
        let tail = end + gap + 2 + trail;
        let length = utf16_count(&text[start..tail]);
        return Some((input.range.start, length, String::new()));
    }
    None
}

/// `wrong_number_of_type_arguments_constructor`: the misplaced `<…>` group
/// after a constructor's named segment moves onto the class name —
/// `C.named<int>()` → `C<int>.named()` (official `MoveTypeArgumentsToClass`
/// — its insertion-plus-deletion folds into the one contiguous replacement
/// the segment already is). The report spans the `<…>` group. A class name
/// already carrying type arguments refuses — the official's own guard.
pub(crate) fn move_type_args_to_class(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let lt = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, lt)? != TokenKind::Lt {
        return None;
    }
    let gt = angle_group_end(tokens, lt)?;
    // The `<…>` follows `.ctor` — walk back over the constructor name and
    // the dot to the class identifier. A `<…>` already after it is the
    // official producer's own refuse shape.
    let ctor = lt.checked_sub(1)?;
    if !matches!(kind_of(tokens, ctor)?, TokenKind::Identifier) {
        return None;
    }
    if kind_of(tokens, ctor - 1)? != TokenKind::Period {
        return None;
    }
    let class = ctor - 2;
    if !matches!(kind_of(tokens, class)?, TokenKind::Identifier) {
        return None;
    }
    let args_end = utf16_end(tokens, text, gt)?;
    let class_end = utf16_end(tokens, text, class)?;
    let args = slice16(text, &(input.range.start..args_end))?;
    Some((
        class_end,
        args_end.checked_sub(class_end)?,
        format!("{args}.{}", tokens.lexeme(text, ctor)),
    ))
}

/// `RemoveNameFromDeclarationClause`: the reported name leaves its clause —
/// the whole clause when it is the only name (official
/// `range.deletionRange(clause)`), the name plus one adjacent comma
/// otherwise (`range.nodeInList`). `extends` drops the clause wholesale;
/// `implements`/`with`/`on` keep a lone name only when it is alone.
pub(crate) fn drop_clause_name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _last) = hit_tokens(tokens, &input.range)?;
    // The reported name is one identifier (a prefixed `a.B` name spans two
    // — the producer drops the whole NamedType either way). The clause
    // keyword sits behind it: `extends`, `implements`, `with`, or `on`.
    let clause_kw = {
        let mut cursor = first;
        loop {
            let index = cursor.checked_sub(1)?;
            match kind_of(tokens, index)? {
                TokenKind::Keyword(
                    Keyword::Extends | Keyword::Implements | Keyword::With | Keyword::On,
                ) => break index,
                TokenKind::Comma | TokenKind::Identifier | TokenKind::Period => cursor = index,
                TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                    cursor = angle_group_start(tokens, index)?;
                }
                TokenKind::Question => cursor = index,
                _ => return None,
            }
        }
    };
    // The clause's name list: the dotted/generic names after the keyword,
    // comma-separated. A lone name drops the whole clause; a member of a
    // list drops itself with one adjacent comma (`nodeInList`).
    let name_end = |start: usize| -> Option<usize> {
        let mut tail = start;
        loop {
            match kind_of(tokens, tail + 1) {
                Some(TokenKind::Period) => tail += 2,
                Some(TokenKind::Lt) => tail = angle_group_end(tokens, tail + 1)?,
                _ => return Some(tail),
            }
        }
    };
    let mut count = 0usize;
    let mut scan = clause_kw + 1;
    loop {
        match kind_of(tokens, scan) {
            Some(TokenKind::Identifier | TokenKind::Keyword(_)) => count += 1,
            _ => break,
        }
        scan = name_end(scan)? + 1;
        if kind_of(tokens, scan) == Some(TokenKind::Comma) {
            scan += 1;
            continue;
        }
        break;
    }
    if count == 1 {
        // The whole clause goes — the keyword through the name's end.
        let start = utf16_end(tokens, text, clause_kw.checked_sub(1)?)?;
        let end = utf16_end(tokens, text, name_end(first)?)?;
        return Some((start, end.checked_sub(start)?, String::new()));
    }
    // A member of the list: `nodeInList` — a non-last name cuts
    // `startStart(node, next)` — through the next name's own start, comma
    // and gap included — the last name cuts `endEnd(previous, node)`.
    let end_tok = name_end(first)?;
    if kind_of(tokens, end_tok + 1) == Some(TokenKind::Comma) {
        let start = tokens.get(first)?.utf16_offset;
        let end = tokens.get(end_tok + 2)?.utf16_offset;
        return Some((start, end.checked_sub(start)?, String::new()));
    }
    // The last name: the preceding comma goes with it.
    let comma = {
        let mut c = first.checked_sub(1)?;
        while kind_of(tokens, c) == Some(TokenKind::Period)
            || matches!(
                kind_of(tokens, c),
                Some(TokenKind::Identifier | TokenKind::Keyword(_))
            )
            || matches!(
                kind_of(tokens, c),
                Some(TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt)
            )
        {
            if matches!(
                kind_of(tokens, c),
                Some(TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt)
            ) {
                c = angle_group_start(tokens, c)?;
            }
            c = c.checked_sub(1)?;
        }
        if kind_of(tokens, c) == Some(TokenKind::Comma) {
            c
        } else {
            return None;
        }
    };
    // `endEnd(previous, node)`: from the previous name's end — the gap
    // before its comma rides — through this name's end.
    let start = utf16_end(tokens, text, comma.checked_sub(1)?)?;
    let end = utf16_end(tokens, text, end_tok)?;
    Some((start, end.checked_sub(start)?, String::new()))
}

/// `duplicate_hidden_name` / `duplicate_shown_name` / `undefined_hidden_name` /
/// `undefined_shown_name`: the reported name leaves its `show`/`hide`
/// combinator (official `RemoveNameFromCombinator`'s
/// `rangeForNameInCombinator`). A listed name cuts `startStart(name, next)`
/// when it is not last and `endEnd(previous, name)` when it is; the clause's
/// only name takes the whole combinator down (`rangeForCombinator` — the
/// previous directive token's end through the clause's end, or the
/// neighboring combinator's edge when the directive carries several). The
/// report anchors the name token.
pub(crate) fn drop_combinator_name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The combinator keyword sits behind the reported name across its
    // comma-separated siblings; the walk stops at the directive head.
    let keyword = {
        let mut cursor = first;
        loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::Keyword(Keyword::Show | Keyword::Hide) => break cursor,
                TokenKind::Identifier
                | TokenKind::Comma
                | TokenKind::Keyword(_)
                | TokenKind::String => {}
                TokenKind::CloseParen => cursor = matching_begin(tokens, cursor)?,
                _ => return None,
            }
        }
    };
    // The clause's name list: comma-separated names after the keyword, in
    // written order, with the reported name's position.
    let mut count = 0usize;
    let mut hit_index = None;
    let mut scan = keyword;
    let mut clause_last = keyword;
    loop {
        match kind_of(tokens, scan + 1)? {
            TokenKind::Identifier | TokenKind::Keyword(_) => {
                scan += 1;
                if scan == first {
                    hit_index = Some(count);
                }
                count += 1;
                clause_last = scan;
                if kind_of(tokens, scan + 1) == Some(TokenKind::Comma) {
                    scan += 1;
                    continue;
                }
                break;
            }
            _ => break,
        }
    }
    let hit_index = hit_index?;
    // Shape verification over the owning directive: behind the clause the
    // walk may cross only the directive's own tokens before its head
    // keyword, and past the clause end only further combinator keywords to
    // the `;` — `next_combinator` is the clause's right neighbor when one
    // stands there.
    let mut cursor = keyword;
    loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::Keyword(Keyword::Import | Keyword::Export) => break,
            TokenKind::Keyword(Keyword::Show | Keyword::Hide)
            | TokenKind::Identifier
            | TokenKind::Comma
            | TokenKind::Keyword(_)
            | TokenKind::String => {}
            TokenKind::CloseParen => cursor = matching_begin(tokens, cursor)?,
            _ => return None,
        }
    }
    let mut next_combinator = None;
    cursor = clause_last;
    loop {
        cursor += 1;
        match kind_of(tokens, cursor)? {
            TokenKind::Keyword(Keyword::Show | Keyword::Hide) => {
                next_combinator.get_or_insert(cursor);
            }
            TokenKind::Semicolon | TokenKind::Eof => break,
            _ => return None,
        }
    }
    // `endEnd(previous, last)`: the previous token's end through the last
    // token's end — the gap between them rides.
    let end_end = |previous: usize, last: usize| -> Option<(u32, u32, String)> {
        let start16 = utf16_end(tokens, text, previous)?;
        let end16 = utf16_end(tokens, text, last)?;
        Some((start16, end16.checked_sub(start16)?, String::new()))
    };
    // `startStart(first, last)`: the first token's start through the last
    // token's start.
    let start_start = |first: usize, last: usize| -> Option<(u32, u32, String)> {
        let start16 = tokens.get(first)?.utf16_offset;
        let end16 = tokens.get(last)?.utf16_offset;
        Some((start16, end16.checked_sub(start16)?, String::new()))
    };
    if count == 1 {
        // The whole combinator goes: `endEnd(previousToken, combinator)`
        // when it is the directive's only or last one — the previous
        // combinator's end is the token before this keyword then — and
        // `startStart(combinator, next)` before a following one.
        match next_combinator {
            None => end_end(keyword - 1, clause_last),
            Some(next) => start_start(keyword, next),
        }
    } else if hit_index + 1 < count {
        // Not the last name: `startStart(name, next)` — the next name's
        // own start bounds the cut, comma and gap included.
        start_start(first, first + 2)
    } else {
        // The last name: `endEnd(previous, name)` — the preceding name's
        // end through this name's end.
        end_end(first - 2, first)
    }
}

/// `override_on_non_overriding_member`: the `@override` annotation leaves —
/// the `@` through the trivia run to the next real token, stopping at that
/// token's attached comments (official `RemoveAnnotation`'s
/// `startStart(annotation, followingToken.precedingComments ?? followingToken)`
/// deletion over the member's metadata). The report anchors the member's
/// name; the walk back ends at the previous member's end.
pub(crate) fn remove_override_annotation(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    let at = {
        let mut cursor = first;
        loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::At if word_at(tokens, text, cursor + 1, "override") => break cursor,
                TokenKind::Semicolon
                | TokenKind::OpenCurly
                | TokenKind::CloseCurly
                | TokenKind::Eof => {
                    return None;
                }
                _ => {}
            }
        }
    };
    // The annotation's end token: a bare `@override` or one with an
    // argument list (`@override()`).
    let end_token = if kind_of(tokens, at + 2) == Some(TokenKind::OpenParen) {
        tokens.matching_end(at + 2)?
    } else {
        at + 1
    };
    let next = end_token + 1;
    if kind_of(tokens, next)? == TokenKind::Eof {
        return None;
    }
    let ann_end = token_end(tokens.get(end_token)?);
    let next_start = tokens.get(next)?.byte_offset as usize;
    // The run between two real tokens is trivia — whitespace and comments
    // only — so the first `/` is the attached comments' start, where the
    // official deletion stops.
    let cut = text
        .get(ann_end..next_start)?
        .find('/')
        .map_or(next_start, |i| ann_end + i);
    let start16 = tokens.get(at)?.utf16_offset;
    let cut16 = utf16_count(&text[..cut]);
    Some((start16, cut16.checked_sub(start16)?, String::new()))
}

/// `invocation_of_non_function_expression`: the `()` after the non-function
/// callee goes (official `RemoveParenthesesInGetterInvocation` — the parent
/// `FunctionExpressionInvocation`'s `range.node(argumentList)` deletion). The
/// report anchors the callee expression; a trailing type-argument group rides
/// it (`f<int>()`).
pub(crate) fn drop_getter_invocation_parens(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (_, mut last) = hit_tokens(tokens, &input.range)?;
    if matches!(
        kind_of(tokens, last)?,
        TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt
    ) {
        last = angle_group_start(tokens, last)?;
    }
    let open = last + 1;
    if kind_of(tokens, open)? != TokenKind::OpenParen {
        return None;
    }
    let close = tokens.matching_end(open)?;
    let start = tokens.get(open)?.utf16_offset;
    let end = utf16_end(tokens, text, close)?;
    Some((start, end.checked_sub(start)?, String::new()))
}

/// `missing_default_value_for_parameter`: `required ` lands at the
/// parameter's head (official `AddRequiredKeyword` — the insertion at the
/// parameter's first token after its metadata). A redundant `@required`
/// annotation folds into the replacement when it stands last in the
/// metadata (the official delete-then-insert is one contiguous span there);
/// annotations after it make the two edits disjoint and refuse. The
/// named-parameter face: the group opener must be `{` — the positional
/// shapes the shared registry name covers are `MakeVariableNullable`'s legs
/// and stay absent. The report anchors the parameter's name.
pub(crate) fn add_required_keyword(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The parameter head: walk back over the parameter's own tokens —
    // metadata with argument groups, modifiers, type — to the enclosing
    // group's boundary.
    let mut cursor = first;
    let boundary = loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                cursor = matching_begin(tokens, cursor)?;
            }
            TokenKind::Comma
            | TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly => break cursor,
            TokenKind::Eof => return None,
            _ => {}
        }
    };
    if kind_of(tokens, boundary)? != TokenKind::OpenCurly {
        return None;
    }
    // The metadata run: whole annotations from the boundary, then the
    // parameter's own first token. The walk recognizes the annotation
    // shapes (`@name`, `@name()`, `@p.name`, `@p.name<T>()`); anything else
    // ends the run.
    let annotation_end = |at: usize| -> Option<usize> {
        let mut cursor = at + 1;
        if kind_of(tokens, cursor + 1)? == TokenKind::Period {
            cursor += 2;
        }
        if kind_of(tokens, cursor + 1)? == TokenKind::Lt {
            cursor = tokens.matching_end(cursor + 1)?;
        }
        if kind_of(tokens, cursor + 1)? == TokenKind::OpenParen {
            cursor = tokens.matching_end(cursor + 1)?;
        }
        Some(cursor)
    };
    let mut scan = boundary;
    let mut required_at = None;
    loop {
        let next = scan + 1;
        if kind_of(tokens, next)? != TokenKind::At {
            break;
        }
        if word_at(tokens, text, next + 1, "required") {
            required_at = Some(next);
        }
        scan = annotation_end(next)?;
    }
    // The parameter's own first token — already `required` is the shape the
    // diagnostic cannot fire on.
    let head = scan + 1;
    if word_at(tokens, text, head, "required") {
        return None;
    }
    let insert_at = tokens.get(head)?.utf16_offset;
    let Some(at) = required_at else {
        return Some((insert_at, 0, "required ".to_owned()));
    };
    // The `@required` deletion folds in only when it stands last in the
    // metadata: its deletion end is then the insertion point.
    let end_token = annotation_end(at)?;
    if end_token + 1 != head {
        return None;
    }
    let start = tokens.get(at)?.utf16_offset;
    Some((start, insert_at.checked_sub(start)?, "required ".to_owned()))
}

/// `unreachable_switch_case` / `unreachable_switch_default`: the member's
/// lines go — the `case`/`default` head through its statements' end (the
/// official `RemoveDeadCode` `SwitchMember` arm deletes the member's
/// `deletionRange`; a previous member falling through keeps the statements
/// — the row's own head is the probe). The report anchors the `case` /
/// `default` keyword.
pub(crate) fn drop_switch_member(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let head = token_starting_at(tokens, input.range.start)?;
    match kind_of(tokens, head)? {
        TokenKind::Keyword(Keyword::Case | Keyword::Default) => {}
        _ => return None,
    }
    // The member's end: the next `case`/`default`/`}` at depth 0 — the
    // member's own `:`/`=>` opens its body, and the body's statements run
    // to the next member head or the switch's `}`.
    let mut cursor = head;
    let end = loop {
        cursor += 1;
        match kind_of(tokens, cursor)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                // A `{` at depth 0 in the member head position is a block
                // statement of the body — keep walking; the next case
                // keyword still ends the member.
                cursor = tokens.matching_end(cursor)?;
            }
            // The member ends before the switch's own `}` or the next
            // member head either way.
            TokenKind::CloseCurly | TokenKind::Keyword(Keyword::Case | Keyword::Default) => {
                break cursor;
            }
            TokenKind::Eof => return None,
            _ => {}
        }
    };
    // `case`/`default` head line through the line before the next head (or
    // the switch's `}`) — the lines the member occupies.
    let line_start = {
        let byte = utf16_offset_to_byte(text, input.range.start);
        text[..byte].rfind('\n').map_or(0, |i| i + 1)
    };
    let end_byte = utf16_offset_to_byte(
        text,
        if kind_of(tokens, end) == Some(TokenKind::CloseCurly)
            || matches!(
                kind_of(tokens, end),
                Some(TokenKind::Keyword(Keyword::Case | Keyword::Default))
            )
        {
            // The member ends before this token — the previous token's end.
            utf16_end(tokens, text, end.checked_sub(1)?)?
        } else {
            utf16_end(tokens, text, end)?
        },
    );
    let line_end = text[end_byte..]
        .find('\n')
        .map_or(text.len(), |i| end_byte + i);
    let (offset, length) = cut_line(text, line_start, line_end);
    Some((offset, length, String::new()))
}

/// `dead_code_on_catch_subtype`: the reported catch clause goes whole — the
/// official `RemoveDeadCode`'s `CatchClause` arm deletes
/// `endEnd(previous, clause)`, where `previous` is the try block or the
/// prior catch clause; the span lands on the clause's `on`/`catch` head.
pub(crate) fn drop_dead_catch(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let head = token_starting_at(tokens, input.range.start)?;
    // The clause head is `on` or `catch`; the clause runs through its
    // body's `}`.
    if !matches!(
        kind_of(tokens, head)?,
        TokenKind::Keyword(Keyword::On | Keyword::Catch)
    ) {
        return None;
    }
    // Find the clause body's `{`: the first depth-0 `{` after the head.
    let mut cursor = head;
    let body_open = loop {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenCurly => break cursor,
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                cursor = tokens.matching_end(cursor)?;
            }
            TokenKind::Eof | TokenKind::Semicolon => return None,
            _ => {}
        }
        cursor += 1;
    };
    let body_close = tokens.matching_end(body_open)?;
    // `endEnd(previous, clause)`: the previous sibling's end — the try
    // block's or a prior catch's `}` — through the clause's own `}`.
    let prev_end = utf16_end(tokens, text, head.checked_sub(1)?)?;
    let end = utf16_end(tokens, text, body_close)?;
    Some((prev_end, end.checked_sub(prev_end)?, String::new()))
}

/// `dead_null_aware_expression`: the dead right operand of a `??`/`??=`
/// goes — `endEnd(left, right)` on the binary face (the `?? rhs` tail), or
/// the whole expression statement on the `??=` bare-statement face
/// (official `RemoveDeadIfNull`). The report spans the right operand.
pub(crate) fn drop_dead_if_null(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _last) = hit_tokens(tokens, &input.range)?;
    // The `??`/`??=` sits before the right operand at depth 0.
    let mut cursor = first;
    let op = loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::QuestionQuestion | TokenKind::QuestionQuestionEq => break cursor,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                cursor = matching_begin(tokens, cursor)?;
            }
            TokenKind::Semicolon | TokenKind::Eof => return None,
            _ => {}
        }
    };
    if kind_of(tokens, op) == Some(TokenKind::QuestionQuestion) {
        // `endEnd(leftOperand, node)` — the cut starts at the left
        // operand's own end, before the `??` gap.
        let start = utf16_end(tokens, text, op.checked_sub(1)?)?;
        return Some((start, input.range.end.checked_sub(start)?, String::new()));
    }
    // `??=` — the official face deletes the whole statement when the
    // assignee is a bare identifier; otherwise `endEnd(lhs, rhs)`.
    let mut head = op;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev)? {
            TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Period => head = prev,
            TokenKind::CloseSquare | TokenKind::CloseParen | TokenKind::CloseCurly => {
                head = matching_begin(tokens, prev)?;
            }
            _ => break,
        }
    }
    let assignee_text = span_over(text, tokens, head, op - 1)?;
    let assignee = slice16(text, &(assignee_text.0..assignee_text.0 + assignee_text.1))?;
    if !assignee.chars().all(is_identifier_char) || assignee.is_empty() {
        // Not a bare identifier — `endEnd(lhs, rhs)`: from the left side's
        // own end through the operand.
        let start = utf16_end(tokens, text, op.checked_sub(1)?)?;
        return Some((start, input.range.end.checked_sub(start)?, String::new()));
    }
    // Bare-identifier statement: the whole line range goes.
    drop_lines_range(text, assignee_text.0, input.range.end).map(|(o, l)| (o, l, String::new()))
}

/// `dead_code`: the reported dead interval goes — the official
/// `RemoveDeadCode` cuts `getLinesRange` when the interval heads a
/// statement run (a whole-line deletion), and `endEnd(leftOperand, node)`
/// when it starts mid-line on a short-cut operator's right operand
/// (`true || dead` — the operator rides the span's head and the cut
/// begins at the live operand's own end).
pub(crate) fn drop_dead_code(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    let end = utf16_offset_to_byte(text, input.range.end);
    if start >= end || end > text.len() {
        return None;
    }
    let line_head = text[..start].rfind('\n').map_or(0, |i| i + 1);
    if text[line_head..start].trim().is_empty() {
        // The interval heads its line — a statement run's lines-range.
        let (offset, length) = cut_line(
            text,
            line_head,
            text[end..].find('\n').map_or(text.len(), |i| end + i),
        );
        return Some((offset, length, String::new()));
    }
    // A mid-line head is the dead right operand's operator — the cut
    // starts at the left operand's end (`endEnd`), the token ending at or
    // before the span's own start.
    let tokens = input.tokens?;
    let mut cursor = token_starting_at(tokens, input.range.start)?;
    if !matches!(
        kind_of(tokens, cursor)?,
        TokenKind::AmpersandAmpersand | TokenKind::BarBar | TokenKind::QuestionQuestion
    ) {
        return None;
    }
    cursor = cursor.checked_sub(1)?;
    // Step back over the operand's own closers to its last real token —
    // `endEnd` reads the operand's end, and a parenthesized operand ends
    // at its `)`; the operand boundary is the token right before the
    // operator.
    let cut_start = utf16_end(tokens, text, cursor)?;
    Some((
        cut_start,
        input.range.end.checked_sub(cut_start)?,
        String::new(),
    ))
}

/// `unused_element`: the declared local function's whole declaration goes
/// (official `RemoveUnusedElement`'s lines-range deletion); the report
/// spans the name, so the head walks back to the declaration's first token
/// — return type included — and the tail runs through the body's `}`.
pub(crate) fn drop_unused_element(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let name = token_starting_at(tokens, input.range.start)?;
    if !matches!(
        kind_of(tokens, name)?,
        TokenKind::Identifier | TokenKind::Keyword(_)
    ) {
        return None;
    }
    // The declaration head: the previous `;`, `}`, or `{` bounds the
    // statement run — the declaration's first token.
    let mut head = name;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev)? {
            TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly => break,
            TokenKind::CloseParen | TokenKind::CloseSquare => {
                // A `)` behind the name belongs to an earlier construct —
                // a function declaration's own parameter list follows the
                // name, not before it.
                head = prev;
            }
            _ => head = prev,
        }
    }
    // The declaration's tail: the body's `}` (or a `;` for a declaration
    // without a body — a local function always carries a body). The name's
    // own `(` is the first one forward — no group sits between the name
    // and its parameter list.
    let mut params = name;
    let params = loop {
        params += 1;
        match kind_of(tokens, params)? {
            TokenKind::OpenParen => break params,
            // Generic parameter lists and `=>` bodies precede it.
            TokenKind::Semicolon | TokenKind::Eof => return None,
            _ => {}
        }
    };
    let params_close = tokens.matching_end(params)?;
    let mut body_open = params_close;
    let body_open = loop {
        body_open += 1;
        match kind_of(tokens, body_open)? {
            TokenKind::OpenCurly | TokenKind::Semicolon => break body_open,
            TokenKind::Eof => return None,
            _ => {}
        }
    };
    let end = if kind_of(tokens, body_open) == Some(TokenKind::OpenCurly) {
        tokens.matching_end(body_open)?
    } else {
        body_open
    };
    let head16 = tokens.get(head)?.utf16_offset;
    let end16 = utf16_end(tokens, text, end)?;
    let (offset, length) = drop_lines_range(text, head16, end16)?;
    Some((offset, length, String::new()))
}

/// `unused_field`: the reported field declaration's line goes — the
/// official `RemoveUnusedField`'s lines-range deletion. The report spans
/// the field's name; the declarator's own span (with its initializer and
/// one adjacent comma on a multi-declarator list) is rebuilt from the
/// tokens.
pub(crate) fn drop_unused_field(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let name = token_starting_at(tokens, input.range.start)?;
    if !matches!(kind_of(tokens, name)?, TokenKind::Identifier) {
        return None;
    }
    // The declarator's tail: `;` or `,` at depth 0.
    let mut tail = name;
    loop {
        match kind_of(tokens, tail)? {
            TokenKind::Semicolon => break,
            TokenKind::Comma => break,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                tail = tokens.matching_end(tail)?;
            }
            TokenKind::Eof => return None,
            _ => {}
        }
        tail += 1;
    }
    // The declaration head: the first token after the previous `;`/`{`/`}`.
    let mut head = name;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev)? {
            TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly => break,
            _ => head = prev,
        }
    }
    if kind_of(tokens, tail) == Some(TokenKind::Comma) {
        // A non-last declarator: itself through the next declarator's
        // start goes — the official's `nodeInList` face (`startStart(node,
        // next)`). The list's modifiers and type stay with the survivors.
        let start = tokens.get(name)?.utf16_offset;
        let end = tokens.get(tail + 1)?.utf16_offset;
        return Some((start, end.checked_sub(start)?, String::new()));
    }
    if name > head && kind_of(tokens, name - 1) == Some(TokenKind::Comma) {
        // The last declarator of a multi-declarator list: `endEnd` of the
        // previous declarator through this one — the preceding comma's
        // gap rides with it.
        let start = utf16_end(tokens, text, name.checked_sub(2)?)?;
        let end = utf16_end(tokens, text, tail.checked_sub(1)?)?;
        return Some((start, end.checked_sub(start)?, String::new()));
    }
    // A lone declarator: the whole declaration's lines go.
    let head16 = tokens.get(head)?.utf16_offset;
    let end16 = utf16_end(tokens, text, tail)?;
    let (offset, length) = drop_lines_range(text, head16, end16)?;
    Some((offset, length, String::new()))
}

/// `unused_element_parameter`: the reported parameter leaves its list —
/// the official `RemoveUnusedParameter`'s faces: a lone parameter drops
/// the list's interior (`endStart(lparen, rparen)`), the first parameter
/// drops itself plus its comma (`startStart(parameter, following)`), the
/// lone member inside `[`/`{` drops with the delimiters, and the plain
/// case drops `endEnd(preceding, parameter)`.
pub(crate) fn drop_unused_parameter(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let name = token_starting_at(tokens, input.range.start)?;
    if !matches!(kind_of(tokens, name)?, TokenKind::Identifier) {
        return None;
    }
    // The enclosing parameter list's `(`: the nearest open paren before the
    // name whose group is still open at it.
    let mut open = None;
    let mut cursor = name;
    while let Some(index) = cursor.checked_sub(1) {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen => {
                open = Some(index);
                break;
            }
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                cursor = matching_begin(tokens, index)?;
                continue;
            }
            TokenKind::Semicolon | TokenKind::OpenCurly => break,
            _ => {}
        }
        cursor = index;
    }
    let open = open?;
    let close = tokens.matching_end(open)?;
    // The parameter's own extent: from its first token (type/modifiers
    // included) to its last — the depth-0 `,`/`]`/`}`/`)` bounds it.
    let mut first = name;
    while let Some(prev) = first.checked_sub(1) {
        if prev <= open {
            break;
        }
        match kind_of(tokens, prev)? {
            TokenKind::Comma | TokenKind::OpenSquare | TokenKind::OpenCurly => break,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                first = matching_begin(tokens, prev)?;
            }
            _ => first = prev,
        }
    }
    let mut last = name;
    loop {
        match kind_of(tokens, last + 1)? {
            TokenKind::Comma
            | TokenKind::CloseSquare
            | TokenKind::CloseCurly
            | TokenKind::CloseParen => break,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                last = tokens.matching_end(last + 1)?;
            }
            TokenKind::Eof => return None,
            _ => last += 1,
        }
    }
    let is_delimiter = |i: usize| {
        matches!(
            kind_of(tokens, i),
            Some(
                TokenKind::OpenSquare
                    | TokenKind::OpenCurly
                    | TokenKind::CloseSquare
                    | TokenKind::CloseCurly
            )
        )
    };
    // The sibling parameter boundary: a `,` or the closer right after the
    // parameter's last token. No comma and no `[`/`{` delimiter at this
    // level means the parameter is the list's only member.
    let after = last + 1;
    let only_in_list = first == open + 1
        && kind_of(tokens, after) == Some(TokenKind::CloseParen)
        && !(open + 1..after)
            .any(|i| kind_of(tokens, i) == Some(TokenKind::Comma) || is_delimiter(i));
    if only_in_list {
        let start = utf16_end(tokens, text, open)?;
        let end16 = tokens.get(close)?.utf16_offset;
        return Some((start, end16.checked_sub(start)?, String::new()));
    }
    let prev_is_delim = first > open + 1 && is_delimiter(first - 1);
    if first == open + 1 || prev_is_delim {
        // First in its group: `param ,` goes — the comma's end through.
        if kind_of(tokens, after) == Some(TokenKind::Comma) {
            let start = tokens.get(first)?.utf16_offset;
            let end = utf16_end(tokens, text, after)?;
            return Some((start, end.checked_sub(start)?, String::new()));
        }
        // The lone member inside `[`/`{`: the delimiters ride the cut.
        if prev_is_delim && is_delimiter(after) {
            let start = utf16_end(tokens, text, first - 1)?;
            let end16 = tokens.get(after)?.utf16_offset;
            return Some((start, end16.checked_sub(start)?, String::new()));
        }
        return None;
    }
    // The plain case: `endEnd(preceding, parameter)` — from the previous
    // parameter's own end through this one's end.
    let mut prev = first - 1;
    if kind_of(tokens, prev) == Some(TokenKind::Comma) {
        prev = prev.checked_sub(1)?;
        // A `]`/`}`/`(` is a boundary, not a sibling parameter — the comma
        // before it belongs to this parameter's own cut.
        if is_delimiter(prev) || prev <= open {
            let start = tokens.get(prev + 1)?.utf16_offset;
            let end = utf16_end(tokens, text, last)?;
            return Some((start, end.checked_sub(start)?, String::new()));
        }
    }
    let prev_end = utf16_end(tokens, text, prev)?;
    let end = utf16_end(tokens, text, last)?;
    Some((prev_end, end.checked_sub(prev_end)?, String::new()))
}

/// `body_might_complete_normally_nullable`: `return null;` lands inside
/// the reported block — the official `AddReturnNull` insertion at the last
/// statement's end, or at the block's offset + 1 when it is empty. The
/// report spans the function/method declaration's head; the body's `{` is
/// rebuilt from the tokens.
pub(crate) fn add_return_null(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _last) = hit_tokens(tokens, &input.range)?;
    // The body's `{`: the first depth-0 `{` after the declaration head.
    let mut cursor = first;
    let body_open = loop {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenCurly => break cursor,
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                cursor = tokens.matching_end(cursor)?;
            }
            TokenKind::Semicolon | TokenKind::Eof | TokenKind::Function => return None,
            _ => {}
        }
        cursor += 1;
    };
    let body_close = tokens.matching_end(body_open)?;
    // The last statement before `}` — the insertion sits at its end; an
    // empty block inserts at `{` + 1 with the surrounding line's prefix.
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let open_end16 = utf16_end(tokens, text, body_open)?;
    let close_start16 = tokens.get(body_close)?.utf16_offset;
    // Statements between the braces?
    let has_statements = (body_open + 1..body_close).any(|i| {
        !matches!(kind_of(tokens, i), Some(TokenKind::Eof))
            && tokens.get(i).is_some_and(|t| t.byte_len > 0)
    });
    if has_statements {
        let last_end = utf16_end(tokens, text, body_close.checked_sub(1)?)?;
        // `utils.getNodePrefix(lastStatement)` — the last statement's own
        // line indent, reused for the new line.
        let indent = {
            let byte = utf16_offset_to_byte(text, last_end);
            let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
            let line = &text[line_start..byte];
            line[..line.len() - line.trim_start().len()].to_owned()
        };
        return Some((last_end, 0, format!("{eol}{indent}return null;")));
    }
    // Empty block: insert at `{` + 1 with the block's own line prefix plus
    // one indent level, and the close's line prefix after the statement.
    let open_byte = utf16_offset_to_byte(text, tokens.get(body_open)?.utf16_offset);
    let block_line_start = text[..open_byte].rfind('\n').map_or(0, |i| i + 1);
    let prefix: String = text[block_line_start..open_byte]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let one_indent = "  ";
    let _ = close_start16;
    Some((
        open_end16,
        0,
        format!("{eol}{prefix}{one_indent}return null;{eol}{prefix}"),
    ))
}

/// `unnecessary_null_comparison` / `unnecessary_nan_comparison` /
/// `unnecessary_type_check`: the reported comparison drops out of its
/// `&&`/`||` chain — official `RemoveComparison`'s operand-drop arm. The
/// chain operator's leg selects on the verdict polarity the message
/// carries (`always 'true'` chains on `&&`, `always 'false'` on `||`); a
/// non-last operand cuts `startStart(node, nextOperand)`, the last cuts
/// `endEnd(leftOperand, node)`. The `if`/`assert`/`conditional` parent
/// arms rewrite a wider construct and stay unserved.
pub(crate) fn drop_comparison_operand(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    // The verdict polarity lives in the message's `'true'`/`'false'`
    // display.
    let chain_op = if input.message.contains("'true'") {
        TokenKind::AmpersandAmpersand
    } else if input.message.contains("'false'") {
        TokenKind::BarBar
    } else {
        return None;
    };
    chain_operand_drop(input, chain_op)
}

/// `avoid_null_checks_in_equality_operators`: the reported null check
/// drops its `&&` flank — the official `RemoveComparison` hardcodes
/// `_conditionIsTrue` for this diagnostic, so only the `&&` chain arm of
/// [`drop_comparison_operand`] applies (the `||` face, the assert/if/?:
/// parents, and the `?.` face's not-null-aware rewrite are absent legs).
pub(crate) fn equality_null_operand(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    chain_operand_drop(input, TokenKind::AmpersandAmpersand)
}

/// The shared operand cut of `RemoveComparison`'s binary-expression arm:
/// the hit span is one operand of an `&&`/`||` chain — a non-last
/// operand cuts `startStart(node, nextOperand)`, the last operand cuts
/// `endEnd(prevOperand, node)`.
pub(crate) fn chain_operand_drop(input: &FixInput<'_>, chain_op: TokenKind) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    // A non-last operand: the same chain operator follows the span — the
    // cut runs to the next operand's own start (`startStart(node,
    // nextOperand)`).
    if kind_of(tokens, last + 1) == Some(chain_op) {
        let end = tokens.get(last + 2)?.utf16_offset;
        return Some((
            input.range.start,
            end.checked_sub(input.range.start)?,
            String::new(),
        ));
    }
    // The last operand: the chain operator precedes the span — the cut
    // runs from the previous operand's end (the operator's own start
    // minus its gap reads the same boundary).
    if first > 0 && kind_of(tokens, first - 1) == Some(chain_op) {
        let start = utf16_end(tokens, text, first - 2)?;
        return Some((start, input.range.end.checked_sub(start)?, String::new()));
    }
    None
}

// ---------------------------------------------------------------------------
// Semantic source: the null-check family
//
// The three nullable-value codes the engine emits, served against the
// official `AddNullCheck` producer (upstream Dart SDK source,
// pkg/analysis_server/lib/src/services/correction/fix_internal.dart). The
// edit inserts the non-null assertion at the producer's target expression
// or rewrites the chain's null-aware operator to its `!` form; the paren
// wrap below postfix precedence folds its two insertions into one
// whole-run replacement. The producer's assignability gate reads resolved
// types; this face reads the same gate off the diagnostic's message,
// which carries both type displays — the fix is offered only where the
// promoted `from` display is the `to` display, the nullable-only mismatch
// the gate serves.
// ---------------------------------------------------------------------------

/// The message-carried assignability gate: the pair of quoted type
/// displays behind `prefix`, accepted only when `from` reads as `to` plus
/// the trailing `?` — promote-to-non-null assignability, provable from
/// the displays alone.
pub(crate) fn promotable_pair(message: &str, prefix: &str) -> Option<()> {
    const MID: &str = " can't be assigned to ";
    // The prefix ends at the from-display's opening quote; the closing
    // quote bounds `from`, and the `to` display sits inside the next
    // quote pair behind the midriff (`a variable of type` /
    // `the parameter type`).
    let rest = message.strip_prefix(prefix)?;
    let from_end = rest.find('\'')?;
    let from = &rest[..from_end];
    let tail = rest.get(from_end + 1..)?;
    let mid = tail.find(MID)?;
    let after = tail.get(mid + MID.len()..)?;
    let to = after.get(after.find('\'')? + 1..)?;
    let to_end = to.find('\'')?;
    (from.strip_suffix('?')? == &to[..to_end]).then_some(())
}

/// The chain scan of a token run: a pure member chain (`primary
/// selector*` over `.`, `?.`, `?..`, postfix `!`, calls, indexes, and
/// generic-invocation type arguments) — `Chain` carries the rightmost
/// null-aware operator the official `_hasNullAware` walk returns;
/// anything else is `NotAChain`.
pub(crate) enum ChainScan {
    Chain { null_aware: Option<usize> },
    NotAChain,
}

pub(crate) fn scan_chain(tokens: &TokenTable, first: usize, last: usize) -> ChainScan {
    scan_chain_inner(tokens, first, last).unwrap_or(ChainScan::NotAChain)
}

/// The `Option`-shaped body of [`scan_chain`]: `None` is not-a-chain.
pub(crate) fn scan_chain_inner(tokens: &TokenTable, first: usize, last: usize) -> Option<ChainScan> {
    // The chain head: an identifier, a literal, one balanced group, or
    // one of the keywords that may stand as an expression's head
    // (`await`, `switch`, `const`, `throw` sit below postfix precedence
    // and take the paren wrap, never the chain).
    let mut cursor = match kind_of(tokens, first) {
        Some(
            TokenKind::Identifier
            | TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String,
        ) => first + 1,
        Some(TokenKind::Keyword(
            Keyword::This | Keyword::Super | Keyword::True | Keyword::False | Keyword::Null,
        )) => first + 1,
        Some(TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly) => {
            tokens.matching_end(first)? + 1
        }
        _ => return Some(ChainScan::NotAChain),
    };
    let mut null_aware = None;
    loop {
        if cursor > last {
            return Some(ChainScan::Chain { null_aware });
        }
        match kind_of(tokens, cursor)? {
            TokenKind::Period => {
                expect_member_name(tokens, cursor + 1)?;
                cursor += 2;
            }
            TokenKind::QuestionPeriod | TokenKind::QuestionPeriodPeriod => {
                expect_member_name(tokens, cursor + 1)?;
                null_aware = Some(cursor);
                cursor += 2;
            }
            // The null-aware index `?[…]` — the scanner splits the `?`.
            TokenKind::Question if kind_of(tokens, cursor + 1)? == TokenKind::OpenSquare => {
                null_aware = Some(cursor);
                cursor = tokens.matching_end(cursor + 1)? + 1;
            }
            TokenKind::Bang => cursor += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                cursor = tokens.matching_end(cursor)? + 1;
            }
            // Generic invocation type arguments: `<…>` and the call that
            // must follow it.
            TokenKind::Lt => {
                let close = angle_group_end(tokens, cursor)?;
                if kind_of(tokens, close + 1)? != TokenKind::OpenParen {
                    return Some(ChainScan::NotAChain);
                }
                cursor = tokens.matching_end(close + 1)? + 1;
            }
            _ => return Some(ChainScan::NotAChain),
        }
    }
}

/// The property name a member operator must stand before (`a.new` keeps
/// the contextual keyword slot).
pub(crate) fn expect_member_name(tokens: &TokenTable, index: usize) -> Option<()> {
    matches!(
        kind_of(tokens, index)?,
        TokenKind::Identifier | TokenKind::Keyword(_)
    )
    .then_some(())
}

/// The replace face: the chain's null-aware operator goes to its `!` form
/// (the official `!${lexeme.substring(1)}` rewrite — `?.` → `!.`,
/// `?..` → `!..`, `?[` → `![`).
pub(crate) fn replace_null_aware(tokens: &TokenTable, text: &str, op: usize) -> Option<(u32, u32, String)> {
    let token = tokens.get(op)?;
    let replacement = match token.kind {
        TokenKind::QuestionPeriod | TokenKind::QuestionPeriodPeriod => {
            format!("!{}", &tokens.lexeme(text, op)[1..])
        }
        TokenKind::Question => "!".to_owned(),
        _ => return None,
    };
    Some((
        token.utf16_offset,
        utf16_count(tokens.lexeme(text, op)),
        replacement,
    ))
}

/// The append face: `!` lands at the target run's end; a run whose
/// precedence sits below postfix (anything but one member chain or one
/// balanced group) wraps in parentheses — the official producer's two
/// insertions folded into one whole-run replacement.
pub(crate) fn append_bang(input: &FixInput<'_>, first: usize, last: usize) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (start16, length16) = span_over(text, tokens, first, last)?;
    if matches!(scan_chain(tokens, first, last), ChainScan::Chain { .. }) {
        return Some((start16 + length16, 0, "!".to_owned()));
    }
    let slice = slice16(text, &(start16..start16 + length16))?;
    Some((start16, length16, format!("({slice})!")))
}

/// The shared target face: a run carrying the chain's null-aware operator
/// rewrites it, everything else takes the append face.
pub(crate) fn null_check_target(
    input: &FixInput<'_>,
    first: usize,
    last: usize,
) -> Option<(u32, u32, String)> {
    if let ChainScan::Chain {
        null_aware: Some(op),
    } = scan_chain(input.tokens?, first, last)
    {
        return replace_null_aware(input.tokens?, input.text, op);
    }
    append_bang(input, first, last)
}

/// The outermost depth-0 operator of a condition run — the binary tree's
/// root, rightmost of the loosest precedence among the ternary `?`, `??`,
/// `||`, `&&`, and `as` (left-associative roots sit rightmost). `None`
/// when none stands.
/// Whether a depth-0 `:` follows in `from..=last` — the mark that splits
/// a ternary `?` from a nullable type's `?` (as in `x as bool?`, whose
/// `?` is no operator at all).
pub(crate) fn has_depth0_colon(tokens: &TokenTable, from: usize, last: usize) -> Option<bool> {
    let mut depth = 0u32;
    for index in from..=last {
        match tokens.get(index)?.kind {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Colon if depth == 0 => return Some(true),
            _ => {}
        }
    }
    Some(false)
}

pub(crate) fn condition_root(
    tokens: &TokenTable,
    first: usize,
    last: usize,
) -> Option<(u8, TokenKind, usize)> {
    let mut best: Option<(u8, TokenKind, usize)> = None;
    let mut depth = 0u32;
    for index in first..=last {
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
        if depth != 0 {
            continue;
        }
        // A bare `?` is the ternary — the null-aware and `??` spellings
        // are their own token kinds, and `?[` the null-aware index. A `?`
        // with no depth-0 `:` behind it is a nullable type's (a ternary
        // always carries its colon), never an operator root.
        let (prec, kind) = match token.kind {
            TokenKind::Question
                if kind_of(tokens, index + 1)? != TokenKind::OpenSquare
                    && has_depth0_colon(tokens, index + 1, last)? =>
            {
                (1, TokenKind::Question)
            }
            TokenKind::QuestionQuestion => (2, TokenKind::QuestionQuestion),
            TokenKind::BarBar => (3, TokenKind::BarBar),
            TokenKind::AmpersandAmpersand => (4, TokenKind::AmpersandAmpersand),
            TokenKind::Keyword(Keyword::As) => (5, TokenKind::Keyword(Keyword::As)),
            _ => continue,
        };
        if best.is_none_or(|(bp, _, bi)| prec < bp || (prec == bp && index > bi)) {
            best = Some((prec, kind, index));
        }
    }
    best
}

/// `unchecked_use_of_nullable_value`: the official `AddNullCheck` over the
/// engine's condition face — the bare identifier condition, the
/// outermost `&&`/`||` root's left operand, and the `as` inner expression
/// take the producer's target (`!` at the target's end, or the chain's
/// null-aware operator rewritten); a `??` or ternary root resolves no
/// target in the official descent, and neither does any other shape (a
/// property-access condition among them).
pub(crate) fn null_check_condition(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if let ChainScan::Chain {
        null_aware: Some(op),
    } = scan_chain(tokens, first, last)
    {
        return replace_null_aware(tokens, input.text, op);
    }
    if first == last && kind_of(tokens, first)? == TokenKind::Identifier {
        return null_check_target(input, first, last);
    }
    let (_, kind, index) = condition_root(tokens, first, last)?;
    match kind {
        TokenKind::AmpersandAmpersand | TokenKind::BarBar | TokenKind::Keyword(Keyword::As) => {
            null_check_target(input, first, index.checked_sub(1)?)
        }
        _ => None,
    }
}

/// `invalid_assignment`: the official `AddNullCheck` assignment face —
/// the `!` lands at the right-hand side's end (wrapped below postfix
/// precedence) or the chain's null-aware operator goes `!`-form, gated on
/// the message's type-display pair. A right-hand side that is one
/// balanced group stays unfixed: the official fix reaches it only on the
/// statement-assignment face, which the row cannot tell apart from the
/// declaration face. The official kinds whose edits need resolved types
/// (`AddExplicitCast`, `ChangeTypeAnnotation`, `MakeVariableNullable`,
/// `AddAwait`, `AddMissingClosureParameters`) have no edit here.
pub(crate) fn null_check_assignment(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    promotable_pair(input.message, "A value of type '")?;
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if let ChainScan::Chain {
        null_aware: Some(op),
    } = scan_chain(tokens, first, last)
    {
        return replace_null_aware(tokens, text, op);
    }
    if kind_of(tokens, first)? == TokenKind::OpenParen && tokens.matching_end(first)? == last {
        return None;
    }
    append_bang(input, first, last)
}

/// `argument_type_not_assignable`: the official `AddNullCheck` argument
/// face, the same message gate — a bare-identifier argument takes the `!`
/// after it, an argument that is one member chain rewrites its
/// null-aware operator, and every other argument shape resolves no target
/// in the official descent (property-access and call arguments among
/// them).
pub(crate) fn null_check_argument(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    promotable_pair(input.message, "The argument type '")?;
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if let ChainScan::Chain {
        null_aware: Some(op),
    } = scan_chain(tokens, first, last)
    {
        return replace_null_aware(tokens, text, op);
    }
    if first == last && kind_of(tokens, first)? == TokenKind::Identifier {
        return append_bang(input, first, last);
    }
    None
}

/// `!(x is T)` → `x is! T` (or `!(x is! T)` where the parentheses must
/// stay): the `!` goes and an `!` lands after the `is` — the official
/// `ConvertIntoIsNot` deletions and insertion folded into one
/// whole-span replacement.
pub(crate) fn is_not_operator(
    tokens: &TokenTable,
    text: &str,
    range: &core::ops::Range<u32>,
) -> Option<(u32, u32, String)> {
    let (first, last) = hit_tokens(tokens, range)?;
    if kind_of(tokens, first)? != TokenKind::Bang {
        return None;
    }
    let lparen = first + 1;
    if kind_of(tokens, lparen)? != TokenKind::OpenParen || tokens.matching_end(lparen)? != last {
        return None;
    }
    let is = depth0_find(tokens, text, lparen + 1, last, |kind, _| {
        kind == TokenKind::Keyword(Keyword::Is)
    })?;
    let is_end = token_end(tokens.get(is)?);
    let keep_parens = !first
        .checked_sub(1)
        .and_then(|index| tokens.get(index))
        .is_some_and(|prev| paren_removable(prev.kind));
    let (base, body_end) = if keep_parens {
        (token_end(tokens.get(first)?), token_end(tokens.get(last)?))
    } else {
        (
            token_end(tokens.get(lparen)?),
            tokens.get(last)?.byte_offset as usize,
        )
    };
    let body = &text[base..body_end];
    let at = is_end - base;
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!("{}!{}", &body[..at], &body[at..]),
    ))
}

/// The `(condition-op, non-null operand token range, then, else)` of a
/// null-check conditional — the shared read of the two null-aware
/// rewrites. The separators are the depth-0 `?` and `:` of the span.
pub(crate) type NullCheckParts = (
    TokenKind,
    core::ops::Range<usize>,
    core::ops::Range<usize>,
    core::ops::Range<usize>,
);

pub(crate) fn null_check_tokens(
    tokens: &TokenTable,
    text: &str,
    first: usize,
    last: usize,
) -> Option<NullCheckParts> {
    let question = depth0_find(tokens, text, first, last + 1, |kind, _| {
        kind == TokenKind::Question
    })?;
    // The conditional's own `:`: each depth-0 `?` after the first opens a
    // nested conditional (a bare conditional then-arm is legal), whose
    // `:` closes it before the outer's can appear.
    let mut depth = 0u32;
    let mut pending = 0u32;
    let mut colon = None;
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
                    colon = Some(index);
                    break;
                }
                pending -= 1;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let op = depth0_find(tokens, text, first, question, |kind, _| {
        matches!(kind, TokenKind::EqEq | TokenKind::BangEq)
    })?;
    let operand = if is_null_token(tokens, op + 1) {
        first..op
    } else if op > first && is_null_token(tokens, op - 1) {
        op + 1..question
    } else {
        return None;
    };
    Some((
        kind_of(tokens, op)?,
        operand,
        question + 1..colon,
        colon + 1..last + 1,
    ))
}
