use super::*;


/// `unawaited_futures`: `await ` lands at the reported expression
/// statement's head — the official `AddAwait.unawaited` arm (`await ` at
/// the expression's offset, refused inside a cascade). The lint's own
/// gates already restrict reports to expression statements inside async
/// bodies, so the producer's convert-to-async arm never engages; the
/// `WrapInUnawaited` face (a `dart:async` import insertion) is not served.
///
/// The statement head is the token after the enclosing `;`/`{`/`}`/`:`
/// boundary — the lint only reports expression statements, so that head
/// is the expression's first token.
pub(crate) fn await_future_expression(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let selector = token_starting_at(tokens, input.range.start)?;
    // The statement's head: walk back to the statement boundary, group
    // interiors skipped (`{...}`/`(...)`/`[...]` tails inside the
    // expression never read as boundaries).
    let mut head = selector;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev) {
            Some(
                close @ (TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly),
            ) => {
                head = matching_open(tokens, prev, close)?;
            }
            Some(TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::Colon) => break,
            _ => head = prev,
        }
    }
    // The cascade gate (official `node is CascadeExpression` /
    // `node.parent?.parent is CascadeExpression` drops): a `..`/`?..`/
    // `...`/`...?` anywhere between the head and the selector at depth 0
    // puts the report inside a cascade section — the producer drops it.
    let mut depth = 0u32;
    for index in head..selector {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::PeriodPeriod
            | TokenKind::QuestionPeriodPeriod
            | TokenKind::PeriodPeriodPeriod
            | TokenKind::PeriodPeriodPeriodQuestion
                if depth == 0 =>
            {
                return None;
            }
            _ => {}
        }
    }
    let start16 = tokens.get(head)?.utf16_offset;
    Some((start16, 0, "await ".to_owned()))
}

/// Whether the block body at `brace` is a lambda's — upstream
/// `body.parent is FunctionExpression && parent.parent is! FunctionDeclaration`.
/// Token face: the token before the body's `async`-family tail is the
/// parameter list's `)`; a name (`f(`), an `operator` head, a getter's
/// identifier or a type-parameter list `f<T>(` ahead of its `(` makes the
/// body a declaration's. Anything else — `= (`, `, (`, `( (`, `return (`
/// — is a lambda.
pub(crate) fn body_is_lambda(tokens: &TokenTable, brace: usize) -> bool {
    let mut tail = brace;
    // The `async`/`sync` modifier words ride before `{` (a `*` tail never
    // reaches here — generators refused earlier).
    while let Some(prev) = tail.checked_sub(1) {
        match kind_of(tokens, prev) {
            Some(TokenKind::Keyword(Keyword::Async | Keyword::Sync)) => tail = prev,
            _ => break,
        }
    }
    let Some(prev) = tail.checked_sub(1) else {
        return false;
    };
    if kind_of(tokens, prev) != Some(TokenKind::CloseParen) {
        // A getter `int get x { }` — an identifier head, a declaration.
        return false;
    }
    let Some(open) = matching_open(tokens, prev, TokenKind::CloseParen) else {
        return false;
    };
    let Some(head) = open.checked_sub(1) else {
        return true; // `(` at file start — lambda either way
    };
    match kind_of(tokens, head) {
        Some(TokenKind::Identifier) => false,
        Some(TokenKind::Gt) => {
            // `f<T>() {}` declaration vs `<T>() {}` lambda — the token
            // before the angle group decides.
            match angle_group_start(tokens, head).and_then(|lt| lt.checked_sub(1)) {
                Some(before_lt) => {
                    !matches!(kind_of(tokens, before_lt), Some(TokenKind::Identifier))
                }
                None => true,
            }
        }
        // `operator ==(o) { }` — an operator head is a declaration.
        Some(_) => head
            .checked_sub(1)
            .and_then(|p| kind_of(tokens, p))
            .is_none_or(|kind| kind != TokenKind::Keyword(Keyword::Operator)),
        None => false,
    }
}

/// The begin-group index matching the close-group token at `close` — the
/// reverse walk `TokenTable::matching_end` answers forward.
pub(crate) fn matching_open(tokens: &TokenTable, close: usize, kind: TokenKind) -> Option<usize> {
    let opens = match kind {
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
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            k if k == opens
                || (opens == TokenKind::OpenCurly
                    && k == TokenKind::StringInterpolationExpression) =>
            {
                if depth == 0 {
                    return Some(cursor);
                }
                depth -= 1;
            }
            // A different open kind closes its nested group; at depth 0 the
            // stream is unbalanced relative to `close`.
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
}

/// `prefer_foreach`: the reported `for` statement folds into a `forEach`
/// tear-off — `for (var e in it) { f(e); }` → `it.forEach(f);` (the
/// official `ConvertToForEach`: the loop head through the iterable's start
/// goes, the iterable's tail through the invocation's start becomes
/// `.forEach(`, and the argument list through the statement's end becomes
/// `);`). The lint's own gates stand in for the producer's checks — the
/// declaration-form header, the single-argument body invocation, the
/// argument's identity as the loop variable.
///
/// The rule reports the `ForIn` node, whose span may sit one token short
/// of the statement's own end, so the replacement is bounded structurally
/// from the `for` keyword: a block body ends at its matching `}` — the
/// official's `node.end` — and a bare body at its `;`.
pub(crate) fn for_each_tear_off(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _last) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first)? != TokenKind::Keyword(Keyword::For) {
        return None;
    }
    let lparen = first + 1;
    if kind_of(tokens, lparen)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(lparen)?;
    // The iterable: the tokens between the `in` keyword and the header's
    // close — the head stays, everything after becomes `.forEach(`.
    let in_kw = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Keyword(Keyword::In)
    })?;
    if in_kw + 1 >= rparen {
        return None;
    }
    // The body: a single expression-statement invocation inside `{}` or
    // bare — `body` is the block or the statement itself.
    let mut cursor = rparen + 1;
    let braced = kind_of(tokens, cursor) == Some(TokenKind::OpenCurly);
    if braced {
        cursor += 1;
    }
    // The statement's invocation: `<target>.name(args)` or `name(args)` —
    // the argument list's open paren is the first depth-0 `(` inside the
    // statement (`depth0_find` counts the opener before testing, so the
    // group open itself needs a manual depth walk).
    let stmt_start = cursor;
    let stmt_end = depth0_find(tokens, text, cursor, tokens.len(), |kind, _| {
        kind == TokenKind::Semicolon
    })?;
    let mut invoke_paren = None;
    {
        let mut depth = 0u32;
        for index in stmt_start..stmt_end {
            match kind_of(tokens, index)? {
                TokenKind::OpenParen => {
                    if depth == 0 {
                        invoke_paren = Some(index);
                        break;
                    }
                    depth += 1;
                }
                TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth = depth.checked_sub(1)?;
                }
                _ => {}
            }
        }
    }
    let invoke_paren = invoke_paren?;
    // The callee is the invocation minus its argument list — the whole
    // postfix chain (`obj.m(e)` → `obj.m`, `f(e)` → `f`). Walk back: the
    // callee name first (a `f<T>(` generic tail skipped), then each
    // `.`/`?.`/`!` link with the receiver atom behind it (a group tail or
    // a plain name).
    let mut invoke_start = invoke_paren.checked_sub(1)?; // the callee name
    if matches!(
        kind_of(tokens, invoke_start),
        Some(TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt)
    ) && let Some(lt) = angle_group_start(tokens, invoke_start)
    {
        invoke_start = lt.checked_sub(1)?;
    }
    loop {
        let prev = match invoke_start.checked_sub(1) {
            Some(i) if i >= stmt_start => i,
            _ => break,
        };
        if !matches!(
            kind_of(tokens, prev),
            Some(TokenKind::Period | TokenKind::QuestionPeriod | TokenKind::Bang)
        ) {
            break;
        }
        let atom = prev.checked_sub(1)?;
        if atom < stmt_start {
            return None;
        }
        invoke_start = match kind_of(tokens, atom)? {
            close @ (TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly) => {
                matching_open(tokens, atom, close)?
            }
            TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::Int
            | TokenKind::Double
            | TokenKind::Hexadecimal
            | TokenKind::String => atom,
            _ => return None,
        };
    }
    // The iterable's text and the invocation's callee text (the invocation
    // minus its argument list — the tear-off), the statement's end
    // structural from the body's shape.
    let iterable = token_text(tokens, text, in_kw + 1..rparen)?;
    let callee = token_text(tokens, text, invoke_start..invoke_paren)?;
    let end_tok = if braced {
        tokens.matching_end(rparen + 1)?
    } else {
        stmt_end
    };
    let (start16, _) = span_over(text, tokens, first, first)?;
    let end16 = utf16_end(tokens, text, end_tok)?;
    Some((
        start16,
        end16.checked_sub(start16)?,
        format!("{iterable}.forEach({callee});"),
    ))
}

/// `prefer_expression_function_bodies`: the reported `{ return e; }` block
/// body folds into `=> e;` — the official `ConvertToExpressionFunctionBody`
/// replacement of the whole body, `async` keyword carried, comments riding
/// the `return`/`;`/`}` trivia preserved in order. The official gates the
/// lint cannot see are rebuilt: a generator body (`sync*`/`async*`) and a
/// non-factory constructor parent both refuse.
pub(crate) fn expression_function_body(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    // The report opens on the body `{` — the `async`/`sync` modifier words
    // ride the body's payload, outside its token range.
    let brace = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, brace)? != TokenKind::OpenCurly {
        return None;
    }
    // The modifier words sit immediately before `{` — `async`, `sync`, or
    // either plus `*` (a generator never converts). They ride the parent's
    // payload outside the replaced span and stay put.
    if let Some(prev) = brace.checked_sub(1) {
        if kind_of(tokens, prev) == Some(TokenKind::Star) {
            return None; // `sync*`/`async*` — the official generator bail
        }
    }
    let close = tokens.matching_end(brace)?;
    // One statement inside: `return e;` or `e;` (the producer's
    // ReturnStatement and ExpressionStatement arms — the lint reports only
    // the `return` face, the ExpressionStatement arm is assist-side).
    let stmt_first = brace + 1;
    if stmt_first >= close {
        return None;
    }
    let (expr_start, expr_end) =
        if kind_of(tokens, stmt_first)? == TokenKind::Keyword(Keyword::Return) {
            let semi = depth0_find(tokens, text, stmt_first + 1, close, |kind, _| {
                kind == TokenKind::Semicolon
            })?;
            if semi + 1 != close || stmt_first + 1 == semi {
                return None;
            }
            (stmt_first + 1, semi)
        } else {
            let semi = depth0_find(tokens, text, stmt_first, close, |kind, _| {
                kind == TokenKind::Semicolon
            })?;
            if semi + 1 != close {
                return None;
            }
            (stmt_first, semi)
        };
    // The constructor gate: the body's parent is a constructor iff the
    // parameter list behind it belongs to a member named like the
    // enclosing class (`C(...)`, `C.name(...)`) or carries a `:`
    // initializer clause before the body — both token-visible.
    if is_constructor_body(tokens, text, brace) {
        return None;
    }
    // Comments ride the trivia between the `{`/`return`/expr/`;`/`}`
    // boundaries — reassemble them in the producer's write order:
    // comments before `return`/`e` lead, comments before `;`/`}` trail.
    let expr_text = slice16(
        text,
        &utf16_span_between(text, tokens, expr_start, expr_end),
    )?;
    let pre_return = trivia_text(text, token_end_byte(tokens, brace), tokens, stmt_first);
    let pre_expr = if kind_of(tokens, stmt_first)? == TokenKind::Keyword(Keyword::Return) {
        trivia_text(text, token_end_byte(tokens, stmt_first), tokens, expr_start)
    } else {
        String::new()
    };
    let pre_semi = trivia_text(text, token_end_byte(tokens, expr_end - 1), tokens, expr_end);
    let pre_close = trivia_text(text, token_end_byte(tokens, expr_end), tokens, close);
    let mut replacement = String::new();
    // The official write order: `=> `, then the comments before
    // `return`/the expression, then the expression, then the comments
    // before `;`/`}` — `;` rides last, and only when the body is a
    // declared function/method (a lambda's `=> e` never takes `;` —
    // upstream Dart SDK source, pkg/analysis_server/lib/src/services/
    // correction/dart/convert_to_expression_function_body.dart:96-127).
    replacement.push_str("=> ");
    for piece in [&pre_return, &pre_expr] {
        if !piece.is_empty() {
            replacement.push_str(piece);
            replacement.push(' ');
        }
    }
    replacement.push_str(expr_text.trim());
    if !pre_semi.is_empty() {
        replacement.push(' ');
        replacement.push_str(&pre_semi);
    }
    if !pre_close.is_empty() {
        if pre_semi.is_empty() {
            replacement.push(' ');
        }
        replacement.push_str(&pre_close);
    }
    // The `;` is the declaration's terminator — a lambda body (the token
    // before its parameter list isn't a declaration head: `= (`, `, (`,
    // `( (`…) writes no `;`.
    if !body_is_lambda(tokens, brace) {
        replacement.push(';');
    }
    // The rewrite covers `[brace, close]` — the body's token range (the
    // `async` word before it is the parent's payload, not ours).
    let (start16, length16) = span_over(text, tokens, brace, close)?;
    Some((start16, length16, replacement))
}

/// The byte offset at a token's end.
pub(crate) fn token_end_byte(tokens: &TokenTable, index: usize) -> usize {
    token_end(tokens.get(index).expect("token index"))
}

/// The comment text in `[byte_at, tokens[next].byte_offset)` — the trivia
/// run between two tokens, comments kept, whitespace dropped (each comment
/// emitted as `// …` or `/* … */` separated by spaces).
pub(crate) fn trivia_text(text: &str, byte_at: usize, tokens: &TokenTable, next: usize) -> String {
    let end = tokens
        .get(next)
        .map(|t| t.byte_offset as usize)
        .unwrap_or(text.len());
    let slice = text.get(byte_at..end).unwrap_or("");
    let mut out = String::new();
    let bytes = slice.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"//") {
            let end = bytes[at..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(bytes.len(), |i| at + i);
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(slice[at..end].trim_end());
            at = end;
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
            let comment_end = if depth == 0 { i } else { bytes.len() };
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(slice[at..comment_end].trim());
            at = comment_end;
        } else {
            at += 1;
        }
    }
    out
}

/// Whether the block `{` at `brace` is a constructor's body — the
/// declaration head before it reads `Name(...)`/`Name.x(...)` (the second
/// identifier dotted) or `Name<...>(...)` with no return type, or a `:`
/// initializer clause trails the parameter list. `factory`-led heads are
/// constructors too — the caller pairs this with the producer's own
/// `factoryKeyword == null` refusal (a `factory` head converts fine).
pub(crate) fn is_constructor_body(tokens: &TokenTable, text: &str, brace: usize) -> bool {
    // The member head ends at the body's `async` keyword or at `brace`
    // itself; the parameter list's `)` is the last `)` before it whose
    // group closes there — walk back to it. A `:` initializer clause
    // between `)` and the body is constructor-only.
    let mut tail = brace;
    while let Some(prev) = tail.checked_sub(1) {
        match kind_of(tokens, prev) {
            Some(TokenKind::CloseParen) => {
                let Some(open) = matching_open(tokens, prev, TokenKind::CloseParen) else {
                    return false;
                };
                tail = open;
                continue;
            }
            Some(TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::Comma) => {
                tail = prev;
                continue;
            }
            Some(TokenKind::Colon) => return true,
            Some(
                TokenKind::Semicolon
                | TokenKind::OpenCurly
                | TokenKind::CloseCurly
                | TokenKind::Function,
            ) => {
                return false;
            }
            _ => return false,
        }
    }
    // No `:` clause — the member's name precedes the parameter list's `(`;
    // a `Name.name` shape is a named constructor, a bare `Name` matching
    // the enclosing class is the unnamed one.
    let Some(close_paren) = tail
        .checked_sub(1)
        .filter(|&i| kind_of(tokens, i) == Some(TokenKind::CloseParen))
    else {
        return false;
    };
    let Some(open_paren) = matching_open(tokens, close_paren, TokenKind::CloseParen) else {
        return false;
    };
    let Some(name) = open_paren.checked_sub(1) else {
        return false;
    };
    if kind_of(tokens, name) != Some(TokenKind::Identifier)
        && !matches!(kind_of(tokens, name), Some(TokenKind::Keyword(_)))
    {
        return false;
    }
    // `Name.name(` — the dot-joined pair is a named constructor.
    if name >= 2 && kind_of(tokens, name - 1) == Some(TokenKind::Period) {
        return true;
    }
    // The bare-name face: a constructor iff the name is the enclosing
    // declaration's. The member head before the name is scanned for a
    // return type — one identifier or a qualified/generic type — and the
    // enclosing container's own name.
    if let Some(class_name) = enclosing_container_name(tokens, text, open_paren) {
        return tokens.lexeme(text, name) == class_name;
    }
    false
}

/// The name of the `class`/`enum`/`mixin`/`extension`/`extension type`
/// declaration whose body encloses token `at` — found by walking the
/// brace pairs out to the container's `{` and its header's name.
pub(crate) fn enclosing_container_name<'s>(tokens: &TokenTable, text: &'s str, at: usize) -> Option<&'s str> {
    // The enclosing `{`: the innermost begin-group opened before `at` and
    // closed after it.
    let mut depth = 0u32;
    let mut cursor = at;
    let container = loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenCurly => {
                if depth == 0 {
                    break cursor;
                }
                depth -= 1;
            }
            _ => {}
        }
    };
    // The container's head: `class Name`, `enum Name`, `mixin Name`,
    // `extension type Name`, `extension Name` — the keyword's next word
    // token (modifiers like `base`/`abstract` skipped) before `{`.
    let mut head = container;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev) {
            Some(TokenKind::Keyword(Keyword::Class | Keyword::Enum | Keyword::Mixin)) => {
                return Some(tokens.lexeme(text, head));
            }
            Some(TokenKind::Keyword(Keyword::Extension)) => {
                // `extension type Name(...)` — the name follows `type`.
                let mut name = head;
                while kind_of(tokens, name) == Some(TokenKind::Keyword(Keyword::Typedef))
                    || word_at(tokens, text, name, "type")
                {
                    name += 1;
                }
                return Some(tokens.lexeme(text, name));
            }
            Some(TokenKind::Semicolon | TokenKind::CloseCurly) => return None,
            _ => head = prev,
        }
    }
    None
}

/// The UTF-16 `[start, end)` covering token `first` through the byte-end
/// of `last`'s predecessor — the token range's span.
pub(crate) fn utf16_span_between(
    text: &str,
    tokens: &TokenTable,
    first: usize,
    last: usize,
) -> core::ops::Range<u32> {
    let start = tokens.get(first).map(|t| t.utf16_offset).unwrap_or(0);
    let end = last
        .checked_sub(1)
        .and_then(|i| utf16_end(tokens, text, i))
        .unwrap_or(start);
    start..end
}

/// `always_put_required_named_parameters_first`: every `required` named
/// parameter after the first optional named one moves ahead of it — the
/// official `MakeRequiredNamedParametersFirst` reorder, expressed here as
/// one replacement of the `{...}` group's interior: required segments
/// (with their comments) first, then the optional ones, each in its
/// original order.
pub(crate) fn required_named_parameters_first(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The named-parameters group: the `{` group enclosing the hit.
    let mut depth = 0u32;
    let mut cursor = first;
    let group_open = loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::CloseCurly | TokenKind::CloseParen | TokenKind::CloseSquare => depth += 1,
            TokenKind::OpenCurly => {
                if depth == 0 {
                    break cursor;
                }
                depth -= 1;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
    };
    let group_close = tokens.matching_end(group_open)?;
    // The parameter segments split at the group's depth-0 commas.
    let mut bounds = Vec::new();
    let mut start = group_open + 1;
    let mut d = 0u32;
    let mut trailing_comma = false;
    for index in (group_open + 1)..group_close {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => d += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                d = d.checked_sub(1)?
            }
            TokenKind::Comma if d == 0 => {
                bounds.push((start, index));
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < group_close {
        bounds.push((start, group_close));
    } else {
        // The last comma abuts `}` — a trailing comma the official edit
        // keeps.
        trailing_comma = true;
    }
    // A segment is `required` iff its first keyword reads it, or its
    // metadata carries `@required` (the lint's `metadata.hasRequired`
    // gate fires on it — upstream Dart SDK source,
    // pkg/linter/lib/src/rules/always_put_required_named_parameters_first.dart:59).
    // Metadata `@x` rides the segment head.
    let is_required = |range: (usize, usize)| -> Option<bool> {
        let mut i = range.0;
        while i < range.1 {
            match kind_of(tokens, i)? {
                TokenKind::At => {
                    // `@required`/`@required(...)`: the annotation's head
                    // word decides (`required` scans as a keyword, not an
                    // identifier); any other `@meta` is inert.
                    let annotation_required =
                        (matches!(kind_of(tokens, i + 1), Some(TokenKind::Identifier))
                            && tokens.lexeme(text, i + 1) == "required")
                            || kind_of(tokens, i + 1)
                                == Some(TokenKind::Keyword(Keyword::Required));
                    // `@a.b(...)`: the annotation runs over
                    // byte-adjacent `.`-chained words, then an argument
                    // list — a space-separated token ends it.
                    let mut j = i + 1;
                    while matches!(
                        kind_of(tokens, j),
                        Some(TokenKind::Identifier | TokenKind::Period | TokenKind::Keyword(_))
                    ) && token_end_byte(tokens, j - 1)
                        == tokens.get(j).map(|t| t.byte_offset as usize).unwrap_or(0)
                    {
                        j += 1;
                    }
                    if kind_of(tokens, j) == Some(TokenKind::OpenParen)
                        && token_end_byte(tokens, j - 1)
                            == tokens.get(j).map(|t| t.byte_offset as usize).unwrap_or(0)
                    {
                        j = tokens.matching_end(j)? + 1;
                    }
                    i = j;
                    if annotation_required {
                        return Some(true);
                    }
                }
                TokenKind::Keyword(Keyword::Required) => return Some(true),
                _ => return Some(false),
            }
        }
        None
    };
    let first_optional = bounds.iter().position(|&b| is_required(b) == Some(false))?;
    let mut moved = Vec::new();
    let mut kept = Vec::new();
    for (i, &b) in bounds.iter().enumerate() {
        if i > first_optional && is_required(b) == Some(true) {
            moved.push(b);
        } else {
            kept.push(b);
        }
    }
    if moved.is_empty() {
        return None;
    }
    // The reported parameter must be the first offending one — the
    // official producer fires only on it.
    if moved
        .first()
        .is_none_or(|&b| !(first >= b.0 && first < b.1))
    {
        return None;
    }
    // Rebuild: kept order with the moved segments inserted where the
    // first optional sat — the group's whole interior rewrites.
    let mut interior = String::new();
    let mut ordered = Vec::new();
    for &b in &kept {
        if b == bounds[first_optional] {
            ordered.extend_from_slice(&moved);
        }
        ordered.push(b);
    }
    if kept.iter().all(|&b| b != bounds[first_optional]) {
        ordered.extend_from_slice(&moved);
    }
    for (i, &b) in ordered.iter().enumerate() {
        // A segment's comments ride with it — the trivia between the
        // boundary token (the preceding `,`, or `{` for the head segment)
        // and its first token re-emits ahead of the segment text (the
        // official moves `nodeWithComments` ranges — upstream Dart SDK
        // source, pkg/analysis_server/lib/src/services/correction/dart/
        // make_required_named_parameters_first.dart:60-73).
        let boundary = if bounds.first().is_some_and(|&f| f == b) {
            group_open
        } else {
            b.0 - 1 // the comma ending the previous segment
        };
        let comments = trivia_text(text, token_end_byte(tokens, boundary), tokens, b.0);
        if i > 0 {
            interior.push_str(", ");
        }
        if !comments.is_empty() {
            interior.push_str(&comments);
            interior.push(' ');
        }
        interior.push_str(segment_text(text, tokens, b).trim());
    }
    if trailing_comma {
        interior.push(',');
    }
    // The rewrite starts at the head segment's comments — the official
    // anchors at `firstComments?.offset ?? firstParameter.offset`, so the
    // moved parameters land ahead of them rather than erasing the trivia.
    let head_start_byte = {
        let trivia_start = token_end_byte(tokens, group_open);
        let first_byte = tokens.get(bounds[0].0)?.byte_offset as usize;
        let trivia = &text[trivia_start..first_byte];
        // The head parameter's first comment token — `/*` or `//`,
        // whichever leads (upstream `firstComments?.offset`).
        [trivia.find("/*"), trivia.find("//")]
            .into_iter()
            .flatten()
            .min()
            .map_or(first_byte, |rel| trivia_start + rel)
    };
    let start16 = utf16_count(&text[..head_start_byte]);
    let end16 = tokens.get(group_close)?.utf16_offset;
    Some((start16, end16.checked_sub(start16)?, interior))
}

/// The raw source text of a token-index range (comments and trivia
/// preserved verbatim).
pub(crate) fn segment_text(text: &str, tokens: &TokenTable, range: (usize, usize)) -> String {
    let start = tokens
        .get(range.0)
        .map(|t| t.byte_offset as usize)
        .unwrap_or(0);
    let end = range
        .1
        .checked_sub(1)
        .and_then(|i| tokens.get(i))
        .map(|t| token_end(t))
        .unwrap_or(start);
    text.get(start..end).unwrap_or("").to_owned()
}

/// The `{`/`}` token pair of the class-like body enclosing token `at`
/// — the innermost `{` opened before `at` and closed after it that heads
/// a `class`/`enum`/`mixin`/`extension`/`extension type` declaration
/// (distinguished from a function body by the `class`-family keyword in
/// its header).
pub(crate) fn enclosing_class_body(tokens: &TokenTable, at: usize) -> Option<(usize, usize)> {
    let mut depth = 0u32;
    let mut cursor = at;
    'outer: loop {
        let body = loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::CloseCurly => depth += 1,
                TokenKind::OpenCurly => {
                    if depth == 0 {
                        break cursor;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        };
        // Confirm the `{` heads a class-like declaration: scan its header
        // for the keyword. A method or block `{` fails the check — keep
        // walking out past it (its matching `}` lies after `at`, so the
        // depth count is untouched).
        let mut head = body;
        while let Some(prev) = head.checked_sub(1) {
            match kind_of(tokens, prev) {
                Some(TokenKind::Keyword(
                    Keyword::Class | Keyword::Enum | Keyword::Mixin | Keyword::Extension,
                )) => return Some((body, tokens.matching_end(body)?)),
                Some(TokenKind::Semicolon | TokenKind::CloseCurly) => {
                    cursor = body;
                    continue 'outer;
                }
                _ => head = prev,
            }
        }
        return None;
    }
}

/// The member boundaries inside a class-like body: `(start, end)` token
/// pairs — each member's first and last token (doc comments and metadata
/// ride the gap, matching the official `nodeWithComments` read on move).
/// `members` splits `[from, close)` at its depth-0 member ends (`;` fields
/// and `}`-closed bodies, `=>`-arrow members end at `;`).
pub(crate) fn class_members(tokens: &TokenTable, from: usize, close: usize) -> Vec<(usize, usize)> {
    let mut members = Vec::new();
    let mut start = from;
    let mut cursor = from;
    while cursor < close {
        match kind_of(tokens, cursor).unwrap_or(TokenKind::Eof) {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::Lt => {
                // Parameter/type groups ride the member head.
                if let Some(end) = tokens.matching_end(cursor) {
                    cursor = end;
                }
            }
            TokenKind::OpenCurly => {
                // A member body (`{ ... }`) ends the member.
                if let Some(end) = tokens.matching_end(cursor) {
                    members.push((start, end + 1));
                    start = end + 1;
                    cursor = end;
                }
            }
            TokenKind::Semicolon => {
                members.push((start, cursor + 1));
                start = cursor + 1;
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    if start < close {
        members.push((start, close));
    }
    members
}

/// `sort_constructors_first`: the reported constructor moves to the head
/// of its class body — the official `SortConstructorFirst` relocation:
/// `[insertion, ctor_end)` becomes `newlineAndIndent + ctorText +
/// blankLineSeparator + (the members that stood before the ctor)`. The
/// insertion point is the class `{`'s end; for an enum it is just after
/// the constant list's `;` (or the last constant). `ctorText` is the
/// member's full source — doc comments and metadata ride it (the official
/// `nodeWithComments` range).
pub(crate) fn constructor_first(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    let (body_open, body_close) = enclosing_class_body(tokens, first)?;
    // The insertion offset: the class `{`'s end; for an enum the constant
    // list's `;` end (or the `{` when no `;` stands). These are byte
    // positions inside trivia — the official's `leftBracket.end` /
    // `semicolon.end`.
    let (insertion_byte, members_from, enum_body) = if is_enum_body(tokens, body_open) {
        let mut semi = None;
        let mut d = 0u32;
        for index in body_open + 1..body_close {
            match kind_of(tokens, index)? {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => d += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    d = d.checked_sub(1)?;
                }
                TokenKind::Semicolon if d == 0 => {
                    semi = Some(index);
                    break;
                }
                _ => {}
            }
        }
        match semi {
            Some(s) => (token_end_byte(tokens, s), s + 1, true),
            None => (token_end_byte(tokens, body_open), body_open + 1, true),
        }
    } else {
        (token_end_byte(tokens, body_open), body_open + 1, false)
    };
    let members = class_members(tokens, members_from, body_close);
    let target_idx = members.iter().position(|&(s, e)| first >= s && first < e)?;
    if target_idx == 0 {
        return None; // already first — no edit the official would offer.
    }
    let target = members[target_idx];
    // The gap `[prev_member_end, node_with_comments_start)` is separator
    // whitespace — its last newline plus indentation becomes the relocated
    // member's leading line break; `nodeWithComments` runs from the
    // leading comment block, the plain member head otherwise.
    let prev_member_end = member_end_byte(text, tokens, members[target_idx - 1].1 - 1);
    // The official `nodeWithComments` range starts at the comment/node's own
    // offset — the leading indentation of the block's first line rides the
    // gap, not the moved text.
    let block_start = member_block_start(text, tokens.get(target.0)?.utf16_offset);
    let moved_start = block_start
        + text[block_start..]
            .find(|c: char| !matches!(c, ' ' | '\t'))
            .unwrap_or(0);
    let gap = text.get(prev_member_end..moved_start).unwrap_or("");
    let newline_indent = match gap.rfind('\n') {
        Some(i) => &gap[i..],
        None => gap,
    };
    let moved_end = member_end_byte(text, tokens, target.1 - 1);
    let moved = text.get(moved_start..moved_end)?;
    // A blank line between the previous member and the constructor was
    // its separator — it re-lands after the relocated member unless a
    // blank line already follows the insertion point.
    let had_blank = gap.matches('\n').count() > 1;
    let after = text.get(insertion_byte..moved_start).unwrap_or("");
    let already_blank_after = {
        // The line after `insertion_byte` is blank from it through the
        // next newline, and the following line is empty (the official
        // `_hasBlankLineAfter`).
        let line_rest = after.split('\n').next().unwrap_or("");
        let next_line = after
            .split('\n')
            .nth(1)
            .map(|s| s.trim().is_empty())
            .unwrap_or(false);
        line_rest.trim().is_empty() && next_line
    };
    let blank_sep = if had_blank && !already_blank_after {
        "\n"
    } else {
        ""
    };
    let leading_blank = if enum_body && had_blank { "\n" } else { "" };
    // One rewrite over `[insertion_byte, ctor_end)`: the deleted
    // `[prev_member_end, ctor_end)` text re-lands at the head, the members
    // that stood before it unchanged between.
    let middle = text.get(insertion_byte..prev_member_end)?;
    let start16 = utf16_count(&text[..insertion_byte]);
    let end16 = utf16_count(&text[..moved_end]);
    Some((
        start16,
        end16.checked_sub(start16)?,
        format!("{leading_blank}{newline_indent}{moved}{blank_sep}{middle}"),
    ))
}

/// Whether the class-body `{` at `open` heads an `enum` declaration.
pub(crate) fn is_enum_body(tokens: &TokenTable, open: usize) -> bool {
    let mut head = open;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev) {
            Some(TokenKind::Keyword(Keyword::Enum)) => return true,
            Some(TokenKind::Keyword(Keyword::Class | Keyword::Mixin | Keyword::Extension)) => {
                return false;
            }
            Some(TokenKind::Semicolon | TokenKind::CloseCurly) => return false,
            _ => head = prev,
        }
    }
    false
}

/// A member's first-token byte offset.
pub(crate) fn member_start_byte(_text: &str, tokens: &TokenTable, index: usize) -> usize {
    tokens
        .get(index)
        .map(|t| t.byte_offset as usize)
        .unwrap_or(0)
}

/// A member's last-token byte end.
pub(crate) fn member_end_byte(_text: &str, tokens: &TokenTable, index: usize) -> usize {
    tokens.get(index).map(token_end).unwrap_or(0)
}

/// `sort_unnamed_constructors_first`: the reported unnamed/`new`
/// constructor moves ahead of the first constructor in its class body —
/// the official `SortUnnamedConstructorFirst` move of the `[prevMember.end,
/// ctor.end)` range (the gap before the member rides with it) to just
/// after the member that precedes the first constructor (or the body's
/// head / enum `;` when the first constructor leads).
pub(crate) fn unnamed_constructor_first(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    let (body_open, body_close) = enclosing_class_body(tokens, first)?;
    // The insertion offset when the first constructor leads the body: the
    // official `topOfBodyOffset` — the `{`'s end for a class, the constant
    // list's `;` end (or the `{`'s end when no `;` stands) for an enum.
    let (top_of_body, members_from) = if is_enum_body(tokens, body_open) {
        let mut semi = None;
        let mut d = 0u32;
        for index in body_open + 1..body_close {
            match kind_of(tokens, index)? {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => d += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    d = d.checked_sub(1)?;
                }
                TokenKind::Semicolon if d == 0 => {
                    semi = Some(index);
                    break;
                }
                _ => {}
            }
        }
        match semi {
            Some(s) => (token_end_byte(tokens, s), s + 1),
            None => (token_end_byte(tokens, body_open), body_open + 1),
        }
    } else {
        (token_end_byte(tokens, body_open), body_open + 1)
    };
    let members = class_members(tokens, members_from, body_close);
    let target_idx = members.iter().position(|&(s, e)| first >= s && first < e)?;
    if !is_unnamed_constructor(tokens, text, members[target_idx]) {
        return None;
    }
    // The first constructor in member order — the unnamed one must land
    // ahead of it. The predicate is gated on the enclosing container's
    // name (the official `whereType<ConstructorDeclaration>` filter):
    // `Identifier (`/`Identifier . Identifier (` also matches a
    // return-type-less method `helper() {}`/`Other.name() {}`.
    let class_name = enclosing_container_name(tokens, text, first)?;
    let first_ctor = members
        .iter()
        .position(|&m| is_constructor_member(tokens, text, m, class_name))?;
    if target_idx == first_ctor {
        return None;
    }
    // The insertion offset: the member before the first constructor's end
    // — or the body's head when the first constructor leads.
    let insertion_byte = if first_ctor == 0 {
        top_of_body
    } else {
        member_end_byte(text, tokens, members[first_ctor - 1].1 - 1)
    };
    let prev_end = member_end_byte(text, tokens, members[target_idx - 1].1 - 1);
    let ctor_end = member_end_byte(text, tokens, members[target_idx].1 - 1);
    let moved = text.get(prev_end..ctor_end)?;
    let middle = text.get(insertion_byte..prev_end)?;
    let region_start16 = utf16_count(&text[..insertion_byte]);
    let region_end16 = utf16_count(&text[..ctor_end]);
    Some((
        region_start16,
        region_end16.checked_sub(region_start16)?,
        format!("{moved}{middle}"),
    ))
}

/// Whether the member range is a constructor declaration — a `factory`
/// keyword or a `Name(`/`Name.name(` head whose `Name` is the enclosing
/// container's own (the official `whereType<ConstructorDeclaration>`);
/// any other identifier-headed member is a return-type-less method.
pub(crate) fn is_constructor_member(
    tokens: &TokenTable,
    text: &str,
    member: (usize, usize),
    class_name: &str,
) -> bool {
    let (s, e) = member;
    let mut i = s;
    // Leading keywords (`const`, `factory`, `external`, metadata already
    // folded into the gap by `class_members`).
    while i < e
        && matches!(
            kind_of(tokens, i),
            Some(TokenKind::Keyword(
                Keyword::Const | Keyword::Factory | Keyword::External | Keyword::Static
            ))
        )
    {
        i += 1;
    }
    if i >= e || !matches!(kind_of(tokens, i), Some(TokenKind::Identifier)) {
        return false;
    }
    // The head's first identifier must be the container's name — a
    // `helper()`/`Other.named()` face is a method, not a constructor.
    if tokens.lexeme(text, i) != class_name {
        return false;
    }
    let mut j = i + 1;
    if kind_of(tokens, j) == Some(TokenKind::Period) {
        j += 1;
        // `C.new(` — `new` scans as a keyword; every other named face is
        // a plain identifier.
        if !matches!(
            kind_of(tokens, j),
            Some(TokenKind::Identifier | TokenKind::Keyword(Keyword::New))
        ) {
            return false;
        }
        j += 1;
    }
    kind_of(tokens, j) == Some(TokenKind::OpenParen)
}

/// Whether the member range is an unnamed (`Name(`/`Name.new(`)
/// constructor — the official `name == null || name.lexeme == 'new'`
/// read.
pub(crate) fn is_unnamed_constructor(tokens: &TokenTable, text: &str, member: (usize, usize)) -> bool {
    let (s, e) = member;
    let mut i = s;
    // Skip leading keywords (`const`, `factory`, `external`).
    while i < e
        && matches!(
            kind_of(tokens, i),
            Some(TokenKind::Keyword(
                Keyword::Const | Keyword::Factory | Keyword::External
            ))
        )
    {
        i += 1;
    }
    if i >= e || !matches!(kind_of(tokens, i), Some(TokenKind::Identifier)) {
        return false;
    }
    match kind_of(tokens, i + 1) {
        Some(TokenKind::OpenParen) => true,
        Some(TokenKind::Period) => word_at(tokens, text, i + 2, "new"),
        _ => false,
    }
}

/// `unnecessary_getters_setters`: the trivial getter/setter pair folds back
/// into the field it wraps — the official `MakeFieldPublic` deletes the
/// getter and the setter member ranges and renames the `_x` field to `x`.
/// One span rewrite over the class body: inside the span covering the
/// three members, the getter and setter ranges drop and the field's
/// `_name` token becomes `name`; interleaved members keep their text.
pub(crate) fn field_public(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    // The hit is width-0 on the getter's name token.
    let name = token_starting_at(tokens, input.range.start)?;
    let getter_name = tokens.lexeme(text, name);
    let (body_open, body_close) = enclosing_class_body(tokens, name)?;
    let members = class_members(tokens, body_open + 1, body_close);
    // The getter member covers the name and carries `get`.
    let getter_idx = members.iter().position(|&(s, e)| name >= s && name < e)?;
    if !member_has_keyword(tokens, members[getter_idx], Keyword::Get) {
        return None;
    }
    // The sibling setter `set name` and the `_name` field member.
    let setter_idx = members.iter().position(|&m| {
        member_has_keyword(tokens, m, Keyword::Set)
            && member_name(tokens, text, m) == Some(getter_name)
    })?;
    let field_name = format!("_{getter_name}");
    let field_idx = members.iter().position(|&m| {
        !member_has_keyword(tokens, m, Keyword::Get)
            && !member_has_keyword(tokens, m, Keyword::Set)
            && member_declares_name(tokens, text, m, &field_name)
    });
    // The renamed token: the field's `_name` inside the field member — or,
    // when no field member stands, the declaring `final`/`var _name`
    // parameter of the class's primary-constructor head (upstream Dart SDK
    // source, pkg/analysis_server/lib/src/services/correction/dart/
    // make_field_public.dart:73-77,160-175).
    let (name_byte, name_end) = match field_idx {
        Some(field_idx) => {
            let f = members[field_idx];
            let name_tok = (f.0..f.1).find(|&i| {
                matches!(kind_of(tokens, i), Some(TokenKind::Identifier))
                    && tokens.lexeme(text, i) == field_name
                    && matches!(
                        kind_of(tokens, i + 1),
                        Some(TokenKind::Semicolon | TokenKind::Eq | TokenKind::Comma)
                    )
            })?;
            let start = tokens.get(name_tok)?.byte_offset as usize;
            (start, start + field_name.len())
        }
        None => {
            let name_tok = primary_ctor_declaring_param(tokens, text, body_open, &field_name)?;
            let start = tokens.get(name_tok)?.byte_offset as usize;
            (start, start + field_name.len())
        }
    };
    // The official `_deleteMembers` runs over member indices: a contiguous
    // removed run at index 0 deletes `[members[0].start, members[upper+1].start)`
    // — or whole lines when the run swallows every member — and any other
    // run deletes `[prevMember.end, lastRemoved.end)`; getter and setter
    // group under `IndexRange.contiguousSubRanges`.
    let mut removed = vec![getter_idx, setter_idx];
    removed.sort_unstable();
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &idx in &removed {
        if let Some(last) = groups.last_mut()
            && idx == last.1 + 1
        {
            last.1 = idx;
            continue;
        }
        groups.push((idx, idx));
    }
    let mut spans = Vec::with_capacity(groups.len());
    for (lo_idx, hi_idx) in groups {
        let span = if lo_idx == 0 && hi_idx == members.len() - 1 {
            // Whole lines from the first to the last removed member.
            let start = line_start_before(text, member_start_byte(text, tokens, members[lo_idx].0));
            let mut end = member_end_byte(text, tokens, members[hi_idx].1 - 1);
            if let Some(rel) = text.get(end..).and_then(|t| t.find('\n')) {
                end += rel + 1;
            }
            (start, end)
        } else if lo_idx == 0 {
            (
                member_start_byte(text, tokens, members[lo_idx].0),
                member_start_byte(text, tokens, members[hi_idx + 1].0),
            )
        } else {
            (
                member_end_byte(text, tokens, members[lo_idx - 1].1 - 1),
                member_end_byte(text, tokens, members[hi_idx].1 - 1),
            )
        };
        spans.push(span);
    }
    let lo = spans.iter().map(|s| s.0).min()?.min(name_byte);
    let hi = spans.iter().map(|s| s.1).max()?.max(name_end);
    if !(lo <= name_byte && name_end <= hi) {
        return None;
    }
    // Rebuild `[lo, hi)`: the dropped spans leave, the field's `_name`
    // becomes `name`, and every other byte stays.
    let mut rebuilt = String::new();
    let mut at = lo;
    for &(ds, de) in &spans {
        if de <= at || ds >= hi {
            continue;
        }
        push_with_rename(&mut rebuilt, text, at..ds, name_byte, name_end, getter_name);
        at = de;
    }
    push_with_rename(&mut rebuilt, text, at..hi, name_byte, name_end, getter_name);
    let start16 = utf16_count(&text[..lo]);
    let len16 = utf16_count(&text[lo..hi]);
    Some((start16, len16, rebuilt))
}

/// Appends `text[range]` to `out`, replacing the `[name, name_end)` byte
/// span with `rename` when it lands inside.
pub(crate) fn push_with_rename(
    out: &mut String,
    text: &str,
    range: core::ops::Range<usize>,
    name: usize,
    name_end: usize,
    rename: &str,
) {
    if range.start <= name && name_end <= range.end {
        out.push_str(&text[range.start..name]);
        out.push_str(rename);
        out.push_str(&text[name_end..range.end]);
    } else {
        out.push_str(text.get(range).unwrap_or(""));
    }
}

/// Whether the member range carries keyword `kw` anywhere in it.
pub(crate) fn member_has_keyword(tokens: &TokenTable, member: (usize, usize), kw: Keyword) -> bool {
    (member.0..member.1).any(|i| kind_of(tokens, i) == Some(TokenKind::Keyword(kw)))
}

/// The declared name a `get`/`set`/method member carries — the identifier
/// immediately before its `(` parameter list or its `=>`/`{`/`;`
/// terminator.
pub(crate) fn member_name<'s>(tokens: &TokenTable, text: &'s str, member: (usize, usize)) -> Option<&'s str> {
    let (s, e) = member;
    for i in s..e {
        match kind_of(tokens, i) {
            Some(TokenKind::OpenParen) => {
                // The token right before `(` is the member name; a `get`/`set`
                // keyword may precede it (`set name(`), the type before that.
                let name = i.checked_sub(1)?;
                return if matches!(kind_of(tokens, name), Some(TokenKind::Identifier)) {
                    Some(tokens.lexeme(text, name))
                } else {
                    None
                };
            }
            Some(TokenKind::Function | TokenKind::Semicolon | TokenKind::OpenCurly) => {
                // `get name =>`, `get name {`, `type name;` — the identifier
                // right before the terminator is the name.
                let mut name = i.checked_sub(1)?;
                while name > s
                    && matches!(
                        kind_of(tokens, name),
                        Some(
                            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
                        )
                    )
                {
                    name = name.checked_sub(1)?;
                }
                return if matches!(kind_of(tokens, name), Some(TokenKind::Identifier)) {
                    Some(tokens.lexeme(text, name))
                } else {
                    None
                };
            }
            _ => {}
        }
    }
    None
}

/// The `_name` token of a declaring `final`/`var` parameter in the
/// primary-constructor head of the class whose body opens at `body_open`
/// — `class C(final int _x)` / `class C<T>.name(var _x)`. The parameter
/// list is the `(` group after the class name's `<T>`/`.name` tail; a
/// parameter declares when its segment carries `final`/`var` (upstream
/// `isDeclaringParameter` — the `finalOrVarKeyword` read).
pub(crate) fn primary_ctor_declaring_param(
    tokens: &TokenTable,
    text: &str,
    body_open: usize,
    name: &str,
) -> Option<usize> {
    // The `class` keyword heading the body (modifiers ride before it).
    let mut head = body_open;
    let class_kw = loop {
        head = head.checked_sub(1)?;
        match kind_of(tokens, head)? {
            TokenKind::Keyword(Keyword::Class) => break head,
            TokenKind::Semicolon | TokenKind::CloseCurly | TokenKind::OpenCurly => return None,
            _ => {}
        }
    };
    // After `class`: the name, then an optional `<T>` type-parameter group
    // and `.name` ctor selector, then the parameter list's `(`.
    let mut i = class_kw + 1;
    if !matches!(kind_of(tokens, i), Some(TokenKind::Identifier)) {
        return None;
    }
    i += 1;
    if kind_of(tokens, i) == Some(TokenKind::Lt) {
        i = angle_group_end(tokens, i)? + 1;
    }
    if kind_of(tokens, i) == Some(TokenKind::Period) {
        i += 1;
        if !matches!(
            kind_of(tokens, i),
            Some(TokenKind::Identifier | TokenKind::Keyword(Keyword::New))
        ) {
            return None;
        }
        i += 1;
    }
    if kind_of(tokens, i) != Some(TokenKind::OpenParen) {
        return None;
    }
    let group_close = tokens.matching_end(i)?;
    if group_close > body_open {
        return None;
    }
    let mut seg_start = i + 1;
    let mut depth = 0u32;
    let mut cursor = i + 1;
    while cursor <= group_close {
        let kind = if cursor == group_close {
            // The group's own `)` ends the last segment.
            Some(TokenKind::Comma)
        } else {
            kind_of(tokens, cursor)
        };
        match kind? {
            TokenKind::Comma | TokenKind::Semicolon if depth == 0 => {
                // Segment `[seg_start, cursor)` — a `final`/`var` keyword
                // makes it a declaring parameter (upstream
                // `isDeclaringParameter`); its `_name` identifier is the
                // token to rename.
                let mut declares = false;
                for j in seg_start..cursor {
                    match kind_of(tokens, j) {
                        Some(TokenKind::Keyword(Keyword::Final | Keyword::Var)) => {
                            declares = true;
                        }
                        Some(TokenKind::Identifier)
                            if declares && tokens.lexeme(text, j) == name =>
                        {
                            return Some(j);
                        }
                        _ => {}
                    }
                }
                seg_start = cursor + 1;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::Lt => {
                depth += 1
            }
            TokenKind::CloseParen
            | TokenKind::CloseSquare
            | TokenKind::CloseCurly
            | TokenKind::Gt => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}
/// official `member is FieldDeclaration` + `member.fields.variables` scan
/// (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/make_field_public.dart:61-67).
/// The token face of a field declaration: the member ends at a depth-0
/// `;` with no `(`/`{`/`=>` ahead of the declared name — a method's
/// `_x = 1;` body or `void m() => _x;` never qualifies.
pub(crate) fn member_declares_name(
    tokens: &TokenTable,
    text: &str,
    member: (usize, usize),
    name: &str,
) -> bool {
    let (s, e) = member;
    // A field declaration's last token is the `;` ending it.
    if kind_of(tokens, e - 1) != Some(TokenKind::Semicolon) {
        return false;
    }
    let mut depth = 0u32;
    for i in s..e {
        match kind_of(tokens, i) {
            Some(TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::Lt) => depth += 1,
            Some(TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::Gt) => {
                depth = depth.saturating_sub(1);
            }
            // A `(`/`{`/`=>` at depth 0 before the name is a parameter
            // list, a body, or an arrow — never a field declaration.
            Some(TokenKind::OpenCurly | TokenKind::Function) if depth == 0 => return false,
            Some(TokenKind::Identifier) if depth == 0 && tokens.lexeme(text, i) == name => {
                return matches!(
                    kind_of(tokens, i + 1),
                    Some(TokenKind::Semicolon | TokenKind::Eq | TokenKind::Comma)
                );
            }
            _ => {}
        }
    }
    false
}

/// `cascade_invocations`: the reported run of statements on one receiver
/// folds into cascades — `x = C(); x.f(); x.g();` becomes
/// `x = C()..f()..g();` (the official `ConvertToCascade`: each later
/// statement's `target`/`operator` collapses to `..`, the previous
/// statement's `;` drops, and an assignment's initializer wraps in parens
/// when it binds below postfix).
///
/// The report covers statements 2..n of the run; the producer also
/// touches statement 1 — the target's declaration or first use — which
/// the token walk recovers as the statement right before the span.
pub(crate) fn cascade_merge(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    // The reported statements: the `;`-terminated runs inside the span,
    // group interiors skipped (a lambda body's `;` never splits).
    let mut runs = Vec::new();
    let mut start = first;
    let mut index = first;
    while index <= last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                index = tokens.matching_end(index)?;
            }
            TokenKind::Semicolon => {
                runs.push((start, index));
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if runs.is_empty() {
        return None;
    }
    // The anchor statement — the one right before the report (the
    // producer's `statements[indexOf(firstCascadeStatement) - 1]`): the
    // token run ending at the `;`/`}`/`{`/`:` preceding the span.
    let mut boundary = first;
    let anchor_end = loop {
        match boundary.checked_sub(1).and_then(|p| kind_of(tokens, p)) {
            Some(TokenKind::Semicolon) => break boundary - 1,
            Some(TokenKind::OpenCurly | TokenKind::CloseCurly | TokenKind::Colon) | None => {
                return None;
            }
            _ => boundary -= 1,
        }
    };
    let mut anchor_start = anchor_end;
    // A `:` walking back is a boundary only as a label/case terminator — a
    // conditional's `:` pairs with a `?` earlier in the same statement.
    // Parked colons stack; each `?` pops its nearest pair, and any colon
    // left when the walk ends is the real boundary.
    let mut colons: Vec<usize> = Vec::new();
    while let Some(p) = anchor_start.checked_sub(1) {
        match kind_of(tokens, p)? {
            TokenKind::Semicolon | TokenKind::OpenCurly => break,
            // A `}`/`)`/`]` closing the anchor's tail (a lambda or
            // collection literal: `var f = () { … };`) jumps to its
            // matching open — the statement's real start sits earlier.
            close @ (TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly) => {
                anchor_start = matching_open(tokens, p, close)?;
            }
            TokenKind::Colon => {
                colons.push(p);
                anchor_start = p;
            }
            TokenKind::Question if !colons.is_empty() => {
                colons.pop();
                anchor_start = p;
            }
            _ => anchor_start = p,
        }
    }
    if let Some(&colon) = colons.last() {
        // The nearest unpaired `:` — the anchor statement begins after it.
        anchor_start = colon + 1;
    }
    // The anchor's own face: a `var x = …;`/`x = …;` declaration or
    // assignment keeps its text (its initializer may wrap in parens); an
    // expression statement `x.f();` collapses its `.` to `..`; a cascade
    // `x..f();` keeps. `switch`/`case` labels end at `:` — the boundary
    // walk already accepts it.
    let anchor_body = (anchor_start, anchor_end); // tokens [start, semi)
    let mut out = String::new();
    // The anchor text — `[anchor_start, anchor_end)` (excluding `;`).
    let a_s = tokens.get(anchor_body.0)?.byte_offset as usize;
    let a_e = token_end(tokens.get(anchor_body.1 - 1)?);
    let mut anchor_text = text.get(a_s..a_e)?.to_owned();
    // The initializer-parens arm: a `=` assignment/declaration whose RHS
    // binds below postfix wraps (`x = a ? b : c` → `x = (a ? b : c)`).
    if let Some(eq) = depth0_find(tokens, text, anchor_body.0, anchor_body.1, |kind, _| {
        kind == TokenKind::Eq
    }) {
        let rhs_s = eq + 1;
        let rhs_e = anchor_body.1;
        let low = rhs_s < rhs_e
            && (depth0_find(tokens, text, rhs_s, rhs_e, |kind, _| {
                matches!(
                    kind,
                    TokenKind::QuestionQuestion
                        | TokenKind::Question
                        | TokenKind::AmpersandAmpersand
                        | TokenKind::BarBar
                        | TokenKind::EqEq
                        | TokenKind::BangEq
                        | TokenKind::Lt
                        | TokenKind::Gt
                        | TokenKind::LtEq
                        | TokenKind::GtEq
                        | TokenKind::Ampersand
                        | TokenKind::Bar
                        | TokenKind::Caret
                        | TokenKind::Keyword(Keyword::As | Keyword::Is)
                )
            })
            .is_some()
                || matches!(
                    kind_of(tokens, rhs_s),
                    Some(TokenKind::Bang | TokenKind::Minus | TokenKind::Tilde)
                ));
        if low {
            let ps = tokens.get(rhs_s)?.byte_offset as usize - a_s;
            let pe = token_end(tokens.get(rhs_e - 1)?) - a_s;
            anchor_text = format!(
                "{}({}){}",
                &anchor_text[..ps],
                &anchor_text[ps..pe],
                &anchor_text[pe..]
            );
        }
        // The `x.f = v` lhs arm: a `PrefixedIdentifier` left side doubles
        // its `.` (the official `previousOperator` insertion for an
        // `ExpressionStatement` anchor — `x.f = 0; x.g = 1;` becomes
        // `x..f = 0..g = 1;`).
        if let Some(op) = cascade_operator(tokens, text, anchor_body.0, eq, true) {
            let dot = tokens.get(op)?.byte_offset as usize - a_s;
            anchor_text.insert(dot, '.');
        }
    } else if let Some(op) = cascade_operator(tokens, text, anchor_body.0, anchor_body.1, false) {
        // An expression-statement anchor (`x.f();`) doubles its operator.
        let dot = tokens.get(op)?.byte_offset as usize - a_s;
        anchor_text.insert(dot, '.');
    }
    out.push_str(&anchor_text);
    // Each later statement: the receiver text drops, the `.`/`?..`
    // operator doubles (`..`/`?..`), the previous `;` goes (the trivia
    // between it and the statement start survives — the official deletes
    // the `;` alone), and the selector tail (`f(args)` / `f = v`) stays.
    let mut prev_semi_end = token_end(tokens.get(anchor_body.1)?);
    for &(s, e) in &runs {
        let op = cascade_operator(tokens, text, s, e, false)?;
        // The trivia run between the previous `;` and this statement's
        // first token (`\n  `, comments) rides along.
        out.push_str(
            text.get(prev_semi_end..tokens.get(s)?.byte_offset as usize)
                .unwrap_or(""),
        );
        // The selector text: from the operator's end through the
        // statement's last real token.
        let op_end = token_end(tokens.get(op)?);
        let stmt_end = token_end(tokens.get(e - 1)?);
        let sel = text.get(op_end..stmt_end).unwrap_or("");
        match kind_of(tokens, op)? {
            TokenKind::Period | TokenKind::PeriodPeriod => out.push_str(".."),
            TokenKind::QuestionPeriodPeriod => out.push_str("?.."),
            _ => return None,
        }
        out.push_str(sel);
        prev_semi_end = token_end(tokens.get(e)?);
    }
    out.push(';');
    let start16 = tokens.get(anchor_start)?.utf16_offset;
    let end16 = utf16_end(tokens, text, runs.last()?.1)?;
    Some((start16, end16.checked_sub(start16)?, out))
}

/// The `.`-family token splitting a statement `recv.sel…` into receiver
/// and selector — the `.`/`..`/`?..` immediately before the selector
/// identifier: for an invocation the identifier right before the
/// outermost `(`; for an assignment the identifier right before `=`;
/// for a bare run (`bare_lhs_ok` — the anchor's `x.f = v` left side) the
/// run's last identifier.
pub(crate) fn cascade_operator(
    tokens: &TokenTable,
    text: &str,
    s: usize,
    e: usize,
    bare_lhs_ok: bool,
) -> Option<usize> {
    if s >= e {
        return None;
    }
    // The statement's last token (before `;` — `e` is the `;` index).
    let last = e.checked_sub(1)?;
    let selector_name;
    match kind_of(tokens, last)? {
        TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
            // `recv.sel(args)` / `recv[…].sel(…)` — the selector name is
            // the identifier right before the matching open of the last
            // group (through `args`), with generic `<…>` tails skipped.
            let close_kind = kind_of(tokens, last)?;
            let open = matching_open(tokens, last, close_kind)?;
            let mut name = open.checked_sub(1)?;
            if kind_of(tokens, name) == Some(TokenKind::Gt) {
                let lt = angle_group_start(tokens, name)?;
                name = lt.checked_sub(1)?;
            }
            selector_name = name;
        }
        _ => {
            // `recv.sel = v` / `recv.sel` — scan back over the `=` tail to
            // the selector name: the last identifier before a top-level
            // `=` (or the last identifier when none stands).
            let mut name;
            if let Some(eq) = depth0_find(tokens, text, s, e, |kind, _| kind == TokenKind::Eq) {
                // The lhs is `[s, eq)`; the selector name is its last
                // identifier.
                let mut i = eq;
                loop {
                    i = i.checked_sub(1)?;
                    if i < s {
                        return None;
                    }
                    if matches!(kind_of(tokens, i), Some(TokenKind::Identifier)) {
                        name = i;
                        break;
                    }
                    match kind_of(tokens, i)? {
                        TokenKind::CloseParen | TokenKind::CloseSquare => {
                            let open = matching_open(tokens, i, kind_of(tokens, i)?)?;
                            i = open;
                        }
                        _ => {}
                    }
                    if i <= s {
                        return None;
                    }
                }
            } else {
                // A bare `recv.sel` run — either a `PropertyAccess`
                // statement (refused: the official `_getTargetAndOperator`
                // only takes invocations, cascade sections and
                // `prefixed = v` assignments) or a `recv.sel` left side
                // whose `=` the caller clipped at `e` (the anchor's
                // `PrefixedIdentifier` arm — the last identifier *is* the
                // selector).
                if !bare_lhs_ok {
                    return None;
                }
                name = last;
                while !matches!(kind_of(tokens, name), Some(TokenKind::Identifier)) {
                    name = name.checked_sub(1)?;
                    if name < s {
                        return None;
                    }
                }
            }
            selector_name = name;
        }
    }
    // The operator is the `.`-family token right before the selector name.
    let op = selector_name.checked_sub(1)?;
    match kind_of(tokens, op)? {
        TokenKind::Period | TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod => {}
        _ => return None,
    }
    // The receiver `[s, op)` must stand as a bare simple identifier —
    // the upstream gates: `MethodInvocation.target` is a
    // `SimpleIdentifier` for the lint to have fired, and the assignment
    // arm takes `lhs is PrefixedIdentifier` whose `prefix` is that
    // identifier.
    if op != s + 1 || kind_of(tokens, s)? != TokenKind::Identifier {
        return None;
    }
    Some(op)
}

/// `avoid_private_typedef_functions`: the private typedef folds into its
/// one use site — the official `InlineTypedef` deletes the typedef's line
/// and writes `R Function(params)` at the single `NamedType` reference.
/// The report spans the typedef's name; the single in-file use is located
/// as the lone `_Name` identifier outside the declaration (every such
/// token is a `NamedType` — a private typedef's name only ever reads in
/// type position). A zero- or multi-use file drops the fix, exactly the
/// official's `finder.count != 1` refusal.
pub(crate) fn inline_typedef(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let name_idx = token_starting_at(tokens, input.range.start)?;
    let name = tokens.lexeme(text, name_idx);
    if !name.starts_with('_') {
        return None;
    }
    // The typedef declaration: `typedef R _F(params);` (FunctionTypeAlias)
    // or `typedef _F = R Function(params);` (GenericTypeAlias without its
    // own type params). Locate the `typedef` keyword behind the name.
    let mut head = name_idx;
    let typedef_kw = loop {
        match kind_of(tokens, head)? {
            TokenKind::Keyword(Keyword::Typedef) => break head,
            TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly => return None,
            _ => {
                head = head.checked_sub(1)?;
            }
        }
    };
    // The declaration's end: its `;`.
    let decl_end = depth0_find(tokens, text, typedef_kw, tokens.len(), |kind, _| {
        kind == TokenKind::Semicolon
    })?;
    // The replacement text: `R Function<…>(params)` — the pieces read off
    // the two syntax faces.
    let (ret, tparams, params) = typedef_signature(tokens, text, typedef_kw, name_idx, decl_end)?;
    // The lone in-file use: an identifier `_F` outside `[typedef_kw,
    // decl_end]`, not preceded by `.` (a prefixed `p._F` names another
    // declaration). Only `NamedType` positions count (the official
    // `_ReferenceFinder` visits `NamedType` alone — upstream Dart SDK
    // source, pkg/analysis_server/lib/src/services/correction/dart/
    // inline_typedef.dart:145-161): `show`/`hide` combinator names,
    // `@` annotation heads and string-interpolation identifiers never
    // resolve to the typedef.
    let mut uses = Vec::new();
    // Open `${` interpolation group ends (they nest); a token inside one
    // is string content, not a `NamedType`.
    let mut interp_ends: Vec<usize> = Vec::new();
    for i in 0..tokens.len() {
        while interp_ends.last().is_some_and(|&end| i >= end) {
            interp_ends.pop();
        }
        match kind_of(tokens, i) {
            Some(TokenKind::StringInterpolationExpression) => {
                if let Some(end) = tokens.matching_end(i) {
                    interp_ends.push(end);
                }
                continue;
            }
            _ => {}
        }
        if !interp_ends.is_empty() || (i >= typedef_kw && i <= decl_end) {
            continue;
        }
        if matches!(kind_of(tokens, i), Some(TokenKind::Identifier))
            && tokens.lexeme(text, i) == name
            && !matches!(kind_of(tokens, i.wrapping_sub(1)), Some(TokenKind::Period))
            && (i == 0 || !matches!(kind_of(tokens, i - 1), Some(TokenKind::Period)))
            && (i == 0
                || !matches!(
                    kind_of(tokens, i - 1),
                    Some(TokenKind::At | TokenKind::StringInterpolationIdentifier)
                ))
            && !in_combinator_list(tokens, i)
        {
            uses.push(i);
        }
    }
    if uses.len() != 1 {
        return None;
    }
    let use_idx = uses[0];
    // The use's full span covers a trailing `<typeArgs>` when written.
    let mut use_end_idx = use_idx;
    if kind_of(tokens, use_idx + 1) == Some(TokenKind::Lt) {
        // `F<…>` — the angle group ends the NamedType.
        let gt = angle_group_end(tokens, use_idx + 1)?;
        use_end_idx = gt;
    }
    let mut replacement = String::new();
    if let Some(r) = ret {
        replacement.push_str(&r);
        replacement.push(' ');
    }
    replacement.push_str("Function");
    if let Some(t) = tparams {
        replacement.push_str(&t);
    }
    replacement.push('(');
    replacement.push_str(&params);
    replacement.push(')');
    // The two sites: declaration span and use span — one contiguous
    // replacement over `[min, max)` splicing both (the declaration's whole
    // line block drops; the use becomes the Function type).
    let decl_start_byte = {
        // line start of the `typedef` keyword
        let kw_byte = tokens.get(typedef_kw)?.byte_offset as usize;
        text[..kw_byte].rfind('\n').map_or(0, |i| i + 1)
    };
    let decl_end_byte = {
        let semi_end = token_end(tokens.get(decl_end)?);
        text[semi_end..]
            .find('\n')
            .map_or(text.len(), |i| semi_end + i + 1)
    };
    let use_start = tokens.get(use_idx)?.byte_offset as usize;
    let use_end = token_end(tokens.get(use_end_idx)?);
    if decl_start_byte < use_start {
        // `[decl_line_start, use_end)` → `"" + middle + replacement`
        let middle = text.get(decl_end_byte..use_start).unwrap_or("");
        let start16 = utf16_count(&text[..decl_start_byte]);
        let end16 = utf16_count(&text[..use_end]);
        return Some((
            start16,
            end16.checked_sub(start16)?,
            format!("{middle}{replacement}"),
        ));
    }
    if use_end < decl_start_byte {
        let middle = text.get(use_end..decl_start_byte).unwrap_or("");
        let start16 = utf16_count(&text[..use_start]);
        let decl_line_end16 = utf16_count(&text[..decl_end_byte]);
        return Some((
            start16,
            decl_line_end16.checked_sub(start16)?,
            format!("{replacement}{middle}"),
        ));
    }
    None
}

/// Whether the identifier at `i` sits in an `import`/`export` `show`/`hide`
/// combinator list — the walk back over `Identifier`/`Comma` lands on the
/// combinator keyword (a `NamedType` never reads there). The keyword only
/// counts when it trails the directive head — a string literal (the URI)
/// or an `as`/`deferred` prefix — so `f(show, x)` argument lists never
/// match.
pub(crate) fn in_combinator_list(tokens: &TokenTable, mut i: usize) -> bool {
    loop {
        match i.checked_sub(1).and_then(|p| kind_of(tokens, p)) {
            Some(TokenKind::Identifier | TokenKind::Comma) => i -= 1,
            Some(TokenKind::Keyword(Keyword::Show | Keyword::Hide)) => {
                return i
                    .checked_sub(2)
                    .and_then(|p| kind_of(tokens, p))
                    .is_some_and(|kind| {
                        matches!(
                            kind,
                            TokenKind::String
                                | TokenKind::Identifier
                                | TokenKind::Keyword(Keyword::As | Keyword::Deferred)
                        )
                    });
            }
            _ => return false,
        }
    }
}

/// The typedef declaration's `R Function(params)` pieces over
/// `[typedef_kw, decl_end)`: (return type text, type-parameter list text,
/// parameter list text) — the two syntax faces (`typedef R F(params);`
/// and `typedef F = R Function(params);`).
pub(crate) fn typedef_signature(
    tokens: &TokenTable,
    text: &str,
    typedef_kw: usize,
    name_idx: usize,
    decl_end: usize,
) -> Option<(Option<String>, Option<String>, String)> {
    // GenericTypeAlias face: `typedef F = R Function(params);` — the `=`
    // after the name leads the aliased type.
    if let Some(eq) = depth0_find(tokens, text, name_idx, decl_end, |kind, _| {
        kind == TokenKind::Eq
    }) {
        // A `typedef F<T> = …` face is refused upstream (typeParameters on
        // the typedef itself).
        if kind_of(tokens, name_idx + 1) == Some(TokenKind::Lt) {
            return None;
        }
        // `R Function(params)` — find `Function` and its `(`.
        let mut i = eq + 1;
        let mut ret = None;
        if !word_at(tokens, text, i, "Function") {
            // collect the return type up to `Function`.
            let mut j = i;
            while j < decl_end && !word_at(tokens, text, j, "Function") {
                j += 1;
            }
            ret = Some(token_text(tokens, text, i..j)?);
            i = j;
        }
        if !word_at(tokens, text, i, "Function") {
            return None;
        }
        let mut k = i + 1;
        // `<T>` after `Function`.
        let mut tparams = None;
        if kind_of(tokens, k) == Some(TokenKind::Lt) {
            let gt = angle_group_end(tokens, k)?;
            tparams = Some(token_text(tokens, text, k..gt + 1)?);
            k = gt + 1;
        }
        if kind_of(tokens, k) != Some(TokenKind::OpenParen) {
            return None;
        }
        let cp = tokens.matching_end(k)?;
        let params = params_text(tokens, text, k + 1..cp)?;
        return Some((ret, tparams, params));
    }
    // FunctionTypeAlias face: `typedef R? F(params);` — return type is
    // `[typedef_kw+1, name_idx)`, params the group after the name.
    let ret = {
        let t = token_text(tokens, text, typedef_kw + 1..name_idx)?
            .trim()
            .to_owned();
        (!t.is_empty()).then_some(t)
    };
    // `<T>` on the typedef name itself.
    let mut k = name_idx + 1;
    let mut tparams = None;
    if kind_of(tokens, k) == Some(TokenKind::Lt) {
        let gt = angle_group_end(tokens, k)?;
        tparams = Some(token_text(tokens, text, k..gt + 1)?);
        k = gt + 1;
    }
    if kind_of(tokens, k) != Some(TokenKind::OpenParen) {
        return None;
    }
    let cp = tokens.matching_end(k)?;
    let params = params_text(tokens, text, k + 1..cp)?;
    Some((ret, tparams, params))
}

/// The parameter-list interior text — verbatim except a trailing comma,
/// which the official parameter-by-parameter rebuild drops.
pub(crate) fn params_text(tokens: &TokenTable, text: &str, range: core::ops::Range<usize>) -> Option<String> {
    let mut end = range.end;
    if end > range.start && kind_of(tokens, end - 1)? == TokenKind::Comma {
        end -= 1;
    }
    token_text(tokens, text, range.start..end).map(|s| s.trim().to_owned())
}

// ---------------------------------------------------------------------------
// Multi-edit producers — the M-band disjoint-edit face
//
// The producers below answer [`crate::fix_table::FixAction::MultiRewrite`]:
// several disjoint same-file spans, verified together and offered whole or
// not at all. The applier's snapshot pass keeps a change coherent — every
// span locates against the buffer as it stood at birth — so the producers
// never order their edits against each other's writes.
// ---------------------------------------------------------------------------

/// `const_instance_field`: `static ` lands at the member's head — the
/// official `AddStatic` insertion at
/// `declaration.firstTokenAfterCommentAndMetadata.offset` (the member's
/// own first token, annotations and doc comments left above). The report
/// anchors the `const` keyword itself; the member head is the contiguous
/// modifier run's first token — `const`/`late`/`var`/`final`/`covariant`
/// — walking back while such a modifier stands.
pub(crate) fn add_static_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let mut head = token_starting_at(tokens, input.range.start)?;
    while let Some(prev) = head.checked_sub(1) {
        match kind_of(tokens, prev)? {
            TokenKind::Keyword(
                Keyword::Const | Keyword::Late | Keyword::Var | Keyword::Final | Keyword::Covariant,
            ) => head = prev,
            _ => break,
        }
    }
    Some(vec![(tokens.get(head)?.utf16_offset, 0, "static ".to_owned())])
}

/// The head of the variable declaration list owning the reported name —
/// the insertion-target walk the `AddLate`/`RemoveConst` faces share:
/// from the hit's first token back over the list's modifiers and written
/// type (the official `VariableDeclarationList`/`keyword`/`type` read).
/// Returns `(list_head, type_head, keyword)` — the declaration's first
/// token, the written type's first token when the list is typed, and the
/// `var`/`final`/`const`/`late` keyword's index when one heads it. The
/// walk stops at the declaration's boundaries — `,`, `;`, `=`, the
/// statement openers — so a `for`-part or parameter list never swallows
/// the report's own declarator.
pub(crate) fn variable_list_head(
    tokens: &TokenTable,
    text: &str,
    name_index: usize,
) -> Option<(usize, Option<usize>, Option<usize>)> {
    let _ = text;
    let head = name_index;
    // The list head: modifiers and the written type sit before the
    // declarator, in that order — keep walking while the predecessor is a
    // declaration keyword or a type-shaped token run (identifier with its
    // `.`/`<…>`/`?` tail), remembering where each run started.
    let mut list_head = head;
    let mut keyword = None;
    let mut type_run_start = None;
    loop {
        let Some(prev) = list_head.checked_sub(1) else {
            break;
        };
        match kind_of(tokens, prev)? {
            TokenKind::Keyword(
                Keyword::Var | Keyword::Final | Keyword::Const | Keyword::Late,
            ) => {
                keyword = Some(prev);
                list_head = prev;
            }
            TokenKind::Identifier
            | TokenKind::Keyword(Keyword::Void | Keyword::Dynamic)
            | TokenKind::Question
            | TokenKind::Period => {
                type_run_start = Some(prev);
                list_head = prev;
            }
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                let open = angle_group_start(tokens, prev)?;
                type_run_start = Some(open);
                list_head = open;
            }
            _ => break,
        }
    }
    // A bare `a.b`-shaped walk can land on a statement keyword's operand —
    // `return x`, `case y` — or a `;`-adjacent name that is no type at
    // all. The declaration context never has one of those tokens
    // immediately before its head.
    if let Some(start) = type_run_start
        && keyword.is_none()
        && let Some(prev) = start.checked_sub(1)
        && matches!(
            kind_of(tokens, prev),
            Some(TokenKind::Keyword(
                Keyword::Return
                    | Keyword::Case
                    | Keyword::In
                    | Keyword::Is
                    | Keyword::As
                    | Keyword::Throw
                    | Keyword::Extends
                    | Keyword::Implements
                    | Keyword::With
                    | Keyword::On
                    | Keyword::Import
                    | Keyword::Export
            )) | Some(TokenKind::Semicolon | TokenKind::Comma | TokenKind::Eq)
        )
    {
        type_run_start = None;
        list_head = head;
    }
    Some((list_head, type_run_start, keyword))
}

/// `final_not_initialized` / `not_initialized_non_nullable_instance_field`
/// / `not_initialized_non_nullable_variable`: `late ` lands at the
/// declaration head — the official `AddLate`'s `VariableDeclarationList`
/// face: before the list's keyword (`final int x` → `late final int x`),
/// else before the written type (`int x` → `late int x`), else before the
/// first declarator (`x = …` → `late x = …`), and never at a `const`
/// head. The `MakeVariableNullable` leg — `int x` → `int? x` — is the
/// second official arm; one fix serves one rewrite, so the row carries
/// the `AddLate` arm and names the nullable arm absent. The report
/// anchors the field's or variable's name.
pub(crate) fn add_late_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    let (head, type_head, keyword) = variable_list_head(tokens, input.text, first)?;
    // The `const` head refuses — `late const` is no legal declaration.
    if keyword.is_some_and(|k| kind_of(tokens, k) == Some(TokenKind::Keyword(Keyword::Const))) {
        return None;
    }
    // Already `late` is the shape the diagnostic cannot fire on.
    if keyword.is_some_and(|k| kind_of(tokens, k) == Some(TokenKind::Keyword(Keyword::Late))) {
        return None;
    }
    let at = keyword.or(type_head).unwrap_or(head);
    Some(vec![(tokens.get(at)?.utf16_offset, 0, "late ".to_owned())])
}

/// `non_bool_condition`: ` != null` lands at the reported expression's
/// end (the official `AddNeNull` insertion at `problemMessage.offset +
/// length`; the `AddAwait.nonBool` arm's resolved-type gate reads the
/// expression's static type and stays absent). The report anchors the
/// whole condition expression.
pub(crate) fn append_ne_null_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    Some(vec![(input.range.end, 0, " != null".to_owned())])
}

/// `extension_type_declares_member_of_object`: the member's lines go —
/// the official `RemoveMethodDeclaration` `getLinesRange` deletion over
/// the member (doc comments and annotations ride, the
/// [`member_block_start`] walk). The report anchors the member's name.
pub(crate) fn drop_object_member_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let (offset, length, replacement) = remove_override_member(input)?;
    Some(vec![(offset, length, replacement)])
}

/// `const_initialized_with_non_constant_value`: the `const` drops — the
/// official `RemoveConst`/`RemoveNew` face over the report's owning
/// declaration. A `const`-headed variable list rewrites `const` → `final`
/// (upstream's `VariableDeclarationList` arm — the declarator's value is
/// no longer const, but the declaration stays assign-once); a `const` or
/// `new` keyword heading the reported creation drops with its trivia run
/// (`startStart(keyword, next)`, comments included). The push-`const`-
/// down legs (the `_PushConstVisitor` rewrite over the sibling diagnostic
/// set) read the file's diagnostic inventory the row cannot see — absent.
/// The report anchors the non-constant value expression.
pub(crate) fn drop_head_const_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The owning list's head: the report sits on the value expression,
    // so the `=` boundary inside `variable_list_head` stops at the name.
    // Walk the declarator first — the name is the last identifier before
    // the `=` (a `const C()` creation's `const` is the value's own head,
    // handled below).
    let declarator = {
        let mut cursor = first;
        loop {
            let Some(prev) = cursor.checked_sub(1) else {
                return None;
            };
            match kind_of(tokens, prev)? {
                TokenKind::Identifier | TokenKind::Keyword(_) => cursor = prev,
                TokenKind::Period | TokenKind::Question => cursor = prev,
                // The `=` before the value — the declarator is the name
                // immediately before it.
                TokenKind::Eq => break prev.checked_sub(1)?,
                TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                    cursor = angle_group_start(tokens, prev)?;
                }
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    cursor = matching_begin(tokens, prev)?;
                }
                _ => break first,
            }
        }
    };
    let (head, type_head, keyword) = variable_list_head(tokens, text, declarator)?;
    if let Some(kw) = keyword {
        match kind_of(tokens, kw)? {
            // `const x = v` → `final x = v` (the upstream list arm).
            TokenKind::Keyword(Keyword::Const) => {
                let start = tokens.get(kw)?.utf16_offset;
                let end = utf16_end(tokens, text, kw)?;
                return Some(vec![(start, end - start, "final".to_owned())]);
            }
            // `var`/`final`-headed lists never carry the const the
            // diagnostic reports — the keyword sits on the creation
            // itself; fall through to the `new`/`const` hunt below.
            _ => {}
        }
    }
    // A `const`/`new` heading the reported creation: `const C(...)`,
    // `const <T>[...]`, `new C(...)`. The report anchors the whole
    // creation — the keyword is its first token, or stands one modifier
    // back when the report opens inside the creation (`const <T>[` — the
    // report's head can be the `<`).
    let _ = (head, type_head);
    let expr_head = match kind_of(tokens, first)? {
        TokenKind::Keyword(Keyword::Const | Keyword::New) => first,
        _ => {
            let mut head = first;
            loop {
                match kind_of(tokens, head.checked_sub(1)?)? {
                    TokenKind::Identifier
                    | TokenKind::Period
                    | TokenKind::Question
                    | TokenKind::Keyword(Keyword::Void | Keyword::Dynamic) => head -= 1,
                    TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                        head = angle_group_start(tokens, head - 1)?;
                    }
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        head = matching_begin(tokens, head - 1)?;
                    }
                    _ => break,
                }
            }
            head
        }
    };
    match kind_of(tokens, expr_head)? {
        TokenKind::Keyword(Keyword::Const | Keyword::New) => {
            // `startStart(keyword, keyword.next!)` — the trivia run
            // between two real tokens rides (comments included).
            let start = tokens.get(expr_head)?.utf16_offset;
            let end = tokens.get(expr_head + 1)?.utf16_offset;
            Some(vec![(start, end.checked_sub(start)?, String::new())])
        }
        _ => {
            let prev = expr_head.checked_sub(1)?;
            match kind_of(tokens, prev)? {
                TokenKind::Keyword(Keyword::Const | Keyword::New) => {
                    let start = tokens.get(prev)?.utf16_offset;
                    let end = tokens.get(expr_head)?.utf16_offset;
                    Some(vec![(start, end.checked_sub(start)?, String::new())])
                }
                _ => None,
            }
        }
    }
}

/// `invalid_constant`: upstream `RemoveConst` — the reported expression's
/// `constantContext` resolves to a `const`/`new`-headed literal or
/// creation whose keyword drops through its trivia run
/// (`startStart(constKeyword, constKeyword.next!)`), or to a
/// `const`-headed `VariableDeclarationList` whose keyword becomes
/// `final`. The token-stream provable arms ride the two existing
/// rewriters; an Annotation or const-pattern context carries no const
/// token upstream's `constantContext` would resolve — both rewriters
/// answer `None` there and the row stays silent rather than fabricate.
/// Upstream's `ExpressionImpl → constantContext` dispatch and the
/// `SourceRange(offset, length + 1)` CompilationUnit arm read resolved
/// nodes the fix pass does not carry and stay absent.
pub(crate) fn invalid_const_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    // The declaration-list arm first — a `const`/`final` keyword heading
    // the owning list wins over any `const` the value expression itself
    // carries (the list arm is upstream's `VariableDeclarationList`
    // dispatch; the value's own keyword is the literal/creation arm).
    if let Some(edits) = drop_head_const_entry(input) {
        return Some(edits);
    }
    drop_enclosing_const(input).map(|edit| vec![edit])
}

/// `constant_pattern_with_non_constant_expression`: `const ` lands before
/// the reported operand — the official `AddConst` `ConstantPattern` arm.
/// A const-eligible single operand (literal, identifier, named reference,
/// an already-parenthesized expression's contents) takes the bare `const `
/// insertion; any other expression shape wraps whole — `const (` before
/// and `)` after (upstream `addParensAndConst`). The `canBeConst` gate
/// upstream reads is the operand's own shape; the producer's type-literal
/// parens arm needs the resolved-type answer and stays absent.
pub(crate) fn add_const_pattern_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    // A single-atom operand: a literal, identifier, `null`/`true`/`false`,
    // a named type (`a.B`), a qualified reference — takes `const `.
    let atom = if first == last {
        matches!(
            kind_of(tokens, first)?,
            TokenKind::Identifier
                | TokenKind::Int
                | TokenKind::Double
                | TokenKind::Hexadecimal
                | TokenKind::String
                | TokenKind::Keyword(
                    Keyword::Null
                        | Keyword::True
                        | Keyword::False
                        | Keyword::This
                        | Keyword::Super
                )
        )
    } else {
        // `a.b` / `a<T>` / `a.b<T>`-shaped reference runs stay atomic;
        // anything with an operator at depth 0 wraps.
        let mut atomic = true;
        let mut cursor = first;
        while cursor <= last {
            match kind_of(tokens, cursor)? {
                TokenKind::Identifier
                | TokenKind::Period
                | TokenKind::Question
                | TokenKind::Keyword(Keyword::Void | Keyword::Dynamic) => cursor += 1,
                TokenKind::Lt => cursor = angle_group_end(tokens, cursor)? + 1,
                _ => {
                    atomic = false;
                    break;
                }
            }
        }
        atomic
    };
    if atom {
        return Some(vec![(input.range.start, 0, "const ".to_owned())]);
    }
    Some(vec![
        (input.range.start, 0, "const (".to_owned()),
        (input.range.end, 0, ")".to_owned()),
    ])
}

/// `undefined_identifier_await`: `async ` lands at the enclosing
/// function's body — the official `AddAsync` default face
/// (`convertFunctionFromSyncToAsync`'s `body.offset` insertion; the
/// `await` lexeme is already written — the diagnostic is it parsing as
/// an identifier, so the fix carries no `await` insertion). The
/// return-type rewrite (`Future<…>`-wrapping) reads the resolved type
/// and stays absent; an already-`async`/`sync*`/`async*` body refuses
/// (the producer's `body.keyword == null` gate). The report anchors the
/// `await` identifier miss.
pub(crate) fn add_async_await_entry(input: &FixInput<'_>) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The enclosing function body: the nearest `{` or `=>` opener the
    // backward walk meets at a boundary — `(` groups skip whole so a
    // nested call's argument list never reads as the body.
    let mut cursor = first;
    let body_head = loop {
        cursor = cursor.checked_sub(1)?;
        match kind_of(tokens, cursor)? {
            TokenKind::OpenCurly => break cursor,
            TokenKind::Function => break cursor,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                cursor = matching_begin(tokens, cursor)?;
            }
            // A statement boundary behind the body opener ends the hunt —
            // the report's expression has no enclosing function body.
            TokenKind::Semicolon | TokenKind::Eof => return None,
            _ => {}
        }
    };
    // The body's own modifiers sit immediately before its opener
    // (`async`/`sync*`); the `*` of the generator marks belongs to the
    // keyword's token in this stream (`async*` is `async` + `*`). An
    // already-marked body refuses.
    if kind_of(tokens, body_head.checked_sub(1)?) == Some(TokenKind::Star)
        || matches!(
            kind_of(tokens, body_head.checked_sub(1)?),
            Some(TokenKind::Keyword(Keyword::Async | Keyword::Sync))
        )
    {
        return None;
    }
    // `addInsertion(body.offset, …)` — a ` ` lead when the body opener
    // fuses with the previous token (`f()=>` writes ` async `).
    let at = tokens.get(body_head)?.utf16_offset;
    let fused =
        body_head > 0 && utf16_end(tokens, input.text, body_head - 1)? == at;
    let text = if fused { " async " } else { "async " };
    Some(vec![(at, 0, text.to_owned())])
}

// ---------------------------------------------------------------------------
// The lint face's disjoint producers (the M-band constructor and directive
// rows)
// ---------------------------------------------------------------------------

/// One formal parameter's token span inside a `(`…`)` list — the
/// `(start, end)` run between the group's depth-0 commas (the default
/// value's `=`/`:` tail rides the run). `list` is the `(` index.
pub(crate) fn formal_spans(tokens: &TokenTable, list: usize) -> Option<Vec<(usize, usize)>> {
    let close = tokens.matching_end(list)?;
    let mut spans = Vec::new();
    let mut start = list + 1;
    let mut cursor = list + 1;
    while cursor < close {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                cursor = tokens.matching_end(cursor)?;
            }
            TokenKind::Lt => {
                cursor = angle_group_end(tokens, cursor)?;
            }
            TokenKind::Comma => {
                spans.push((start, cursor));
                start = cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    if start < close {
        spans.push((start, close));
    }
    Some(spans)
}

/// The parameter name's token index inside one formal span — the span's
/// last identifier before an optional `:`/`=` default tail (the named
/// parameter's `required`, type, and `covariant` lead it; the `this.` /
/// `super.` receiver's own name side is still the parameter name).
pub(crate) fn formal_name_token(tokens: &TokenTable, text: &str, span: (usize, usize)) -> Option<usize> {
    let mut end = span.1;
    // The default tail (`= expr` / `: expr`) is not part of the
    // declaration shape — walk it off at depth 0 of the span.
    let mut cursor = span.0;
    let mut depth = 0u32;
    while cursor < end {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Eq | TokenKind::Colon if depth == 0 => {
                end = cursor;
                break;
            }
            _ => {}
        }
        cursor += 1;
    }
    let mut name = None;
    let mut cursor = span.0;
    while cursor < end {
        if matches!(
            kind_of(tokens, cursor),
            Some(TokenKind::Identifier | TokenKind::Keyword(_))
        ) && !matches!(
            kind_of(tokens, cursor),
            Some(TokenKind::Keyword(
                Keyword::Required
                    | Keyword::Covariant
                    | Keyword::Final
                    | Keyword::Var
                    | Keyword::This
                    | Keyword::Super
                    | Keyword::Void
                    | Keyword::Dynamic
            ))
        ) {
            name = Some(cursor);
        }
        cursor += 1;
    }
    let _ = text;
    name
}

/// The `(` list of the constructor owning the reported initializer or
/// body assignment — walking back from the hit over the `:`-list entries
/// or the body's own text to the `)` closing the parameter list, then its
/// `(`.
pub(crate) fn constructor_params_list(tokens: &TokenTable, at: usize) -> Option<usize> {
    let mut cursor = at;
    loop {
        // A candidate `(`: constructor parameters are followed by `:`,
        // `{`, `=>`, or `;`.
        if kind_of(tokens, cursor)? == TokenKind::OpenParen {
            let close = tokens.matching_end(cursor)?;
            match kind_of(tokens, close + 1)? {
                TokenKind::Colon
                | TokenKind::OpenCurly
                | TokenKind::Function
                | TokenKind::Semicolon => return Some(cursor),
                _ => {}
            }
        }
        match kind_of(tokens, cursor)? {
            TokenKind::CloseParen | TokenKind::CloseCurly | TokenKind::CloseSquare => {
                cursor = matching_begin(tokens, cursor)?;
            }
            TokenKind::Semicolon | TokenKind::Eof => return None,
            _ => cursor = cursor.checked_sub(1)?,
        }
    }
}

/// The enclosing class body's field member with the declarator `name` —
/// `(member, declarator_name_token)`: the `;`-terminated member whose
/// depth-0 declarator list contains `name` (a `T x = e, y` group reports
/// each declarator). The `name` token must be a declarator head —
/// followed at its own level by `=`, `,`, or `;`.
pub(crate) fn field_member_named<'m>(
    tokens: &TokenTable,
    text: &str,
    members: &'m [(usize, usize)],
    name: &str,
) -> Option<((usize, usize), usize)> {
    for &member in members {
        let (start, end) = member;
        // A `this : …` primary-body row or a constructor (name followed
        // by `(`) is no field declaration — the declarator hunt below
        // would read the `:`-entry names as fields.
        if matches!(
            kind_of(tokens, start),
            Some(TokenKind::Keyword(Keyword::This | Keyword::Super))
        ) {
            continue;
        }
        let mut cursor = start;
        while cursor < end {
            match kind_of(tokens, cursor)? {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    cursor = tokens.matching_end(cursor)?;
                }
                TokenKind::Colon => {
                    // A `:` before any match marks the row a
                    // constructor's initializer list — skip it.
                    break;
                }
                TokenKind::Lt => {
                    cursor = angle_group_end(tokens, cursor)?;
                }
                TokenKind::Identifier | TokenKind::Keyword(_)
                    if tokens.lexeme(text, cursor) == name =>
                {
                    match kind_of(tokens, cursor + 1)? {
                        TokenKind::Eq | TokenKind::Comma | TokenKind::Semicolon => {
                            return Some((member, cursor));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
    }
    None
}

/// The `nodeInList` deletion range for entry `index` inside a
/// comma-separated list — the official `range.nodeInList` shape the
/// initializer and parameter removals share: a non-last entry cuts
/// `startStart(entry, next)` (the next entry's own start bounds the cut,
/// comma and gap included); the last entry cuts `endEnd(prev, entry)`
/// (the previous entry's end through this entry's end).
pub(crate) fn node_in_list_cut(
    tokens: &TokenTable,
    text: &str,
    list: &[(usize, usize)],
    index: usize,
) -> Option<(u32, u32, String)> {
    let (start, end) = if index + 1 < list.len() {
        (
            tokens.get(list[index].0)?.utf16_offset,
            tokens.get(list[index + 1].0)?.utf16_offset,
        )
    } else {
        let prev_end = utf16_end(tokens, text, list[index].0.checked_sub(1)?)?;
        let this_end = utf16_end(tokens, text, list[index].1.checked_sub(1)?)?;
        (prev_end, this_end)
    };
    Some((start, end.checked_sub(start)?, String::new()))
}

/// The `(` list's own entries for `node_in_list_cut` — a
/// [`formal_spans`-style] split over an arbitrary `(start, close)` token
/// range split at its depth-0 commas (the `:`-initializer list shape).
pub(crate) fn comma_entries(
    tokens: &TokenTable,
    from: usize,
    to: usize,
) -> Option<Vec<(usize, usize)>> {
    let mut entries = Vec::new();
    let mut start = from;
    let mut cursor = from;
    while cursor < to {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                cursor = tokens.matching_end(cursor)?;
            }
            TokenKind::Lt => {
                cursor = angle_group_end(tokens, cursor)?;
            }
            TokenKind::Comma => {
                entries.push((start, cursor));
                start = cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    if start < to {
        entries.push((start, to));
    }
    Some(entries)
}

/// `initialize_in_field_declaration`: the `: x = expr` initializer moves
/// to the field's declarator — the official
/// `MoveInitializationToFieldDeclaration` pair: the initializer leaves
/// its list (`nodeInList`) and ` = expr` lands at the field declaration's
/// end (the field must declare `x` without an initializer). The report
/// anchors the initializer's field name.
pub(crate) fn move_init_to_field_entry(
    input: &FixInput<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    // The `:` heading the initializer list — the report's own name sits
    // inside the list: walk back over the entry tokens to it.
    let colon = {
        let mut cursor = first;
        loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::Colon => break cursor,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    cursor = matching_begin(tokens, cursor)?;
                }
                TokenKind::Semicolon
                | TokenKind::OpenCurly
                | TokenKind::Eof => return None,
                _ => {}
            }
        }
    };
    // The `=` after the reported name, then the expression through the
    // next `,` or the list's end (the body's `{`, `=>`, or `;`).
    let eq = first + 1;
    if kind_of(tokens, eq)? != TokenKind::Eq {
        return None;
    }
    let (body_open, body_close) = enclosing_class_body(tokens, first)?;
    // The reported entry's expression runs to the next `,` or the list's
    // end; the initializer list itself runs to the constructor body's
    // `{`, `=>`, or `;`.
    let mut expr_end = eq + 1;
    let mut depth = 0u32;
    let entry_end = loop {
        match kind_of(tokens, expr_end)? {
            TokenKind::OpenCurly | TokenKind::Function | TokenKind::Semicolon | TokenKind::Eof
            | TokenKind::Comma
                if depth == 0 =>
            {
                break expr_end;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        expr_end += 1;
    };
    let mut list_end = entry_end;
    let mut depth = 0u32;
    loop {
        match kind_of(tokens, list_end)? {
            TokenKind::OpenCurly | TokenKind::Function | TokenKind::Semicolon | TokenKind::Eof
                if depth == 0 =>
            {
                break;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            _ => {}
        }
        list_end += 1;
    }
    // The initializer list runs `colon + 1 .. list_end`, comma-split.
    // A single entry in a `this : …;` primary body cuts
    // `endStart(owner.thisKeyword, owner.body)` — the ` : y = x` run goes
    // whole, leaving `this;` (upstream `nodeInList`'s
    // `PrimaryConstructorBody` arm).
    let entries = comma_entries(tokens, colon + 1, list_end)?;
    let entry = entries.iter().position(|&(s, e)| first >= s && first < e)?;
    let delete = if entries.len() == 1 {
        // The `this` keyword heading the primary body — the token before
        // the `:` that is not `)`/`;`/`{` (the parameter list's close or
        // a previous member's terminator).
        let mut this_tok = colon;
        loop {
            this_tok = this_tok.checked_sub(1)?;
            match kind_of(tokens, this_tok)? {
                TokenKind::Keyword(Keyword::This) => break,
                _ => return None,
            }
        }
        let start = utf16_end(tokens, text, this_tok)?;
        let stop = tokens.get(list_end)?.utf16_offset;
        (start, stop.checked_sub(start)?, String::new())
    } else {
        node_in_list_cut(tokens, text, &entries, entry)?
    };
    // The field: a same-named, initializer-free declarator in the class
    // body (the official's `fieldDeclaration.equals == null &&
    // initializer == null` gate).
    let name = tokens.lexeme(text, first);
    let members = class_members(tokens, body_open + 1, body_close);
    let (_, field_tok) = field_member_named(tokens, text, &members, name)?;
    if kind_of(tokens, field_tok + 1)? == TokenKind::Eq {
        return None;
    }
    let insert_at = utf16_end(tokens, text, field_tok)?;
    let expr_text = slice16(
        text,
        &(tokens.get(eq + 1)?.utf16_offset..utf16_end(tokens, text, expr_end - 1)?),
    )?;
    Some(vec![
        delete,
        (insert_at, 0, format!(" = {}", expr_text.trim())),
    ])
}

/// `prefer_initializing_formals`: the reported `x = p` initializer (or
/// `this.x = p;` body assignment) folds into `this.x` on the parameter —
/// the official `ConvertToInitializingFormal` pair: `this.` lands at the
/// parameter's name (the parameter's written type drops when it equals
/// the field's — the lint's own resolved-subtype gate is the shape of
/// record), and the initializer entry or assignment statement goes.
/// The report anchors the assignment: the lint covers the initializer
/// list entry and the `this.x = p;` body statement faces; the named-
/// parameter private-field rename (`_x`), doc-comment reference, and
/// extra-reference legs are feature- or element-gated and absent.
pub(crate) fn convert_to_init_formal_entry(
    input: &FixInput<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    // The `=` inside the report: the field name precedes it (a `this.x`
    // prefix rides the body-assignment face), the parameter's identifier
    // follows it.
    let eq = depth0_find(tokens, text, first, last + 1, |kind, _| kind == TokenKind::Eq)?;
    let field_tok = {
        let mut cursor = eq;
        loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::Identifier | TokenKind::Keyword(_) => break cursor,
                TokenKind::Period => {}
                _ => return None,
            }
        }
    };
    let field_name = tokens.lexeme(text, field_tok);
    // The parameter: the last identifier of the report — the RHS is the
    // bare parameter by the lint's own gate.
    let param_name = tokens.lexeme(text, last);
    let list = constructor_params_list(tokens, first)?;
    let spans = formal_spans(tokens, list)?;
    let param = spans.iter().copied().find(|&span| {
        formal_name_token(tokens, text, span)
            .is_some_and(|name| tokens.lexeme(text, name) == param_name)
    })?;
    let param_name_tok = formal_name_token(tokens, text, param)?;
    // The parameter's written type: the run before the name after any
    // `required`/`covariant` lead (a `this.`-formal or untyped parameter
    // has none — the lint refuses both). The type spans `param.0 .. name`
    // minus the lead keywords.
    let mut type_start = param.0;
    while type_start < param_name_tok
        && matches!(
            kind_of(tokens, type_start),
            Some(TokenKind::Keyword(
                Keyword::Required | Keyword::Covariant | Keyword::Final | Keyword::Var
            ))
        )
    {
        type_start += 1;
    }
    // The field's written type for the equality check — the declarator's
    // own member, the type run before the declarator name.
    let (body_open, body_close) = enclosing_class_body(tokens, first)?;
    let members = class_members(tokens, body_open + 1, body_close);
    let (field_member, field_name_tok) = field_member_named(tokens, text, &members, field_name)?;
    let mut field_type_start = field_member.0;
    while field_type_start < field_name_tok
        && matches!(
            kind_of(tokens, field_type_start),
            Some(TokenKind::Keyword(
                Keyword::Static
                    | Keyword::Late
                    | Keyword::Final
                    | Keyword::Var
                    | Keyword::Const
                    | Keyword::Covariant
            ))
        )
    {
        field_type_start += 1;
    }
    let mut edits = Vec::new();
    // The parameter edit, upstream's three writes composed: `this.`
    // inserted at the name (`addSimpleInsertion(name.offset, 'this.')`),
    // the name renamed to the field's when they differ
    // (`addSimpleReplacement(name, fieldName)` — a positional pair; the
    // named-parameter mismatch is upstream's refuse leg), and the
    // written type dropped when it reads identically to the field's
    // (`parameter.type == field.type`, the same-lexeme face — the
    // `range.deletionRange(type)` cut covers the type plus its trailing
    // trivia run).
    let replacement = format!("this.{field_name}");
    let param_type = (type_start < param_name_tok).then(|| (type_start, param_name_tok));
    let field_type =
        (field_type_start < field_name_tok).then(|| (field_type_start, field_name_tok));
    let same_type = match (param_type, field_type) {
        (Some((ps, pe)), Some((fs, fe))) => {
            let p_text = span_over(text, tokens, ps, pe - 1)?;
            let f_text = span_over(text, tokens, fs, fe - 1)?;
            let p_slice = slice16(text, &(p_text.0..p_text.0 + p_text.1))?;
            let f_slice = slice16(text, &(f_text.0..f_text.0 + f_text.1))?;
            p_slice.trim() == f_slice.trim()
        }
        _ => false,
    };
    if same_type
        && let Some((ps, _)) = param_type
    {
        // `int v` → `this.x` — the type through the name goes in one
        // span (the type deletion plus the `this.`-insert plus the
        // rename compose to the same replacement).
        let start = tokens.get(ps)?.utf16_offset;
        let end = utf16_end(tokens, text, param_name_tok)?;
        edits.push((start, end - start, replacement));
    } else {
        // `num v` → `num this.x` — the type stays; `this.` plus the
        // rename rewrite the name's own span.
        let start = tokens.get(param_name_tok)?.utf16_offset;
        let end = utf16_end(tokens, text, param_name_tok)?;
        edits.push((start, end - start, replacement));
    }
    // The initializer/assignment removal: an initializer-list entry cuts
    // `nodeInList`; a body assignment drops its own statement (`{ this.x
    // = p; }` single-statement body rewrites to `;` — the upstream
    // `statements.length == 1` face).
    let colon = {
        let mut cursor = first;
        let mut found = None;
        while let Some(prev) = cursor.checked_sub(1) {
            match kind_of(tokens, prev)? {
                TokenKind::Colon => {
                    found = Some(prev);
                    break;
                }
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    cursor = matching_begin(tokens, prev)?;
                }
                TokenKind::OpenCurly | TokenKind::Semicolon | TokenKind::Eof => break,
                _ => cursor = prev,
            }
        }
        found
    };
    if let Some(colon) = colon {
        // The entry list: `colon + 1 .. body` (`{`/`=>`/`;`).
        let mut end = colon + 1;
        let mut depth = 0u32;
        let list_end = loop {
            match kind_of(tokens, end)? {
                TokenKind::OpenCurly | TokenKind::Function | TokenKind::Semicolon
                    if depth == 0 =>
                {
                    break end;
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth = depth.checked_sub(1)?;
                }
                _ => {}
            }
            end += 1;
        };
        let entries = comma_entries(tokens, colon + 1, list_end)?;
        let index = entries.iter().position(|&(s, e)| first >= s && first < e)?;
        if entries.len() == 1 {
            // The only initializer: `endEnd(parameterList, initializer)`
            // — the ` : x = p` run goes whole.
            let start = utf16_end(tokens, text, tokens.matching_end(list)?)?;
            let stop = utf16_end(tokens, text, list_end - 1)?;
            edits.push((start, stop - start, String::new()));
        } else {
            edits.push(node_in_list_cut(tokens, text, &entries, index)?);
        }
        Some(edits)
    } else {
        // The body-statement face: `this.x = p;` inside the constructor's
        // `{ … }`. The statement's `;` is the next `;` at the report's own
        // depth — the first `;` after the value, `(`/`<` groups skipped
        // whole (a nested call's argument list never reads as the
        // terminator).
        let semi = {
            let mut cursor = last;
            loop {
                cursor += 1;
                match kind_of(tokens, cursor)? {
                    TokenKind::Semicolon => break cursor,
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        cursor = tokens.matching_end(cursor)?;
                    }
                    TokenKind::Lt => {
                        cursor = angle_group_end(tokens, cursor)?;
                    }
                    TokenKind::Eof | TokenKind::CloseCurly => return None,
                    _ => {}
                }
            }
        };
        // The enclosing body `{`: innermost unmatched opener before the
        // statement.
        let mut cursor = semi;
        let block = loop {
            cursor = cursor.checked_sub(1)?;
            match kind_of(tokens, cursor)? {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    cursor = matching_begin(tokens, cursor)?;
                }
                TokenKind::OpenCurly => break cursor,
                TokenKind::Eof => return None,
                _ => {}
            }
        };
        let block_close = tokens.matching_end(block)?;
        // The statement's head: the report's first token, pulled back
        // over a `this.` receiver prefix (the lint's span can drop it).
        let stmt_head = if first.checked_sub(2).is_some_and(|prev| {
            kind_of(tokens, prev + 1) == Some(TokenKind::Period)
                && kind_of(tokens, prev) == Some(TokenKind::Keyword(Keyword::This))
        }) {
            first - 2
        } else {
            first
        };
        // Lone-statement body: the statement is the block's whole content
        // — its head is the first token inside `{` and its `;` the last
        // before `}` — so `endEnd(parameterList, functionBody)` rewrites
        // the body to `;` (upstream `statements.length == 1`).
        if stmt_head == block + 1 && semi + 1 == block_close {
            let start = utf16_end(tokens, text, tokens.matching_end(list)?)?;
            let stop = utf16_end(tokens, text, block_close)?;
            edits.push((start, stop - start, ";".to_owned()));
        } else {
            // `nodeInList(statements, assignment)` — a non-last statement
            // cuts `startStart(stmt, next)` (the next statement's own
            // head bounds the cut, `;` and gap included); the last cuts
            // `endEnd(prev, stmt)`.
            if semi + 1 == block_close {
                // The previous statement's end — the `;` or `}`-closed
                // statement immediately before the head at the block's
                // own level (`stmt_head - 1` is exactly that token).
                let prev_end = utf16_end(tokens, text, stmt_head.checked_sub(1)?)?;
                let stop = utf16_end(tokens, text, semi)?;
                edits.push((prev_end, stop - prev_end, String::new()));
            } else {
                let start = tokens.get(stmt_head)?.utf16_offset;
                let stop = tokens.get(semi + 1)?.utf16_offset;
                edits.push((start, stop - start, String::new()));
            }
        }
        Some(edits)
    }
}

/// `use_declaring_parameters`: the reported primary-constructor parameter
/// becomes a declaring parameter and its field goes — the official
/// `ConvertToDeclaringParameter` faces. A `this.f` field formal replaces
/// its `this.` region with `final `/`var ` (the field's own type lands
/// when the parameter writes none); a plain parameter assigned by a
/// body-side `: f = p` initializer takes `final `/`var ` before its type
/// and the initializer goes with the field. `final` vs `var` reads the
/// field declaration's own keyword. The metadata-move, body-block
/// assignment, multi-declarator-field, and constructor-adjacent-field
/// legs are upstream's secondary shapes and stay absent (the
/// single-declarator field is the bounded face). The report anchors the
/// parameter's name.
pub(crate) fn convert_to_declaring_param_entry(
    input: &FixInput<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    let name = tokens.lexeme(text, first);
    // The class body: the parameter name sits in the primary
    // constructor's `(` list, BEFORE the `{` — walk forward to the
    // list's `)` then the body's opener (`class C(…)` / `class C(…);`
    // for a body-less primary, which the bounded face refuses anyway).
    let (body_open, body_close) = {
        let mut cursor = first;
        let list = loop {
            match kind_of(tokens, cursor)? {
                TokenKind::OpenParen => break cursor,
                TokenKind::Semicolon | TokenKind::Eof => return None,
                _ => cursor = cursor.checked_sub(1)?,
            }
        };
        let close = tokens.matching_end(list)?;
        match kind_of(tokens, close + 1)? {
            TokenKind::OpenCurly => (close + 1, tokens.matching_end(close + 1)?),
            _ => return None,
        }
    };
    let members = class_members(tokens, body_open + 1, body_close);
    // The field: same name, or the `_x`-under-`x` pair the lint's
    // `names_match` allows.
    let (field_member, field_name_tok) = field_member_named(tokens, text, &members, name)
        .or_else(|| {
            field_member_named(tokens, text, &members, &format!("_{name}"))
        })?;
    // The field declaration's `final`/`var`/absent keyword — the
    // storage keyword the declaring parameter spells (`var` for the
    // mutable faces).
    let is_final = {
        let (start, end) = field_member;
        (start..end).any(|i| kind_of(tokens, i) == Some(TokenKind::Keyword(Keyword::Final)))
    };
    let keyword = if is_final { "final" } else { "var" };
    // The field's written type — its declarator's prefix minus modifiers
    // — for the untyped-parameter carry.
    let field_type = {
        let mut cursor = field_member.0;
        while cursor < field_name_tok
            && matches!(
                kind_of(tokens, cursor),
                Some(TokenKind::Keyword(
                    Keyword::Static
                        | Keyword::Late
                        | Keyword::Final
                        | Keyword::Var
                        | Keyword::Const
                        | Keyword::Covariant
                ))
            )
        {
            cursor += 1;
        }
        (cursor < field_name_tok).then(|| (cursor, field_name_tok))
    };
    // The field's declarator is single — the upstream's own lines-range
    // face; a multi-declarator list is out of the bounded face.
    if kind_of(tokens, field_name_tok + 1)? == TokenKind::Comma {
        return None;
    }
    if (field_member.0..field_name_tok).any(|i| kind_of(tokens, i) == Some(TokenKind::Comma)) {
        return None;
    }
    let mut edits = Vec::new();
    // The parameter rewrite: a `this.` field formal replaces
    // `[this_start, name_start)` with the keyword plus the field's type
    // when the parameter writes none; a plain parameter inserts
    // `'$keyword '` before its type (or ` $keyword` after `required`),
    // the field's type following an untyped one.
    let this_tok = first.checked_sub(2).and_then(|t| {
        (kind_of(tokens, t) == Some(TokenKind::Keyword(Keyword::This))
            && kind_of(tokens, t + 1) == Some(TokenKind::Period))
        .then_some(t)
    });
    if let Some(this_tok) = this_tok {
        // `this.f` — the field-formal face. Anything before `this.` (a
        // written type) makes the `this.` a pure deletion; a bare
        // `this.f` replaces the prefix with the keyword, then the type
        // carry when untyped.
        let mut cursor = this_tok;
        let mut head = this_tok;
        while let Some(prev) = cursor.checked_sub(1) {
            match kind_of(tokens, prev)? {
                TokenKind::Identifier
                | TokenKind::Question
                | TokenKind::Period
                | TokenKind::Keyword(Keyword::Void | Keyword::Dynamic) => {
                    head = prev;
                    cursor = prev;
                }
                TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                    head = angle_group_start(tokens, prev)?;
                    cursor = head;
                }
                _ => break,
            }
        }
        let bare = head == this_tok;
        let start = tokens.get(this_tok)?.utf16_offset;
        let end = tokens.get(first)?.utf16_offset;
        if bare {
            let mut replacement = format!("{keyword} ");
            if let Some((fs, fe)) = field_type {
                let (o, l) = span_over(text, tokens, fs, fe - 1)?;
                replacement.push_str(slice16(text, &(o..o + l))?.trim());
                replacement.push(' ');
            }
            edits.push((start, end - start, replacement));
        } else {
            // `int this.f` — the `this.` goes; the keyword lands before
            // the written type (the `!insertedVariable` arm).
            edits.push((start, end - start, String::new()));
            edits.push((tokens.get(head)?.utf16_offset, 0, format!("{keyword} ")));
        }
    } else {
        // The plain-parameter face: `int f` → `final int f`, `f` →
        // `var f` + the field's type; `required` rides before the
        // keyword (upstream `' $keyword'` after it). The name changes to
        // the field's when the pair differs — upstream
        // `addSimpleReplacement(range.token(parameterName), fieldName)`.
        let field_name = tokens.lexeme(text, field_name_tok);
        if field_name != name {
            let start = tokens.get(first)?.utf16_offset;
            let end = utf16_end(tokens, text, first)?;
            edits.push((start, end - start, field_name.to_owned()));
        }
        let required_tok = first.checked_sub(1).filter(|&t| {
            kind_of(tokens, t) == Some(TokenKind::Keyword(Keyword::Required))
        });
        // The parameter's lead: back to its `(`/`,` boundary, then its
        // written type is what the modifier lead leaves before the name.
        let mut boundary = first;
        while let Some(prev) = boundary.checked_sub(1) {
            match kind_of(tokens, prev)? {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
                | TokenKind::Comma => break,
                _ => boundary = prev,
            }
        }
        let mut type_start = boundary;
        while type_start < first
            && matches!(
                kind_of(tokens, type_start),
                Some(TokenKind::Keyword(
                    Keyword::Required | Keyword::Covariant | Keyword::Final | Keyword::Var
                ))
            )
        {
            type_start += 1;
        }
        let has_type = type_start < first;
        // The keyword plus, for an untyped parameter, the field's own
        // written type — composed into one insertion (upstream writes
        // the two at the same offset; one edit keeps the composition
        // order explicit).
        let mut inserted = format!("{keyword} ");
        if !has_type
            && let Some((fs, fe)) = field_type
        {
            let (o, l) = span_over(text, tokens, fs, fe - 1)?;
            inserted.push_str(slice16(text, &(o..o + l))?.trim());
            inserted.push(' ');
        }
        if let Some(required) = required_tok {
            let trimmed = inserted.trim_end().to_owned();
            edits.push((utf16_end(tokens, text, required)?, 0, format!(" {trimmed}")));
        } else {
            edits.push((tokens.get(type_start)?.utf16_offset, 0, inserted));
        }
    }
    // The initializer removal for the non-declaring face — the primary
    // body's own `this` row: `this : f = p;` (empty body) deletes the
    // whole `this`-row lines when it is the only initializer;
    // `this { … }` cuts `startStart(colon, body.beginToken)`; a
    // multi-entry list cuts `nodeInList`. The `this` row's head is the
    // depth-0 `this`/`;` before the class-body close.
    let mut field_plus_ctor: Option<(u32, u32)> = None;
    if this_tok.is_none() {
        // The primary body: `this : …` / `this { … }` / `this;` — the
        // member row whose first token is `this`.
        let ctor = members
            .iter()
            .copied()
            .find(|&(s, _)| kind_of(tokens, s) == Some(TokenKind::Keyword(Keyword::This)));
        if let Some((ctor_start, ctor_end)) = ctor {
            // `:` heading the initializer list inside this member.
            let colon = (ctor_start..ctor_end).find(|&i| {
                kind_of(tokens, i) == Some(TokenKind::Colon)
            });
            // The body token after the initializer list: `{` (block) or
            // `;` (empty body).
            let body_tok = colon.and_then(|colon| {
                (colon + 1..ctor_end).find(|&i| {
                    matches!(
                        kind_of(tokens, i),
                        Some(TokenKind::OpenCurly | TokenKind::Semicolon)
                    )
                })
            });
            if let (Some(colon), Some(body_tok)) = (colon, body_tok) {
                let entries = comma_entries(tokens, colon + 1, body_tok)?;
                let single = entries.len() == 1;
                let empty_body = kind_of(tokens, body_tok) == Some(TokenKind::Semicolon);
                if single && empty_body {
                    // `_getLinesRangeWithNextBlankLine(constructorBody)` —
                    // the whole `this : _x = x;` row's lines. When the
                    // field's own `;` runs directly into `this` (the
                    // `fieldNode.endToken.next == this` arm), the two
                    // delete as one `startEnd(field, constructor)` span.
                    let ctor_start16 = tokens.get(ctor_start)?.utf16_offset;
                    let member_start = member_block_start(text, ctor_start16);
                    let ctor_end_byte = token_end(tokens.get(ctor_end - 1)?);
                    let line_end = text[ctor_end_byte..]
                        .find('\n')
                        .map_or(text.len(), |i| ctor_end_byte + i);
                    let (o16, l16) = cut_line(text, member_start, line_end);
                    // The merge: the field's `;` is the token right
                    // before `this` (nothing between the member rows).
                    let field_last = field_member.1 - 1;
                    if field_last + 1 == ctor_start
                        && kind_of(tokens, field_last) == Some(TokenKind::Semicolon)
                    {
                        // `startEnd(fieldNode, constructorNode)` lines —
                        // one span from the field's line head through
                        // the constructor row's line end (`getLinesRange`
                        // — no next-blank tail on this arm).
                        let field_head16 = tokens.get(field_member.0)?.utf16_offset;
                        let field_start = member_block_start(text, field_head16);
                        let (fo16, _) = cut_line(text, field_start, field_start);
                        field_plus_ctor = Some((fo16, (o16 + l16).checked_sub(fo16)?));
                    } else {
                        // `_getLinesRangeWithNextBlankLine` — the row's
                        // lines plus one trailing blank.
                        let blank = {
                            let end_byte = utf16_offset_to_byte(text, o16 + l16);
                            let rest = text.get(end_byte..).unwrap_or("");
                            rest.strip_prefix("\r\n\r\n")
                                .map(|_| 4usize)
                                .or_else(|| rest.strip_prefix("\n\n").map(|_| 2))
                                .unwrap_or(0)
                        };
                        edits.push((o16, l16 + u32::try_from(blank).ok()?, String::new()));
                    }
                } else if single {
                    // `startStart(colon, body.beginToken)` — the ` : _x = x`
                    // run goes whole, trivia to `{` included.
                    let start = tokens.get(colon)?.utf16_offset;
                    let end = tokens.get(body_tok)?.utf16_offset;
                    edits.push((start, end - start, String::new()));
                } else if let Some(index) = entries.iter().position(|&(s, e)| {
                    (s..e).any(|i| {
                        let lexeme = tokens.lexeme(text, i);
                        lexeme == name || lexeme == format!("_{name}").as_str()
                    })
                }) {
                    edits.push(node_in_list_cut(tokens, text, &entries, index)?);
                } else {
                    return None;
                }
            }
        }
    }
    // The field's lines go — the member's own line block plus the blank
    // line after it (`_getLinesRangeWithNextBlankLine`); the merge arm
    // already carries the pair as one span.
    if let Some((off16, len16)) = field_plus_ctor {
        edits.push((off16, len16, String::new()));
    } else {
        let member_start = member_block_start(text, tokens.get(field_member.0)?.utf16_offset);
        let member_end = token_end(tokens.get(field_member.1 - 1)?);
        let (off16, len16) = cut_line(text, member_start, {
            text[member_end..]
                .find('\n')
                .map_or(text.len(), |i| member_end + i)
        });
        let end_byte = utf16_offset_to_byte(text, off16 + len16);
        let rest = text.get(end_byte..).unwrap_or("");
        let blank = rest
            .strip_prefix("\r\n\r\n")
            .map(|_| 4usize)
            .or_else(|| rest.strip_prefix("\n\n").map(|_| 2))
            .unwrap_or(0);
        edits.push((off16, len16 + u32::try_from(blank).ok()?, String::new()));
    }
    Some(edits)
}

/// One top-level directive's piece for the organize pass: its sort
/// priority (the upstream `DirectiveSortPriority` ordinal), its URI text
/// for the alphabetical compare, the URI literal's UTF-16 offset (the
/// removal legs' match anchor), the shown names the file's
/// `unused_shown_name` rows flag (imports only), and the byte span of the
/// text that moves — leading comments through the trailing same-line
/// comment.
pub(crate) struct DirectivePiece {
    pub(crate) priority: u8,
    pub(crate) uri: String,
    uri16: u32,
    unused_shown: Vec<String>,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

/// The diagnostic codes the official organizer reads off
/// `DiagnosticCode.isUnresolvedIdentifier` (`diagnostic.g.dart` flags the
/// six; the `organize` removals stand down when the file carries any of
/// them — `ImportOrganizer.hasUnresolvedIdentifierError`).
pub(crate) const UNRESOLVED_IDENTIFIER_CODES: [&str; 6] = [
    "creation_with_non_type",
    "extends_non_class",
    "non_type_as_type_argument",
    "undefined_class",
    "undefined_function",
    "undefined_identifier",
];

/// The alphabetical directive-URI compare (the upstream
/// `compareDirectiveUri` — package URIs compare path-first).
pub(crate) fn compare_directive_uri(a: &str, b: &str) -> core::cmp::Ordering {
    if !(a.starts_with("package:") && b.starts_with("package:"))
        || (a.starts_with('/') && b.starts_with('/'))
    {
        let (ia, ib) = (a.find('/'), b.find('/'));
        if let (Some(ia), Some(ib)) = (ia, ib) {
            let head = a[..ia].cmp(&b[..ib]);
            if head != core::cmp::Ordering::Equal {
                return head;
            }
            return a[ia + 1..].cmp(&b[ib + 1..]);
        }
    }
    a.cmp(b)
}

/// `directives_ordering`: the whole directive block re-sorts — the
/// official `OrganizeImports` face over the file's own tokens. `dart:`
/// imports first, then `package:`, other schemes, and relative URIs —
/// alphabetical inside a group, a blank line between groups; exports
/// follow imports through the same four groups, `part` directives last.
/// Each directive's leading comments move with it (the pseudo-first
/// directive's doc/annotation lines and blank-separated comment groups
/// stay at the top, as upstream); a trailing `//` on the directive's own
/// line rides. The removal legs read the file's diagnostic set
/// (`input.diagnostics`, the official `ImportOrganizer`'s diagnostics
/// parameter): a directive whose URI carries an `unused_import`,
/// `duplicate_import`, or `unnecessary_import` row drops whole, an import
/// row whose text repeats the previous kept row's drops, and shown names
/// flagged `unused_shown_name` excise from their `show` clause — all
/// three standing down when the file carries an unresolved-identifier
/// error (`hasUnresolvedIdentifierError`). The library directive's doc
/// comment sorts its `@docImport` lines the same priority groups (the
/// `_organizeDocImports` legs, line and block comments both). A `library`
/// directive or non-directive content inside the block bounds the face
/// and drops the fix; a rebuild that changes nothing drops it too (the
/// official `organize()` returns no edits then).
pub(crate) fn organize_directives_entry(
    input: &FixInput<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let tokens = input.tokens?;
    let text = input.text;
    let line_starts = byte_line_starts(text);
    let eol = eol_of(text);
    // The removal legs' admission sets (empty under the unresolved gate —
    // the official `if (!hasUnresolvedIdentifierError)` wraps every
    // removal decision).
    let unresolved_gate = input
        .diagnostics
        .iter()
        .any(|d| UNRESOLVED_IDENTIFIER_CODES.contains(&d.code.name));
    let unused_uris: FxHashSet<u32> = if unresolved_gate {
        FxHashSet::default()
    } else {
        input
            .diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d.code.name,
                    "duplicate_import" | "unused_import" | "unnecessary_import"
                )
            })
            .map(|d| d.range.offset)
            .collect()
    };
    let unused_shown: FxHashSet<u32> = if unresolved_gate {
        FxHashSet::default()
    } else {
        input
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_shown_name")
            .map(|d| d.range.offset)
            .collect()
    };
    // The directive rows at the top level: `import`/`export`/`part`
    // keywords at depth 0 through their `;`. `library` is indexed but
    // never sorted (the last one owns the doc-import leg, as upstream's
    // `libraryDirective` variable).
    let mut library_kw = None::<usize>;
    let mut spans: Vec<(usize, usize, u8)> = Vec::new(); // (kw_index, semi_index, priority)
    let mut cursor = 0usize;
    let len = tokens.as_slice().len();
    while cursor < len {
        match kind_of(tokens, cursor)? {
            TokenKind::Keyword(Keyword::Library) => {
                library_kw = Some(cursor);
                let semi = depth0_find(tokens, text, cursor + 1, len, |kind, _| {
                    kind == TokenKind::Semicolon
                })?;
                cursor = semi + 1;
            }
            TokenKind::Keyword(Keyword::Import | Keyword::Export | Keyword::Part) => {
                let kw = cursor;
                let semi = depth0_find(tokens, text, cursor + 1, len, |kind, _| {
                    kind == TokenKind::Semicolon
                })?;
                let uri = {
                    // The URI literal is the first string token of the
                    // directive.
                    let mut found = None;
                    for index in kw + 1..semi {
                        if kind_of(tokens, index)? == TokenKind::String {
                            let lexeme = tokens.lexeme(text, index);
                            found = Some(
                                lexeme
                                    .trim_matches(|c| matches!(c, '\'' | '"' | 'r'))
                                    .to_owned(),
                            );
                            break;
                        }
                    }
                    found?
                };
                let kind = kind_of(tokens, kw)?;
                let priority = match (kind, uri.as_str()) {
                    (TokenKind::Keyword(Keyword::Import), u) if u.starts_with("dart:") => 0u8,
                    (TokenKind::Keyword(Keyword::Import), u) if u.starts_with("package:") => 1,
                    (TokenKind::Keyword(Keyword::Import), u) if u.contains("://") => 2,
                    (TokenKind::Keyword(Keyword::Import), _) => 3,
                    (TokenKind::Keyword(Keyword::Export), u) if u.starts_with("dart:") => 4,
                    (TokenKind::Keyword(Keyword::Export), u) if u.starts_with("package:") => 5,
                    (TokenKind::Keyword(Keyword::Export), u) if u.contains("://") => 6,
                    (TokenKind::Keyword(Keyword::Export), _) => 7,
                    _ => 8,
                };
                spans.push((kw, semi, priority));
                cursor = semi + 1;
            }
            // Metadata and the script tag ride the scan; a member or
            // declaration keyword ends the directive run for good.
            TokenKind::ScriptTag
            | TokenKind::Semicolon
            | TokenKind::At
            | TokenKind::Identifier
            | TokenKind::Period
            | TokenKind::Comma => cursor += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                cursor = tokens.matching_end(cursor)? + 1;
            }
            TokenKind::Lt => {
                cursor = angle_group_end(tokens, cursor)? + 1;
            }
            TokenKind::String => cursor += 1,
            TokenKind::Eof => break,
            _ => break,
        }
    }
    if spans.is_empty() && library_kw.is_none() {
        return None; // nothing to organize
    }
    // A `library` directive must precede the block — inside it the
    // replace span would swallow it (upstream's own edge, unfixable in
    // the row model).
    if let Some(lib) = library_kw {
        // The `library`'s `;` must precede the first sortable keyword.
        let lib_end = depth0_find(tokens, text, lib + 1, len, |kind, _| {
            kind == TokenKind::Semicolon
        });
        if lib_end.is_none_or(|end| end > spans.first().map_or(usize::MAX, |s| s.0)) {
            return None;
        }
    }
    // The text regions: each directive's own line block, leading
    // comments included — every line between the previous directive's
    // trailing line and this one's start attaches (blank lines inside
    // the attached run ride the substring). For the first sortable
    // directive with no `library`, only the adjacent non-doc comment
    // group attaches; `///`/`/**` docs, `@` annotations, and
    // blank-separated groups stay at the top (upstream's
    // pseudo-library-directive face).
    let line_of = |byte: usize| -> usize {
        line_starts.partition_point(|&s| s <= byte) - 1
    };
    let line_start_byte = |line: usize| -> usize { line_starts[line] };
    let line_end_byte = |line: usize| -> usize {
        if line + 1 < line_starts.len() {
            line_starts[line + 1]
        } else {
            text.len()
        }
    };
    let mut pieces: Vec<DirectivePiece> = Vec::new();
    // The `library` row bounds the attach walk — the row model must not
    // climb across it (the official walk reads the directive's own leading
    // trivia, which stops at the library's tokens).
    let library_floor = library_kw
        .and_then(|lib| {
            depth0_find(tokens, text, lib + 1, len, |kind, _| kind == TokenKind::Semicolon)
        })
        .and_then(|semi| tokens.get(semi).map(|t| line_of(t.byte_offset as usize)));
    let mut prev_trailing_line = None::<usize>;
    for (index, &(kw, semi, priority)) in spans.iter().enumerate() {
        let kw_byte = tokens.get(kw)?.byte_offset as usize;
        let semi_end = token_end(tokens.get(semi)?);
        let dir_start_line = line_of(kw_byte);
        let dir_end_line = line_of(semi_end);
        // The region ends at the `;`'s end — the line's own `\n` stays
        // (upstream `directive.end` is "after the last token, including
        // the end-of-line comment": a trailing `//` on the `;`'s line
        // extends the end to the comment's own end, still before `\n`).
        let line_text = text.get(semi_end..line_end_byte(dir_end_line))?;
        let mut end_byte = semi_end;
        if let Some(at) = line_text.find("//") {
            end_byte = semi_end + at + line_text[at..].find('\n').unwrap_or(line_text.len() - at);
        }
        // The leading attach: lines between the previous region's end
        // line and the directive's first line. Only comment/blank lines
        // qualify — anything else (a stray statement) refuses the pass.
        let floor_line = prev_trailing_line
            .map_or(0, |l| l + 1)
            .max(library_floor.map_or(0, |l| l + 1));
        let is_first = index == 0 && library_kw.is_none();
        let attach_line = if is_first {
            // Only the LAST contiguous comment group adjacent to the
            // directive attaches; doc/annotation lines never do; a
            // `// @dart=` language-version line never does.
            let mut scan = dir_start_line;
            while scan > floor_line {
                let prev_text = text
                    .get(line_start_byte(scan - 1)..line_end_byte(scan - 1))?
                    .trim();
                if prev_text.is_empty()
                    || prev_text.starts_with("///")
                    || prev_text.starts_with("/**")
                    || prev_text.starts_with('*')
                    || prev_text.starts_with('@')
                    || prev_text.starts_with("// @dart")
                {
                    break;
                }
                if prev_text.starts_with("//") || prev_text.starts_with("/*") {
                    scan -= 1;
                    continue;
                }
                break;
            }
            scan
        } else {
            // Every line back to the previous region's trailing line
            // attaches — the block must be comments/blank only.
            let mut scan = dir_start_line;
            while scan > floor_line {
                let prev_text = text
                    .get(line_start_byte(scan - 1)..line_end_byte(scan - 1))?
                    .trim();
                if prev_text.is_empty()
                    || prev_text.starts_with("//")
                    || prev_text.starts_with("/*")
                    || prev_text.starts_with('*')
                    || prev_text.starts_with("*/")
                    || prev_text.starts_with("///")
                {
                    scan -= 1;
                    continue;
                }
                return None;
            }
            scan
        };
        // The first attach line starts at the first comment's own
        // offset — blank lines above it stay outside (upstream's
        // `offset = leadingComment.offset`).
        let mut start_byte = line_start_byte(attach_line);
        if !is_first {
            while start_byte < kw_byte
                && text
                    .get(start_byte..line_end_byte(line_of(start_byte)))
                    .is_some_and(|l| l.trim().is_empty())
            {
                start_byte = line_start_byte(line_of(start_byte) + 1);
            }
        }
        let uri = {
            let mut found = String::new();
            for i in kw + 1..semi {
                if kind_of(tokens, i)? == TokenKind::String {
                    found = tokens
                        .lexeme(text, i)
                        .trim_matches(|c| matches!(c, '\'' | '"' | 'r'))
                        .to_owned();
                    break;
                }
            }
            found
        };
        // The URI literal's UTF-16 offset (the removal rows' match anchor —
        // the official `_isUnusedImport` compares `directive.uri.offset`);
        // a directive with no string token (a broken file) can never match
        // a removal row, so the sentinel stands in.
        let uri16 = (kw + 1..semi)
            .find(|&i| kind_of(tokens, i) == Some(TokenKind::String))
            .and_then(|i| tokens.get(i))
            .map(|token| token.utf16_offset)
            .unwrap_or(u32::MAX);
        // The shown names the file flags `unused_shown_name` (imports only
        // — the official collection gates on `directive is ImportDirective`),
        // in written order: identifiers between a `show` keyword and the
        // directive's `;` (`hide` ends the run) whose own offset a row
        // sits on.
        let mut flagged_shown = Vec::new();
        if !unused_shown.is_empty()
            && kind_of(tokens, kw)? == TokenKind::Keyword(Keyword::Import)
        {
            let mut in_show = false;
            for i in kw + 1..semi {
                match kind_of(tokens, i)? {
                    TokenKind::Keyword(Keyword::Show) => in_show = true,
                    TokenKind::Keyword(Keyword::Hide) => in_show = false,
                    TokenKind::Identifier if in_show => {
                        if unused_shown.contains(&tokens.get(i)?.utf16_offset) {
                            flagged_shown.push(tokens.lexeme(text, i).to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }
        pieces.push(DirectivePiece {
            priority,
            uri,
            uri16,
            unused_shown: flagged_shown,
            start: start_byte,
            end: end_byte,
            text: text.get(start_byte..end_byte)?.to_owned(),
        });
        prev_trailing_line = Some(dir_end_line);
    }
    // The sort: priority group, then the URI compare (the upstream
    // `compareDirectiveUri` — package URIs compare path-first), then the
    // verbatim text.
    let mut sorted: Vec<&DirectivePiece> = pieces.iter().collect();
    sorted.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| compare_directive_uri(&a.uri, &b.uri))
            .then_with(|| a.text.cmp(&b.text))
    });
    // The join: eol between rows, a blank line between priority groups,
    // right-trimmed (upstream `sb + trimRight`). The removal legs walk the
    // sorted rows exactly as the official write loop does — an unused
    // import or an exact repeat of the previous kept row's text drops
    // whole (and never updates that text), a flagged shown name excises
    // from its `show` clause by the official string surgery (`name, `
    // whole-comma first, else `, name`, both searched from the clause's
    // own `show`), and the resulting row text is what the repeat compare
    // sees next.
    let mut out = String::new();
    let mut current = None::<u8>;
    let mut previous_text = String::new();
    for piece in &sorted {
        if !unresolved_gate
            && (unused_uris.contains(&piece.uri16) || previous_text == piece.text)
        {
            continue;
        }
        let mut written = piece.text.clone();
        if !piece.unused_shown.is_empty()
            && let Some(show_at) = written.find("show")
        {
            for name in &piece.unused_shown {
                // The official surgery (`replaceFirst` from the `show`):
                // the comma containment decides the arm, the comma-space
                // needle removes.
                let needle = if written.contains(&format!("{name},")) {
                    format!("{name}, ")
                } else {
                    format!(", {name}")
                };
                if let Some(at) = written[show_at..].find(&needle) {
                    let at = show_at + at;
                    written.replace_range(at..at + needle.len(), "");
                }
            }
        }
        if let Some(prev) = current {
            out.push_str(eol);
            if prev != piece.priority {
                out.push_str(eol);
            }
        }
        current = Some(piece.priority);
        out.push_str(written.trim_end_matches(['\n', '\r']));
        previous_text = written;
    }
    let block_edit = if pieces.is_empty() {
        None
    } else {
        let replace_start = pieces.first().map(|p| p.start)?;
        let replace_end = pieces.last().map(|p| p.end)?;
        if out == text[replace_start..replace_end] {
            None // the official no-edit face: organize changed nothing
        } else {
            let (s16, e16) = (
                utf16_count(&text[..replace_start]),
                utf16_count(&text[..replace_end]),
            );
            Some((s16, e16 - s16, out))
        }
    };
    // The doc-import leg: the library directive's doc comment sorts its
    // `@docImport` lines (upstream `_organizeDocImports`, run before the
    // directive write loop and independent of it).
    let doc_edit = library_kw
        .and_then(|lib| organize_doc_imports(tokens, text, lib, &line_starts, eol));
    match (doc_edit, block_edit) {
        (None, None) => None,
        (doc, block) => {
            let mut edits = Vec::new();
            if let Some(edit) = doc {
                edits.push(edit);
            }
            if let Some(edit) = block {
                edits.push(edit);
            }
            Some(edits)
        }
    }
}

/// The doc-import leg: the last `library` directive's doc comment sorts
/// its `@docImport` lines into the same priority groups as imports — the
/// official `_organizeDocImports` (line comments) and
/// `_organizeDocImportsInBlockComment` (one `/** */` token) over the
/// file's own text. Returns the `(utf16 offset, length, replacement)` for
/// the doc-comment region, or `None` when there is no attached doc
/// comment, no parseable `@docImport` line in it, or the rebuild changes
/// nothing.
///
/// Detection mirrors the comment builder: a line is a doc import when its
/// marker-stripped content (whitespace skipped) starts with
/// `'@docImport '` and the rest opens with a string literal — the
/// synthetic `import` parse's success path (`_parseDocImport`); a failed
/// parse keeps its line in place. Block comments drop the `*/` tail on
/// the closing line and the `* `/lone-`*` marker on the others (the first
/// line's `/**` prefix stays — a doc import sharing its line with the
/// opening is not one, the analyzer's own edge).
pub(crate) fn organize_doc_imports(
    tokens: &TokenTable,
    text: &str,
    lib_kw: usize,
    line_starts: &[usize],
    eol: &str,
) -> Option<(u32, u32, String)> {
    let lib_byte = tokens.get(lib_kw)?.byte_offset as usize;
    let line_of = |byte: usize| -> Option<usize> {
        line_starts.partition_point(|&s| s <= byte).checked_sub(1)
    };
    // The line's `[start, end)` byte span without its terminator.
    let line_span = |line: usize| -> Option<(usize, usize)> {
        let start = *line_starts.get(line)?;
        let raw_end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        let mut end = raw_end;
        if text[..end].ends_with('\n') {
            end -= 1;
            if text[..end].ends_with('\r') {
                end -= 1;
            }
        }
        Some((start, end))
    };
    let lib_line = line_of(lib_byte)?;
    if lib_line == 0 {
        return None;
    }
    // The attached doc comment: the contiguous run ending on the line
    // directly above the `library` keyword (a blank line or any other
    // content between unattaches it).
    let above = lib_line - 1;
    let (top, block) = {
        let (above_start, above_end) = line_span(above)?;
        let above_text = text[above_start..above_end].trim();
        if above_text.starts_with("///") {
            let mut top = above;
            while top > 0 {
                let (s, e) = line_span(top - 1)?;
                if !text[s..e].trim_start().starts_with("///") {
                    break;
                }
                top -= 1;
            }
            (top, false)
        } else if above_text.ends_with("*/") {
            let mut top = above;
            loop {
                let (s, e) = line_span(top)?;
                let trimmed = text[s..e].trim_start();
                if trimmed.starts_with("/**") {
                    break;
                }
                if top == 0 || !(trimmed.starts_with('*') || trimmed.starts_with("/*")) {
                    return None;
                }
                top -= 1;
            }
            (top, true)
        } else {
            return None;
        }
    };
    // The physical lines of the doc comment as byte spans.
    let lines: Vec<(usize, usize)> = (top..=above).map(line_span).collect::<Option<_>>()?;
    let line_count = lines.len();
    // The official `rawContent` for a block-comment line: the closing
    // line's `\s*\*/$` strip, the first line's `^/\*\*\s?` strip, the
    // others' `^\s*\*\s?` strip.
    let raw_content = |index: usize| -> &str {
        let (start, end) = lines[index];
        let raw = &text[start..end];
        let body = if index + 1 == line_count {
            raw.strip_suffix("*/").map(str::trim_end).unwrap_or(raw)
        } else {
            raw
        };
        if index == 0 {
            let head = body.strip_prefix("/**").unwrap_or(body);
            head.strip_prefix(' ').unwrap_or(head)
        } else {
            let body = body.trim_start();
            body.strip_prefix("* ")
                .unwrap_or(body.strip_prefix('*').unwrap_or(body))
        }
    };
    // The builder's per-line content — the doc-import detection face. The
    // first block line keeps its `/**` prefix (never a doc import); the
    // closing line drops `*/`; the others drop `* `/lone-`*`.
    let builder_content = |index: usize| -> &str {
        if !block {
            // The `///` token text minus its marker.
            let (start, end) = lines[index];
            let raw = text[start..end].trim_start();
            return raw[3..].trim_start();
        }
        if index == 0 {
            let (start, end) = lines[index];
            return &text[start..end];
        }
        let (start, end) = lines[index];
        let raw = &text[start..end];
        let body = if index + 1 == line_count {
            raw.strip_suffix("*/").unwrap_or(raw)
        } else {
            raw
        };
        let body = body.trim_start();
        let body = match body.strip_prefix("* ") {
            Some(rest) => rest,
            None if body == "*" => "",
            None => body,
        };
        body.trim_start()
    };
    // The doc imports: line index, sort priority, URI, compare/print text.
    let mut doc_imports: Vec<(usize, u8, String, &str)> = Vec::new();
    for index in 0..line_count {
        let Some(rest) = builder_content(index).strip_prefix("@docImport ") else {
            continue;
        };
        let Some(uri) = doc_import_uri(rest) else {
            continue;
        };
        let priority = if uri.starts_with("dart:") {
            0
        } else if uri.starts_with("package:") {
            1
        } else if uri.contains("://") {
            2
        } else {
            3
        };
        // The official `_DocImportInfo` text: the whole `///` line for a
        // line comment, the marker-stripped content for a block comment.
        let info_text = if block {
            raw_content(index)
        } else {
            let (start, end) = lines[index];
            text[start..end].trim_start()
        };
        doc_imports.push((index, priority, uri, info_text));
    }
    if doc_imports.is_empty() {
        return None;
    }
    // The insertion position is the first doc-import line in FILE order
    // (upstream `lines.indexWhere(docImportByLine.containsKey)`), captured
    // before the sort reorders `doc_imports`.
    let first_doc = doc_imports[0].0;
    let last_doc = doc_imports[doc_imports.len() - 1].0;
    doc_imports.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| compare_directive_uri(&a.2, &b.2))
            .then_with(|| a.3.cmp(&b.3))
    });
    // A blank comment line separates priority groups, styled by the first
    // sorted row (`docCommentLinePrefix`); a block comment uses the empty
    // string the re-marking formats as ` *`.
    let blank: &str = if block {
        ""
    } else {
        let marker = doc_imports[0].3;
        if marker.starts_with('*') {
            "*"
        } else {
            "///"
        }
    };
    let mut sorted_texts: Vec<&str> = Vec::new();
    let mut previous_priority = None;
    for &(_, priority, _, info_text) in &doc_imports {
        if previous_priority.is_some_and(|prev| prev != priority) {
            sorted_texts.push(blank);
        }
        sorted_texts.push(info_text);
        previous_priority = Some(priority);
    }
    // The untouched lines keep their order; the sorted block sits where the
    // first doc-import line was; blank comment lines between doc-import
    // lines drop (the grouping already separates the groups).
    let is_doc_line = |index: usize| doc_imports.iter().any(|info| info.0 == index);
    let is_blank_between = |index: usize| -> bool {
        index > first_doc
            && index < last_doc
            && matches!(
                text[lines[index].0..lines[index].1].trim(),
                "///" | "*"
            )
    };
    let mut others: Vec<&str> = Vec::new();
    for index in 0..line_count {
        if is_doc_line(index) || is_blank_between(index) {
            continue;
        }
        if block {
            // The official `otherContents`: a closing line holding only the
            // `*/` marker contributes nothing (it is re-added below).
            let content = raw_content(index);
            if index + 1 == line_count && content.is_empty() {
                continue;
            }
            others.push(content);
        } else {
            let (start, end) = lines[index];
            others.push(text[start..end].trim_start());
        }
    }
    let before = (0..first_doc)
        .filter(|&index| !is_doc_line(index) && !is_blank_between(index))
        .count();
    let mut new_texts: Vec<&str> = Vec::new();
    new_texts.extend(others[..before].iter().copied());
    new_texts.extend(sorted_texts.iter().copied());
    new_texts.extend(others[before..].iter().copied());
    // The rebuild: a line comment joins the kept texts; a block comment
    // re-marks every line (`/** …` first, ` * …` after, ` */` last — the
    // official `newLines` shape).
    let rebuilt = if block {
        let mut out = String::new();
        for (index, content) in new_texts.iter().copied().enumerate() {
            if index > 0 {
                out.push_str(eol);
            }
            if content.is_empty() {
                out.push_str(if index == 0 { "/**" } else { " *" });
            } else {
                out.push_str(if index == 0 { "/** " } else { " * " });
                out.push_str(content);
            }
        }
        out.push_str(eol);
        out.push_str(" */");
        out
    } else {
        new_texts.join(eol)
    };
    let region_start = lines[0].0;
    let region_end = lines[line_count - 1].1;
    if rebuilt == text[region_start..region_end] {
        return None;
    }
    let (s16, e16) = (
        utf16_count(&text[..region_start]),
        utf16_count(&text[..region_end]),
    );
    Some((s16, e16 - s16, rebuilt))
}

/// The URI of a synthetic `import` body: adjacent simple string literals'
/// raw inner text joined (`stringValue`'s adjacent-literal concatenation),
/// or `None` when the body does not open with one (the official parse
/// failure keeps the line in place).
pub(crate) fn doc_import_uri(rest: &str) -> Option<String> {
    let mut s = rest.trim_start();
    let mut uri = String::new();
    loop {
        let body = s.strip_prefix('r').unwrap_or(s);
        let quote = body.as_bytes().first().copied()?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let quote = quote as char;
        let close = body[1..].find(quote)?;
        uri.push_str(&body[1..1 + close]);
        s = body[1 + close + 1..].trim_start();
        let next = s.strip_prefix('r').unwrap_or(s);
        if next.starts_with('\'') || next.starts_with('"') {
            continue;
        }
        return Some(uri);
    }
}
