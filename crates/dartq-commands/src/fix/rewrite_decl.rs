use super::*;


pub(crate) fn add_member_annotation(input: &FixInput<'_>, name: &str) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    // The annotation rides the member's own line: the official
    // `AddOverride` anchors the member's begin token — the first real
    // token, below any doc comment — and carries the member's indent
    // (upstream add_override_test.dart `test_method_with_doc_comment`).
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..line_indent_end(text, line_start)];
    Some((
        utf16_count(&text[..line_start]),
        0,
        format!("{indent}{name}\n"),
    ))
}

/// The `annotate_overrides` entry: `@override` (the official `AddOverride`).
pub(crate) fn annotate_overrides_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    add_member_annotation(input, "@override")
}

/// The `annotate_redeclares` entry: `@redeclare` (the official
/// `AddRedeclare`).
pub(crate) fn annotate_redeclares_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    add_member_annotation(input, "@redeclare")
}

/// `async_return_with_no_await`: `await ` lands before the returned
/// expression, after the whitespace run (the official `AddAwait.return_`'s
/// expression insertion; the lint reports at the `return` keyword or `=>`).
pub(crate) fn await_return_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if slice != "return" && slice != "=>" {
        return None;
    }
    let rest = input
        .text
        .get(utf16_offset_to_byte(input.text, input.range.end)..)?;
    if rest.trim_start().starts_with([';', ')']) {
        return None;
    }
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let at = input.range.end + u32::try_from(ws).ok()?;
    Some((at, 0, "await ".to_owned()))
}

/// The type-deletion face of `RemoveTypeAnnotation.other` over a
/// parameter-position type (`avoid_annotating_with_dynamic`,
/// `avoid_return_types_on_setters`, `avoid_types_on_closure_parameters`,
/// `type_init_formals`): the reported type goes with the whitespace run to
/// the next token (`range.startStart(type, next)`).
pub(crate) fn remove_reported_type(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let first = slice.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$' || first == '<') {
        return None;
    }
    remove_keyword(input.text, input.range.start, slice)
}

/// `omit_local_variable_types` / `omit_obvious_local_variable_types` /
/// `omit_obvious_property_types`: the written type becomes `var` (the
/// official `ReplaceWithVar`), or the type-through-name gap does when the
/// list carries several declarators (the official `RemoveTypeAnnotation`
/// declaration-list face). A type with type arguments drops — the official
/// moves `<…>` into the initializer as a second edit — and so does a
/// `{…}` initializer (the official ambiguity guard: `var x = {}` changes
/// the literal's meaning).
pub(crate) fn omit_type_with_var(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if slice.is_empty()
        || slice
            .chars()
            .any(|c| matches!(c, ',' | '=' | ';' | '<' | '>' | '(' | ')'))
    {
        return None;
    }
    let rest = text.get(utf16_offset_to_byte(text, input.range.end)..)?;
    let after_ws = rest.trim_start_matches([' ', '\t']);
    let name_len = after_ws
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .map(char::len_utf8)
        .sum::<usize>();
    if name_len == 0 {
        return None;
    }
    let after_name = after_ws[name_len..].trim_start_matches([' ', '\t']);
    if after_name.starts_with('=') {
        let initializer = after_name[1..].trim_start_matches([' ', '\t']);
        if initializer.starts_with('{') {
            return None;
        }
        return Some((
            input.range.start,
            input.range.end - input.range.start,
            "var".to_owned(),
        ));
    }
    if after_name.starts_with(',') {
        // Multi-declarator: `var ` replaces the type plus the whitespace
        // before the first name.
        let cut_bytes = rest.len() - after_ws.len()
            + name_len
            + (after_ws[name_len..].len() - after_name.len());
        let start = utf16_offset_to_byte(text, input.range.start);
        let end = utf16_offset_to_byte(text, input.range.end);
        return Some((
            input.range.start,
            utf16_count(text.get(start..end + cut_bytes)?),
            "var ".to_owned(),
        ));
    }
    if after_name.starts_with("in") && !after_name[2..].starts_with(is_identifier_char) {
        return Some((
            input.range.start,
            input.range.end - input.range.start,
            "var".to_owned(),
        ));
    }
    None
}

/// `prefer_final_locals` / `prefer_final_fields`: the reported `var`
/// keyword becomes `final` (the official `MakeFinal` keyword face), the
/// written type gains `final ` (the type face). A multi-variable list
/// drops — the official bails unless the list holds exactly one variable.
pub(crate) fn make_declaration_final(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if slice == "var" {
        return single_declarator_follows(text, input.range.end).then_some((
            input.range.start,
            3,
            "final".to_owned(),
        ));
    }
    if !slice.is_empty() && slice.chars().next()?.is_ascii_alphabetic() {
        return single_declarator_follows(text, input.range.end).then_some((
            input.range.start,
            0,
            "final ".to_owned(),
        ));
    }
    None
}

/// Whether `name =` (one declarator with an initializer) follows the
/// declaration's keyword or type — the official `MakeFinal` single-variable
/// gate, read textually (`name in` — the for-each shape — also passes).
pub(crate) fn single_declarator_follows(text: &str, end16: u32) -> bool {
    let Some(rest) = text.get(utf16_offset_to_byte(text, end16)..) else {
        return false;
    };
    let after_ws = rest.trim_start_matches([' ', '\t']);
    let name_len = after_ws
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .count();
    if name_len == 0 {
        return false;
    }
    let after_name = after_ws[name_len..].trim_start_matches([' ', '\t']);
    after_name
        .starts_with(['=', ','])
        .then(|| !after_name.starts_with(','))
        == Some(true)
        || after_name.starts_with("in") && !after_name[2..].starts_with(is_identifier_char)
}

/// `prefer_final_parameters` / `prefer_final_in_for_each`: `final ` lands
/// at the parameter's (or for-each variable's) first content token; a
/// leading `var` swaps to `final` and a `covariant` modifier keeps its
/// head position (the official `MakeFinal` faces).
pub(crate) fn make_parameter_final(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if slice == "var" {
        // The for-each variable's reported keyword swaps whole (the
        // official `MakeFinal`: `for (var e …)` becomes `final e`).
        return Some((input.range.start, 3, "final".to_owned()));
    }
    if let Some(rest) = slice.strip_prefix("var ")
        && !rest.starts_with(is_identifier_char)
    {
        return Some((input.range.start, 3, "final".to_owned()));
    }
    if let Some(rest) = slice.strip_prefix("covariant")
        && rest.starts_with([' ', '\t'])
    {
        let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let at = input.range.start + u32::try_from(9 + ws).ok()?;
        return Some((at, 0, "final ".to_owned()));
    }
    let first = slice.chars().next()?;
    if first.is_ascii_alphabetic() || first == '_' || first == '$' {
        return Some((input.range.start, 0, "final ".to_owned()));
    }
    None
}

/// `prefer_const_constructors` / `prefer_const_constructors_in_immutables`
/// / `prefer_const_literals_to_create_immutables`: `const ` lands before
/// the reported creation or literal (the official `AddConst` insert face),
/// and a `new` keyword swaps to `const` (the official
/// `ReplaceNewWithConst`). Inner depth-0 `const` keywords fold out in the
/// same replacement (the official `_ConstRangeFinder` deletions).
pub(crate) fn add_const_prefix(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if let Some(rest) = slice.strip_prefix("new") {
        if rest.starts_with(is_identifier_char) {
            return None;
        }
        let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        return Some((
            input.range.start,
            3 + u32::try_from(ws).ok()?,
            "const ".to_owned(),
        ));
    }
    let first = slice.chars().next()?;
    if !(first.is_ascii_alphabetic()
        || first == '_'
        || first == '$'
        || first == '<'
        || first == '('
        || first == '[')
    {
        return None;
    }
    // Inner `const` keywords fold out (the official producer deletes them
    // while inserting the outer one); the span is rebuilt from its tokens.
    let tokens = input.tokens?;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let mut depth = 0u32;
    let mut inner = Vec::new();
    for index in first..=last {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Keyword(Keyword::Const) if depth == 0 && index > first => {
                inner.push(index);
            }
            _ => {}
        }
    }
    if inner.is_empty() {
        return Some((input.range.start, 0, "const ".to_owned()));
    }
    let (start16, length16) = span_over(text, tokens, first, last)?;
    let start = utf16_offset_to_byte(text, start16);
    let end = utf16_offset_to_byte(text, start16 + length16);
    let mut rebuilt = String::from("const ");
    let mut cursor = start;
    for index in inner {
        let keyword_start = utf16_offset_to_byte(text, tokens.get(index)?.utf16_offset);
        let keyword_end = token_end(tokens.get(index)?);
        rebuilt.push_str(text.get(cursor..keyword_start)?);
        let rest = text.get(keyword_end..end)?;
        rebuilt.push_str(rest.trim_start_matches([' ', '\t']));
        cursor = keyword_end + rest.len() - rest.trim_start_matches([' ', '\t']).len();
    }
    rebuilt.push_str(text.get(cursor..end)?);
    Some((start16, length16, rebuilt))
}

/// `prefer_void_to_null`: the reported `null`/`Null` type display becomes
/// `void` (the official `ReplaceNullWithVoid` replaces the diagnostic
/// span).
pub(crate) fn replace_null_with_void(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if !slice.ends_with("null") && !slice.ends_with("Null") {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        "void".to_owned(),
    ))
}

/// `prefer_int_literals`: the integral double literal loses its fraction
/// (`1.0` → `1` — the official `ConvertToIntLiteral` deletes from the
/// decimal point) or is rewritten whole when an exponent carries the value
/// (`1.0e2` → `100`, the official's `$intValue` face; a value outside the
/// int range drops).
pub(crate) fn double_to_int_literal(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if slice.is_empty()
        || !slice
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        return None;
    }
    if !slice.contains(['e', 'E']) {
        let dot = slice.find('.')?;
        if slice[dot + 1..].chars().all(|c| c.is_ascii_digit()) {
            return Some((
                input.range.start + u32::try_from(dot).ok()?,
                u32::try_from(slice.len() - dot).ok()?,
                String::new(),
            ));
        }
        return None;
    }
    let value = slice.parse::<f64>().ok()?;
    if value.fract() != 0.0 || value.abs() >= 9.223_372_036_854_776e18 {
        return None;
    }
    let int = value as i64;
    Some((
        input.range.start,
        input.range.end - input.range.start,
        int.to_string(),
    ))
}

/// `future_sync_value`: the reported `value` selector renames to
/// `syncValue` (the official `ReplaceWithSyncValue` replaces the covering
/// node's range with the name).
pub(crate) fn sync_value_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    if slice16(input.text, &input.range)? != "value" {
        return None;
    }
    Some((input.range.start, 5, "syncValue".to_owned()))
}

/// `implicit_call_tearoffs`: `.call` lands after the reported expression,
/// parenthesized when its precedence sits below postfix (the official
/// `AddExplicitCall`).
pub(crate) fn add_explicit_call_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let slice = slice16(text, &input.range)?;
    let wrap = binds_loose(tokens, first..last + 1);
    Some((
        input.range.start,
        input.range.end - input.range.start,
        if wrap {
            format!("({slice}).call")
        } else {
            format!("{slice}.call")
        },
    ))
}

/// `leading_newlines_in_multiline_strings`: the line terminator lands
/// right after the (optionally raw) opening triple quote (the official
/// `AddLeadingNewlineToString` inserts at the literal's contents offset).
pub(crate) fn add_leading_newline(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let skip = slice
        .strip_prefix("r'''")
        .or_else(|| slice.strip_prefix("r\"\"\""))
        .map(|_| 4u32)
        .or_else(|| slice.strip_prefix("'''").map(|_| 3u32))
        .or_else(|| slice.strip_prefix("\"\"\"").map(|_| 3u32))?;
    Some((input.range.start + skip, 0, "\n".to_owned()))
}

/// `unnecessary_ignore`: the ignore comment's line goes (the official
/// `RemoveComment` deletes the comment's lines range — [`drop_span_statement`]'s
/// shape).
pub(crate) fn remove_ignore_comment(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if !slice.starts_with("//") || !slice.contains("ignore") {
        return None;
    }
    drop_span_statement(input.text, &input.range)
}

/// `unnecessary_library_name`: the name between the `library` keyword and
/// its `;` goes (the official `RemoveLibraryName` deletes keyword-end
/// through semicolon-start).
pub(crate) fn drop_library_name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    let head = text.get(..start)?.trim_end_matches([' ', '\t']);
    if !head.ends_with("library") {
        return None;
    }
    let before = &head[..head.len() - 7];
    if before.ends_with(is_identifier_char) {
        return None;
    }
    Some((
        u32::try_from(utf16_count(head)).ok()?,
        input.range.end - u32::try_from(utf16_count(head)).ok()?,
        String::new(),
    ))
}

/// `unnecessary_raw_strings`: the `r` prefix of the reported literal goes
/// (the official `RemoveUnnecessaryRawString` deletes the one character).
pub(crate) fn remove_raw_prefix(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    if !matches!(slice.as_bytes().first(), Some(b'r'))
        || !matches!(slice.as_bytes().get(1), Some(b'\'' | b'"'))
    {
        return None;
    }
    Some((input.range.start, 1, String::new()))
}

/// `use_raw_strings`: the literal becomes raw with the backslashes of its
/// contents gone (the official `ConvertToRawString` inserts `r` and
/// deletes each backslash — folded into one whole-literal replacement).
/// An escape outside the `\\`/`\$` pair the lint admits refuses the fix.
pub(crate) fn convert_to_raw_string_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let quote = *slice.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    if slice.as_bytes().last() != Some(&quote) || slice.len() < 2 {
        return None;
    }
    let contents = &slice[1..slice.len() - 1];
    let mut rebuilt = String::with_capacity(contents.len() + 2);
    rebuilt.push('r');
    rebuilt.push(quote as char);
    let mut chars = contents.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // One backslash leaves per escape; the escaped character
            // keeps (the official `ConvertToRawString`'s per-backslash
            // deletions — `\\` and `\$` read verbatim in a raw string).
            let escaped = chars.next()?;
            if !matches!(escaped, '\\' | '$') {
                return None;
            }
            rebuilt.push(escaped);
            continue;
        }
        rebuilt.push(c);
    }
    rebuilt.push(quote as char);
    Some((
        input.range.start,
        input.range.end - input.range.start,
        rebuilt,
    ))
}

/// `slash_for_doc_comments`: the `/** … */` block comment is rebuilt as
/// `///` lines (the official `ConvertDocumentationIntoLine` state machine:
/// first line's `/**` stripped, `*` rail lines trimmed, a pure `*/`
/// terminator ends the block, the comment's own indentation prefixes every
/// emitted line).
pub(crate) fn doc_comment_to_line(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    if !slice.starts_with("/**") || !slice.ends_with("*/") {
        return None;
    }
    let start = utf16_offset_to_byte(text, input.range.start);
    let prefix = &text[text[..start].rfind('\n').map_or(0, |i| i + 1)..start];
    let eol = if slice.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::new();
    let mut line_prefix = String::new();
    for (position, line) in slice.split(eol).enumerate() {
        if position == 0 {
            let body = line.strip_prefix("/**")?.trim();
            let body = body.strip_suffix("*/").map_or(body, str::trim);
            if body.is_empty() {
                continue;
            }
            out.push_str("/// ");
            out.push_str(body);
            line_prefix = format!("{eol}{prefix}");
        } else {
            let trimmed = line.trim();
            if trimmed.ends_with("*/") && trimmed[..trimmed.len() - 2].chars().all(|c| c == '*') {
                break;
            }
            let body = if let Some(rest) = trimmed.strip_prefix('*') {
                rest
            } else if trimmed.is_empty() {
                ""
            } else {
                // Everything inside `/**` and `*/` is documentation, even
                // when the line is not prefixed with a `*`.
                trimmed
            };
            let body = body.strip_suffix("*/").map_or(body, str::trim_end);
            out.push_str(&line_prefix);
            out.push_str("///");
            out.push_str(body);
            line_prefix = format!("{eol}{prefix}");
        }
    }
    if out.is_empty() {
        return None;
    }
    Some((input.range.start, input.range.end - input.range.start, out))
}

/// `flutter_style_todos`: the comment lexeme is rebuilt in the flutter
/// TODO shape — one leading `// ` space, `TODO` upper-cased at index 3,
/// the author's `)` closed by `: `, stray later `TODO`s back-ticked
/// (the official `ConvertToFlutterStyleTodo` pipeline); a rebuilt comment
/// that still fails the lint's expected shape refuses the fix.
pub(crate) fn flutter_style_todo(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let mut content = slice16(input.text, &input.range)?.to_owned();
    if let Some(rest) = content.strip_prefix("///") {
        content = format!("/{rest}");
    }
    let mut todo_index = content.find("TODO").or_else(|| content.find("todo"))?;
    if todo_index > 3 && content[3..todo_index].trim().is_empty() {
        content.replace_range(3..todo_index, "");
        todo_index = 3;
    }
    if !content.starts_with("// ") {
        content = content.replacen("//", "// ", 1);
    }
    if todo_index == 3 && content.len() > 7 && content[7..].starts_with(' ') {
        content.replace_range(7..8, "");
    }
    if todo_index == 3 {
        let colon_index = content.find(')').map_or(0, |at| at + 1);
        if content.len() > colon_index && !content[colon_index..].starts_with(':') {
            content = content.replacen(')', "):", 1);
        }
    }
    if let Some(rest) = content.strip_prefix("// todo") {
        content = format!("// TODO{rest}");
    }
    // Stray `TODO`s in the message body gain back ticks (after the head).
    let mut rebuilt = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut cursor = 0;
    while cursor + 4 <= bytes.len() {
        if content[cursor..].len() >= 4 && content[cursor..cursor + 4].eq_ignore_ascii_case("TODO")
        {
            if cursor > 4 {
                rebuilt.push('`');
                rebuilt.push_str(&content[cursor..cursor + 4]);
                rebuilt.push('`');
            } else {
                rebuilt.push_str(&content[cursor..cursor + 4]);
            }
            cursor += 4;
        } else {
            rebuilt.push(content[cursor..].chars().next()?);
            cursor += content[cursor..].chars().next()?.len_utf8();
        }
    }
    rebuilt.push_str(&content[cursor..]);
    // The official gate: the comment must still read as a TODO comment and
    // not as an already-styled one.
    if !todo_comment_head(&rebuilt) || flutter_todo_styled(&rebuilt) {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        rebuilt,
    ))
}

/// The lint's `_todoRegExp`: `//+` `ws*` `TODO` + word boundary
/// (case-insensitive).
pub(crate) fn todo_comment_head(content: &str) -> bool {
    let rest = content.trim_start_matches('/');
    if rest.len() == content.len() {
        return false;
    }
    let rest = rest.trim_start_matches([' ', '\t']);
    rest.get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case("TODO"))
        && !rest[4..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
}

/// The lint's `_todoExpectedRegExp`: `//` `ws*` `TODO(author): `.
pub(crate) fn flutter_todo_styled(content: &str) -> bool {
    let Some(rest) = content.strip_prefix("//") else {
        return false;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(rest) = rest.strip_prefix("TODO(") else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut consumed = first.len_utf8();
    for c in chars {
        if c == ')' {
            return rest[consumed + 1..].starts_with("): ");
        }
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '.') {
            return false;
        }
        consumed += c.len_utf8();
    }
    false
}

/// `avoid_returning_null_for_void`: the `null` between the `return` (or
/// `=>`) and its terminator goes — `return null;` becomes `return;`, the
/// expression-body `=> null` becomes `{}` (the official
/// `RemoveReturnedValue`'s two faces).
pub(crate) fn remove_returned_value(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    let end = utf16_offset_to_byte(text, input.range.end);
    if text.get(start..end)? != "null" {
        return None;
    }
    let head = text.get(..start)?.trim_end_matches([' ', '\t']);
    if let Some(rest) = head.strip_suffix("=>") {
        if rest.ends_with(is_identifier_char) {
            return None;
        }
        return Some((
            u32::try_from(utf16_count(head) - 2).ok()?,
            input.range.end - u32::try_from(utf16_count(head) - 2).ok()?,
            "{}".to_owned(),
        ));
    }
    if head.ends_with("return") && !head[..head.len() - 6].ends_with(is_identifier_char) {
        return Some((
            u32::try_from(utf16_count(head)).ok()?,
            input.range.end - u32::try_from(utf16_count(head)).ok()?,
            String::new(),
        ));
    }
    None
}

/// `avoid_void_async`: the `void` return type spelled right before the
/// reported function name becomes `Future<void>` (the official
/// `ReplaceReturnTypeFuture` replaces the annotation).
pub(crate) fn return_type_future(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    let head = text.get(..start)?.trim_end_matches([' ', '\t']);
    let rest = head.strip_suffix("void")?;
    if rest.ends_with(is_identifier_char) {
        return None;
    }
    let at = u32::try_from(utf16_count(head) - 4).ok()?;
    Some((at, 4, "Future<void>".to_owned()))
}

/// `avoid_print`: the reported `print` selector expands to its invocation
/// and the statement's line content goes (the official `RemovePrint`'s
/// lines-range face; content after the invocation on the same line keeps
/// everything but the invocation itself, [`drop_span_statement`]'s shape).
pub(crate) fn remove_print_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last || !word_at(tokens, text, first, "print") {
        return None;
    }
    if kind_of(tokens, first + 1)? != TokenKind::OpenParen {
        return None;
    }
    let rparen = tokens.matching_end(first + 1)?;
    let mut after = rparen + 1;
    if kind_of(tokens, after) == Some(TokenKind::Semicolon) {
        after += 1;
    } else {
        after = rparen + 1;
    }
    let (start16, length16) = span_over(text, tokens, first, after - 1)?;
    drop_span_statement(text, &(start16..start16 + length16))
}

/// `cast_nullable_to_non_nullable`: the `!` lands after the cast's operand
/// (the official `AddNullCheck.withoutAssignabilityCheck`'s as-expression
/// face — the cast itself stays).
pub(crate) fn cast_null_check(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let as_token = depth0_find(tokens, text, first, last + 1, |kind, _| {
        kind == TokenKind::Keyword(Keyword::As)
    })?;
    if as_token == first {
        return None;
    }
    append_bang(input, first, as_token - 1)
}

/// `combinators_ordering`: the reported combinator's names are rebuilt in
/// sorted order (the official `SortCombinators` replaces each name with
/// the sorted name at its position — folded into one combinator
/// replacement; the sort is the official `List.sort` byte order).
pub(crate) fn sort_combinator_names(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let (keyword, names_raw) = slice.split_once([' ', '\t'])?;
    if keyword != "show" && keyword != "hide" {
        return None;
    }
    let mut names: Vec<&str> = names_raw.split(',').map(str::trim).collect();
    if names.len() < 2 || names.iter().any(|name| !word_shape(name)) {
        return None;
    }
    names.sort_unstable();
    let rebuilt = format!("{keyword} {}", names.join(", "));
    if rebuilt == slice {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        rebuilt,
    ))
}

/// `dangling_library_doc_comments`: a leading doc comment with nothing but
/// whitespace before it gains the explicit `library;` after its block (the
/// official `MoveDocCommentToLibraryDirective`'s in-place face); the
/// relocation face (comment below other directives, metadata unions) is a
/// second edit and is not served.
pub(crate) fn add_library_directive_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    if !text.get(..start)?.trim().is_empty() {
        return None;
    }
    // `library;` lands after the comment line's terminator (the official
    // insertion sits at the lines range' end, past the `\r\n` pair).
    let end = utf16_offset_to_byte(text, input.range.end);
    let rest = text.get(end..)?;
    let (at, insertion) = match rest.find('\n') {
        Some(i) => (end + i + 1, "library;\n"),
        None => (text.len(), "\nlibrary;"),
    };
    Some((utf16_count(&text[..at]), 0, insertion.to_owned()))
}

/// `library_annotations`: a leading annotation with nothing but whitespace
/// before it gains the explicit `library;` beneath itself (the official
/// `MoveAnnotationToLibraryDirective`'s in-place face folded into one
/// whole-annotation replacement; the relocation face is a second edit and
/// is not served). The replacement ends at the directive — the
/// annotation's own line terminator stays in the untouched text and
/// closes the inserted line, so no blank line opens between the two.
pub(crate) fn annotation_library_directive_entry(
    input: &FixInput<'_>,
) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    if !text.get(..start)?.trim().is_empty() {
        return None;
    }
    let slice = slice16(text, &input.range)?;
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("{slice}\nlibrary;"),
    ))
}

/// `no_duplicate_case_values`: the duplicate case clause leaves — the case
/// head alone when the previous clause's statements fall through into it
/// (the official `RemoveDuplicateCase`'s `startEnd(node, colon)` face), the
/// whole clause with its statements when the previous member has
/// statements of its own (the `range.node(node)` face).
pub(crate) fn remove_duplicate_case(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (hit, hit_last) = hit_tokens(tokens, &input.range)?;
    // Walk back to the clause's `case` keyword (the report sits on the
    // duplicated value expression).
    let mut case = hit;
    while case > 0 && !word_at(tokens, text, case, "case") {
        case -= 1;
    }
    if case == 0 && !word_at(tokens, text, case, "case") {
        return None;
    }
    // The clause's `:` follows the value's last token (the bound is
    // exclusive — one past it).
    let colon = depth0_find(tokens, text, case, hit_last + 2, |kind, _| {
        kind == TokenKind::Colon
    })?;
    // The previous clause: the nearest `case`/`default` keyword or the
    // switch body's `{` behind this one.
    let mut previous = None;
    let mut index = case;
    while index > 0 {
        index -= 1;
        let kind = kind_of(tokens, index)?;
        if matches!(kind, TokenKind::OpenCurly) {
            previous = Some(index);
            break;
        }
        if word_at(tokens, text, index, "case") || word_at(tokens, text, index, "default") {
            // The previous clause's colon: the first depth-0 `:` after it.
            let previous_colon = depth0_find(tokens, text, index, case, |kind, _| {
                kind == TokenKind::Colon
            })?;
            previous = Some(previous_colon);
            break;
        }
    }
    let previous = previous?;
    // Statements between the previous clause's colon (or the body `{`) and
    // this clause keep this clause's statements attached to it: the head
    // alone goes; an empty gap takes the whole clause.
    let previous_holds_statements = depth0_find(tokens, text, previous + 1, case, |kind, _| {
        kind == TokenKind::Semicolon
    })
    .is_some();
    let (start, length) = if previous_holds_statements {
        // Whole clause: from `case` through the last statement before the
        // next clause keyword or the body's `}`.
        let mut end = colon;
        let mut cursor = colon + 1;
        let mut depth = 0u32;
        while cursor < tokens.as_slice().len() {
            match kind_of(tokens, cursor)? {
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ if depth == 0
                    && (word_at(tokens, text, cursor, "case")
                        || word_at(tokens, text, cursor, "default")) =>
                {
                    break;
                }
                _ if depth == 0 && kind_of(tokens, cursor)? == TokenKind::Semicolon => {
                    end = cursor;
                }
                _ => {}
            }
            cursor += 1;
        }
        let (a, b) = span_over(text, tokens, case, end)?;
        drop_lines_range(text, a, a + b)?
    } else {
        let (a, b) = span_over(text, tokens, case, colon)?;
        drop_lines_range(text, a, a + b)?
    };
    Some((start, length, String::new()))
}

/// `unnecessary_overrides`: the reported member's line block goes — the
/// lines the declaration occupies, annotations and doc comment with it
/// (the official `RemoveMethodDeclaration` deletes the member's lines
/// range). The member's end is rebuilt from the token stream: through the
/// parameter list and body block, or to the `;` / `=>` statement's
/// semicolon.
pub(crate) fn remove_override_member(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (mut cursor, _) = hit_tokens(tokens, &input.range)?;
    let end;
    loop {
        match kind_of(tokens, cursor)? {
            TokenKind::OpenParen => cursor = tokens.matching_end(cursor)? + 1,
            TokenKind::OpenCurly => {
                end = tokens.matching_end(cursor)?;
                break;
            }
            TokenKind::Semicolon => {
                end = cursor;
                break;
            }
            // `=> expr;` — the member ends at its statement's `;`.
            TokenKind::Function => {
                end = depth0_find(
                    tokens,
                    text,
                    cursor + 1,
                    tokens.as_slice().len(),
                    |kind, _| kind == TokenKind::Semicolon,
                )?;
                break;
            }
            TokenKind::Eof => return None,
            _ => cursor += 1,
        }
    }
    let member_start = member_block_start(text, input.range.start);
    let end_byte = token_end(tokens.get(end)?);
    let line_end = text[end_byte..]
        .find('\n')
        .map_or(text.len(), |i| end_byte + i);
    let (offset, length) = cut_line(text, member_start, line_end);
    Some((offset, length, String::new()))
}

/// `use_if_null_to_convert_nulls_to_bools`: the `bool?`-vs-null comparison
/// becomes the null-aware fallback (`x == null` → `x ?? false`, `x != null`
/// → `x ?? true`; the official `ConvertToIfNull.useToConvertNullsToBools`
/// replaces the comparison, parenthesized when the parent binds tighter
/// than `??`).
pub(crate) fn if_null_bools(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let op = depth0_find(tokens, text, first, last + 1, |kind, _| {
        matches!(kind, TokenKind::EqEq | TokenKind::BangEq)
    })?;
    let (left, right) = (first..op, op + 1..last + 1);
    let nullable = if !is_bool_literal_pair(tokens, text, left.clone()) {
        left
    } else if !is_bool_literal_pair(tokens, text, right.clone()) {
        right
    } else {
        return None;
    };
    let value = if kind_of(tokens, op)? == TokenKind::EqEq {
        "false"
    } else {
        "true"
    };
    let mut rebuilt = format!("{} ?? {value}", token_text(tokens, text, nullable)?,);
    // Outer parentheses when the comparison's parent binds tighter than
    // `??`: an operator-flanked span keeps its parens.
    let before = text
        .get(..utf16_offset_to_byte(text, input.range.start))?
        .trim_end();
    let after = text
        .get(utf16_offset_to_byte(text, input.range.end)..)?
        .trim_start();
    let followed_by_operator = after
        .chars()
        .next()
        .is_some_and(|c| "+-*/%<>=!&|^?~".contains(c));
    let preceded_by_operator = match before.chars().last() {
        // A lone `=` is the assignment — it binds below `??` and needs no
        // parens; a compound or comparison `=` rides an operator run and
        // keeps them.
        Some('=') => before
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| "+-*/%<>=!&|^?~".contains(c)),
        Some(c) => "+-*/%<>=!&|^?~".contains(c),
        None => false,
    };
    if followed_by_operator || preceded_by_operator {
        rebuilt = format!("({rebuilt})");
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        rebuilt,
    ))
}

/// Whether the token range is exactly one `true`/`false` literal.
pub(crate) fn is_bool_literal_pair(tokens: &TokenTable, text: &str, range: core::ops::Range<usize>) -> bool {
    range.len() == 1 && bool_literal(tokens, text, range.start).is_some()
}

/// `use_null_aware_elements`: the null-check if-element becomes the
/// null-aware element — `if (x != null) x` → `?x`, the spread
/// `...?x`, the bang element `x!` loses its operator (the official
/// `ConvertNullCheckToNullAwareElementOrEntry`'s element faces; the
/// map-entry faces need the canonical-element resolution and are not
/// served).
pub(crate) fn null_aware_element(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if !word_at(tokens, text, first, "if") {
        return None;
    }
    let lparen = first + 1;
    let rparen = tokens.matching_end(lparen)?;
    if depth0_find(tokens, text, rparen + 1, last + 1, |kind, lexeme| {
        kind == TokenKind::Keyword(Keyword::Else) || lexeme == "case"
    })
    .is_some()
    {
        return None;
    }
    let op = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        matches!(kind, TokenKind::EqEq | TokenKind::BangEq)
    })?;
    if depth0_find(tokens, text, op + 1, rparen, |kind, _| {
        matches!(kind, TokenKind::EqEq | TokenKind::BangEq)
    })
    .is_some()
    {
        return None;
    }
    let checked = if is_null_token(tokens, op + 1) {
        lparen + 1..op
    } else if op > lparen + 1 && is_null_token(tokens, op - 1) {
        op + 1..rparen
    } else {
        return None;
    };
    if checked.len() != 1 {
        return None;
    }
    // The then element: the checked identifier bare, spread, or
    // bang-postfixed.
    let element = rparen + 1..last + 1;
    let (spread, head) = match kind_of(tokens, element.start)? {
        TokenKind::PeriodPeriodPeriod => (true, element.start + 1),
        _ => (false, element.start),
    };
    let bang = element.end > head + 1
        && kind_of(tokens, element.end - 1) == Some(TokenKind::Bang)
        && kind_of(tokens, element.end - 2) != Some(TokenKind::Bang);
    let operand = if bang {
        head..element.end - 1
    } else {
        head..element.end
    };
    let name = tokens.lexeme(text, checked.start).to_owned();
    if !same_tokens(tokens, text, checked, operand) {
        return None;
    }
    let marker = if spread { "...?" } else { "?" };
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("{marker}{name}"),
    ))
}

/// `use_truncating_division`: the parenthesized division's `.truncate()`
/// wrapper folds into the `~/` operator — `(a / b).truncate()` →
/// `a ~/ b` (or `(a ~/ b)` when the surrounding context binds at
/// multiplicative strength or tighter; the official
/// `UseEffectiveIntegerDivision`'s two faces).
pub(crate) fn truncating_division(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first)? != TokenKind::OpenParen || last < first + 4 {
        return None;
    }
    let lparen = first;
    let rparen = tokens.matching_end(lparen)?;
    // `… . truncate ( )` must close the span exactly.
    if rparen + 4 != last
        || kind_of(tokens, rparen + 1)? != TokenKind::Period
        || tokens.lexeme(text, rparen + 2) != "truncate"
        || kind_of(tokens, rparen + 3)? != TokenKind::OpenParen
        || kind_of(tokens, rparen + 4)? != TokenKind::CloseParen
    {
        return None;
    }
    // The parenthesized target's depth-0 operator is a plain `/`; a `~/`
    // or a looser operator sharing the group is not this rewrite's shape.
    let slash = depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        kind == TokenKind::Slash
    })?;
    if depth0_find(tokens, text, lparen + 1, rparen, |kind, _| {
        matches!(
            kind,
            TokenKind::Question | TokenKind::Eq | TokenKind::Semicolon
        )
    })
    .is_some()
    {
        return None;
    }
    let head = token_text(tokens, text, lparen + 1..slash)?;
    let tail = token_text(tokens, text, slash + 1..rparen)?;
    let mut rebuilt = format!("{head} ~/ {tail}");
    // The wrapper parens survive only when the context binds at
    // multiplicative strength or tighter (the official's parent-precedence
    // gate): a multiplicative follower or a selector/call/index suffix
    // keeps them, as does a unary minus before; anything delimiter- or
    // lower-precedence-flanked drops them.
    let before = text
        .get(..utf16_offset_to_byte(text, input.range.start))?
        .trim_end();
    let after = text
        .get(utf16_offset_to_byte(text, input.range.end)..)?
        .trim_start();
    let unary_minus = before.ends_with('-')
        && !before.ends_with(|c: char| c.is_alphanumeric() || "_)]".contains(c));
    if after.starts_with(['*', '/', '%', '.', '(', '[']) || unary_minus {
        rebuilt = format!("({rebuilt})");
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        rebuilt,
    ))
}

/// The analyzer's binary-operator precedence (`Precedence`), or `None` for
/// tokens that never bind an expression — the boundary read the operand
/// walks use (`a + b` belongs to `a + b == true`, a `,` never does).
pub(crate) fn binary_precedence(kind: TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::Eq
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
        | TokenKind::QuestionQuestionEq => 1,
        TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod => 2,
        TokenKind::Question => 3,
        TokenKind::QuestionQuestion => 4,
        TokenKind::BarBar => 5,
        TokenKind::AmpersandAmpersand => 6,
        TokenKind::EqEq | TokenKind::BangEq => 7,
        TokenKind::Lt
        | TokenKind::LtEq
        | TokenKind::Gt
        | TokenKind::GtEq
        | TokenKind::Keyword(Keyword::Is)
        | TokenKind::Keyword(Keyword::As) => 8,
        TokenKind::Bar => 9,
        TokenKind::Caret => 10,
        TokenKind::Ampersand => 11,
        TokenKind::LtLt | TokenKind::GtGt | TokenKind::GtGtGt => 12,
        TokenKind::Plus | TokenKind::Minus => 13,
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent | TokenKind::TildeSlash => 14,
        _ => return None,
    })
}

/// Whether the token range carries a depth-0 operator binding at `max`
/// precedence or tighter — the official `needsParens(operator)` read
/// (`Precedence.forTokenType`).
pub(crate) fn binds_at_or_below(tokens: &TokenTable, range: core::ops::Range<usize>, max: u8) -> bool {
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
            _ if depth == 0 && binary_precedence(kind).is_some_and(|p| p <= max) => return true,
            _ => {}
        }
    }
    false
}

/// Whether the kind is one of the logic operators (`|`, `||`, `&`, `&&`)
/// whose literal operand the official `isPositiveCase` treats as an
/// identity candidate.
pub(crate) fn is_bool_logic(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Bar | TokenKind::BarBar | TokenKind::Ampersand | TokenKind::AmpersandAmpersand
    )
}

/// `no_literal_bool_comparisons`: the comparison keeps its non-literal
/// operand — negated when the literal's value survives the flip (`x ==
/// false` → `!x`), untouched otherwise (`x == true` → `x`), and an
/// identity operand (`x || true`, `x && false`) keeps the literal (the
/// official `ConvertToBooleanExpression`'s binary faces, parenthesized by
/// the operand's own precedence).
pub(crate) fn bool_comparison(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (literal, _) = hit_tokens(tokens, &input.range)?;
    let value = bool_literal(tokens, text, literal)?;
    let op = if kind_of(tokens, literal.wrapping_sub(1)).is_some_and(is_comparison_kind) {
        (literal - 1, false)
    } else if kind_of(tokens, literal + 1).is_some_and(is_comparison_kind) {
        (literal + 1, true)
    } else {
        return None;
    };
    let mine = binary_precedence(kind_of(tokens, op.0)?)?;
    // The other operand reaches the first separator, enclosing-delimiter
    // edge, or looser operator on the far side of the comparison.
    let other = if op.1 {
        let mut depth = 0u32;
        let mut end = op.0;
        let mut index = op.0;
        loop {
            index += 1;
            match kind_of(tokens, index)? {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Comma | TokenKind::Semicolon | TokenKind::Colon if depth == 0 => break,
                kind if depth == 0 && binary_precedence(kind).is_some_and(|p| p < mine) => break,
                _ => {}
            }
            end = index;
        }
        if end == op.0 {
            return None;
        }
        op.0 + 1..end + 1
    } else {
        let mut depth = 0u32;
        let mut start = op.0;
        let mut index = op.0;
        while index > 0 {
            index -= 1;
            match kind_of(tokens, index)? {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Comma | TokenKind::Semicolon | TokenKind::Colon if depth == 0 => break,
                kind if depth == 0 && binary_precedence(kind).is_some_and(|p| p < mine) => break,
                _ => {}
            }
            start = index;
        }
        if start == op.0 {
            return None;
        }
        start..op.0
    };
    let op_kind = kind_of(tokens, op.0)?;
    // `x || true` / `x && false` keep the literal; everything else keeps
    // the operand (negated unless the comparison is already positive).
    let identity = (value && matches!(op_kind, TokenKind::Bar | TokenKind::BarBar))
        || (!value
            && matches!(
                op_kind,
                TokenKind::Ampersand | TokenKind::AmpersandAmpersand
            ));
    let (span_first, span_last) = if literal < other.start {
        (literal, other.end - 1)
    } else {
        (other.start, literal)
    };
    let (start, length) = span_over(text, tokens, span_first, span_last)?;
    let replacement = if identity {
        tokens.lexeme(text, literal).to_owned()
    } else {
        let positive = match op_kind {
            TokenKind::EqEq => value,
            TokenKind::BangEq | TokenKind::Caret => !value,
            TokenKind::Bar
            | TokenKind::BarBar
            | TokenKind::Ampersand
            | TokenKind::AmpersandAmpersand => true,
            _ => return None,
        };
        if !positive {
            bang_wrapped(tokens, text, other)?
        } else if binds_loose(tokens, other.clone()) {
            format!("({})", token_text(tokens, text, other.clone())?)
        } else {
            token_text(tokens, text, other)?
        }
    };
    Some((start, length, replacement))
}

/// The comparison operators the rewrite anchors on: the equality family
/// (precedence 7) and the logic operators.
pub(crate) fn is_comparison_kind(kind: TokenKind) -> bool {
    binary_precedence(kind) == Some(7) || is_bool_logic(kind)
}

/// `avoid_bool_literals_in_conditional_expressions`: the conditional
/// flattens to a boolean expression (the official
/// `ConvertToBooleanExpression`'s conditional faces): equal arms keep one
/// literal, an arm literal folds into `||`/`&&` with the condition
/// negated when the arm was `false`, and `c ? false : true` becomes the
/// negated condition — each part parenthesized by the official
/// `needsParens` gates.
pub(crate) fn bool_conditional(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let (question, colon) = ternary_split(tokens, text, first, last)?;
    let cond = first..question;
    let then_arm = question + 1..colon;
    let else_arm = colon + 1..last + 1;
    let then_value = (then_arm.len() == 1).then(|| bool_literal(tokens, text, then_arm.start))?;
    let else_value = (else_arm.len() == 1).then(|| bool_literal(tokens, text, else_arm.start))?;
    let whole = (input.range.start, input.range.end - input.range.start);
    match (then_value, else_value) {
        (Some(tv), Some(ev)) => {
            if tv == ev {
                // `c ? t : t` keeps the then literal.
                let lit = tokens.lexeme(text, then_arm.start).to_owned();
                Some((whole.0, whole.1, lit))
            } else {
                // `c ? t : e` keeps the condition — the bang rides inside
                // the parens here (the official `bangBeforeParens: false`).
                let body = token_text(tokens, text, cond.clone())?;
                let negated = !tv && ev;
                let s = match (binds_loose(tokens, cond.clone()), negated) {
                    (true, true) => format!("!({body})"),
                    (true, false) => format!("({body})"),
                    (false, true) => format!("!{body}"),
                    (false, false) => body,
                };
                Some((whole.0, whole.1, s))
            }
        }
        (Some(tv), None) => {
            // `c ? t : y` folds the then literal into `||`/`&&`.
            let (op, op_prec) = if tv { ("||", 5u8) } else { ("&&", 6) };
            let negated = !tv;
            let cond_text = token_text(tokens, text, cond.clone())?;
            let cond_part = if negated {
                if binds_loose(tokens, cond.clone()) {
                    format!("!({cond_text})")
                } else {
                    format!("!{cond_text}")
                }
            } else if binds_at_or_below(tokens, cond.clone(), op_prec) {
                format!("({cond_text})")
            } else {
                cond_text
            };
            let else_text = token_text(tokens, text, else_arm.clone())?;
            let else_part = if binds_at_or_below(tokens, else_arm, op_prec) {
                format!("({else_text})")
            } else {
                else_text
            };
            Some((whole.0, whole.1, format!("{cond_part} {op} {else_part}")))
        }
        (None, Some(ev)) => {
            // `c ? x : t` folds the else literal into `||`/`&&`.
            let (op, op_prec) = if ev { ("||", 5u8) } else { ("&&", 6) };
            let negated = ev;
            let cond_text = token_text(tokens, text, cond.clone())?;
            let cond_part = if negated {
                if binds_loose(tokens, cond.clone()) {
                    format!("!({cond_text})")
                } else {
                    format!("!{cond_text}")
                }
            } else if binds_at_or_below(tokens, cond.clone(), op_prec) {
                format!("({cond_text})")
            } else {
                cond_text
            };
            let then_text = token_text(tokens, text, then_arm.clone())?;
            let then_part = if binds_at_or_below(tokens, then_arm, op_prec) {
                format!("({then_text})")
            } else {
                then_text
            };
            Some((whole.0, whole.1, format!("{cond_part} {op} {then_part}")))
        }
        (None, None) => None,
    }
}

/// `prefer_if_elements_to_conditional_expressions`: the conditional becomes
/// an if-element in place — `[a, c ? t : e]` → `[a, if (c) t else e]`
/// (the official `ConvertConditionalExpressionToIfElement` writes the
/// unparenthesized branches into the reported element; only list/set
/// element positions serve, never a map entry or an argument).
pub(crate) fn conditional_to_if_element(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    let (question, colon) = ternary_split(tokens, text, first, last)?;
    let start_b = utf16_offset_to_byte(text, input.range.start);
    let end_b = utf16_offset_to_byte(text, input.range.end);
    let before = text.get(..start_b)?.trim_end();
    let after = text.get(end_b..)?.trim_start();
    // The reported element's neighbours: a list/set element, an if-element
    // branch, or a for-element body opens at `[`/`{`/`,`/`)` and closes at
    // `,`/`]`/`}` — a spread, a map separator, or an argument never does.
    if !matches!(before.as_bytes().last(), Some(b'[' | b'{' | b',' | b')')) {
        return None;
    }
    if !matches!(after.as_bytes().first(), Some(b',' | b']' | b'}')) {
        return None;
    }
    let unparen = |mut a: usize, mut b: usize| -> (usize, usize) {
        while b > a + 1
            && kind_of(tokens, a) == Some(TokenKind::OpenParen)
            && kind_of(tokens, b) == Some(TokenKind::CloseParen)
            && tokens.matching_end(a) == Some(b)
        {
            a += 1;
            b -= 1;
        }
        (a, b)
    };
    let (cf, cl) = unparen(first, question - 1);
    let (tf, tl) = unparen(question + 1, colon - 1);
    let (ef, el) = unparen(colon + 1, last);
    let cond = token_text(tokens, text, cf..cl + 1)?;
    let then = token_text(tokens, text, tf..tl + 1)?;
    let els = token_text(tokens, text, ef..el + 1)?;
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("if ({cond}) {then} else {els}"),
    ))
}

/// `use_full_hex_values_for_flutter_colors`: the short hex literal is
/// padded to eight digits — plain literals re-render from the value
/// (`0xFF` → `0x000000ff`), separator-carrying literals pad in place and
/// the `aa_bb_cc` triplet shape gains a `00_` group (the official
/// `ReplaceWithEightDigitHex`'s three faces; the prefix case is kept).
pub(crate) fn eight_digit_hex(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let slice = slice16(input.text, &input.range)?;
    let prefix = if let Some(rest) = slice.strip_prefix("0x") {
        ("0x", rest)
    } else if let Some(rest) = slice.strip_prefix("0X") {
        ("0X", rest)
    } else {
        return None;
    };
    let digits_raw = prefix.1;
    let stripped: String = digits_raw.chars().filter(|c| *c != '_').collect();
    if stripped.is_empty() || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let body = if digits_raw.contains('_') {
        let triplet = digits_raw.len() == 8
            && digits_raw.bytes().enumerate().all(|(i, b)| match i {
                2 | 5 => b == b'_',
                _ => b.is_ascii_hexdigit(),
            });
        if triplet {
            format!("00_{digits_raw}")
        } else {
            let pad = 8usize.checked_sub(stripped.len())?;
            format!("{}{digits_raw}", "0".repeat(pad))
        }
    } else {
        let value = u64::from_str_radix(&stripped, 16).ok()?;
        format!("{value:08x}")
    };
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("{}{}", prefix.0, body),
    ))
}

/// `prefer_generic_function_type_aliases`: the old-form `typedef` (no `=`)
/// rebuilds as a generic function type — `typedef void F<T>(int x);` →
/// `typedef F<T> = void Function(int x);` — the name and written tail
/// carried verbatim (the official `ConvertToGenericFunctionSyntax`'s alias
/// face over `[typedef.next, semicolon)`; an untyped parameter refuses).
pub(crate) fn generic_alias_syntax(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let first = token_at(tokens, input.range.start)?;
    // The `typedef` keyword walks back over an optional return type
    // (`typedef void F(…)`); anything else between refuses.
    let mut typedef = first;
    loop {
        if typedef == 0 {
            return None;
        }
        if tokens.lexeme(text, typedef - 1) == "typedef" {
            break;
        }
        typedef -= 1;
        match kind_of(tokens, typedef)? {
            TokenKind::Question
            | TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Period => {}
            kind if matches!(kind, TokenKind::Keyword(_)) || wordish(tokens, text, typedef) => {}
            _ => return None,
        }
        if first - typedef > 16 {
            return None;
        }
    }
    // The name carries optional type parameters, then the parameter list
    // closes straight into the `;` (the written tail is verbatim).
    let mut tp_end = first;
    if kind_of(tokens, first + 1) == Some(TokenKind::Lt) {
        let mut angle = 0u32;
        let mut index = first + 1;
        loop {
            match kind_of(tokens, index)? {
                TokenKind::Lt => angle += 1,
                TokenKind::Gt => angle -= 1,
                TokenKind::GtGt => angle -= 2,
                TokenKind::GtGtGt => angle -= 3,
                _ => {}
            }
            if angle == 0 {
                tp_end = index;
                break;
            }
            index += 1;
        }
    }
    if kind_of(tokens, tp_end + 1)? != TokenKind::OpenParen {
        return None;
    }
    let params_open = tp_end + 1;
    let params_close = tokens.matching_end(params_open)?;
    if kind_of(tokens, params_close + 1)? != TokenKind::Semicolon {
        return None;
    }
    // Every parameter reads as typed (the official gate).
    let mut start = params_open + 1;
    let mut typed = true;
    let mut index = params_open + 1;
    loop {
        match kind_of(tokens, index)? {
            TokenKind::Comma if depth_of(tokens, params_open + 1, index)? == 0 => {
                typed &= parameter_typed(tokens, text, start, index)?;
                start = index + 1;
            }
            TokenKind::Semicolon => break,
            _ => {}
        }
        index += 1;
    }
    typed &= parameter_typed(tokens, text, start, params_close)?;
    if !typed {
        return None;
    }
    // `{name}{<T>} = {returnType }Function{parameters}` over the written
    // tail.
    let name = token_text(tokens, text, first..tp_end + 1)?;
    let return_type = if typedef == first {
        String::new()
    } else {
        format!("{} ", token_text(tokens, text, typedef..first)?)
    };
    let parameters = token_text(tokens, text, params_open..params_close + 1)?;
    // The written tail from the `typedef` walk's landing token — the
    // return type's first token when one is spelled, the name otherwise
    // (`[typedef.next, semicolon)`).
    let (start, length) = span_over(text, tokens, typedef, params_close)?;
    Some((
        start,
        length,
        format!("{name} = {return_type}Function{parameters}"),
    ))
}

/// `use_function_type_syntax_for_parameters`: the old-style function-typed
/// parameter rebuilds as a `Function`-typed one — `void cb(int x)` →
/// `void Function(int x) cb`, `required covariant this.cb()?` kept in
/// order (the official `ConvertToGenericFunctionSyntax`'s parameter face
/// over the whole parameter; an untyped inner parameter refuses).
pub(crate) fn function_type_syntax(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (mut first, last) = hit_tokens(tokens, &input.range)?;
    // Leading modifiers and receiver prefixes, in their written order.
    let mut required = false;
    let mut covariant = false;
    loop {
        match tokens.lexeme(text, first) {
            "required" if !required && !covariant => {
                required = true;
                first += 1;
            }
            "covariant" if !covariant => {
                covariant = true;
                first += 1;
            }
            _ => break,
        }
    }
    let mut prefix = "";
    if (tokens.lexeme(text, first) == "this" || tokens.lexeme(text, first) == "super")
        && kind_of(tokens, first + 1) == Some(TokenKind::Period)
    {
        prefix = if tokens.lexeme(text, first) == "this" {
            "this."
        } else {
            "super."
        };
        first += 2;
    }
    // The suffix group closes at `last`, optionally `?`-null-marked.
    let body_end = if kind_of(tokens, last) == Some(TokenKind::Question) {
        last - 1
    } else {
        last
    };
    if kind_of(tokens, body_end)? != TokenKind::CloseParen {
        return None;
    }
    // The matching `(` walks back over the group body only.
    let mut depth = 0u32;
    let mut open = None;
    let mut index = body_end;
    while index > first {
        index -= 1;
        match kind_of(tokens, index)? {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen => {
                if depth == 0 {
                    open = Some(index);
                    break;
                }
                depth -= 1;
            }
            TokenKind::OpenSquare | TokenKind::OpenCurly => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let open = open?;
    // Optional type parameters sit between the name and the group.
    let mut name_idx = open - 1;
    if kind_of(tokens, name_idx)? == TokenKind::Gt {
        let mut angle = 0i32;
        loop {
            match kind_of(tokens, name_idx)? {
                TokenKind::Gt => angle += 1,
                TokenKind::GtGt => angle += 2,
                TokenKind::GtGtGt => angle += 3,
                TokenKind::Lt => angle -= 1,
                _ => {}
            }
            if angle == 0 {
                break;
            }
            name_idx -= 1;
        }
        name_idx -= 1;
    }
    if !wordish(tokens, text, name_idx) || name_idx < first {
        return None;
    }
    // The inner parameters all read as typed (the official gate).
    let mut start = open + 1;
    let mut typed = true;
    let mut index = open + 1;
    while index < body_end {
        if kind_of(tokens, index) == Some(TokenKind::Comma)
            && depth_of(tokens, open + 1, index)? == 0
        {
            typed &= parameter_typed(tokens, text, start, index)?;
            start = index + 1;
        }
        index += 1;
    }
    typed &= parameter_typed(tokens, text, start, body_end)?;
    if !typed {
        return None;
    }
    // `{required }{covariant }{returnType }Function{<T>}{(params)}{?} {this.|super.}{name}`.
    let return_type = if name_idx > first {
        format!("{} ", token_text(tokens, text, first..name_idx)?)
    } else {
        String::new()
    };
    let has_tp = matches!(kind_of(tokens, name_idx + 1), Some(TokenKind::Lt));
    let type_params = if has_tp {
        token_text(tokens, text, name_idx + 1..open)?
    } else {
        String::new()
    };
    let parameters = token_text(tokens, text, open..body_end + 1)?;
    let question = if body_end != last { "?" } else { "" };
    let name = tokens.lexeme(text, name_idx).to_owned();
    let head = format!(
        "{}{}{}Function{type_params}{parameters}{question} ",
        if required { "required " } else { "" },
        if covariant { "covariant " } else { "" },
        return_type,
    );
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("{head}{prefix}{name}"),
    ))
}

/// `require_trailing_commas`: the comma lands right after the group's last
/// item — the token before the reported closer, whose end is the insertion
/// point (the official `AddTrailingComma` inserts at `lastNode.end`,
/// comments and all).
pub(crate) fn add_missing_trailing_comma(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    match kind_of(tokens, first)? {
        TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {}
        _ => return None,
    }
    match kind_of(tokens, first.checked_sub(1)?)? {
        // An empty group has no last item; an existing comma never reports.
        TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::Comma => {
            return None;
        }
        _ => {}
    }
    let (start, length) = span_over(text, tokens, first - 1, first - 1)?;
    Some((start + length, 0, ",".to_owned()))
}

/// `avoid_multiple_declarations_per_line`: the declaration list splits —
/// each declarator gains `;` plus a same-indent copy of the modifiers and
/// type (the official `SplitMultipleDeclarations` between-declarator
/// replacements, folded over the written span; comments, metadata, and
/// `for`-part lists refuse).
pub(crate) fn split_declarations(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (hit, _) = hit_tokens(tokens, &input.range)?;
    // The statement's first token: back past initializers and separators
    // to the boundary (`;`, a block edge); a bare `(` means a `for`-part
    // list, which never splits.
    let mut depth = 0u32;
    let mut stmt = hit;
    while stmt > 0 {
        stmt -= 1;
        match kind_of(tokens, stmt)? {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen | TokenKind::OpenSquare => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            TokenKind::OpenCurly | TokenKind::Semicolon if depth == 0 => break,
            TokenKind::OpenCurly | TokenKind::Semicolon => depth -= 1,
            _ => {}
        }
    }
    stmt += 1;
    // The list runs to its depth-0 `;`, splitting at depth-0 commas.
    let mut commas = Vec::new();
    depth = 0;
    let mut index = stmt;
    let semi = loop {
        match kind_of(tokens, index)? {
            TokenKind::Semicolon if depth == 0 => break index,
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => commas.push(index),
            _ => {}
        }
        index += 1;
    };
    if commas.is_empty() || commas[0] >= hit {
        return None;
    }
    // The first declarator's name ends the head: the token before the
    // first separator-or-initializer marker.
    let mut marker = stmt;
    loop {
        match kind_of(tokens, marker)? {
            TokenKind::Comma | TokenKind::Eq => break,
            TokenKind::Semicolon => return None,
            _ => {}
        }
        marker += 1;
    }
    if marker == stmt || !wordish(tokens, text, marker - 1) {
        return None;
    }
    let name_b = utf16_offset_to_byte(text, tokens.get(marker - 1)?.utf16_offset);
    let head_b = utf16_offset_to_byte(text, tokens.get(stmt)?.utf16_offset);
    if name_b <= head_b {
        return None;
    }
    // Comments anywhere in the written span refuse (the official
    // preceding-comments gate, read over the whole list).
    let semi_b = utf16_offset_to_byte(text, tokens.get(semi)?.utf16_offset);
    let span = text.get(head_b..semi_b)?;
    if span.contains("//") || span.contains("/*") {
        return None;
    }
    // The split text: the written head and first declarator stay verbatim
    // (up to the first separator); every further declarator gains `;`, the
    // line indent, and a verbatim copy of the head.
    let indent_start = text[..head_b].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[indent_start..head_b];
    if !indent.bytes().all(|b| matches!(b, b' ' | b'\t')) {
        return None;
    }
    let eol = {
        let nl = text
            .get(semi_b..)?
            .find('\n')
            .map_or(semi_b, |i| semi_b + i);
        if nl > 0 && text.as_bytes().get(nl - 1) == Some(&b'\r') {
            "\r\n"
        } else {
            "\n"
        }
    };
    let head_raw = text
        .get(head_b..name_b)?
        .trim_end_matches([' ', '\t'])
        .to_owned();
    let mut rebuilt = text
        .get(head_b..utf16_offset_to_byte(text, tokens.get(commas[0])?.utf16_offset))?
        .trim_end()
        .to_owned();
    for window in commas.windows(2) {
        let seg_b = utf16_offset_to_byte(text, tokens.get(window[0] + 1)?.utf16_offset);
        let seg_end = utf16_offset_to_byte(text, tokens.get(window[1])?.utf16_offset);
        let seg = text.get(seg_b..seg_end)?.trim();
        if seg.is_empty() || !seg.chars().next().is_some_and(is_identifier_char) {
            return None;
        }
        rebuilt.push_str(&format!(";{eol}{indent}{head_raw} {seg}"));
    }
    let last_seg_b =
        utf16_offset_to_byte(text, tokens.get(commas[commas.len() - 1] + 1)?.utf16_offset);
    let tail = text
        .get(last_seg_b..utf16_offset_to_byte(text, tokens.get(semi)?.utf16_offset))?
        .trim();
    if tail.is_empty() || !tail.chars().next().is_some_and(is_identifier_char) {
        return None;
    }
    rebuilt.push_str(&format!(";{eol}{indent}{head_raw} {tail}"));
    let start = tokens.get(stmt)?.utf16_offset;
    Some((start, tokens.get(semi)?.utf16_offset - start, rebuilt))
}

/// Which list the removed item sits in — arguments read a `name:` step-up
/// and never meet delimiters; parameters walk back over their type run and
/// meet the `[…]`/`{…}` groups.
#[derive(Clone, Copy)]
pub(crate) enum ItemMode {
    Argument,
    Parameter,
}

/// The shared list-item removal (the official `RemoveArgument`'s
/// `nodeInList` face and `RemoveUnusedParameter`'s parameter faces): the
/// item goes with one adjacent comma — the comma before it among kind
/// peers, the comma after it when it opens the list — and the first item
/// in a `[…]`/`{…}` group takes the delimiters with it when it is also the
/// last (an optional `this.x` field formal additionally folds its default
/// into a fresh initializer when the constructor has none).
pub(crate) fn remove_listed_item(input: &FixInput<'_>, mode: ItemMode) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (mut first, last) = hit_tokens(tokens, &input.range)?;
    if matches!(mode, ItemMode::Argument)
        && first >= 3
        && kind_of(tokens, first - 1) == Some(TokenKind::Colon)
        && wordish(tokens, text, first - 2)
        && matches!(
            kind_of(tokens, first - 3),
            Some(TokenKind::OpenParen | TokenKind::Comma)
        )
    {
        // `stepUpNamedExpression`: the whole `name: value` is the argument.
        first -= 2;
    }
    if matches!(mode, ItemMode::Parameter) && first == last && wordish(tokens, text, first) {
        // The reported name walks back over its type run (`covariant
        // List<int>? x`, `this.x`).
        while first > 0 {
            match kind_of(tokens, first - 1)? {
                TokenKind::Question | TokenKind::Period => first -= 1,
                TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                    let mut angle = 0i32;
                    loop {
                        first -= 1;
                        if first == 0 {
                            return None;
                        }
                        match kind_of(tokens, first)? {
                            TokenKind::Gt => angle += 1,
                            TokenKind::GtGt => angle += 2,
                            TokenKind::GtGtGt => angle += 3,
                            TokenKind::Lt => {
                                angle -= 1;
                                if angle == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                kind if wordish(tokens, text, first - 1)
                    || matches!(kind, TokenKind::Keyword(_)) =>
                {
                    first -= 1;
                }
                _ => break,
            }
        }
    }
    // Back to the previous comma, the delimiter opening, or the list's `(`.
    let mut depth = 0u32;
    let mut prev_comma = None;
    let mut delimiter = None;
    let mut paren = None;
    let mut index = first;
    while index > 0 {
        index -= 1;
        match kind_of(tokens, index)? {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenSquare | TokenKind::OpenCurly if depth == 0 => {
                delimiter = Some(index);
                break;
            }
            TokenKind::Comma if depth == 0 => {
                prev_comma = Some(index);
                break;
            }
            TokenKind::OpenParen if depth == 0 => {
                paren = Some(index);
                break;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth -= 1,
            _ => {}
        }
    }
    // A delimiter before the previous comma makes that peer optional too.
    let mut prev_optional = false;
    if prev_comma.is_some() {
        let mut d = 0u32;
        loop {
            if index == 0 {
                return None;
            }
            index -= 1;
            match kind_of(tokens, index)? {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => d += 1,
                TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if d == 0 {
                        prev_optional = true;
                        break;
                    }
                    d -= 1;
                }
                TokenKind::OpenParen => {
                    if d == 0 {
                        paren = Some(index);
                        break;
                    }
                    d -= 1;
                }
                _ => {}
            }
        }
    }
    // The item must sit inside a `(`-list; nothing past the gate reads it.
    let _paren = paren?;
    let item_optional = matches!(mode, ItemMode::Parameter)
        && (delimiter.is_some() || (prev_comma.is_some() && prev_optional));
    // Forward to the next boundary: the item's own groups ride in the
    // residual depth, the delimiters' closers are skipped, and the first
    // depth-0 comma or the list's `)` ends the scan.
    let mut depth = 0u32;
    for at in first..=last {
        match kind_of(tokens, at)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    let mut next_comma = None;
    let mut closer = None;
    let mut index = last;
    loop {
        index += 1;
        match kind_of(tokens, index)? {
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
            TokenKind::CloseSquare | TokenKind::CloseCurly if depth == 0 && item_optional => {}
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                if depth == 0 {
                    closer = Some(index);
                    break;
                }
                depth -= 1;
            }
            TokenKind::Comma if depth == 0 => {
                next_comma = Some(index);
                break;
            }
            _ => {}
        }
    }
    let closer = closer?;
    let item_start = tokens.get(first)?.utf16_offset;
    let (span_start, span_len) = span_over(text, tokens, first, last)?;
    let item_end = span_start + span_len;
    let (start, length) = if let Some(pc) = prev_comma {
        let (prev_start, prev_len) = span_over(text, tokens, pc - 1, pc - 1)?;
        let prev_end = prev_start + prev_len;
        if item_optional && !prev_optional {
            match next_comma {
                // First optional item with a peer behind: delimiters stay.
                Some(nx) => (item_start, tokens.get(nx)?.utf16_offset - item_start),
                // Last inside the delimiters: they go with it (a trailing
                // comma before the `)` survives).
                None => {
                    let mut trailing = closer;
                    if kind_of(tokens, closer - 1) == Some(TokenKind::Comma) {
                        trailing = closer - 1;
                    }
                    (prev_end, tokens.get(trailing)?.utf16_offset - prev_end)
                }
            }
        } else {
            (prev_end, item_end - prev_end)
        }
    } else {
        match next_comma {
            Some(nx) => (item_start, tokens.get(nx)?.utf16_offset - item_start),
            // Sole item: when a delimiter wraps it, the delimiters go too.
            None => {
                let face = match delimiter {
                    Some(d) if item_optional => tokens.get(d)?.utf16_offset,
                    _ => item_start,
                };
                (face, tokens.get(closer)?.utf16_offset - face)
            }
        }
    };
    // An optional `this.x` formal folds its default into a fresh
    // initializer when the constructor carries none yet.
    if matches!(mode, ItemMode::Parameter)
        && item_optional
        && next_comma.is_none()
        && tokens.lexeme(text, first) == "this"
        && kind_of(tokens, first + 1) == Some(TokenKind::Period)
        && matches!(
            kind_of(tokens, closer + 1),
            Some(TokenKind::OpenCurly | TokenKind::Semicolon)
        )
    {
        let name = tokens.lexeme(text, first + 2);
        let default = match depth0_find(tokens, text, first, last + 1, |kind, _| {
            kind == TokenKind::Eq
        }) {
            Some(eq) => token_text(tokens, text, eq + 1..last + 1)?,
            None => "null".to_owned(),
        };
        let (closer_start, closer_len) = span_over(text, tokens, closer, closer)?;
        return Some((
            start,
            closer_start + closer_len - start,
            format!(") : {name} = {default}"),
        ));
    }
    Some((start, length, String::new()))
}

/// `avoid_redundant_argument_values`: the redundant argument leaves its
/// list (the official `RemoveArgument`).
pub(crate) fn remove_argument_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    remove_listed_item(input, ItemMode::Argument)
}

/// `avoid_unused_constructor_parameters`: the unused parameter leaves the
/// constructor's list (the official `RemoveUnusedParameter`).
pub(crate) fn remove_parameter_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    remove_listed_item(input, ItemMode::Parameter)
}

/// `prefer_iterable_wheretype`: `x.where((e) => e is T)` becomes
/// `x.whereType<T>()` (the official `ConvertToWhereType` over the reported
/// method name: one parameter, an `=>` body or a single return whose `is`
/// test applies the parameter, and no `is!`).
pub(crate) fn where_type_entry(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    if !word_at(tokens, text, first, "where") {
        return None;
    }
    if kind_of(tokens, first + 1)? != TokenKind::OpenParen {
        return None;
    }
    let args_open = first + 1;
    let args_close = tokens.matching_end(args_open)?;
    // Exactly one argument: no depth-0 comma inside the list.
    if depth0_find(tokens, text, args_open + 1, args_close, |kind, _| {
        kind == TokenKind::Comma
    })
    .is_some()
    {
        return None;
    }
    // The argument is a function expression — `(params) => body` or
    // `(params) { return body; }` — whose parameter names the tested
    // expression.
    if kind_of(tokens, args_open + 1)? != TokenKind::OpenParen {
        return None;
    }
    let params_open = args_open + 1;
    let params_close = tokens.matching_end(params_open)?;
    if params_close + 1 >= args_close {
        return None;
    }
    if !wordish(tokens, text, params_close - 1) {
        return None;
    }
    let param_name = tokens.lexeme(text, params_close - 1);
    let (body_first, body_last) = if tokens.lexeme(text, params_close + 1) == "=>" {
        (params_close + 2, args_close - 1)
    } else if kind_of(tokens, params_close + 1)? == TokenKind::OpenCurly
        && tokens.lexeme(text, params_close + 2) == "return"
        && kind_of(tokens, args_close - 1)? == TokenKind::CloseCurly
        && kind_of(tokens, args_close - 2)? == TokenKind::Semicolon
    {
        (params_close + 3, args_close - 3)
    } else {
        return None;
    };
    if body_first > body_last
        || tokens.lexeme(text, body_first) != param_name
        || kind_of(tokens, body_first + 1)? != TokenKind::Keyword(Keyword::Is)
    {
        return None;
    }
    if kind_of(tokens, body_first + 2) == Some(TokenKind::Bang) {
        return None;
    }
    let ty = token_text(tokens, text, body_first + 2..body_last + 1)?;
    if ty.is_empty() {
        return None;
    }
    let (start, length) = span_over(text, tokens, first, args_close)?;
    Some((start, length, format!("whereType<{ty}>()")))
}

/// `avoid_types_as_parameter_names`: the reported catch-clause name `T`
/// converts the clause to `on T` — `catch (T)` → `on T`,
/// `catch (T, s)` → `on T catch (_, s)` (the official `ConvertToOnType`;
/// only the catch face serves — the formal-parameter and type-parameter
/// faces carry a different diagnostic code upstream and the producer
/// drops them).
pub(crate) fn catch_to_on_type(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last || !wordish(tokens, text, first) {
        return None;
    }
    let name = tokens.lexeme(text, first);
    // Walk back: `(` then `catch`.
    let lparen = first.checked_sub(1)?;
    if kind_of(tokens, lparen) != Some(TokenKind::OpenParen) {
        return None;
    }
    let catch_kw = lparen.checked_sub(1)?;
    if kind_of(tokens, catch_kw) != Some(TokenKind::Keyword(Keyword::Catch)) {
        return None;
    }
    let rparen = tokens.matching_end(lparen)?;
    let next = first + 1;
    if next == rparen {
        // `catch (T)` — no stack-trace parameter.
        let (start16, length16) = span_over(text, tokens, catch_kw, rparen)?;
        return Some((start16, length16, format!("on {name}")));
    }
    // `catch (T, s)` — the comma sits at depth 0 between the name and the
    // stack-trace parameter; `startStart(catchKeyword,
    // stackTraceParameter)` keeps the parameter and `)` untouched.
    if kind_of(tokens, next) != Some(TokenKind::Comma) {
        return None;
    }
    let stack = next + 1;
    if stack >= rparen || !wordish(tokens, text, stack) || stack + 1 != rparen {
        return None;
    }
    let (start16, _) = span_over(text, tokens, catch_kw, first)?;
    let end16 = tokens.get(stack)?.utf16_offset;
    Some((
        start16,
        end16.checked_sub(start16)?,
        format!("on {name} catch (_, "),
    ))
}

/// `empty_container_bodies`: the reported `{}` pair (whitespace-only gap)
/// plus the gap before it fold into `;` — the official
/// `RemoveEmptyContainerBody` replaces `[prevToken.end, node.end)` with
/// `;`, the whitespace ride included.
pub(crate) fn drop_empty_container_body(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let slice = slice16(text, &input.range)?;
    let bytes = slice.as_bytes();
    if bytes.first() != Some(&b'{') || bytes.last() != Some(&b'}') {
        return None;
    }
    let inner = slice.get(1..slice.len() - 1)?;
    if !inner.chars().all(char::is_whitespace) {
        return None;
    }
    // `prevToken.end`: the last non-whitespace byte before the `{` — the
    // gap walks back over newlines too.
    let start_byte = utf16_offset_to_byte(text, input.range.start);
    let prefix = &text[..start_byte];
    let prev_end = prefix.trim_end().len();
    let end_byte = utf16_offset_to_byte(text, input.range.end);
    let start16 = byte_offset_to_utf16(text, prev_end);
    let length16 = byte_offset_to_utf16(text, end_byte).checked_sub(start16)?;
    Some((start16, length16, ";".to_owned()))
}

/// `prefer_spread_collections`: the `..addAll(arg)` section on a list
/// literal folds into one spread element — `...arg`, `...?x` for an
/// `x ?? []` argument, `if (c) ...t` for a `c ? t : []` conditional
/// (the official `ConvertAddAllToSpread` non-inline face; the list-literal
/// argument is `prefer_inlined_adds`'s face and never reaches this lint).
pub(crate) fn spread_addition(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (name_index, _) = hit_tokens(tokens, &input.range)?;
    if tokens.lexeme(text, name_index) != "addAll"
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
    // The argument is NOT a list literal (that face belongs to
    // `prefer_inlined_adds`); the lint's own gate guarantees it, and the
    // spread producer's `_computeElementText` reads the shape.
    if kind_of(tokens, lparen + 1) == Some(TokenKind::OpenSquare) {
        return None;
    }
    // Backwards: the cascade marker, then the target literal's `]`.
    match kind_of(tokens, name_index.checked_sub(1)?) {
        Some(TokenKind::PeriodPeriod | TokenKind::QuestionPeriodPeriod) => {}
        _ => return None,
    }
    let close_square = name_index - 2;
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
    // `_computeElementText`: `a ?? []` → `...?a`; `c ? t : []` →
    // `if (c) ...t`; everything else → `...<arg>`.
    let element = spread_element_text(tokens, text, lparen + 1, rparen)?;
    let target_nonempty = close_square > open_square + 1;
    let (start16, length16) = span_over(text, tokens, close_square, rparen)?;
    let replacement = if close_square == open_square {
        // `[]..addAll(x)` — the single `[]` token rewrites whole.
        format!("[{element}]")
    } else {
        format!("{}{}]", if target_nonempty { ", " } else { "" }, element)
    };
    Some((start16, length16, replacement))
}

/// The composed spread-element source of the `addAll` argument at
/// `arg_start..arg_end` (`_computeElementText`): `x ?? []` → `...?x`,
/// `c ? t : []` → `if (c) ...t`, else `...<arg>`. The empty-list test
/// reads the literal's token shape (`[]`, `const []`, `<T>[]`).
pub(crate) fn spread_element_text(
    tokens: &TokenTable,
    text: &str,
    arg_start: usize,
    arg_end: usize,
) -> Option<String> {
    let argument = token_text(tokens, text, arg_start..arg_end)?;
    if argument.is_empty() {
        return None;
    }
    // `a ?? []` at depth 0.
    if let Some(op) = depth0_find(tokens, text, arg_start, arg_end, |kind, _| {
        kind == TokenKind::QuestionQuestion
    }) && empty_list_literal(tokens, op + 1, arg_end)
    {
        let left = token_text(tokens, text, arg_start..op)?;
        return Some(format!("...?{left}"));
    }
    // `c ? t : []` at depth 0.
    if let Some(question) = depth0_find(tokens, text, arg_start, arg_end, |kind, _| {
        kind == TokenKind::Question
    }) && let Some(colon) = depth0_find(tokens, text, question + 1, arg_end, |kind, _| {
        kind == TokenKind::Colon
    }) && empty_list_literal(tokens, colon + 1, arg_end)
    {
        let condition = token_text(tokens, text, arg_start..question)?;
        let then = token_text(tokens, text, question + 1..colon)?;
        return Some(format!("if ({condition}) ...{then}"));
    }
    Some(format!("...{argument}"))
}

/// The token range is exactly one empty list literal — `[]`, `<T>[]`, or
/// `const`/`const <T>` led (the official `isEmptyListLiteral` face).
pub(crate) fn empty_list_literal(tokens: &TokenTable, mut start: usize, end: usize) -> bool {
    if kind_of(tokens, start) == Some(TokenKind::Keyword(Keyword::Const)) {
        start += 1;
    }
    if kind_of(tokens, start) == Some(TokenKind::Lt) {
        let Some(close) = angle_group_end(tokens, start) else {
            return false;
        };
        start = close + 1;
    }
    kind_of(tokens, start) == Some(TokenKind::Index) && start + 1 == end
}

/// `simplify_variable_pattern`: the reported pattern-field name drops —
/// `isEven: var isEven` keeps `: var isEven` (the official
/// `RemoveUnnecessaryName` deletes `range.token(name)` — the name token
/// alone; the `:` and the variable pattern survive).
pub(crate) fn drop_pattern_field_name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last || !wordish(tokens, text, first) {
        return None;
    }
    // The field name sits immediately before its `:` — verify the pair,
    // and that the variable pattern's own name repeats it (`var`/`final`
    // then the same identifier).
    if kind_of(tokens, first + 1) != Some(TokenKind::Colon) {
        return None;
    }
    let name = tokens.lexeme(text, first);
    let mut cursor = first + 2;
    if matches!(tokens.lexeme(text, cursor), "var" | "final") {
        cursor += 1;
    }
    if tokens.lexeme(text, cursor) != name {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        String::new(),
    ))
}

/// `type_literal_in_constant_pattern`: the reported constant pattern
/// wraps in `const (…)` (the official `ConvertToConstantPattern`
/// insertions at `constantPattern.offset`/`end` folded into one
/// replacement — the first-listed producer in the registry pair wins).
pub(crate) fn const_pattern_literal(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let inner = slice16(input.text, &input.range)?;
    let inner = inner.trim();
    if inner.is_empty() || inner.starts_with("const") {
        return None;
    }
    // A type-literal shape: a dotted-identifier run, optional type
    // arguments, optional `?` tail. Anything wider (a real expression)
    // is not this lint's face.
    let bytes = inner.as_bytes();
    let mut pos = 0usize;
    loop {
        // One identifier segment.
        let id_start = pos;
        while pos < bytes.len() && is_identifier_char(bytes[pos] as char) {
            pos += 1;
        }
        if pos == id_start {
            return None;
        }
        if bytes[pos..].starts_with(b"<") {
            // Type arguments: balance `<`/`>` over the run.
            let mut depth = 0u32;
            let mut i = pos;
            while i < bytes.len() {
                match bytes[i] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if depth != 0 {
                return None;
            }
            pos = i + 1;
        }
        if bytes[pos..].starts_with(b".") {
            pos += 1;
            continue;
        }
        break;
    }
    if bytes[pos..].starts_with(b"?") {
        pos += 1;
    }
    if pos != bytes.len() {
        return None;
    }
    let (start, length) = (input.range.start, input.range.end - input.range.start);
    Some((start, length, format!("const ({inner})")))
}

/// `unnecessary_lambdas`: the reported lambda reduces to its callee's
/// tear-off — `(x) => f(x)` → `f`, `(x) { return f(x); }` → `f`; a
/// `new`/`const`-marked creation or a dotted `C.name(...)` head appends
/// `.new` when the constructor is unnamed (the official
/// `ReplaceWithTearOff`). The bare `C(...)` single-name head is
/// ambiguous between function and unnamed-constructor calls and drops —
/// the invocation face writes `C`, the creation face `C.new`.
pub(crate) fn tear_off_lambda(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first) != Some(TokenKind::OpenParen) {
        return None;
    }
    let params_close = tokens.matching_end(first)?;
    // `=> expr` or `{ expr; }` / `{ return expr; }`.
    let (expr_first, expr_last) = match kind_of(tokens, params_close + 1)? {
        TokenKind::Function => (params_close + 2, last),
        TokenKind::OpenCurly => {
            let block_close = tokens.matching_end(params_close + 1)?;
            if block_close != last {
                return None;
            }
            let mut inner_first = params_close + 2;
            if kind_of(tokens, inner_first) == Some(TokenKind::Keyword(Keyword::Return)) {
                inner_first += 1;
            }
            // `{ return expr; }` — the `;` sits before `}`.
            if kind_of(tokens, block_close - 1) != Some(TokenKind::Semicolon) {
                return None;
            }
            (inner_first, block_close - 2)
        }
        _ => return None,
    };
    if expr_first > expr_last {
        return None;
    }
    // Unwrap outer parens of the expression (`unParenthesized`).
    let mut e_first = expr_first;
    let mut e_last = expr_last;
    while kind_of(tokens, e_first) == Some(TokenKind::OpenParen)
        && tokens.matching_end(e_first) == Some(e_last)
    {
        e_first += 1;
        e_last -= 1;
    }
    // The invocation's callee: the argument list's `(` walks back to the
    // callee end — the function text is `startEnd(expression,
    // expression.function)`, the creation's constructor name plus the
    // optional `.name`.
    let mut cursor = e_first;
    // Strip a leading `new`/`const` marker (creation keywords).
    let creation_keyword = matches!(
        kind_of(tokens, cursor),
        Some(TokenKind::Keyword(Keyword::New | Keyword::Const))
    );
    if creation_keyword {
        cursor += 1;
    }
    // Find the invocation's argument-list `(`: scan the callee run — a
    // dotted identifier chain, optional type arguments, then `(`.
    let callee_start = cursor;
    let mut head = cursor;
    loop {
        match kind_of(tokens, head)? {
            TokenKind::Identifier | TokenKind::Keyword(Keyword::This) => head += 1,
            TokenKind::Period => {
                head += 1;
                if kind_of(tokens, head)? != TokenKind::Identifier {
                    return None;
                }
                head += 1;
            }
            TokenKind::Lt => {
                // Explicit type arguments on the callee — the lint bails
                // on them (:285), so none should reach here; still,
                // keep the textual shape.
                head = angle_group_end(tokens, head)? + 1;
            }
            TokenKind::OpenParen => break,
            _ => return None,
        }
    }
    if head <= callee_start || head > e_last {
        return None;
    }
    let arg_open = head;
    let arg_close = tokens.matching_end(arg_open)?;
    if arg_close != e_last {
        return None;
    }
    let callee = token_text(tokens, text, callee_start..arg_open)?;
    if callee.is_empty() {
        return None;
    }
    // Distinguish the creation head from a plain invocation: a `new` or
    // `const` keyword marks it; a bare `C` single-segment head is
    // ambiguous and drops (the constructor face would append `.new`, the
    // invocation face would not).
    if !creation_keyword && !callee.contains('.') {
        return None;
    }
    let replacement = if creation_keyword && !callee.contains('.') {
        format!("{callee}.new")
    } else {
        callee
    };
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, replacement))
}

/// `unnecessary_primary_constructor_body`: the reported `this` keyword's
/// whole `PrimaryConstructorBody` — `this(...) ;` / `this(...) {}` /
/// `this(...) : inits;` — drops by its lines range (the official
/// `RemovePrimaryConstructorBody` deletes `getLinesRange(body)`).
pub(crate) fn drop_primary_constructor_body(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first) != Some(TokenKind::Keyword(Keyword::This)) {
        return None;
    }
    // The body runs from `this` through its terminating `;` or `{}`
    // block — `this (params) ;`, `this (params) { }`, `this (params) :
    // x = e ;`. The trailing block is the end when no `;` follows.
    let end = statement_end(tokens, text, first).or_else(|| {
        // `this (params) {}` — no `;` after the block; walk the optional
        // parameter list and take the matching `}`.
        let mut cursor = first + 1;
        if kind_of(tokens, cursor) == Some(TokenKind::OpenParen) {
            cursor = tokens.matching_end(cursor)? + 1;
        }
        if kind_of(tokens, cursor) == Some(TokenKind::OpenCurly) {
            tokens.matching_end(cursor)
        } else {
            None
        }
    })?;
    let (start16, length16) = span_over(text, tokens, first, end)?;
    let (start, length) = drop_lines_range(text, start16, start16 + length16)?;
    Some((start, length, String::new()))
}

/// `unnecessary_type_name_in_constructor`: the reported type-name echo
/// drops with its `new`/`factory` keyword context — `C(...)` → `new()`,
/// `C.named(...)` → `new named(...)`, `C.new(...)` → `new()`,
/// `factory C.name(...)` → `factory name(...)` (the official
/// `RemoveTypeName` branches over `node.typeName`/`name`/`newKeyword`/
/// `factoryKeyword`).
pub(crate) fn drop_constructor_type_name(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    if !wordish(tokens, text, first) {
        return None;
    }
    // The constructor head: `typeName [ . name | . new ]` then `(`; a
    // `factory` or `new` keyword may precede the type name. The reported
    // identifier is the type name itself.
    let mut cursor = first;
    let mut name_token: Option<usize> = None;
    let mut period: Option<usize> = None;
    if kind_of(tokens, cursor + 1) == Some(TokenKind::Period) && wordish(tokens, text, cursor + 2) {
        period = Some(cursor + 1);
        name_token = Some(cursor + 2);
        cursor += 2;
    }
    if kind_of(tokens, cursor + 1) != Some(TokenKind::OpenParen) {
        return None;
    }
    // `newOrFactoryKeyword`: `factory` or `new` immediately before the
    // type name.
    let kw = first.checked_sub(1).and_then(|index| {
        let lexeme = tokens.lexeme(text, index);
        matches!(
            kind_of(tokens, index),
            Some(TokenKind::Keyword(Keyword::New))
        )
        .then_some(index)
        .or_else(|| (lexeme == "factory").then_some(index))
    });
    // The official `replacementEndToken`: `name` when it is `new` or
    // absent, else `period`.
    let end_token = match name_token {
        Some(name) if tokens.lexeme(text, name) == "new" => name,
        Some(_) => period?,
        None => first,
    };
    let (start16, length16) = span_over(text, tokens, first, end_token)?;
    let end16 = start16 + length16;
    if let Some(kw_index) = kw {
        // `factory`/`new` present: `endEnd(kw, end)` when the head is
        // unnamed and the keyword hugs the type name (`factory C()` →
        // `factory()` deletes the space too), else `startEnd(typeName,
        // end)` (official `RemoveTypeName`'s `name == null` gate).
        let kw_end = utf16_end(tokens, text, kw_index)?;
        let start = if name_token.is_none() && kw_end + 1 == input.range.start {
            kw_end
        } else {
            start16
        };
        return Some((start, end16.checked_sub(start)?, String::new()));
    }
    // No keyword: replace `[typeName.start, endToken.end)` — `new` when
    // the end token is an identifier, `new ` when it is the `.` (the
    // constructor name follows immediately).
    let replacement = if period.is_some() && name_token != Some(end_token) {
        "new "
    } else {
        "new"
    };
    Some((start16, length16, replacement.to_owned()))
}

/// `unrelated_type_equality_checks`: the reported `==`/`!=` becomes
/// `is`/`is!`; when the left operand is the `Type` literal the whole
/// binary reorders to `x is T` (the official `ReplaceWithIs` arms —
/// the left-literal arm reads the operand types' `Type` display off the
/// message).
pub(crate) fn equality_is_check(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if first != last {
        return None;
    }
    let operator = tokens.lexeme(text, first);
    let (is_op, bang) = match operator {
        "==" => ("is", ""),
        "!=" => ("is", "!"),
        _ => return None,
    };
    // The official producer edits only when an operand is a `TypeLiteral`
    // — the message's type display reads `Type` for that operand.
    let left_is_type = input.message.contains("the left operand ('Type')");
    let right_is_type = input.message.contains("the right operand ('Type')");
    if left_is_type {
        // `T == x` → `x is T` / `x is! T`: delete `T == `, append
        // ` is[!] T` after the right operand.
        let left_start = operand_start(tokens, first.checked_sub(1)?)?;
        let right_end = operand_end(tokens, first + 1)?;
        let left = token_text(tokens, text, left_start..first)?;
        let right = token_text(tokens, text, first + 1..right_end + 1)?;
        let (start16, length16) = span_over(text, tokens, left_start, right_end)?;
        return Some((start16, length16, format!("{right} {is_op}{bang} {left}")));
    }
    if !right_is_type {
        return None;
    }
    // `x == T` → `x is T` / `x is! T` — the operator alone rewrites.
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((start16, length16, format!("{is_op}{bang}")))
}

/// `extraneous_modifier` / `duplicated_modifier` /
/// `variable_pattern_keyword_in_declaration_context`: the report span is
/// the offending keyword itself, whichever it is; it goes with one
/// following unit (official `RemoveLexeme.modifier` over the
/// message-named lexeme, `RemoveVar` over the report span — the same
/// deletion).
pub(crate) fn remove_hit_lexeme(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let word = slice16(input.text, &input.range)?;
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    remove_lexeme(input.text, input.range.start, word)
}

/// `abstract_class_member`: `abstract` goes with the whole whitespace run
/// behind it (official `RemoveAbstract`'s class-member leg: the keyword
/// plus one unit, then every whitespace unit — newlines included).
pub(crate) fn remove_abstract_member(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    let start = utf16_offset_to_byte(text, input.range.start);
    let rest = text.get(start..)?.strip_prefix("abstract")?;
    if rest.starts_with(is_identifier_char) {
        return None;
    }
    let mut chars = rest.chars();
    let mut taken = chars.next().map_or(0, char::len_utf8);
    taken += chars
        .take_while(|c| c.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let length = utf16_count(text.get(start..start + "abstract".len() + taken)?);
    Some((input.range.start, length, String::new()))
}

/// `invalid_inside_unary_pattern`: the reported inner pattern wraps in
/// parentheses — the official `SurroundWithParentheses` pair of
/// insertions folded into one whole-span replacement.
pub(crate) fn surround_with_parentheses(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let inner = slice16(input.text, &input.range)?;
    if inner.is_empty() {
        return None;
    }
    Some((
        input.range.start,
        input.range.end - input.range.start,
        format!("({inner})"),
    ))
}

/// `wrong_separator_for_positional_parameter`: the `:` becomes `=`, with
/// a leading space when the colon touches the token before it (official
/// `ReplaceColonWithEquals`: `separator.offset == previous.end`).
pub(crate) fn colon_to_equals(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let text = input.text;
    if slice16(text, &input.range)? != ":" {
        return None;
    }
    let start = utf16_offset_to_byte(text, input.range.start);
    let touches = text[..start]
        .chars()
        .next_back()
        .is_some_and(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'));
    let replacement = if touches { " =" } else { "=" };
    Some((input.range.start, 1, replacement.to_owned()))
}
