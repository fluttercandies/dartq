use super::*;


/// The evidence one built-in assist producer reads: the file text, the
/// request selection (`[offset, offset + length)` in UTF-16), and the
/// parsed unit substrate (token stream plus the arena tree) built once per
/// request. Engine-free by charter: every guard verifies against the
/// text, the tokens, or the tree, and a shape that does not verify drops
/// the assist rather than approximating it.
pub struct AssistContext<'a> {
    pub text: &'a str,
    pub selection: core::ops::Range<u32>,
    pub tokens: &'a TokenTable,
    pub ast: &'a SyntaxTable,
    /// The unit's effective feature set (the parse's own posture): the
    /// language-version gates the official producers read through
    /// `isEnabled(Feature.…)` (`patterns` and friends).
    pub features: Features,
    /// The unit's path — the identity the import/part-uri producers read
    /// (`packageToRelativeImport`'s package attribution). The engine-free
    /// dispatch passes the request path through unchanged.
    pub path: &'a Path,
    /// The unit's semantic query face — mounted by the resolved transport
    /// (the `run_with_semantics` arm; the rust-analyzer assist precedent
    /// of consuming sema), `None` under the engine-free dispatch. A
    /// producer reading it stays silent when unmounted: fewer assists,
    /// never an approximated one.
    pub semantics: Option<&'a dartq_semantic::Analysis<'a>>,
}

impl AssistContext<'_> {
    /// The mounted semantic face (the `Check::semantics` arm), `None`
    /// under the engine-free dispatch.
    #[must_use]
    pub fn semantics(&self) -> Option<&dartq_semantic::Analysis<'_>> {
        self.semantics
    }
}

/// One built-in assist producer: the upstream `dart.assist.*` kind id and
/// title, the upstream `DartAssistKindPriority` number (lower sorts first,
/// the [`PluginFix::priority`] convention), and the edit producer — `None`
/// drops the assist (the official self-guarding dispatch: every generator
/// runs at every request and declines where its covering node does not
/// apply; upstream Dart SDK source,
/// pkg/analysis_server_plugin/lib/src/correction/assist_processor.dart).
pub(crate) struct AssistSpec {
    pub(crate) kind: &'static str,
    pub(crate) title: &'static str,
    pub(crate) priority: i32,
    produce: fn(&AssistContext<'_>) -> Option<Vec<(u32, u32, String)>>,
}

/// The built-in producer table, in kind order (stable within a priority —
/// the dispatch's deterministic order). Each row's producer mirrors its
/// official producer's guard and edit shapes (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/*, the file named
/// on the producer). The table grows with the F5 waves: the if/condition
/// and surround families land on this same substrate. The flutter-gated
/// families resolve `package:flutter` elements upstream and stay out
/// until a Flutter-shaped resolution is exercised over the fix face
/// (facts-blocked, not a divergence).
pub(crate) static ASSIST_TABLE: &[AssistSpec] = &[
    AssistSpec {
        kind: "dart.assist.convert.blockComment",
        title: "Convert to block documentation comment",
        priority: 30,
        produce: convert_to_block_doc,
    },
    AssistSpec {
        kind: "dart.assist.convert.toIntLiteral",
        title: "Convert to int literal",
        priority: 30,
        produce: convert_to_int_literal,
    },
    AssistSpec {
        kind: "dart.assist.convert.lineComment",
        title: "Convert to line documentation comment",
        priority: 30,
        produce: convert_to_line_doc,
    },
    AssistSpec {
        kind: "dart.assist.convert.toMultilineString",
        title: "Convert to multiline string",
        priority: 30,
        produce: convert_to_multiline_string,
    },
    AssistSpec {
        kind: "dart.assist.convert.toDoubleQuotedString",
        title: "Convert to double quoted string",
        priority: 30,
        produce: convert_quotes_to_double,
    },
    AssistSpec {
        kind: "dart.assist.convert.toSingleQuotedString",
        title: "Convert to single quoted string",
        priority: 30,
        produce: convert_quotes_to_single,
    },
    AssistSpec {
        kind: "dart.assist.add.digitSeparators",
        title: "Add digit separators",
        priority: 30,
        produce: add_digit_separators,
    },
    AssistSpec {
        kind: "dart.assist.remove.digitSeparators",
        title: "Remove digit separators",
        priority: 30,
        produce: remove_digit_separators,
    },
    AssistSpec {
        kind: "dart.assist.convert.isNot",
        title: "Convert to 'is!'",
        priority: 30,
        produce: convert_to_is_not,
    },
    AssistSpec {
        kind: "dart.assist.convert.isNotEmpty",
        title: "Convert to 'isNotEmpty'",
        priority: 30,
        produce: convert_to_is_not_empty,
    },
    AssistSpec {
        kind: "dart.assist.convert.toNullAware",
        title: "Convert to use '?.'",
        priority: 30,
        produce: convert_to_null_aware,
    },
    AssistSpec {
        kind: "dart.assist.exchangeOperands",
        title: "Exchange operands",
        priority: 30,
        produce: exchange_operands,
    },
    AssistSpec {
        kind: "dart.assist.invertConditional",
        title: "Invert conditional expression",
        priority: 30,
        produce: invert_conditional,
    },
    AssistSpec {
        kind: "dart.assist.invertIf",
        title: "Invert 'if' statement",
        priority: 30,
        produce: invert_if,
    },
    AssistSpec {
        kind: "dart.assist.splitIfConjunction",
        title: "Split '&&' condition",
        priority: 30,
        produce: split_and_condition,
    },
    AssistSpec {
        kind: "dart.assist.convert.conditionalToIfElse",
        title: "Replace conditional with 'if-else'",
        priority: 30,
        produce: conditional_to_if_else,
    },
    AssistSpec {
        kind: "dart.assist.convert.ifElseToConditional",
        title: "Replace 'if-else' with conditional ('c ? x : y')",
        priority: 30,
        produce: if_else_to_conditional,
    },
    AssistSpec {
        kind: "dart.assist.inlineElseBlock",
        title: "Join 'else' block with inner 'if' statement",
        priority: 30,
        produce: inline_else_block,
    },
    AssistSpec {
        kind: "dart.assist.inlineEnclosingElseBlock",
        title: "Join 'if' statement with outer 'else' block",
        priority: 30,
        produce: inline_enclosing_else_block,
    },
    AssistSpec {
        kind: "dart.assist.joinWithInnerIf",
        title: "Join 'if' statement with inner 'if' statement",
        priority: 30,
        produce: join_with_inner_if,
    },
    AssistSpec {
        kind: "dart.assist.joinWithOuterIf",
        title: "Join 'if' statement with outer 'if' statement",
        priority: 30,
        produce: join_with_outer_if,
    },
    AssistSpec {
        kind: "dart.assist.remove.unnecessaryName",
        title: "Remove unnecessary name from pattern",
        priority: 30,
        produce: remove_unnecessary_name,
    },
    AssistSpec {
        kind: "dart.assist.remove.typeAnnotation",
        title: "Remove type annotation",
        priority: 31,
        produce: remove_type_annotation,
    },
    AssistSpec {
        kind: "dart.assist.convert.toGenericFunctionSyntax",
        title: "Convert to 'Function' syntax",
        priority: 30,
        produce: convert_to_generic_function_syntax,
    },
    AssistSpec {
        kind: "dart.assist.convert.bodyToBlock",
        title: "Convert to block body",
        priority: 30,
        produce: convert_to_block_body,
    },
    // F5c resolved-light rows: the producers decline silently when the
    // resolved face is unmounted, so these rows are simply absent from the
    // engine-free dispatch (fewer assists, never approximated).
    AssistSpec {
        kind: "dart.assist.add.returnType",
        title: "Add return type",
        priority: 30,
        produce: add_return_type,
    },
    AssistSpec {
        kind: "dart.assist.add.typeAnnotation",
        title: "Add type annotation",
        priority: 30,
        produce: add_type_annotation_local,
    },
    AssistSpec {
        kind: "dart.assist.convert.bodyToAsync",
        title: "Convert to async function body",
        priority: 31,
        produce: convert_body_to_async,
    },
    AssistSpec {
        kind: "dart.assist.convert.bodyToExpression",
        title: "Convert to expression body",
        priority: 30,
        produce: convert_body_to_expression,
    },
    // Upstream templates the title ("Convert '{0}' to getter"); the static
    // title contract here carries the general form.
    AssistSpec {
        kind: "dart.assist.convert.finalFieldToGetter",
        title: "Convert to getter",
        priority: 30,
        produce: final_field_to_getter,
    },
    AssistSpec {
        kind: "dart.assist.joinVariableDeclaration",
        title: "Join variable declaration",
        priority: 30,
        produce: join_variable_declaration,
    },
    AssistSpec {
        kind: "dart.assist.remove.async",
        title: "Remove 'async' modifier",
        priority: 30,
        produce: remove_async,
    },
    AssistSpec {
        kind: "dart.assist.splitVariableDeclaration",
        title: "Split variable declaration",
        priority: 30,
        produce: split_variable_declaration,
    },
    AssistSpec {
        kind: "dart.assist.surround.block",
        title: "Surround with block",
        priority: 38,
        produce: surround_with_block,
    },
    AssistSpec {
        kind: "dart.assist.surround.curlyBraces",
        title: "Use curly braces",
        priority: 30,
        produce: use_curly_braces,
    },
    AssistSpec {
        kind: "dart.assist.surround.doWhile",
        title: "Surround with 'do-while'",
        priority: 33,
        produce: surround_with_do_while,
    },
    AssistSpec {
        kind: "dart.assist.surround.forEach",
        title: "Surround with 'for'",
        priority: 34,
        produce: surround_with_for,
    },
    AssistSpec {
        kind: "dart.assist.surround.forIn",
        title: "Surround with 'for-in'",
        priority: 35,
        produce: surround_with_for_in,
    },
    AssistSpec {
        kind: "dart.assist.surround.if",
        title: "Surround with 'if'",
        priority: 37,
        produce: surround_with_if,
    },
    AssistSpec {
        kind: "dart.assist.surround.tryCatch",
        title: "Surround with 'try-catch'",
        priority: 32,
        produce: surround_with_try_catch,
    },
    AssistSpec {
        kind: "dart.assist.surround.tryFinally",
        title: "Surround with 'try-finally'",
        priority: 31,
        produce: surround_with_try_finally,
    },
    AssistSpec {
        kind: "dart.assist.surround.while",
        title: "Surround with 'while'",
        priority: 36,
        produce: surround_with_while,
    },
    // The convert-family wave (#2400): twelve of the census's 23 missing
    // convert kinds, each mirroring its official producer's guard and edit
    // shape (upstream Dart SDK source,
    // pkg/analysis_server/lib/src/services/correction/dart/convert_*.dart).
    // The eleven facts-blocked kinds (classToEnum, classToMixin,
    // partOfToPartUri, relativeToPackageImport, switchExpression,
    // toDeclaringParameter, toDotShorthand, toInBodyConstructor,
    // toInitializingFormal, toPrimaryConstructor, toSuperParameters) stay
    // out — their producers read resolved element/type identities this
    // substrate does not model (the flutter-family posture: facts-blocked,
    // not a divergence). Each row's module doc names the absent face.
    AssistSpec {
        kind: "dart.assist.convert.toIfElement",
        title: "Convert to 'if' element",
        priority: 30,
        produce: convert_to_if_element,
    },
    AssistSpec {
        kind: "dart.assist.convert.toSpread",
        title: "Convert to spread",
        priority: 30,
        produce: convert_to_spread,
    },
    AssistSpec {
        kind: "dart.assist.convert.toMapLiteral",
        title: "Convert to map literal",
        priority: 30,
        produce: convert_to_map_literal,
    },
    AssistSpec {
        kind: "dart.assist.convert.toSetLiteral",
        title: "Convert to set literal",
        priority: 30,
        produce: convert_to_set_literal,
    },
    AssistSpec {
        kind: "dart.assist.convert.toForElement",
        title: "Convert to 'for' element",
        priority: 30,
        produce: convert_to_for_element,
    },
    AssistSpec {
        kind: "dart.assist.convert.getterToFinalField",
        title: "Convert to final field",
        priority: 30,
        produce: getter_to_final_field,
    },
    AssistSpec {
        kind: "dart.assist.convert.ifCaseStatementChain",
        title: "Convert to 'if-case' statement chain",
        priority: 30,
        produce: if_case_statement_chain,
    },
    AssistSpec {
        kind: "dart.assist.convert.fieldFormalToNormal",
        title: "Convert to normal parameter",
        priority: 30,
        produce: field_formal_to_normal,
    },
    AssistSpec {
        kind: "dart.assist.convert.ifCaseStatement",
        title: "Convert to 'if-case' statement",
        priority: 30,
        produce: if_case_statement,
    },
    AssistSpec {
        kind: "dart.assist.convert.forEachToForIndex",
        title: "Convert to for-index loop",
        priority: 30,
        produce: for_each_to_for_index,
    },
    AssistSpec {
        kind: "dart.assist.convert.switchStatement",
        title: "Convert to switch statement",
        priority: 30,
        produce: convert_to_switch_statement,
    },
    AssistSpec {
        kind: "dart.assist.convert.packageToRelativeImport",
        title: "Convert to relative import",
        priority: 30,
        produce: package_to_relative_import,
    },
];

/// The built-in assist rows for one selection: every producer runs (the
/// official self-guarding dispatch), each row's edits anchor against the
/// file's current text, and the rows sort by (priority, kind) — the
/// machine face's deterministic order.
pub(crate) fn builtin_assists(
    text: &str,
    path: &Path,
    offset: u64,
    length: u64,
    fuse: &PlannerFuse,
) -> Vec<PluginFix> {
    builtin_assists_with(text, path, offset, length, None, fuse)
}

/// The dispatch over one semantic-face posture: `Some` mounts the face
/// for the resolved-light producers, `None` is the engine-free dispatch.
/// Each producer trial routes through the planner fuse's counted step and
/// the walk stops consuming specs on a spent budget.
pub(crate) fn builtin_assists_with(
    text: &str,
    path: &Path,
    offset: u64,
    length: u64,
    semantics: Option<&dartq_semantic::Analysis<'_>>,
    fuse: &PlannerFuse,
) -> Vec<PluginFix> {
    let (Ok(offset), Ok(length)) = (u32::try_from(offset), u32::try_from(length)) else {
        return Vec::new();
    };
    let Some(end) = offset
        .checked_add(length)
        .filter(|end| *end <= utf16_len_of(text))
    else {
        return Vec::new();
    };
    let features = unit_features(None, path);
    let parsed = parse_unit(text, features);
    let context = AssistContext {
        text,
        selection: offset..end,
        tokens: &parsed.tokens,
        ast: &parsed.ast,
        features,
        path,
        semantics,
    };
    let mut rows: Vec<(i32, &'static str, PluginFix)> = ASSIST_TABLE
        .iter()
        .filter_map(|spec| {
            if !fuse.step() {
                return None;
            }
            let edits = (spec.produce)(&context)?;
            let anchored = edits
                .into_iter()
                .map(|(edit_offset, edit_length, replacement)| {
                    anchored_edit(text, &parsed.lines, edit_offset, edit_length, &replacement)
                })
                .collect::<Vec<Edit>>();
            (!anchored.is_empty()).then(|| {
                (
                    spec.priority,
                    spec.kind,
                    PluginFix {
                        plugin: BUILTIN_ASSIST_SOURCE.to_owned(),
                        message: spec.title.to_owned(),
                        priority: spec.priority,
                        edits: anchored,
                    },
                )
            })
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    rows.into_iter().map(|(_, _, row)| row).collect()
}

// ---------------------------------------------------------------------------
// Assist substrate: selection walks over the arena tree
// ---------------------------------------------------------------------------

/// The UTF-16 span of one node (first token start through last token end).
pub(crate) fn node_span16(
    tokens: &TokenTable,
    text: &str,
    ast: &SyntaxTable,
    id: u32,
) -> Option<core::ops::Range<u32>> {
    let range = ast.token_range(id);
    let first = tokens.get(range.start as usize)?;
    let last_index = range.end.checked_sub(1)? as usize;
    Some(first.utf16_offset..utf16_end(tokens, text, last_index)?)
}

/// The innermost node whose span covers the selection start. Zero-width
/// synthetic rows (metadata placeholders) cover nothing — only a real
/// span or a cursor resting exactly on a token end counts. The flat
/// expression layer spreads operand rows beside their containers, so when
/// the direct-child peel finds no covering child the window is scanned
/// for the smallest covering row (the deep leaf the peel skipped).
pub(crate) fn covering_node(tokens: &TokenTable, text: &str, ast: &SyntaxTable, offset16: u32) -> Option<u32> {
    let covers = |id: u32| {
        node_span16(tokens, text, ast, id).filter(|span| {
            span.start < span.end && (span.contains(&offset16) || span.end == offset16)
        })
    };
    let mut current = ast.root()?;
    covers(current)?;
    loop {
        let deeper = direct_children(ast, current)
            .into_iter()
            .find(|&child| covers(child).is_some());
        match deeper {
            Some(child) => current = child,
            None => {
                // The flat layer's hidden leaf: the covering row the
                // peel could not reach. When `current` covers only by
                // the end-boundary rule the real leaf is its sibling —
                // search the parent window too. Boundary ties rank the
                // row starting at the offset (the token under the
                // cursor) above an interior cover, above the row merely
                // ending there; smallest span breaks the rest.
                let rank_of = |row: u32| {
                    covers(row).map(|span| {
                        let rank = if span.start == offset16 {
                            0
                        } else if span.end == offset16 {
                            2
                        } else {
                            1
                        };
                        (row, rank, span.end - span.start)
                    })
                };
                let own_end = node_span16(tokens, text, ast, current)
                    .is_some_and(|span| span.end == offset16);
                let hidden = ast
                    .children(current)
                    .chain(
                        own_end
                            .then(|| ast.parent(current))
                            .flatten()
                            .into_iter()
                            .flat_map(|parent| ast.children(parent)),
                    )
                    .filter(|&row| row != current)
                    .filter_map(rank_of)
                    .min_by_key(|(_, rank, width)| (*rank, *width))
                    .map(|(row, _, _)| row);
                return Some(hidden.unwrap_or(current));
            }
        }
    }
}

/// The line's indentation at `offset16` (the leading horizontal run of
/// that line).
pub(crate) fn line_indent_of(text: &str, offset16: u32) -> &str {
    let start = utf16_offset_to_byte(text, offset16);
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent_end = text[line_start..]
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(text[line_start..].len());
    &text[line_start..line_start + indent_end]
}

/// The file's end-of-line spelling (the official `builder.eol` stance: the
/// first terminator, `\n` default).
pub(crate) fn eol_of(text: &str) -> &'static str {
    match text.find('\r') {
        Some(index) if text[index..].starts_with("\r\n") => "\r\n",
        _ => "\n",
    }
}

// ---------------------------------------------------------------------------
// Assist producers — F5a: the lexical / local cluster (engine-free)
// ---------------------------------------------------------------------------

/// The whole string literal node covering the selection (a simple literal
/// or an interpolation: the literal spans every segment token).
pub(crate) fn string_literal_span(context: &AssistContext<'_>) -> Option<core::ops::Range<u32>> {
    let mut current = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    loop {
        if context.ast.kind(current) == NodeKind::StringLiteral {
            return node_span16(context.tokens, context.text, context.ast, current);
        }
        current = context.ast.parent(current)?;
    }
}

/// `convert.toSingleQuotedString` / `convert.toDoubleQuotedString`
/// (`ConvertQuotes` and its two subclasses): the covering literal swaps
/// its quotes, escapes re-verified per segment. The literal's source quote
/// style must match the direction (the `_fromSingle` guard).
pub(crate) fn convert_quotes_direction(
    context: &AssistContext<'_>,
    to_single: bool,
) -> Option<(u32, u32, String)> {
    let span = string_literal_span(context)?;
    convert_quotes(context.tokens, context.text, &span, Some(to_single))
}

pub(crate) fn convert_quotes_to_single(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    Some(vec![convert_quotes_direction(context, true)?])
}

pub(crate) fn convert_quotes_to_double(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    Some(vec![convert_quotes_direction(context, false)?])
}

/// `convert.toMultilineString` (`ConvertToMultilineString`): the opening
/// quote character becomes the triple quote plus a newline, the closing
/// quote the triple quote — raw literals keep their prefix.
pub(crate) fn convert_to_multiline_string(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let span = string_literal_span(context)?;
    let head = slice16(text, &(span.start..span.start + 2))?;
    let raw_skip = usize::from(head.starts_with('r'));
    let quote_byte = *text
        .as_bytes()
        .get(utf16_offset_to_byte(text, span.start) + raw_skip)?;
    if !matches!(quote_byte, b'\'' | b'"') {
        return None;
    }
    // Quote-run length: one is convertible, three already is multiline.
    // The run breaks on the first non-quote byte (the `filter`-count form
    // would re-count the closing delimiter of a one-character literal).
    let lead = (0..3usize)
        .take_while(|step| {
            text.as_bytes()
                .get(utf16_offset_to_byte(text, span.start) + raw_skip + step)
                == Some(&quote_byte)
        })
        .count();
    if lead != 1 {
        return None;
    }
    let new_quote = if quote_byte == b'\'' { "'''" } else { "\"\"\"" };
    let eol = eol_of(text);
    // One edit over the literal body: the opening quote becomes the triple
    // quote plus a newline, the closing the triple quote (upstream's two
    // `addReplacement`/`addSimpleReplacement` writes fused — the row is a
    // single-edit shape).
    let content = slice16(text, &(span.start + raw_skip as u32 + 1..span.end - 1))?;
    Some(vec![(
        span.start + raw_skip as u32,
        span.end - span.start - raw_skip as u32,
        format!("{new_quote}{eol}{content}{new_quote}"),
    )])
}

/// `convert.toIntLiteral` (`ConvertToIntLiteral`): the double literal whose
/// value is integral loses its fraction — the tail after the `.` deletes
/// verbatim, separators included — and an exponential spelling rewrites
/// whole (the producer's two legs).
pub(crate) fn convert_to_int_literal(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let token = covering_token(context)?;
    if context.tokens.get(token)?.kind != TokenKind::Double {
        return None;
    }
    let lexeme = context.tokens.lexeme(text, token);
    let stripped = lexeme.replace('_', "");
    let value: f64 = stripped.parse().ok()?;
    // The official guards: `truncate()` must round-trip and stay in int
    // range (the upstream catch leg).
    let truncated = value.trunc();
    if !value.is_finite() || value != truncated {
        return None;
    }
    let int_value = i64::try_from(truncated as i128).ok()?;
    let start = token_start16(context.tokens, token)?;
    if !lexeme.contains(['e', 'E']) {
        // The lexeme is ASCII (digits, separators, one dot), so the byte
        // index of the dot is its UTF-16 delta.
        let dot = lexeme.find('.')?;
        return Some(vec![(
            start + dot as u32,
            utf16_count(&lexeme[dot..]),
            String::new(),
        )]);
    }
    Some(vec![(start, utf16_count(lexeme), int_value.to_string())])
}

/// `add.digitSeparators` (`AddDigitSeparatorEveryThreeDigits` and the
/// every-two hexadecimal twin, one kind id): the literal's digits regroup
/// — decimal every three (whole / fraction reversed / exponent parts, five
/// digits minimum per part), hexadecimal every two from `0x` (four
/// minimum). Existing separators are scrapped first; an unchanged result
/// never offers.
pub(crate) fn add_digit_separators(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let token = covering_token(context)?;
    let kind = context.tokens.get(token)?.kind;
    if !matches!(
        kind,
        TokenKind::Int | TokenKind::Double | TokenKind::Hexadecimal
    ) {
        return None;
    }
    let lexeme = context.tokens.lexeme(text, token).to_owned();
    let start = token_start16(context.tokens, token)?;
    let replacement = if kind == TokenKind::Hexadecimal {
        let digits = lexeme.get(2..)?.replace('_', "");
        let regrouped = add_separators(&digits, 2, 4)?;
        let candidate = format!("{}{}", &lexeme[..2], regrouped);
        (candidate != lexeme).then_some(candidate)?
    } else {
        let (e_index, has_e) = match lexeme.find(['e', 'E']) {
            Some(at) => (at, true),
            None => (lexeme.len(), false),
        };
        let (whole_end, fraction) = match lexeme.find('.') {
            Some(dot) => (dot, Some(lexeme[dot + 1..e_index].to_owned())),
            None => (e_index, None),
        };
        let whole = lexeme[..whole_end].replace('_', "");
        let exponential = has_e.then(|| {
            let rest = &lexeme[e_index + 1..];
            let negative = rest.starts_with('-');
            let digits = rest.strip_prefix('-').unwrap_or(rest).replace('_', "");
            (e_index, negative, digits)
        });
        let mut out = String::new();
        out.push_str(&add_separators(&whole, 3, 5).unwrap_or_else(|| whole.clone()));
        if let Some(fraction) = fraction {
            out.push('.');
            // The fractional part groups left-aligned: reverse, group,
            // reverse back (the official walk).
            let reversed: String = fraction.chars().rev().collect();
            let regrouped = add_separators(&reversed, 3, 5)
                .map(|grouped| grouped.chars().rev().collect::<String>());
            match regrouped {
                Some(grouped) => out.push_str(&grouped),
                None => out.push_str(&fraction),
            }
        }
        if let Some((e_index, negative, digits)) = exponential {
            out.push_str(&lexeme[e_index..e_index + 1]);
            if negative {
                out.push('-');
            }
            out.push_str(&add_separators(&digits, 3, 5).unwrap_or(digits));
        }
        (out != lexeme).then_some(out)?
    };
    Some(vec![(start, utf16_count(&lexeme), replacement)])
}

/// The official `_addSeparators`: the first group carries the remainder,
/// then fixed-width `_`-joined groups; `None` under the digit minimum.
pub(crate) fn add_separators(digits: &str, per_group: usize, minimum: usize) -> Option<String> {
    if digits.len() < minimum {
        return None;
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / per_group);
    let head = digits.len() % per_group;
    let head = if head == 0 { per_group } else { head };
    out.push_str(&digits[..head]);
    let mut at = head;
    while at < digits.len() {
        out.push('_');
        out.push_str(&digits[at..at + per_group]);
        at += per_group;
    }
    Some(out)
}

/// `remove.digitSeparators` (`RemoveDigitSeparators`): every `_` leaves
/// the literal; an unchanged lexeme never offers.
pub(crate) fn remove_digit_separators(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let text = context.text;
    let token = covering_token(context)?;
    if !matches!(
        context.tokens.get(token)?.kind,
        TokenKind::Int | TokenKind::Double | TokenKind::Hexadecimal
    ) {
        return None;
    }
    let lexeme = context.tokens.lexeme(text, token);
    if !lexeme.contains('_') {
        return None;
    }
    Some(vec![(
        token_start16(context.tokens, token)?,
        utf16_count(lexeme),
        lexeme.replace('_', ""),
    )])
}

/// The doc comment (one `/** … */` or the contiguous `///` line run) whose
/// head line holds the selection, each line verified to sit in a token
/// gap — a `///`-shaped line inside a multiline string never verifies.
/// Returns the run's span plus its style (`"line"` / `"block"`).
pub(crate) fn doc_comment_at(context: &AssistContext<'_>) -> Option<(core::ops::Range<u32>, &'static str)> {
    let text = context.text;
    let byte = utf16_offset_to_byte(text, context.selection.start);
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let indent_len =
        text[line_start..].len() - text[line_start..].trim_start_matches([' ', '\t']).len();
    let head_byte = line_start + indent_len;
    let head = text.get(head_byte..)?;
    // The line's first code position must sit between tokens — inside a
    // multiline string token it belongs to the string, not a comment.
    let in_gap = |offset16: u32| covering_token_at(context.tokens, text, offset16).is_none();
    if head.starts_with("///") {
        let head16 = u32::try_from(head_byte).ok()?;
        if !in_gap(head16) {
            return None;
        }
        let mut run_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |at| line_start + at);
        let mut line_begin = line_start;
        while line_begin < text.len() {
            let next_line = run_end + 1;
            let line = text.get(next_line..)?;
            let trimmed = line.trim_start_matches([' ', '\t']);
            if !trimmed.starts_with("///") {
                break;
            }
            let line_head16 = u32::try_from(next_line + (line.len() - trimmed.len())).ok()?;
            if !in_gap(line_head16) {
                break;
            }
            line_begin = next_line;
            run_end = line[line_head16 as usize - next_line..]
                .find('\n')
                .map_or(text.len(), |at| next_line + at);
        }
        let start16 = u32::try_from(line_start).ok()?;
        let end16 = u32::try_from(run_end).ok()?;
        return Some((start16..end16, "line"));
    }
    if head.starts_with("/**") && !head[3..].starts_with('/') {
        let head16 = u32::try_from(head_byte).ok()?;
        if !in_gap(head16) {
            return None;
        }
        // The block runs to the nesting-aware `*/` (block comments nest).
        let bytes = text.as_bytes();
        let mut depth = 1usize;
        let mut at = head_byte + 2;
        while at < bytes.len() {
            match bytes.get(at..at + 2) {
                Some(b"/*") => {
                    depth += 1;
                    at += 2;
                }
                Some(b"*/") => {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                }
                _ => at += 1,
            }
        }
        let start16 = head16;
        let end16 = u32::try_from(at).ok()?;
        return Some((start16..end16, "block"));
    }
    None
}

/// `convert.blockComment` (`ConvertDocumentationIntoBlock`): the `///` run
/// rebuilds as one `/** */` block, every source line under ` *` and the
/// comment's own indentation carried.
pub(crate) fn convert_to_block_doc(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let (span, style) = doc_comment_at(context)?;
    if style != "line" {
        return None;
    }
    let text = context.text;
    let eol = eol_of(text);
    let indent = line_indent_of(text, span.start);
    let mut out = format!("/**{eol}");
    let start = utf16_offset_to_byte(text, span.start);
    let end = utf16_offset_to_byte(text, span.end);
    for line in text[start..end].split(eol) {
        let trimmed = line.trim_start();
        let body = trimmed.strip_prefix("///")?;
        out.push_str(indent);
        out.push_str(" *");
        out.push_str(body);
        out.push_str(eol);
    }
    out.push_str(indent);
    out.push_str(" */");
    Some(vec![(span.start, span.end - span.start, out)])
}

/// `convert.lineComment` (`ConvertDocumentationIntoLine`): the `/** */`
/// comment rebuilds as `///` lines — the same state machine the
/// `slash_for_doc_comments` fix rides.
pub(crate) fn convert_to_line_doc(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let (span, style) = doc_comment_at(context)?;
    if style != "block" {
        return None;
    }
    let input = FixInput {
        text: context.text,
        range: span,
        message: "",
        tokens: None,
        // The assist face is same-file by construction — no producer here
        // reads the path (only [`FixAction::CreateFile`] does), so the
        // empty path never misaddresses. The organize producer is not on
        // the assist face, so the diagnostic set rides empty too.
        path: Path::new(""),
        diagnostics: &[],
    };
    let (offset, length, replacement) = doc_comment_to_line(&input)?;
    Some(vec![(offset, length, replacement)])
}

/// The token whose span covers the selection start (`None` between
/// tokens).
pub(crate) fn covering_token(context: &AssistContext<'_>) -> Option<usize> {
    covering_token_at(context.tokens, context.text, context.selection.start)
}

/// The token whose span covers `offset16` — width-0 selections at a token
/// start take that token; a position between tokens answers `None`.
pub(crate) fn covering_token_at(tokens: &TokenTable, text: &str, offset16: u32) -> Option<usize> {
    let index = token_at(tokens, offset16)?;
    if tokens.get(index)?.utf16_offset == offset16 {
        return Some(index);
    }
    let previous = index.checked_sub(1)?;
    (offset16 < utf16_end(tokens, text, previous)?).then_some(previous)
}

pub(crate) fn token_start16(tokens: &TokenTable, index: usize) -> Option<u32> {
    Some(tokens.get(index)?.utf16_offset)
}
// ---------------------------------------------------------------------------
// Assist producers — the if / condition cluster (the #1687 wave)
// ---------------------------------------------------------------------------

/// Whether `id` is a statement-list holder — the standalone block or the
/// function body (this AST fuses the body's block into `BlockBody`, whose
/// direct children are the statements).
pub(crate) fn is_statement_block(ast: &SyntaxTable, id: u32) -> bool {
    matches!(ast.kind(id), NodeKind::Block | NodeKind::BlockBody)
}

/// Whether `kind` is a statement-level node (the official `Statement`
/// family the assist faces climb to — this AST's `VariablesDeclaration`
/// is the local variable statement).
pub(crate) fn is_statement_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Block
            | NodeKind::LabeledStatement
            | NodeKind::IfStatement
            | NodeKind::ForStatement
            | NodeKind::WhileStatement
            | NodeKind::DoWhileStatement
            | NodeKind::TryStatement
            | NodeKind::SwitchStatement
            | NodeKind::ReturnStatement
            | NodeKind::RethrowStatement
            | NodeKind::YieldStatement
            | NodeKind::Assert
            | NodeKind::Break
            | NodeKind::Continue
            | NodeKind::EmptyStatement
            | NodeKind::ExpressionStatement
            | NodeKind::VariablesDeclaration
            | NodeKind::PatternVarDeclStatement
            | NodeKind::PatternAssignment
            | NodeKind::LocalFunctionDeclaration
    )
}

/// The innermost statement node covering the selection start, climbing
/// over non-statement wrappers the way `thisOrAncestorOfType<Statement>`
/// does.
pub(crate) fn statement_at(context: &AssistContext<'_>) -> Option<u32> {
    let mut current = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    loop {
        if is_statement_kind(context.ast.kind(current)) {
            return Some(current);
        }
        current = context.ast.parent(current)?;
    }
}

/// The direct children of `id`, in source order. The completion window
/// ([`SyntaxTable::children`]) holds every descendant in post-order ids —
/// not a direct-child list — so the direct children peel off
/// back-to-front: the row before the cursor is a child, and jumping to
/// its window start skips its whole subtree (the canonical tiling from
/// the dartq-ast window contract). Termination invariant: every step
/// consumes the row under the cursor and strictly lowers it, so the walk
/// always advances and stops at the window start.
pub(crate) fn direct_children(ast: &SyntaxTable, id: u32) -> Vec<u32> {
    let window = ast.children(id);
    let mut kids = Vec::new();
    let mut cursor = window.end;
    while cursor > window.start {
        cursor -= 1;
        kids.push(cursor);
        cursor = ast.children(cursor).start;
    }
    kids.reverse();
    kids
}

/// The first child of `id` with the given kind, over the statement tiling
/// (arena direct children — correct for statement-level containers).
pub(crate) fn child_of_kind(ast: &SyntaxTable, id: u32, wanted: NodeKind) -> Option<u32> {
    direct_children(ast, id)
        .into_iter()
        .find(|&child| ast.kind(child) == wanted)
}

/// The first logical child of `id` with the given kind (the flat-layer
/// child read — see [`logical_children`]).
pub(crate) fn logical_child_of_kind(context: &AssistContext<'_>, id: u32, wanted: NodeKind) -> Option<u32> {
    logical_children(context, id)
        .into_iter()
        .find(|&child| context.ast.kind(child) == wanted)
}

/// The children of `id` that are statements, in source order (the if
/// statement's then/else branches, a block's statements).
pub(crate) fn statement_children(ast: &SyntaxTable, id: u32) -> Vec<u32> {
    direct_children(ast, id)
        .into_iter()
        .filter(|&child| is_statement_kind(ast.kind(child)))
        .collect()
}

/// The expression rows forming `id`'s operand run: `id`'s own completion
/// window plus the adopted-below siblings (the flat expression layer leaves
/// the left operand — and, for `IsExpression`/`DotAccess`/`Conditional`,
/// every operand — beside the container inside the parent's window, never
/// inside it). Bounded by the enclosing statement subtree.
pub(crate) fn expression_rows(ast: &SyntaxTable, id: u32) -> Vec<u32> {
    let mut rows: Vec<u32> = Vec::new();
    // Below-window siblings at every flat level: the operand run of a
    // nested expression container (an `InitializedIdentifier` inside a
    // `VariablesDeclaration`) starts before the innermost window opens,
    // so the climb collects each ancestor's left siblings while the
    // ancestor itself sits inside the same flat run. The enclosing
    // statement subtree bounds the walk.
    let mut current = id;
    while let Some(parent) = ast.parent(current) {
        let window_start = ast.children(current).start;
        for sibling in direct_children(ast, parent) {
            if sibling == current {
                break;
            }
            if sibling < window_start {
                rows.push(sibling);
            }
        }
        current = parent;
    }
    rows.extend(ast.children(id));
    rows
}

/// The token index bounding `id`'s left extent: walk the tokens back from
/// `head` over matched groups to the first token that cannot sit inside an
/// expression (a depth-0 delimiter or statement terminator). The flat
/// `Send`/`IsExpression`/`DotAccess`/`Conditional` shapes anchor the node
/// on the operator or selector, so the left operand lives *before* the
/// span head — this recovers where that operand region starts.
pub(crate) fn expression_head_bound(tokens: &TokenTable, text: &str, head: u32) -> u32 {
    const CUT: &[&str] = &[
        ";", ",", ":", "?", "=", "=>", "(", "[", "{", ")", "]", "}", "return", "if", "else",
        "while", "for", "do", "case", "when", "in", "var", "final", "const", "assert", "throw",
        "yield", "break", "continue", "switch",
    ];
    let mut index = head;
    while index > 0 {
        let candidate = index - 1;
        let token = &tokens.as_slice()[candidate as usize];
        // A close token opens a matched group: skip the whole group, it
        // cannot hold the operand head.
        if matches!(
            token.kind,
            TokenKind::CloseCurly | TokenKind::CloseParen | TokenKind::CloseSquare
        ) {
            let mut depth = 1u32;
            let mut scan = candidate;
            while scan > 0 && depth > 0 {
                scan -= 1;
                let kind = tokens.as_slice()[scan as usize].kind;
                if matches!(
                    kind,
                    TokenKind::CloseCurly | TokenKind::CloseParen | TokenKind::CloseSquare
                ) {
                    depth += 1;
                } else if kind.is_begin_group() {
                    depth -= 1;
                }
            }
            index = scan;
            continue;
        }
        if CUT.contains(&tokens.lexeme(text, candidate as usize)) {
            break;
        }
        index = candidate;
    }
    index
}

/// The token index of `id`'s extent left bound: the head-bound scan for
/// the left-anchored kinds (`IsExpression`'s `is`, `DotAccess`'s
/// selector, `Conditional`'s `?`, a member's return-type head), the span
/// head for the rest.
pub(crate) fn expression_extent_start_token(
    tokens: &TokenTable,
    text: &str,
    ast: &SyntaxTable,
    id: u32,
) -> u32 {
    let head = ast.token_range(id).start;
    match ast.kind(id) {
        NodeKind::IsExpression
        | NodeKind::AsExpression
        | NodeKind::DotAccess
        | NodeKind::Conditional
        | NodeKind::FunctionTypedFormalParameter
        | NodeKind::Method
        | NodeKind::Constructor
        | NodeKind::Factory => expression_head_bound(tokens, text, head),
        _ => head,
    }
}

/// `id`'s left extent as a UTF-16 offset (see [`expression_extent_start_token`]).
pub(crate) fn expression_extent_start(
    tokens: &TokenTable,
    text: &str,
    ast: &SyntaxTable,
    id: u32,
) -> Option<u32> {
    let bound = expression_extent_start_token(tokens, text, ast, id);
    Some(tokens.get(bound as usize)?.utf16_offset)
}

/// The UTF-16 end of `id`'s expression extent. A `Send` sealed at its
/// terminating lookahead — and every container sealing at that send —
/// carries one trailing token that never belongs to the operand: a `;`,
/// `,`, `:`, `=` or `=>` always clips; a `?` clips for the flat
/// expression leaves (a `Type`'s own nullable suffix keeps it); a
/// `)`/`]`/`}` clips only when its opener lies outside the node's own
/// range (the enclosing group's close).
pub(crate) fn expression_end16(tokens: &TokenTable, text: &str, ast: &SyntaxTable, id: u32) -> Option<u32> {
    let range = ast.token_range(id);
    // The last real token of the range: synthetic zero-width marks (the
    // `[]` bracket pair) ride at the tail without text of their own.
    let mut last_index = range.end.checked_sub(1)? as usize;
    while last_index > range.start as usize
        && tokens.get(last_index).is_some()
        && tokens.lexeme(text, last_index).is_empty()
    {
        last_index -= 1;
    }
    let last = tokens.get(last_index)?;
    let end = utf16_end(tokens, text, last_index)?;
    let lexeme = tokens.lexeme(text, last_index);
    let first_offset = tokens.get(range.start as usize)?.utf16_offset;
    // The clip never empties the node — a `;`-only leaf (the `EmptyBody`
    // face) owns its token.
    let clip = |end: u32| (end > first_offset).then_some(end);
    if matches!(lexeme, ";" | "," | ":" | "=" | "=>") {
        return clip(last.utf16_offset).or(Some(end));
    }
    if lexeme == "?" && ast.kind(id) != NodeKind::Type {
        return clip(last.utf16_offset).or(Some(end));
    }
    if matches!(lexeme, ")" | "]" | "}") {
        // The closer is the node's own when a begin inside its range pairs
        // to it; an opener outside means the node swallowed the enclosing
        // group's close.
        let own = (range.start..range.end - 1).any(|index| {
            tokens
                .get(index as usize)
                .is_some_and(|token| token.kind.is_begin_group())
                && tokens.matching_end(index as usize) == Some(last_index)
        });
        if !own {
            return Some(last.utf16_offset);
        }
    }
    Some(end)
}

/// The expression extent of `id`: `[left_extent, real_end)` — the region
/// covering every operand piece the flat layer spreads beside the node.
/// Unary operators carry the operand beside (never inside) their operator
/// span: a prefix's extent runs to the operand's end, a postfix's back to
/// its start.
pub(crate) fn expression_extent(
    tokens: &TokenTable,
    text: &str,
    ast: &SyntaxTable,
    id: u32,
) -> Option<core::ops::Range<u32>> {
    match ast.kind(id) {
        NodeKind::UnaryPrefix => {
            let start = tokens.get(ast.token_range(id).start as usize)?.utf16_offset;
            let operand = id.checked_sub(1)?;
            let end = expression_end16(tokens, text, ast, operand)?;
            Some(start..end)
        }
        NodeKind::UnaryPostfix | NodeKind::NonNullAssert => {
            let operand = id.checked_sub(1)?;
            let start = expression_extent_start(tokens, text, ast, operand)?;
            let end = expression_end16(tokens, text, ast, id)?;
            Some(start..end)
        }
        _ => {
            let start = expression_extent_start(tokens, text, ast, id)?;
            let end = expression_end16(tokens, text, ast, id)?;
            Some(start..end)
        }
    }
}

/// The whitespace-trimmed sub-span of `text` inside `span` (`None` when
/// nothing but whitespace remains).
pub(crate) fn trim_span16(text: &str, span: core::ops::Range<u32>) -> Option<core::ops::Range<u32>> {
    let content = slice16(text, &span)?;
    let start_trim = content.len() - content.trim_start().len();
    let end_trim = content.len() - content.trim_end().len();
    let start = span.start + u32::try_from(start_trim).ok()?;
    let end = span.end - u32::try_from(end_trim).ok()?;
    (start < end).then_some(start..end)
}

/// Whether `kind` is a synthetic gap marker (`NoType`, `NoArguments`,
/// `MetadataStar`, …) — a placeholder row, never a logical child.
pub(crate) fn is_placeholder_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::NoPrimaryConstructor
            | NodeKind::NoFormalParameters
            | NodeKind::MetadataStar
            | NodeKind::NoArguments
            | NodeKind::NoCtorRefContinuation
            | NodeKind::NoTypeNameInCtorRef
            | NodeKind::NoIdentifier
            | NodeKind::NoName
            | NodeKind::NoType
            | NodeKind::NoTypeArguments
            | NodeKind::NoTypeVariables
            | NodeKind::NoBody
    )
}

/// `id`'s logical children: the rows of `id`'s operand run whose span
/// starts inside `id`'s extent, maximal by span containment. The flat
/// layer keeps operand components as siblings of their owner inside the
/// run, so direct-child peeling is meaningless here — maximal containment
/// is the logical-child tiling (equal spans resolve to the later row, the
/// post-order parent). Start-containment (not full containment) is the
/// boundary: a swallowed terminator stretches the run root's span past
/// the parent's clipped end — and a child's synthetic-token tail can pass
/// the parent's token range outright (`[]` inside `= []`).
pub(crate) fn logical_children(context: &AssistContext<'_>, id: u32) -> Vec<u32> {
    let ast = context.ast;
    let Some(extent) = expression_extent(context.tokens, context.text, ast, id) else {
        return Vec::new();
    };
    let mut rows = expression_rows(ast, id);
    // Gap markers (`No*`/`MetadataStar`) are synthetic placeholders, never
    // logical children — the `NoTypeVariables` of `typedef F` would
    // otherwise sit inside the parameter list's extent.
    rows.retain(|&row| row != id && !is_placeholder_kind(ast.kind(row)));
    let spans: Vec<Option<core::ops::Range<u32>>> = rows
        .iter()
        .map(|&row| node_span16(context.tokens, context.text, ast, row))
        .collect();
    let mut out = Vec::new();
    for (index, &row) in rows.iter().enumerate() {
        let Some(span) = spans[index].clone() else {
            continue;
        };
        if span.is_empty() || span.start < extent.start || span.start >= extent.end {
            continue;
        }
        let covered = rows.iter().enumerate().any(|(other, &m)| {
            other != index
                && spans[other].as_ref().is_some_and(|m_span| {
                    span.start >= m_span.start
                        && span.end <= m_span.end
                        && (span != *m_span || row < m)
                })
        });
        if !covered {
            out.push(row);
        }
    }
    out
}

/// The nearest logical ancestor-or-self node of `kind` (the official
/// `thisOrAncestorOfType`, rebuilt for the flat expression layer: the arena
/// parent chain skips expression leaves, so the walk climbs the real
/// ancestors' completion windows and keeps the *deepest* — smallest extent
/// — `wanted` row whose extent covers the leaf's).
pub(crate) fn ancestor_or_self_kind(
    tokens: &TokenTable,
    text: &str,
    ast: &SyntaxTable,
    start: u32,
    wanted: NodeKind,
) -> Option<u32> {
    let leaf_span = node_span16(tokens, text, ast, start)?;
    if ast.kind(start) == wanted {
        return Some(start);
    }
    let mut scope = ast.parent(start);
    while let Some(owner) = scope {
        let mut hit: Option<(u32, u32)> = None;
        for row in ast.children(owner) {
            if ast.kind(row) != wanted {
                continue;
            }
            let Some(extent) = expression_extent(tokens, text, ast, row) else {
                continue;
            };
            if extent.start <= leaf_span.start && leaf_span.end <= extent.end {
                let width = extent.end - extent.start;
                if hit.is_none_or(|(_, w)| width < w) {
                    hit = Some((row, width));
                }
            }
        }
        // The owner itself is a covering candidate (the statement and
        // declaration levels adopt their children, so a `wanted` owner is
        // the shallowest possible hit — its window already answered any
        // deeper one).
        if ast.kind(owner) == wanted
            && let Some(extent) = expression_extent(tokens, text, ast, owner)
            && extent.start <= leaf_span.start
            && leaf_span.end <= extent.end
        {
            let width = extent.end - extent.start;
            if hit.is_none_or(|(_, w)| width < w) {
                hit = Some((owner, width));
            }
        }
        if let Some((row, _)) = hit {
            return Some(row);
        }
        scope = ast.parent(owner);
    }
    None
}

/// `getSingleStatement`: a block with exactly one statement yields that
/// statement; any other statement yields itself; nothing else.
pub(crate) fn single_statement(ast: &SyntaxTable, id: u32) -> Option<u32> {
    if ast.kind(id) == NodeKind::Block {
        let statements = statement_children(ast, id);
        (statements.len() == 1).then(|| statements[0])
    } else {
        Some(id)
    }
}

/// The edits of one producer, sorted ascending by offset (the applier's
/// back-to-front order; producers emit non-overlapping spans).
pub(crate) fn sort_edits(mut edits: Vec<(u32, u32, String)>) -> Vec<(u32, u32, String)> {
    edits.sort_by_key(|(offset, _, _)| *offset);
    edits
}

/// Re-indents every line of `source` from `old_indent` to `new_indent`
/// (the official `replaceSourceIndent` posture), with a trailing eol
/// ensured when asked.
pub(crate) fn reindent(source: &str, old_indent: &str, new_indent: &str, eol: &str, trailing: bool) -> String {
    let body = source.strip_suffix(eol).unwrap_or(source);
    let mut out = String::with_capacity(body.len() + new_indent.len());
    for (index, line) in body.split(eol).enumerate() {
        if index > 0 {
            out.push_str(eol);
        }
        match line.strip_prefix(old_indent) {
            Some(rest) => {
                out.push_str(new_indent);
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    if trailing {
        out.push_str(eol);
    }
    out
}

/// One binary-family node's operands and operator: `BinaryExpression` and
/// `Assignment` carry the operator token in payload word 0; `IsExpression`
/// and `AsExpression` anchor on it (the `is`/`as` is the span head). The
/// operand *roots* are the last rows of each side of the run (post-order:
/// the operand's root completes last); the operand *spans* are the
/// operator-bounded regions — the left rides back to the expression head
/// bound, the right ends at the container's real end (a swallowed
/// terminator clips out).
#[allow(clippy::type_complexity)]
pub(crate) fn binary_parts(
    context: &AssistContext<'_>,
    id: u32,
) -> Option<(
    u32,
    u32,
    usize,
    core::ops::Range<u32>,
    core::ops::Range<u32>,
)> {
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let operator = match ast.kind(id) {
        NodeKind::IsExpression | NodeKind::AsExpression => ast.token_range(id).start as usize,
        _ => ast.payload(id).first().copied()? as usize,
    };
    let operator_token = tokens.get(operator)?;
    let op_start = operator_token.utf16_offset;
    let op_end = utf16_end(tokens, text, operator)?;
    // The operand run: `id`'s window rows plus the adopted-below siblings
    // (the left operand may complete outside the window — the
    // `IsExpression`/`DotAccess` faces keep it beside the node).
    let rows = expression_rows(ast, id);
    // Each side's operand root is the highest arena row of its token
    // slice (post-order: the operand's root completes last).
    let left_root = rows
        .iter()
        .filter(|&&row| (ast.token_range(row).start as usize) < operator)
        .max()
        .copied()?;
    let right_root = rows
        .iter()
        .filter(|&&row| (ast.token_range(row).start as usize) > operator)
        .max()
        .copied()?;
    // The left region rides back to the operand head (the expression head
    // bound skips `(`/`=`/`,`/keyword terminators the operator scan would
    // otherwise read through).
    let head_bound = expression_head_bound(tokens, text, operator as u32);
    let left_begin = tokens.get(head_bound as usize)?.utf16_offset;
    let left_span = trim_span16(text, left_begin..op_start)?;
    let right_end = expression_end16(tokens, text, ast, id)?;
    let right_span = trim_span16(text, op_end..right_end)?;
    Some((left_root, right_root, operator, left_span, right_span))
}

/// One `Conditional`'s three arms: `(condition, then, else)` root rows and
/// their trimmed spans. The condition completes *before* the node (the
/// container anchors at the `?`), so the run slices at the payload's `?`
/// and `:` tokens; each arm's root is its run's last row.
#[allow(clippy::type_complexity)]
pub(crate) fn conditional_parts(
    context: &AssistContext<'_>,
    id: u32,
) -> Option<(
    u32,
    u32,
    u32,
    core::ops::Range<u32>,
    core::ops::Range<u32>,
    core::ops::Range<u32>,
)> {
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let payload = ast.payload(id);
    let question = payload.first().copied()? as usize;
    let colon = payload.get(1).copied()? as usize;
    let question_start = tokens.get(question)?.utf16_offset;
    let question_end = utf16_end(tokens, text, question)?;
    let colon_start = tokens.get(colon)?.utf16_offset;
    let colon_end = utf16_end(tokens, text, colon)?;
    let rows = expression_rows(ast, id);
    // Each arm's root is the highest arena row of its token slice
    // (post-order completion order).
    let condition_root = rows
        .iter()
        .filter(|&&row| (ast.token_range(row).start as usize) < question)
        .max()
        .copied()?;
    let then_root = rows
        .iter()
        .filter(|&&row| {
            let start = ast.token_range(row).start as usize;
            start > question && start < colon
        })
        .max()
        .copied()?;
    let else_root = rows
        .iter()
        .filter(|&&row| (ast.token_range(row).start as usize) > colon)
        .max()
        .copied()?;
    let head_bound = expression_head_bound(tokens, text, question as u32);
    let condition_begin = tokens.get(head_bound as usize)?.utf16_offset;
    let condition_span = trim_span16(text, condition_begin..question_start)?;
    let then_span = trim_span16(text, question_end..colon_start)?;
    let else_end = expression_end16(tokens, text, ast, id)?;
    let else_span = trim_span16(text, colon_end..else_end)?;
    Some((
        condition_root,
        then_root,
        else_root,
        condition_span,
        then_span,
        else_span,
    ))
}

/// The operand root of a prefix/postfix unary: the operand always completes
/// in arena order immediately before the operator leaf seals (post-order —
/// `!x`, `x!`, `x++` all finish the operand, then the unary row lands).
pub(crate) fn unary_operand(unary: u32) -> Option<u32> {
    unary.checked_sub(1)
}

/// One `DotAccess`'s receiver and selector: payload word 0 is the dot
/// token. The receiver is the `Send` whose extent holds the dot (the
/// receiver send claims it as its terminating lookahead); the selector is
/// the last expression row before the access marker. Returns
/// `(receiver_row, selector_row, receiver_span, selector_span)`.
#[allow(clippy::type_complexity)]
pub(crate) fn dot_access_parts(
    context: &AssistContext<'_>,
    access: u32,
) -> Option<(u32, u32, core::ops::Range<u32>, core::ops::Range<u32>)> {
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let dot = ast.payload(access).first().copied()? as usize;
    let dot_start = tokens.get(dot)?.utf16_offset;
    let dot_end = utf16_end(tokens, text, dot)?;
    let parent = ast.parent(access)?;
    let mut receiver = None;
    let mut selector = None;
    for row in ast.children(parent) {
        if row == access {
            break;
        }
        let range = ast.token_range(row);
        if (range.start as usize) <= dot && dot < range.end as usize {
            receiver = Some(row);
        }
        if (range.start as usize) > dot {
            selector = Some(row);
        }
    }
    let receiver = receiver?;
    let selector = selector?;
    let receiver_start = tokens
        .get(ast.token_range(receiver).start as usize)?
        .utf16_offset;
    let receiver_span = trim_span16(text, receiver_start..dot_start)?;
    let selector_end = expression_end16(tokens, text, ast, access)?;
    let selector_span = trim_span16(text, dot_end..selector_end)?;
    Some((receiver, selector, receiver_span, selector_span))
}

/// One `if` statement's anatomy: the condition node, the right-paren end,
/// the then branch, the optional `else` keyword span and branch, and
/// whether a `case` clause sits inside the parens (if-case).
pub(crate) struct IfParts {
    pub(crate) condition: u32,
    pub(crate) condition_span: core::ops::Range<u32>,
    pub(crate) rparen16: u32,
    pub(crate) then_id: u32,
    else_keyword: Option<core::ops::Range<u32>>,
    pub(crate) else_id: Option<u32>,
    pub(crate) has_case: bool,
}

pub(crate) fn if_parts(context: &AssistContext<'_>, id: u32) -> Option<IfParts> {
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let parenthesized = child_of_kind(ast, id, NodeKind::ParenthesizedCondition)?;
    let paren_span = node_span16(tokens, text, ast, parenthesized)?;
    let condition = inner_expression(context, parenthesized)?;
    let condition_span = trim_span16(
        text,
        node_span16(tokens, text, ast, condition)?.start
            ..expression_end16(tokens, text, ast, condition)?,
    )?;
    let branches = statement_children(ast, id);
    let then_id = *branches.first()?;
    let else_id = branches.get(1).copied();
    let open = token_at(tokens, paren_span.start)?;
    let close = tokens.matching_end(open)?;
    let has_case =
        depth0_find(tokens, text, open + 1, close, |_, lexeme| lexeme == "case").is_some();
    let else_keyword = else_id.and_then(|_| {
        let then_end = node_span16(tokens, text, ast, then_id)?.end;
        let index = token_at(tokens, then_end)?;
        // Bounded by the table length — the scan always advances, and
        // the word sits within the file when an else branch exists.
        let keyword =
            (index..tokens.as_slice().len()).find(|index| word_at(tokens, text, *index, "else"))?;
        let token = tokens.get(keyword)?;
        let length = u32::try_from(tokens.lexeme(text, keyword).len()).ok()?;
        Some(token.utf16_offset..token.utf16_offset + length)
    });
    Some(IfParts {
        condition,
        condition_span,
        rparen16: paren_span.end,
        then_id,
        else_keyword,
        else_id,
        has_case,
    })
}

/// The condition text of an `if` (the child inside the parenthesized
/// condition, no syntax parens).
pub(crate) fn condition_text<'a>(context: &AssistContext<'a>, parts: &IfParts) -> Option<&'a str> {
    slice16(context.text, &parts.condition_span)
}

/// `shouldWrapParenthesisBeforeAnd`: only a binary expression binding
/// looser than `&&` wraps — a depth-0 `||` or `??` in the span.
pub(crate) fn binds_looser_than_and(context: &AssistContext<'_>, span: &core::ops::Range<u32>) -> bool {
    let open = match token_at(context.tokens, span.start) {
        Some(index) => index,
        None => return false,
    };
    let close = token_at(context.tokens, span.end).unwrap_or(context.tokens.len());
    depth0_find(context.tokens, context.text, open, close, |_, lexeme| {
        lexeme == "||" || lexeme == "??"
    })
    .is_some()
}

/// The operator swap of `exchangeOperands` (`<` trades for `>`).
pub(crate) fn swap_op(op: &str) -> Option<&'static str> {
    match op {
        "<" => Some(">"),
        "<=" => Some(">="),
        ">" => Some("<"),
        ">=" => Some("<="),
        _ => None,
    }
}

/// One inversion step: the node's span, its inverted text, and the
/// outermost operator's precedence (analyzer scale: `&&` 6, `||` 5,
/// relational 8, atomic 15) — a child binding looser than its joiner
/// wraps in parentheses, the `_InvertedCondition._binary` wrap.
pub(crate) fn invert_condition0(
    context: &AssistContext<'_>,
    id: u32,
    parent_precedence: u8,
) -> Option<(core::ops::Range<u32>, String)> {
    let text = context.text;
    let ast = context.ast;
    let span = trim_span16(text, expression_extent(context.tokens, text, ast, id)?)?;
    let emit = |content: String, precedence: u8| {
        let wrapped = if precedence < parent_precedence {
            format!("({content})")
        } else {
            content
        };
        (span.clone(), wrapped)
    };
    match ast.kind(id) {
        NodeKind::LiteralBool => {
            let literal = context
                .tokens
                .lexeme(text, ast.token_range(id).start as usize);
            let flipped = if literal == "true" { "false" } else { "true" };
            Some(emit(flipped.to_owned(), 15))
        }
        NodeKind::BinaryExpression => {
            let (left, right, operator, left_span, right_span) = binary_parts(context, id)?;
            let lexeme = context.tokens.lexeme(text, operator);
            match lexeme {
                "<" | ">" | "<=" | ">=" | "==" | "!=" => {
                    let left_text = slice16(text, &left_span)?;
                    let right_text = slice16(text, &right_span)?;
                    let mirrored = match lexeme {
                        "==" => "!=",
                        "!=" => "==",
                        other => mirror_op(other),
                    };
                    Some(emit(format!("{left_text} {mirrored} {right_text}"), 8))
                }
                "&&" | "||" => {
                    let precedence = if lexeme == "&&" { 6 } else { 5 };
                    let (left_span, _) = invert_condition0(context, left, precedence)?;
                    let (right_span, _) = invert_condition0(context, right, precedence)?;
                    let joiner = if lexeme == "&&" { " || " } else { " && " };
                    Some(emit(
                        format!(
                            "{}{joiner}{}",
                            slice16(text, &left_span)?,
                            slice16(text, &right_span)?
                        ),
                        precedence,
                    ))
                }
                _ => Some(emit(format!("!{}", slice16(text, &span)?), 15)),
            }
        }
        NodeKind::IsExpression => {
            let (_, _, _, expression_span, type_span) = binary_parts(context, id)?;
            // The `not` flag is payload word 0 (`is!` carries it).
            let has_bang = context.ast.payload(id).first().copied() == Some(1);
            Some(emit(
                format!(
                    "{} {} {}",
                    slice16(text, &expression_span)?,
                    if has_bang { "is" } else { "is!" },
                    slice16(text, &type_span)?
                ),
                8,
            ))
        }
        NodeKind::ParenthesizedExpression => {
            let inner = inner_expression(context, id)?;
            invert_condition0(context, inner, parent_precedence)
        }
        NodeKind::UnaryPrefix => {
            // Only the `!` operand unwraps; the operator precedes it.
            let operator = token_at(context.tokens, span.start)?;
            if context.tokens.lexeme(text, operator) != "!" {
                return None;
            }
            let operand = unary_operand(id)?;
            let unwrapped = if ast.kind(operand) == NodeKind::ParenthesizedExpression {
                node_span16(
                    context.tokens,
                    text,
                    ast,
                    inner_expression(context, operand)?,
                )?
            } else {
                node_span16(context.tokens, text, ast, operand)?
            };
            Some((span, slice16(text, &unwrapped)?.to_owned()))
        }
        _ => Some(emit(format!("!{}", slice16(text, &span)?), 15)),
    }
}

/// The inner expression of a wrapper (`ParenthesizedExpression`,
/// `ParenthesizedCondition`, `VariableInitializer`, `ExpressionBody`): the
/// last logical child — the run's root completes last in post-order.
pub(crate) fn inner_expression(context: &AssistContext<'_>, id: u32) -> Option<u32> {
    logical_children(context, id).into_iter().next_back()
}

/// `convert.isNot` (`ConvertIntoIsNot`): `!(x is T)` becomes `x is! T` —
/// the `!` moves behind the `is` operator, and the parentheses go when
/// the surrounding context binds looser than relational (the official
/// `getExpressionParentPrecedence` face).
pub(crate) fn convert_to_is_not(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let is_expression = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::IsExpression,
    )
    .or_else(|| {
        // Selection on the `!` itself: the prefix's parenthesized operand.
        if context.ast.kind(node) != NodeKind::UnaryPrefix {
            return None;
        }
        let operand = unary_operand(node)?;
        let inner = inner_expression(context, operand)?;
        (context.ast.kind(inner) == NodeKind::IsExpression).then_some(inner)
    })?;
    let (_, _, is_operator, _, _) = binary_parts(context, is_expression)?;
    // No existing `is!` (the `not` flag is payload word 0).
    if context.ast.payload(is_expression).first().copied() == Some(1) {
        return None;
    }
    // The `!` prefix: the operand run completes right before the operator
    // leaf — the is-expression's parenthesized wrapper is the row directly
    // under the unary in arena order.
    let parenthesized = context.ast.parent(is_expression)?;
    if context.ast.kind(parenthesized) != NodeKind::ParenthesizedExpression {
        return None;
    }
    let prefix = parenthesized.checked_add(1)?;
    if context.ast.kind(prefix) != NodeKind::UnaryPrefix {
        return None;
    }
    let prefix_span = node_span16(context.tokens, context.text, context.ast, prefix)?;
    let bang = token_at(context.tokens, prefix_span.start)?;
    if context.tokens.lexeme(context.text, bang) != "!" {
        return None;
    }
    let bang16 = prefix_span.start;
    let paren_span = node_span16(context.tokens, context.text, context.ast, parenthesized)?;
    let is_end16 = utf16_end(context.tokens, context.text, is_operator)?;
    let insert = (is_end16, 0, "!".to_owned());
    // The parens go when the token before the `!` flanks a delimiter or
    // a looser-than-relational join (the official
    // `getExpressionParentPrecedence < relational` decision —
    // assignments, returns, delimiters, the logical and equality arms);
    // tighter-binding parents keep them.
    let remove_parens = bang
        .checked_sub(1)
        .and_then(|index| context.tokens.get(index))
        .map_or(false, |token| paren_removable(token.kind));
    if remove_parens {
        Some(sort_edits(vec![
            (bang16, paren_span.start + 1 - bang16, String::new()),
            insert,
            (paren_span.end - 1, 1, String::new()),
        ]))
    } else {
        Some(sort_edits(vec![(bang16, 1, String::new()), insert]))
    }
}

/// `convert.isNotEmpty` (`ConvertIntoIsNotEmpty`): `!x.isEmpty` becomes
/// `x.isNotEmpty`. The element-resolution guards (the property must exist
/// and carry `isNotEmpty`) are beyond the token substrate — the shape
/// (`!` prefix, `.isEmpty` property access) is what verifies here.
pub(crate) fn convert_to_is_not_empty(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let identifier = covering_token(context)?;
    if context.tokens.lexeme(context.text, identifier) != "isEmpty" {
        return None;
    }
    // The `.isEmpty` property access: the covering node climbs to the
    // dot access whose property is this identifier.
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let access = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::DotAccess,
    )?;
    let (_receiver, _selector, target_span, property_span) = dot_access_parts(context, access)?;
    if property_span.start != token_start16(context.tokens, identifier)? {
        return None;
    }
    // The `!` prefix directly over the receiver: the unary leaf seals
    // right after the access marker.
    let prefix = access.checked_add(1)?;
    if context.ast.kind(prefix) != NodeKind::UnaryPrefix {
        return None;
    }
    let prefix_span = node_span16(context.tokens, context.text, context.ast, prefix)?;
    let bang = token_at(context.tokens, prefix_span.start)?;
    if context.tokens.lexeme(context.text, bang) != "!" {
        return None;
    }
    let target_start = target_span.start;
    if prefix_span.start + 1 != target_start {
        return None;
    }
    Some(vec![
        (
            prefix_span.start,
            target_start - prefix_span.start,
            String::new(),
        ),
        (property_span.start, 7, "isNotEmpty".to_owned()),
    ])
}

/// `convert.toNullAware` (`ConvertToNullAware`): `x == null ? null : x.b`
/// becomes `x?.b` — the condition's compared expression is found as a
/// source-text prefix of the non-null arm's access chain (the official
/// walk compares node texts the same way), the chain's first separator
/// gains the `?`, and the rest deletes.
pub(crate) fn convert_to_null_aware(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let conditional = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::Conditional,
    )?;
    let (condition, then_arm, else_arm, ..) = conditional_parts(context, conditional)?;
    // The condition unparenthesizes to a null comparison with exactly one
    // null side.
    let condition = unwrap_parens(context, condition)?;
    let (comparison_left, comparison_right, operator, left_span, right_span) =
        binary_parts(context, condition)?;
    let operator_lexeme = context.tokens.lexeme(context.text, operator).to_owned();
    if !matches!(operator_lexeme.as_str(), "==" | "!=") {
        return None;
    }
    let left_is_null = context.ast.kind(comparison_left) == NodeKind::LiteralNull;
    let right_is_null = context.ast.kind(comparison_right) == NodeKind::LiteralNull;
    let compared_span = match (left_is_null, right_is_null) {
        (false, true) => &left_span,
        (true, false) => &right_span,
        _ => return None,
    };
    let compared_text = slice16(context.text, compared_span)?.to_owned();
    let (null_arm, other_arm) = if operator_lexeme == "==" {
        (then_arm, else_arm)
    } else {
        (else_arm, then_arm)
    };
    let null_literal = unwrap_parens(context, null_arm)?;
    if context.ast.kind(null_literal) != NodeKind::LiteralNull {
        return None;
    }
    let chain = unwrap_parens(context, other_arm)?;
    let chain_span = trim_span16(
        context.text,
        expression_extent(context.tokens, context.text, context.ast, chain)?,
    )?;
    // Walk the chain: accumulate tokens until the accumulated text is the
    // compared expression, with the next token the chain separator.
    let mut cursor = chain_span.start;
    let mut accumulated = String::new();
    while cursor < chain_span.end {
        let index = token_at(context.tokens, cursor)?;
        let token = context.tokens.get(index)?;
        if token.utf16_offset != cursor {
            break;
        }
        let lexeme = context.tokens.lexeme(context.text, index).to_owned();
        if matches!(lexeme.as_str(), "." | "!" | "?.") {
            if accumulated == compared_text {
                break;
            }
            accumulated.push_str(&lexeme);
            cursor += utf16_count(&lexeme);
        } else {
            if accumulated != compared_text && !compared_text.starts_with(accumulated.as_str()) {
                return None;
            }
            accumulated.push_str(&lexeme);
            cursor += utf16_count(&lexeme);
            if accumulated.len() > compared_text.len() {
                return None;
            }
        }
    }
    // `accumulated == compared_text` at the chain end means a bare target
    // with no separator — nothing to null-aware.
    if accumulated != compared_text {
        return None;
    }
    let target_end = cursor;
    let conditional_span =
        expression_extent(context.tokens, context.text, context.ast, conditional)?;
    let mut edits = vec![
        (
            conditional_span.start,
            chain_span.start - conditional_span.start,
            String::new(),
        ),
        (
            chain_span.end,
            conditional_span.end - chain_span.end,
            String::new(),
        ),
    ];
    // The separator edit: `.` gains the `?`, a bare `!` postfix goes.
    if target_end < chain_span.end {
        let index = token_at(context.tokens, target_end)?;
        let lexeme = context.tokens.lexeme(context.text, index).to_owned();
        if lexeme == "." {
            edits.push((target_end, 0, "?".to_owned()));
        } else if lexeme == "!" {
            edits.push((target_end, 1, String::new()));
        }
    }
    Some(sort_edits(edits))
}

/// The inner expression when `id` is a parenthesized expression's wrapper
/// (the official `unParenthesized`).
pub(crate) fn unwrap_parens(context: &AssistContext<'_>, id: u32) -> Option<u32> {
    if context.ast.kind(id) == NodeKind::ParenthesizedExpression {
        inner_expression(context, id)
    } else {
        Some(id)
    }
}

/// `exchangeOperands` (`ExchangeOperands`): the operands of the widest
/// same-operator binary expression swap their source texts, and the
/// comparison operators mirror so the relation survives.
pub(crate) fn exchange_operands(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let binary = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::BinaryExpression,
    )?;
    let (_, _, operator, left_span, right_span) = binary_parts(context, binary)?;
    // `isOperatorSelected`: the selection sits in the operator region or
    // covers the whole expression exactly.
    let operator_selected = (context.selection.start >= left_span.end
        && context.selection.end <= right_span.start)
        || (context.selection.start
            == expression_extent(context.tokens, context.text, context.ast, binary)?.start
            && context.selection.end
                == expression_end16(context.tokens, context.text, context.ast, binary)?);
    if !operator_selected {
        return None;
    }
    // Climb to the widest same-operator expression.
    let mut wide = binary;
    loop {
        let Some(parent) = context.ast.parent(wide) else {
            break;
        };
        if context.ast.kind(parent) != NodeKind::BinaryExpression {
            break;
        }
        let Some((_, _, parent_operator, _, _)) = binary_parts(context, parent) else {
            break;
        };
        if context.tokens.lexeme(context.text, parent_operator)
            != context.tokens.lexeme(context.text, operator)
        {
            break;
        }
        wide = parent;
    }
    let wide_span = trim_span16(
        context.text,
        expression_extent(context.tokens, context.text, context.ast, wide)?,
    )?;
    let small = binary_parts(context, binary)?;
    let small_left_end = small.3.end;
    let small_right_start = small.4.start;
    let operator_lexeme = context.tokens.lexeme(context.text, operator).to_owned();
    let left_text = slice16(context.text, &(small_right_start..wide_span.end))?.to_owned();
    let right_text = slice16(context.text, &(wide_span.start..small_left_end))?.to_owned();
    let mut edits = vec![
        (wide_span.start, small_left_end - wide_span.start, left_text),
        (
            small_right_start,
            wide_span.end - small_right_start,
            right_text,
        ),
    ];
    if let Some(swapped) = swap_op(&operator_lexeme) {
        let op_start = token_start16(context.tokens, operator)?;
        edits.push((op_start, utf16_count(&operator_lexeme), swapped.to_owned()));
    }
    Some(sort_edits(edits))
}

/// `invertConditional` (`InvertConditionalExpression`): the conditional
/// found through the official bounded ancestor walk (method invocation,
/// await, boolean sub-expression, parens, await — then the conditional
/// itself) inverts its condition and swaps the arms' source texts.
pub(crate) fn invert_conditional(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let conditional = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::Conditional,
    )?;
    let (condition, _, _, condition_span, then_span, else_span) =
        conditional_parts(context, conditional)?;
    let (condition_span, inverted) =
        invert_condition0(context, condition, 15).map(|(_, text)| (condition_span, text))?;
    let then_text = slice16(context.text, &then_span)?.to_owned();
    let else_text = slice16(context.text, &else_span)?.to_owned();
    Some(sort_edits(vec![
        (
            condition_span.start,
            condition_span.end - condition_span.start,
            inverted,
        ),
        (then_span.start, then_span.end - then_span.start, else_text),
        (else_span.start, else_span.end - else_span.start, then_text),
    ]))
}

/// `invertIf` (`InvertIfStatement`): an `if` with no case clause whose
/// branches are both blocks — the condition inverts, the block sources
/// swap.
pub(crate) fn invert_if(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    if context.ast.kind(statement) != NodeKind::IfStatement {
        return None;
    }
    let parts = if_parts(context, statement)?;
    if parts.has_case || parts.else_id.is_none() {
        return None;
    }
    let else_id = parts.else_id?;
    if context.ast.kind(parts.then_id) != NodeKind::Block
        || context.ast.kind(else_id) != NodeKind::Block
    {
        return None;
    }
    let (condition_span, inverted) = invert_condition0(context, parts.condition, 15)?;
    let then_span = node_span16(context.tokens, context.text, context.ast, parts.then_id)?;
    let else_span = node_span16(context.tokens, context.text, context.ast, else_id)?;
    let then_text = slice16(context.text, &then_span)?.to_owned();
    let else_text = slice16(context.text, &else_span)?.to_owned();
    Some(sort_edits(vec![
        (
            condition_span.start,
            condition_span.end - condition_span.start,
            inverted,
        ),
        (then_span.start, then_span.end - then_span.start, else_text),
        (else_span.start, else_span.end - else_span.start, then_text),
    ]))
}

/// `splitIfConjunction` (`SplitAndCondition`): the `&&` whose wide
/// condition is the `if`'s own — the right operand moves into a nested
/// `if` around the then statements, which re-indent one level.
pub(crate) fn split_and_condition(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let binary = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::BinaryExpression,
    )?;
    let (_, _, operator, left_span, right_span) = binary_parts(context, binary)?;
    if context.tokens.lexeme(context.text, operator) != "&&" {
        return None;
    }
    let binary_span = trim_span16(
        context.text,
        expression_extent(context.tokens, context.text, context.ast, binary)?,
    )?;
    let operator_selected = (context.selection.start >= left_span.end
        && context.selection.end <= right_span.start)
        || (context.selection.start == binary_span.start
            && context.selection.end == binary_span.end);
    if !operator_selected {
        return None;
    }
    // Climb the same-operator chain to the `if`'s own condition.
    let mut wide = binary;
    loop {
        let Some(parent) = context.ast.parent(wide) else {
            break;
        };
        if context.ast.kind(parent) != NodeKind::BinaryExpression {
            break;
        }
        let Some((_, _, parent_operator, _, _)) = binary_parts(context, parent) else {
            break;
        };
        if context.tokens.lexeme(context.text, parent_operator) != "&&" {
            break;
        }
        wide = parent;
    }
    let if_statement = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        wide,
        NodeKind::IfStatement,
    )?;
    let parts = if_parts(context, if_statement)?;
    if parts.has_case || parts.condition != wide || parts.else_id.is_some() {
        return None;
    }
    let wide_span = trim_span16(
        context.text,
        expression_extent(context.tokens, context.text, context.ast, wide)?,
    )?;
    let right_source = slice16(context.text, &(right_span.start..wide_span.end))?.to_owned();
    let text = context.text;
    let ast = context.ast;
    let eol = eol_of(text);
    let prefix = line_indent_of(text, binary_span.start).to_owned();
    let indent = "  ";
    let inner_prefix = format!("{prefix}{indent}");
    let then_span = node_span16(context.tokens, text, ast, parts.then_id)?;
    let replacement = if ast.kind(parts.then_id) == NodeKind::Block {
        // One replacement of the whole block: the inner `if` opens after
        // the `{`, the statements re-indent, the inner block closes
        // before the outer `}`.
        let statements = statement_children(ast, parts.then_id);
        let first = statements.first()?;
        let last = statements.last()?;
        let first_span = node_span16(context.tokens, text, ast, *first)?;
        let last_span = node_span16(context.tokens, text, ast, *last)?;
        let first_line_start = line_start16(text, first_span.start)?;
        let lines = slice16(text, &(first_line_start..last_span.end))?.to_owned();
        format!(
            "{{{eol}{inner_prefix}if ({right_source}) {{{eol}{}{inner_prefix}}}{eol}{prefix}}}",
            reindent(
                &lines,
                &inner_prefix,
                &format!("{inner_prefix}{indent}"),
                eol,
                true
            ),
        )
    } else {
        // The dangling then statement rides after the new inner `if`,
        // its own lines shifted one level.
        let then_line =
            slice16(text, &(line_start16(text, then_span.start)?..then_span.end))?.to_owned();
        format!(
            "{eol}{inner_prefix}if ({right_source}){eol}{}",
            reindent(
                &then_line,
                &inner_prefix,
                &format!("{inner_prefix}{indent}"),
                eol,
                false
            ),
        )
    };
    let region_start = if ast.kind(parts.then_id) == NodeKind::Block {
        then_span.start
    } else {
        token_end16(
            context.tokens,
            text,
            token_at(context.tokens, parts.rparen16 - 1)?,
        )?
    };
    let region_end = if ast.kind(parts.then_id) == NodeKind::Block {
        then_span.end
    } else {
        then_span.end
    };
    // The head deletion (` && right`) plus the then-side rewrite.
    Some(sort_edits(vec![
        (left_span.end, wide_span.end - left_span.end, String::new()),
        (region_start, region_end - region_start, replacement),
    ]))
}

/// The line start (byte → UTF-16) of the line holding `offset16`.
pub(crate) fn line_start16(text: &str, offset16: u32) -> Option<u32> {
    let byte = utf16_offset_to_byte(text, offset16);
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    u32::try_from(line_start).ok()
}

/// The UTF-16 end of one token.
pub(crate) fn token_end16(tokens: &TokenTable, text: &str, index: usize) -> Option<u32> {
    utf16_end(tokens, text, index)
}

/// `convert.conditionalToIfElse` (`ReplaceConditionalWithIfElse`): the
/// conditional in a `return`, an `=` assignment, or a typed variable's
/// initializer rewrites to an `if/else` with the same arms.
pub(crate) fn conditional_to_if_else(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    let text = context.text;
    let ast = context.ast;
    let eol = eol_of(text);
    let statement_span = node_span16(context.tokens, text, ast, statement)?;
    let prefix = line_indent_of(text, statement_span.start).to_owned();
    let indent = "  ";
    let arms = |conditional: u32| -> Option<(String, String, String)> {
        let (_, _, _, condition_span, then_span, else_span) =
            conditional_parts(context, conditional)?;
        let condition_text = slice16(text, &condition_span)?.to_owned();
        Some((
            condition_text,
            slice16(text, &then_span)?.to_owned(),
            slice16(text, &else_span)?.to_owned(),
        ))
    };
    match ast.kind(statement) {
        NodeKind::ReturnStatement => {
            // The return's expression is the run's last row (post-order).
            let expression = inner_expression(context, statement)?;
            if ast.kind(expression) != NodeKind::Conditional {
                return None;
            }
            let (condition, then_arm, else_arm) = arms(expression)?;
            Some(vec![(
                statement_span.start,
                statement_span.end - statement_span.start,
                format!(
                    "if ({condition}) {{{eol}{prefix}{indent}return {then_arm};{eol}{prefix}}} else {{{eol}{prefix}{indent}return {else_arm};{eol}{prefix}}}"
                ),
            )])
        }
        NodeKind::ExpressionStatement => {
            let expression = inner_expression(context, statement)?;
            if ast.kind(expression) != NodeKind::Assignment {
                return None;
            }
            let (target, right_root, operator, _, right_span) = binary_parts(context, expression)?;
            if context.tokens.lexeme(text, operator) != "=" {
                return None;
            }
            if ast.kind(right_root) != NodeKind::Conditional {
                return None;
            }
            // The conditional must BE the assignment's right side.
            let conditional_span = trim_span16(
                text,
                expression_extent(context.tokens, text, ast, right_root)?,
            )?;
            if conditional_span != right_span {
                return None;
            }
            let target_span =
                trim_span16(text, expression_extent(context.tokens, text, ast, target)?)?;
            let target_text = slice16(text, &target_span)?.to_owned();
            let (condition, then_arm, else_arm) = arms(right_root)?;
            Some(vec![(
                statement_span.start,
                statement_span.end - statement_span.start,
                format!(
                    "if ({condition}) {{{eol}{prefix}{indent}{target_text} = {then_arm};{eol}{prefix}}} else {{{eol}{prefix}{indent}{target_text} = {else_arm};{eol}{prefix}}}"
                ),
            )])
        }
        NodeKind::VariablesDeclaration => {
            // The typed face only (an explicit `Type` child must exist);
            // the inferred-type leg needs the element facts upstream
            // writes through `writeType`.
            logical_child_of_kind(context, statement, NodeKind::Type)?;
            let mut edits = Vec::new();
            for variable in logical_children(context, statement) {
                if ast.kind(variable) != NodeKind::InitializedIdentifier {
                    continue;
                }
                let mut parts = logical_children(context, variable).into_iter();
                let name = parts.next()?;
                let initializer = parts.next()?;
                let initializer = inner_expression(context, initializer).unwrap_or(initializer);
                if ast.kind(initializer) != NodeKind::Conditional {
                    continue;
                }
                let name_span = node_span16(context.tokens, text, ast, name)?;
                let initializer_span = trim_span16(
                    text,
                    expression_extent(context.tokens, text, ast, initializer)?,
                )?;
                if context.selection.start > name_span.end {
                    return None;
                }
                let (condition, then_arm, else_arm) = arms(initializer)?;
                let name_text = slice16(text, &name_span)?.to_owned();
                edits.push((
                    name_span.end,
                    initializer_span.end - name_span.end,
                    String::new(),
                ));
                edits.push((
                    statement_span.end,
                    0,
                    format!(
                        "{eol}{prefix}if ({condition}) {{{eol}{prefix}{indent}{name_text} = {then_arm};{eol}{prefix}}} else {{{eol}{prefix}{indent}{name_text} = {else_arm};{eol}{prefix}}}"
                    ),
                ));
                break;
            }
            (!edits.is_empty()).then(|| sort_edits(edits))
        }
        _ => None,
    }
}

/// `convert.ifElseToConditional` (`ReplaceIfElseWithConditional`): an
/// `if/else` whose branches both `return` — or both assign the same
/// target with `=` — rewrites to the conditional expression.
pub(crate) fn if_else_to_conditional(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    if context.ast.kind(statement) != NodeKind::IfStatement {
        return None;
    }
    let parts = if_parts(context, statement)?;
    if parts.has_case {
        return None;
    }
    let then_statement = single_statement(context.ast, parts.then_id)?;
    let else_id = parts.else_id?;
    let else_statement = single_statement(context.ast, else_id)?;
    let text = context.text;
    let ast = context.ast;
    let condition = condition_text(context, &parts)?.to_owned();
    let arm_expression = |id: u32| -> Option<String> {
        let expression = inner_expression(context, id)?;
        let span = trim_span16(
            text,
            expression_extent(context.tokens, text, ast, expression)?,
        )?;
        Some(slice16(text, &span)?.to_owned())
    };
    let if_span = node_span16(context.tokens, text, ast, statement)?;
    match (ast.kind(then_statement), ast.kind(else_statement)) {
        (NodeKind::ReturnStatement, NodeKind::ReturnStatement) => {
            let then_source = arm_expression(then_statement)?;
            let else_source = arm_expression(else_statement)?;
            // The official edit ends at the `if` node's end — the brace
            // immediately following keeps its leading space, so the
            // replacement lands its own trailing one (`c;  }`).
            Some(vec![(
                if_span.start,
                if_span.end - if_span.start,
                format!("return {condition} ? {then_source} : {else_source}; "),
            )])
        }
        (NodeKind::ExpressionStatement, NodeKind::ExpressionStatement) => {
            let assignment = |id: u32| -> Option<(String, String)> {
                let expression = inner_expression(context, id)?;
                (ast.kind(expression) == NodeKind::Assignment).then_some(())?;
                let (target, _, operator, _, right_span) = binary_parts(context, expression)?;
                (context.tokens.lexeme(text, operator) == "=").then_some(())?;
                let target_span =
                    trim_span16(text, expression_extent(context.tokens, text, ast, target)?)?;
                Some((
                    slice16(text, &target_span)?.to_owned(),
                    slice16(text, &right_span)?.to_owned(),
                ))
            };
            let (then_target, then_source) = assignment(then_statement)?;
            let (else_target, else_source) = assignment(else_statement)?;
            if then_target != else_target {
                return None;
            }
            Some(vec![(
                if_span.start,
                if_span.end - if_span.start,
                format!("{then_target} = {condition} ? {then_source} : {else_source}; "),
            )])
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Assist producers — the join family and the pattern face
// ---------------------------------------------------------------------------

/// The statements of a then-branch as a lines-range slice plus the
/// one-level re-indent (the official `getLinesRangeStatements` +
/// `indentSourceLeftRight` pair). Returns the re-indented source.
pub(crate) fn reindent_branch(context: &AssistContext<'_>, branch: u32) -> Option<String> {
    let statements = if context.ast.kind(branch) == NodeKind::Block {
        statement_children(context.ast, branch)
    } else {
        vec![branch]
    };
    let first = statements.first()?;
    let last = statements.last()?;
    let first_span = node_span16(context.tokens, context.text, context.ast, *first)?;
    let last_span = node_span16(context.tokens, context.text, context.ast, *last)?;
    let first_line = line_start16(context.text, first_span.start)?;
    let lines = slice16(context.text, &(first_line..last_span.end))?.to_owned();
    // One nesting level goes away with the joined `if`.
    Some(reindent(&lines, "  ", "", eol_of(context.text), false))
}

/// The shared `join` rewrite body (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/join_if_with_*.dart):
/// the merged condition (each side paren-wrapped when it binds looser
/// than `&&`, an outer case clause spliced as `case … when …`), the
/// branch statements re-indented one level, and the outer `if` replaced
/// whole.
pub(crate) fn join_rewrite(
    context: &AssistContext<'_>,
    outer: u32,
    outer_parts: &IfParts,
    inner_condition_span: &core::ops::Range<u32>,
    branch: u32,
) -> Option<(u32, u32, String)> {
    let outer_span = node_span16(context.tokens, context.text, context.ast, outer)?;
    let outer_condition = condition_text(context, outer_parts)?.to_owned();
    let mut outer_condition_text = outer_condition.clone();
    if binds_looser_than_and(context, &outer_parts.condition_span) {
        outer_condition_text = format!("({outer_condition_text})");
    }
    let mut inner_condition_text = slice16(context.text, inner_condition_span)?.to_owned();
    if binds_looser_than_and(context, inner_condition_span) {
        inner_condition_text = format!("({inner_condition_text})");
    }
    // The outer case clause splices before the joined `&&` (`case P
    // when W` — the when expression wraps when loose).
    let condition = if outer_parts.has_case {
        let case_region = outer_parts.condition_span.end..outer_parts.rparen16;
        let case_full = slice16(context.text, &case_region)?.to_owned();
        let case_text = case_full.trim().to_owned();
        match case_text.split_once(" when ") {
            Some((pattern, when)) => {
                let when_offset = case_region.start
                    + u32::try_from(case_full.find(" when ").unwrap_or(0)).ok()?;
                let when_span = when_offset + 6..outer_parts.rparen16;
                let mut when_text = when.trim().to_owned();
                if binds_looser_than_and(context, &when_span) {
                    when_text = format!("({when_text})");
                }
                format!(
                    "{outer_condition_text} case {pattern} when {when_text} && {inner_condition_text}"
                )
            }
            None => format!("{outer_condition_text} {case_text} when {inner_condition_text}"),
        }
    } else {
        format!("{outer_condition_text} && {inner_condition_text}")
    };
    let branch_source = reindent_branch(context, branch)?;
    let eol = eol_of(context.text);
    let prefix = line_indent_of(context.text, outer_span.start).to_owned();
    let replacement = format!("if ({condition}) {{\n{branch_source}{eol}{prefix}}}");
    Some((
        outer_span.start,
        outer_span.end - outer_span.start,
        replacement,
    ))
}

/// `joinWithInnerIf` (`JoinIfWithInner`): the `if` whose then holds a
/// single childless `if` — the conditions merge with `&&`.
pub(crate) fn join_with_inner_if(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let target = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::IfStatement,
    )?;
    let parts = if_parts(context, target)?;
    if parts.else_id.is_some() {
        return None;
    }
    let inner = single_statement(context.ast, parts.then_id)?;
    if context.ast.kind(inner) != NodeKind::IfStatement {
        return None;
    }
    let inner_parts = if_parts(context, inner)?;
    if inner_parts.has_case || inner_parts.else_id.is_some() {
        return None;
    }
    let edit = join_rewrite(
        context,
        target,
        &parts,
        &inner_parts.condition_span,
        inner_parts.then_id,
    )?;
    Some(vec![edit])
}

/// `joinWithOuterIf` (`JoinIfWithOuter`): the `if` that is the outer
/// `if`'s sole then (directly or through a one-statement block) — the
/// conditions merge with `&&`, the outer statement replaces whole.
pub(crate) fn join_with_outer_if(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let target = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::IfStatement,
    )?;
    let parts = if_parts(context, target)?;
    if parts.has_case || parts.else_id.is_some() {
        return None;
    }
    // The outer if: through an optional one-statement block.
    let mut parent = context.ast.parent(target)?;
    if context.ast.kind(parent) == NodeKind::Block {
        (statement_children(context.ast, parent).len() == 1).then_some(())?;
        parent = context.ast.parent(parent)?;
    }
    if context.ast.kind(parent) != NodeKind::IfStatement {
        return None;
    }
    let outer_parts = if_parts(context, parent)?;
    if outer_parts.else_id.is_some() {
        return None;
    }
    let edit = join_rewrite(
        context,
        parent,
        &outer_parts,
        &parts.condition_span,
        parts.then_id,
    )?;
    Some(vec![edit])
}

/// The else-if chain behind one `if` statement (the official
/// `_getStatements`): each `else if` extends the chain, the first
/// non-`if` else ends it.
pub(crate) fn else_if_chain(context: &AssistContext<'_>, inner: u32) -> Option<Vec<u32>> {
    let mut chain = vec![inner];
    let mut current = inner;
    loop {
        let parts = if_parts(context, current)?;
        let Some(else_id) = parts.else_id else {
            break;
        };
        chain.push(else_id);
        if context.ast.kind(else_id) == NodeKind::IfStatement {
            current = else_id;
        } else {
            break;
        }
    }
    Some(chain)
}

/// The join-else rewrite body (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/join_else_with_if.dart):
/// the `else` keyword through the else block's end becomes the
/// ` else if (…) { … }` chain; the moved blocks re-indent one level.
/// Comment-bearing regions drop (the official producer's comment faces
/// are beyond the composed-replacement shape — never mis-placed).
pub(crate) fn join_else_rewrite(
    context: &AssistContext<'_>,
    outer_parts: &IfParts,
    else_block: u32,
    inner: u32,
) -> Option<(u32, u32, String)> {
    let chain = else_if_chain(context, inner)?;
    let else_kw = outer_parts.else_keyword.as_ref()?.clone();
    let else_span = node_span16(context.tokens, context.text, context.ast, else_block)?;
    let text = context.text;
    let eol = eol_of(text);
    let prefix = line_indent_of(text, else_span.start).to_owned();
    let indent = "  ";
    let mut source = String::new();
    for statement in chain {
        source.push_str(" else ");
        let mut body = statement;
        if context.ast.kind(statement) == NodeKind::IfStatement {
            let parts = if_parts(context, statement)?;
            let mut condition = condition_text(context, &parts)?.to_owned();
            if parts.has_case {
                let case_text = slice16(text, &(parts.condition_span.end..parts.rparen16))?
                    .trim()
                    .to_owned();
                condition = format!("{condition} {case_text}");
            }
            source.push_str(&format!("if ({condition}) "));
            body = parts.then_id;
        }
        if context.ast.kind(body) == NodeKind::Block {
            let block_span = node_span16(context.tokens, text, context.ast, body)?;
            let block_text = slice16(text, &block_span)?.to_owned();
            if block_text.contains("//") || block_text.contains("/*") {
                return None;
            }
            let shifted = reindent(&block_text, indent, "", eol, false);
            source.push_str(shifted.trim_end());
        } else {
            let body_span = node_span16(context.tokens, text, context.ast, body)?;
            let body_text = slice16(text, &body_span)?.to_owned();
            if body_text.contains("//") || body_text.contains("/*") {
                return None;
            }
            source.push_str(&format!("{{{eol}{indent}{body_text}{eol}{prefix}}}"));
        }
    }
    // The replacement starts one character before the `else` keyword —
    // the separating space rides along (the official
    // `startOffsetEndOffset(elseKeyword.offset - 1, …)`).
    let start = else_kw.start.checked_sub(1)?;
    Some((start, else_span.end - start, source))
}

/// `inlineElseBlock` (`JoinElseWithIf`): the selection sits on the
/// `else` keyword whose block holds one `if` — the blocks join into
/// `else if`.
pub(crate) fn inline_else_block(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    if context.ast.kind(statement) != NodeKind::IfStatement {
        return None;
    }
    let parts = if_parts(context, statement)?;
    let else_keyword = parts.else_keyword.as_ref()?.clone();
    if !else_keyword.contains(&context.selection.start) {
        return None;
    }
    let else_id = parts.else_id?;
    if context.ast.kind(else_id) != NodeKind::Block {
        return None;
    }
    let statements = statement_children(context.ast, else_id);
    if statements.len() != 1 || context.ast.kind(statements[0]) != NodeKind::IfStatement {
        return None;
    }
    let edit = join_else_rewrite(context, &parts, else_id, statements[0])?;
    Some(vec![edit])
}

/// `inlineEnclosingElseBlock` (`JoinIfWithElse`): the selection sits on
/// the inner `if` keyword whose statement is the sole statement of the
/// enclosing `if`'s else block — the same join from the inside.
pub(crate) fn inline_enclosing_else_block(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    if context.ast.kind(statement) != NodeKind::IfStatement {
        return None;
    }
    // The selection sits on the inner `if` keyword.
    let span = node_span16(context.tokens, context.text, context.ast, statement)?;
    let if_keyword = token_at(context.tokens, span.start)?;
    if context.tokens.lexeme(context.text, if_keyword) != "if" {
        return None;
    }
    let keyword_start = token_start16(context.tokens, if_keyword)?;
    let keyword_end = utf16_end(context.tokens, context.text, if_keyword)?;
    if !keyword_range_contains(keyword_start, keyword_end, context.selection.start) {
        return None;
    }
    let block = context.ast.parent(statement)?;
    if context.ast.kind(block) != NodeKind::Block {
        return None;
    }
    let statements = statement_children(context.ast, block);
    if statements.len() != 1 || statements[0] != statement {
        return None;
    }
    let outer = context.ast.parent(block)?;
    if context.ast.kind(outer) != NodeKind::IfStatement {
        return None;
    }
    let outer_parts = if_parts(context, outer)?;
    if outer_parts.else_id != Some(block) {
        return None;
    }
    let edit = join_else_rewrite(context, &outer_parts, block, statement)?;
    Some(vec![edit])
}

/// The width-0 keyword containment (the official `range.token(…).contains`).
pub(crate) fn keyword_range_contains(start: u32, end: u32, offset: u32) -> bool {
    (start..end).contains(&offset)
}

/// `remove.unnecessaryName` (`RemoveUnnecessaryName`): the named pattern
/// field whose name restates the declared variable's name — `Foo(x: var
/// x)` — drops the name token, leaving the shorthand `Foo(: var x)`. The
/// flat layer anchors the `PatternField` on its pattern (the `x:` name
/// sits *before* the span head), so the name is the identifier directly
/// before the field's colon payload word.
pub(crate) fn remove_unnecessary_name(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let leaf_span = node_span16(context.tokens, context.text, context.ast, node)?;
    // The covering PatternField: the selection sits on the field's name or
    // inside its pattern. Field rows live beside their name inside the
    // enclosing pattern's window.
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let mut scope = ast.parent(node);
    let mut found = None;
    'walk: while let Some(owner) = scope {
        for field in ast.children(owner) {
            if ast.kind(field) != NodeKind::PatternField {
                continue;
            }
            // Payload word 0 carries the field's name token (the
            // `x` of `x:` — the bridge stores the colon-side anchor).
            let Some(&name) = ast.payload(field).first() else {
                continue;
            };
            let name_index = name as usize;
            let Some(name_token) = tokens.get(name_index) else {
                continue;
            };
            let Some(name_end) = utf16_end(tokens, text, name_index) else {
                continue;
            };
            let name_span = name_token.utf16_offset..name_end;
            let field_span = node_span16(tokens, text, ast, field)?;
            let on_name = name_span.start <= leaf_span.start && leaf_span.end <= name_span.end;
            let in_pattern = field_span.start <= leaf_span.start && leaf_span.end <= field_span.end;
            if on_name || in_pattern {
                found = Some((field, name_span));
                break 'walk;
            }
        }
        scope = ast.parent(owner);
    }
    let (field, name_span) = found?;
    // The field's declared-variable pattern shares the field's span; the
    // declared name is the pattern's last token (`var x` lands `x` last).
    let field_span = node_span16(tokens, text, ast, field)?;
    let owner = ast.parent(field)?;
    let pattern = ast.children(owner).find(|&row| {
        ast.kind(row) == NodeKind::DeclaredVariablePattern
            && node_span16(tokens, text, ast, row) == Some(field_span.clone())
    })?;
    let last = ast.token_range(pattern).end.checked_sub(1)? as usize;
    let variable_start = tokens.get(last)?.utf16_offset;
    let variable_span = variable_start..utf16_end(tokens, text, last)?;
    if slice16(text, &name_span)? != slice16(text, &variable_span)? {
        return None;
    }
    Some(vec![(
        name_span.start,
        name_span.end - name_span.start,
        String::new(),
    )])
}

// ---------------------------------------------------------------------------
// Assist producers — declarations, bodies, and the curly-brace face
// ---------------------------------------------------------------------------

/// The UTF-16 end of the token immediately before `offset16`.
pub(crate) fn prev_token_end16(tokens: &TokenTable, text: &str, offset16: u32) -> Option<u32> {
    let index = token_at(tokens, offset16)?;
    utf16_end(tokens, text, index.checked_sub(1)?)
}

/// `remove.typeAnnotation` (`RemoveTypeAnnotation`): the written type of
/// a variable declaration (or a formal parameter) gives way — `var `
/// replaces it when no other keyword stands, `final`/`const` keep and the
/// type alone deletes. The typed-literal initializer regains the type's
/// own type arguments (a bare `{…}` stays ambiguous and drops). The
/// dot-shorthand and instance-creation faces need element facts and
/// never fire here.
pub(crate) fn remove_type_annotation(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    // The parameter face: the type before the parameter's name deletes.
    if let Some(parameter) = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::FormalParameter,
    ) {
        if context.ast.kind(parameter) != NodeKind::FunctionTypedFormalParameter {
            let mut children = logical_children(context, parameter).into_iter();
            if let Some(first) = children.next()
                && context.ast.kind(first) == NodeKind::Type
                && let Some(next) = children.next()
                && let Some(type_span) =
                    node_span16(context.tokens, context.text, context.ast, first)
                && let Some(next_span) =
                    node_span16(context.tokens, context.text, context.ast, next)
            {
                return Some(vec![(
                    type_span.start,
                    next_span.start - type_span.start,
                    String::new(),
                )]);
            }
        }
    }
    // The declaration-list face.
    let declaration = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::VariablesDeclaration,
    )?;
    let type_node = logical_child_of_kind(context, declaration, NodeKind::Type)?;
    let type_span = node_span16(context.tokens, context.text, context.ast, type_node)?;
    let variables: Vec<u32> = logical_children(context, declaration)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier)
        .collect();
    let first = *variables.first()?;
    let mut parts = logical_children(context, first).into_iter();
    let name = parts.next()?;
    let initializer = parts.next()?;
    // The initializer wraps the value expression (`VariableInitializer`
    // rides over it) — read through the wrapper.
    let initializer = inner_expression(context, initializer).unwrap_or(initializer);
    let name_span = node_span16(context.tokens, context.text, context.ast, name)?;
    // The `VariableInitializer` wrapper's extent covers its `=` head — the
    // value region starts after it (a `LiteralList`'s own row can be
    // empty, the real `[]` text living on the synthetic `Index` token).
    let mut value_start =
        expression_extent_start(context.tokens, context.text, context.ast, initializer)?;
    if context.ast.kind(initializer) == NodeKind::VariableInitializer {
        value_start = utf16_end(
            context.tokens,
            context.text,
            context.ast.token_range(initializer).start as usize,
        )?;
    }
    let initializer_span = trim_span16(
        context.text,
        value_start..expression_end16(context.tokens, context.text, context.ast, initializer)?,
    )?;
    // The selection sits before the first variable's name end, and the
    // variable must have an initializer (the only other source for the
    // type).
    if context.selection.start > name_span.end || initializer_span.end <= initializer_span.start {
        return None;
    }
    // The type's own type arguments, when written: the flat layer keeps
    // them as the sibling right after the `Type` head inside the
    // declaration's run (`List<int>` → `Type` then `TypeArguments`).
    let type_arguments = expression_rows(context.ast, declaration)
        .into_iter()
        .find(|&row| {
            context.ast.kind(row) == NodeKind::TypeArguments
                && node_span16(context.tokens, context.text, context.ast, row)
                    .is_some_and(|span| span.start == type_span.end)
        })
        .and_then(|id| node_span16(context.tokens, context.text, context.ast, id))
        .and_then(|span| slice16(context.text, &span).map(str::to_owned));
    // A set/map literal initializer stays ambiguous without its type
    // arguments (the official guard); a list literal regains them after
    // the bracket.
    let initializer_text = slice16(context.text, &initializer_span)?;
    let mut insertion: Option<(u32, String)> = None;
    if let Some(arguments) = type_arguments.as_ref()
        && initializer_text.starts_with('[')
    {
        insertion = Some((initializer_span.start + 1, arguments.clone()));
    } else if initializer_text.starts_with('{') {
        let Some(arguments) = type_arguments else {
            return None;
        };
        insertion = Some((initializer_span.start + 1, arguments));
    }
    // The keyword before the type decides the replacement.
    let keyword = slice16(
        context.text,
        &(node_span16(context.tokens, context.text, context.ast, declaration)?.start
            ..type_span.start),
    )?
    .trim();
    let replacement = if keyword.is_empty() { "var " } else { "" };
    let mut edits = vec![(
        type_span.start,
        name_span.start - type_span.start,
        replacement.to_owned(),
    )];
    if let Some((at, arguments)) = insertion {
        edits.push((at, 0, arguments));
    }
    Some(sort_edits(edits))
}

/// Whether every child of the parameter list carries an explicit type
/// (the official `_allParametersHaveTypes`; function-typed parameters
/// pass through).
pub(crate) fn all_parameters_typed(context: &AssistContext<'_>, list: u32) -> bool {
    logical_children(context, list)
        .into_iter()
        .all(|parameter| {
            context.ast.kind(parameter) == NodeKind::FunctionTypedFormalParameter
                || logical_child_of_kind(context, parameter, NodeKind::Type).is_some()
        })
}

/// `convert.toGenericFunctionSyntax` (`ConvertToGenericFunctionSyntax`):
/// the old-style typedef or the function-typed parameter rewrites to
/// `Function` syntax — `typedef int F(int x);` becomes `F = int
/// Function(int x);`, `void cb(int x)` becomes `void Function(int x) cb`.
pub(crate) fn convert_to_generic_function_syntax(
    context: &AssistContext<'_>,
) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    // The nearest enclosing face decides: a `FormalParameters` hit bails
    // (selection inside a parameter list), a `Typedef` or
    // `FunctionTypedFormalParameter` hit takes its face (the official
    // nearest-kind walk ordering).
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let leaf_span = node_span16(tokens, text, ast, node)?;
    enum Face {
        Bail,
        Typedef(u32),
        TypedParameter(u32),
    }
    let mut face = Face::Bail;
    let mut scope = Some(node);
    'bail: while let Some(owner) = scope {
        let face_of = |row: u32| match ast.kind(row) {
            NodeKind::FormalParameters => Some(Face::Bail),
            NodeKind::Typedef => Some(Face::Typedef(row)),
            NodeKind::FunctionTypedFormalParameter => Some(Face::TypedParameter(row)),
            _ => None,
        };
        // The window rows sit deeper than the owner — they answer first
        // (the official nearest-ancestor order), the owner last.
        for row in ast.children(owner) {
            let Some(hit) = face_of(row) else {
                continue;
            };
            let Some(extent) = expression_extent(tokens, text, ast, row) else {
                continue;
            };
            if extent.start <= leaf_span.start && leaf_span.end <= extent.end {
                face = hit;
                break 'bail;
            }
        }
        if owner != node
            && let Some(hit) = face_of(owner)
            && let Some(extent) = expression_extent(tokens, text, ast, owner)
            && extent.start <= leaf_span.start
            && leaf_span.end <= extent.end
        {
            face = hit;
            break 'bail;
        }
        scope = ast.parent(owner);
    }
    let typed_list = |list: u32| -> Option<String> {
        all_parameters_typed(context, list).then(|| ())?;
        let span = node_span16(tokens, text, ast, list)?;
        Some(slice16(text, &span)?.to_owned())
    };
    // The typedef face.
    if let Face::Typedef(typedef) = face {
        let span = node_span16(tokens, text, ast, typedef)?;
        let parameters = logical_child_of_kind(context, typedef, NodeKind::FormalParameters)?;
        let parameters_text = typed_list(parameters)?;
        // The name ends before the parameters: the last identifier child
        // before them (type-variable markers ride between, never an
        // identifier).
        let mut name_end = None;
        for child in logical_children(context, typedef) {
            if child == parameters {
                break;
            }
            if ast.kind(child) == NodeKind::Identifier {
                name_end = Some(node_span16(tokens, text, ast, child)?.end);
            }
        }
        let name_end = name_end?;
        // The head between the `typedef` keyword and the name: the
        // return type, when written. The replacement starts after the
        // keyword's trailing whitespace — it must not eat it.
        let first = token_at(tokens, span.start)?;
        let first_end = utf16_end(tokens, text, first)?;
        let head = slice16(text, &(first_end..name_end))?;
        let head_start = first_end + u32::try_from(head.len() - head.trim_start().len()).ok()?;
        let body = head.split_whitespace().collect::<Vec<_>>();
        let (return_type, name) = match body.len() {
            0 => return None,
            1 => (None, body[0]),
            _ => (Some(body[0]), body[body.len() - 1]),
        };
        let semi = token_at(tokens, span.end - 1)?;
        let semi_start = token_start16(tokens, semi)?;
        let replacement = match return_type {
            Some(return_type) => format!("{name} = {return_type} Function{parameters_text}"),
            None => format!("{name} = Function{parameters_text}"),
        };
        return Some(vec![(head_start, semi_start - head_start, replacement)]);
    }
    // The function-typed parameter face.
    let Face::TypedParameter(parameter) = face else {
        return None;
    };
    let span = expression_extent(tokens, text, ast, parameter)?;
    // Field/super formals keep their receiver prefix — beyond this face.
    let head_index = ast.token_range(parameter).start as usize;
    let head = tokens.lexeme(text, head_index);
    if head == "this" || head == "super" {
        return None;
    }
    let parameters = logical_child_of_kind(context, parameter, NodeKind::FormalParameters)?;
    let parameters_text = typed_list(parameters)?;
    // The trailing `?`.
    let question = tokens.lexeme(text, (ast.token_range(parameter).end - 1) as usize) == "?";
    // The name is the parameter's head identifier (`void cb(int x)`
    // anchors on `cb`); the return type is the logical child before it.
    let name_span = tokens.get(head_index).map(|t| {
        let start = t.utf16_offset;
        start..utf16_end(tokens, text, head_index).unwrap_or(start)
    })?;
    let return_type = logical_children(context, parameter)
        .into_iter()
        .find(|&child| {
            child != parameters
                && matches!(
                    ast.kind(child),
                    NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType
                )
        })
        .and_then(|child| node_span16(tokens, text, ast, child))
        .and_then(|span| slice16(text, &span).map(str::to_owned))
        .unwrap_or_default();
    let name = slice16(text, &name_span)?;
    let modifiers = slice16(text, &(span.start..name_span.start))?
        .split_whitespace()
        .filter(|word| *word != return_type.as_str() || return_type.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let modifiers = if modifiers.is_empty() {
        String::new()
    } else {
        format!("{modifiers} ")
    };
    let return_prefix = if return_type.is_empty() {
        String::new()
    } else {
        format!("{return_type} ")
    };
    let question_mark = if question { "?" } else { "" };
    Some(vec![(
        span.start,
        span.end - span.start,
        format!("{modifiers}{return_prefix}Function{parameters_text}{question_mark} {name}"),
    )])
}

/// `convert.bodyToBlock` (`ConvertIntoBlockBody`): the empty `;` body
/// grows the official TODO block (plus `throw UnimplementedError();`
/// behind a non-void return type), the `=>` body rewrites to a block
/// with the `return` the declared (non-void) return type asks for. The
/// set-literal registration's element-per-element face stays out.
pub(crate) fn convert_to_block_body(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let tokens = context.tokens;
    let text = context.text;
    let ast = context.ast;
    let member = |body: u32| -> Option<u32> {
        let member = ast.parent(body)?;
        matches!(
            ast.kind(member),
            NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::FunctionExpression
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::TopLevelMethod
        )
        .then_some(member)
    };
    // The declared name: the identifier child just before the parameter
    // list, extended left over `A.named` receivers.
    let member_name = |member: u32| -> Option<String> {
        let parameters = logical_child_of_kind(context, member, NodeKind::FormalParameters)?;
        let mut name_span = None;
        for child in logical_children(context, member) {
            if child == parameters {
                break;
            }
            if ast.kind(child) == NodeKind::Identifier
                && let Some(span) = node_span16(tokens, text, ast, child)
            {
                name_span = Some(span);
            }
        }
        let mut span = name_span?;
        while let Some(index) = token_at(tokens, span.start)
            && index > 0
            && tokens.lexeme(text, index - 1) == "."
        {
            let dot = index - 1;
            let receiver = tokens.get(dot.checked_sub(1)?)?;
            span.start = receiver.utf16_offset;
        }
        Some(slice16(text, &span)?.to_owned())
    };
    // The declared non-void face: a written `void` return type (methods)
    // suppresses `return` and the throw; constructors and closures are
    // never void.
    let declared_void = |member: u32| -> bool {
        matches!(
            ast.kind(member),
            NodeKind::Method | NodeKind::TopLevelMethod
        ) && logical_child_of_kind(context, member, NodeKind::VoidType).is_some()
    };
    let eol = eol_of(text);
    // The empty `;` body face.
    if let Some(body) = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::EmptyBody)
        && let Some(owner) = member(body)
    {
        let body_span = node_span16(tokens, text, ast, body)?;
        let semi_end = body_span.end;
        let previous_end = prev_token_end16(tokens, text, body_span.start)?;
        let member_span = node_span16(tokens, text, ast, owner)?;
        let prefix = line_indent_of(text, member_span.start).to_owned();
        let name = member_name(owner)?;
        let mut replacement = format!(" {{{eol}{prefix}  // TODO: implement {name}");
        if !declared_void(owner) {
            replacement.push_str(&format!("{eol}{prefix}  throw UnimplementedError();"));
        }
        replacement.push_str(&format!("{eol}{prefix}}}"));
        return Some(vec![(previous_end, semi_end - previous_end, replacement)]);
    }
    // The `=>` expression body face.
    let body = ancestor_or_self_kind(tokens, text, ast, node, NodeKind::ExpressionBody)?;
    let owner = member(body)?;
    let body_span = node_span16(tokens, text, ast, body)?;
    let expression = inner_expression(context, body)?;
    let expression_span = trim_span16(
        text,
        expression_extent(context.tokens, text, ast, expression)?,
    )?;
    // Deep in a large expression the assist does not reach (the official
    // `selectionOffset >= returnValue.offset` guard).
    if context.selection.start >= expression_span.start {
        return None;
    }
    // Constructors never take this face (a redirecting factory's shape).
    if ast.kind(owner) == NodeKind::Constructor {
        return None;
    }
    let body_start = token_at(tokens, body_span.start)?;
    let body_head = tokens.lexeme(text, body_start).to_owned();
    let asynchronous = body_head == "async";
    let previous_end = if asynchronous {
        prev_token_end16(tokens, text, body_span.start)?
    } else {
        prev_token_end16(tokens, text, body_span.start)?
    };
    let member_span = node_span16(tokens, text, ast, owner)?;
    let prefix = line_indent_of(text, member_span.start).to_owned();
    let expression_text = slice16(text, &expression_span)?.to_owned();
    let return_prefix = if declared_void(owner) {
        String::new()
    } else {
        "return ".to_owned()
    };
    let async_prefix = if asynchronous { "async " } else { "" };
    let replacement =
        format!(" {async_prefix}{{{eol}{prefix}  {return_prefix}{expression_text};{eol}{prefix}}}");
    Some(vec![(
        previous_end,
        body_span.end - previous_end,
        replacement,
    )])
}

/// `surround.curlyBraces` (`UseCurlyBraces`, the #1580 member): the
/// brace-less `if`/`for`/`while`/`do` body gains a block — ` {` and the
/// re-flanked `}` compose into one replacement over the body's region.
/// Trailing same-line comments drop the assist (the official producer
/// places its closing brace after them).
pub(crate) fn use_curly_braces(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let statement = statement_at(context)?;
    let kind = context.ast.kind(statement);
    let parent = context.ast.parent(statement);
    // The body statement of a loop: the last statement-kind child (a `for`
    // carries its `ForParts` declarations as statement-kind children
    // before the body).
    let body_of = |id: u32| -> Option<u32> { statement_children(context.ast, id).last().copied() };
    // The composed block replacement over `[left_end16, region_end16)`.
    let brace = |left_end16: u32, body: u32| -> Option<(u32, u32, String)> {
        let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
        if context.ast.kind(body) == NodeKind::Block || body_span.end <= left_end16 {
            return None;
        }
        // A trailing same-line comment after the body drops the assist.
        let after = slice16(
            context.text,
            &(body_span.end..body_span.end + 64.min(utf16_len_of(context.text) - body_span.end)),
        )?;
        let to_line_end = after.split('\n').next().unwrap_or("");
        if to_line_end.contains("//") {
            return None;
        }
        let body_text = slice16(context.text, &body_span)?.to_owned();
        let prefix = line_indent_of(context.text, body_span.start).to_owned();
        let eol = eol_of(context.text);
        let replacement = format!(" {{{eol}{prefix}  {body_text}{eol}{prefix}}}");
        Some((left_end16, body_span.end - left_end16, replacement))
    };
    // do-while: the region runs to the `while` keyword.
    let do_face = |id: u32| -> Option<(u32, u32, String)> {
        let span = node_span16(context.tokens, context.text, context.ast, id)?;
        let do_keyword = token_at(context.tokens, span.start)?;
        let do_end = utf16_end(context.tokens, context.text, do_keyword)?;
        let body = body_of(id)?;
        let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
        // Bounded by the table length — the scan always advances.
        let while_index = (token_at(context.tokens, body_span.end)?
            ..context.tokens.as_slice().len())
            .find(|index| word_at(context.tokens, context.text, *index, "while"))?;
        let while_start = token_start16(context.tokens, while_index)?;
        let body_text = slice16(context.text, &body_span)?.to_owned();
        if context.ast.kind(body) == NodeKind::Block {
            return None;
        }
        let prefix = line_indent_of(context.text, body_span.start).to_owned();
        let eol = eol_of(context.text);
        Some((
            do_end,
            while_start - do_end,
            format!(" {{{eol}{prefix}  {body_text}{eol}{prefix}}} "),
        ))
    };
    // The while/for left edge: the token before the body (the `)`).
    let loop_face = |id: u32| -> Option<(u32, u32, String)> {
        let body = body_of(id)?;
        let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
        let left_end = prev_token_end16(context.tokens, context.text, body_span.start)?;
        brace(left_end, body)
    };
    // The if faces: the then and else bodies brace independently (the
    // official producer emits both replacements when the selection
    // routes to neither).
    let if_face = |id: u32, target: Option<u32>| -> Option<Vec<(u32, u32, String)>> {
        let parts = if_parts(context, id)?;
        // A chained else-if's `if` never braces itself (the outer else
        // face owns it).
        if let Some(outer) = context.ast.parent(id)
            && context.ast.kind(outer) == NodeKind::IfStatement
            && if_parts(context, outer).and_then(|p| p.else_id) == Some(id)
        {
            return None;
        }
        // The official `thenOrElse`: the else keyword selects the else
        // face, the then statement the then face, everything else both.
        let routed_else = match target {
            Some(node) => Some(node) == parts.else_id,
            None => parts
                .else_keyword
                .as_ref()
                .is_some_and(|keyword| keyword.contains(&context.selection.start)),
        };
        let routed_then = target.is_some_and(|node| node == parts.then_id);
        let mut edits = Vec::new();
        // The then face.
        if context.ast.kind(parts.then_id) != NodeKind::Block && !routed_else {
            let then_span = node_span16(context.tokens, context.text, context.ast, parts.then_id)?;
            let left_end = prev_token_end16(context.tokens, context.text, then_span.start)?;
            if parts.else_keyword.is_none() {
                edits.push(brace(left_end, parts.then_id)?);
            } else {
                let else_kw = parts.else_keyword.as_ref()?.clone();
                let then_text = slice16(context.text, &then_span)?.to_owned();
                let prefix = line_indent_of(context.text, then_span.start).to_owned();
                let eol = eol_of(context.text);
                edits.push((
                    left_end,
                    else_kw.start - left_end,
                    format!(" {{{eol}{prefix}  {then_text}{eol}{prefix}}} "),
                ));
            }
        }
        // The else face: a plain statement only (blocks and else-if
        // chains stay), and not when the selection routed to the then.
        if let Some(else_id) = parts.else_id
            && !routed_then
            && !matches!(
                context.ast.kind(else_id),
                NodeKind::Block | NodeKind::IfStatement
            )
        {
            let else_kw = parts.else_keyword.as_ref()?.clone();
            let else_span = node_span16(context.tokens, context.text, context.ast, else_id)?;
            let else_text = slice16(context.text, &else_span)?.to_owned();
            let prefix = line_indent_of(context.text, else_span.start).to_owned();
            let eol = eol_of(context.text);
            edits.push((
                else_kw.end,
                else_span.end - else_kw.end,
                format!(" {{{eol}{prefix}  {else_text}{eol}{prefix}}}"),
            ));
        }
        (!edits.is_empty()).then_some(edits)
    };
    // The dispatch: own-kind faces, then the parent's.
    if kind == NodeKind::DoWhileStatement {
        return Some(vec![do_face(statement)?]);
    }
    if parent.is_some_and(|id| context.ast.kind(id) == NodeKind::DoWhileStatement) {
        return Some(vec![do_face(parent?)?]);
    }
    if kind == NodeKind::ForStatement {
        return Some(vec![loop_face(statement)?]);
    }
    if parent.is_some_and(|id| context.ast.kind(id) == NodeKind::ForStatement) {
        return Some(vec![loop_face(parent?)?]);
    }
    if kind == NodeKind::IfStatement {
        return if_face(statement, None);
    }
    if parent.is_some_and(|id| context.ast.kind(id) == NodeKind::IfStatement) {
        return if_face(parent?, Some(statement));
    }
    if kind == NodeKind::WhileStatement {
        return Some(vec![loop_face(statement)?]);
    }
    if parent.is_some_and(|id| context.ast.kind(id) == NodeKind::WhileStatement) {
        return Some(vec![loop_face(parent?)?]);
    }
    None
}

// ---------------------------------------------------------------------------
// Assist producers — F5c: the resolved-light declaration/member cluster
// (upstream Dart SDK source,
// pkg/analysis_server/lib/src/services/correction/dart/*.dart, the file
// named per producer). Producers that only need AST facts run
// engine-free; producers needing model-namable types read
// [`AssistContext::semantics`] and stay silent when unmounted — fewer
// assists, never an approximated type. The kinds whose official guards
// need element facts the fold does not publish (sibling setters,
// extension membership) are not landed rather than approximated.
// ---------------------------------------------------------------------------

/// The token index of the lexeme `word` right before `at16`, if it is
/// there.
pub(crate) fn prev_token_is(context: &AssistContext<'_>, at16: u32, word: &str) -> bool {
    let Some(index) = token_at(context.tokens, at16) else {
        return false;
    };
    let Some(previous) = index.checked_sub(1) else {
        return false;
    };
    token_end16(context.tokens, context.text, previous) == Some(at16)
        && word_at(context.tokens, context.text, previous, word)
}

/// The member declaration owning `start`'s body — the nearest
/// ancestor-or-self of the member kinds (top-level functions are
/// `TopLevelMethod` rows; closures never climb to a member kind and
/// decline by falling off the chain).
pub(crate) fn enclosing_member(ast: &SyntaxTable, start: u32) -> Option<u32> {
    let mut current = start;
    loop {
        if matches!(
            ast.kind(current),
            NodeKind::TopLevelMethod
                | NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::FunctionExpression
                | NodeKind::LocalFunctionDeclaration
        ) {
            return Some(current);
        }
        current = ast.parent(current)?;
    }
}

/// The member's function body (the `{ … }` / `=> …` shape); an empty
/// `;` body declines.
pub(crate) fn member_body(ast: &SyntaxTable, member: u32) -> Option<u32> {
    child_of_kind(ast, member, NodeKind::BlockBody)
}

/// The body's `async` keyword span and generator star (the token right
/// before the body's first token).
pub(crate) fn body_async_keyword(
    context: &AssistContext<'_>,
    body_span: &core::ops::Range<u32>,
) -> Option<(core::ops::Range<u32>, bool)> {
    let index = token_at(context.tokens, body_span.start)?.checked_sub(1)?;
    let token = context.tokens.get(index)?;
    if !word_at(context.tokens, context.text, index, "async") {
        return None;
    }
    let start = token.utf16_offset;
    let end = token_end16(context.tokens, context.text, index)?;
    let generator = token_at(context.tokens, end).is_some_and(|next| {
        context
            .tokens
            .get(next)
            .is_some_and(|token| token.utf16_offset == end)
            && context.tokens.lexeme(context.text, next) == "*"
    });
    Some((start..end, generator))
}

/// The model-namable static type of one initializer expression, in
/// display form: the fixed literal types, a bare read's flow row, or the
/// family-(d) faces (creation, invocation callee, member access).
/// Anything else — and any non-`Named` type — under-reports `None`.
pub(crate) fn initializer_type_text(
    context: &AssistContext<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    expr: u32,
) -> Option<String> {
    let text = context.text;
    let named = |ty: Option<dartq_semantic::model::Ty>| -> Option<String> {
        match ty? {
            ty @ dartq_semantic::model::Ty::Named { .. } => Some(ty.render(
                analysis.strings(),
                &dartq_semantic::model::ArgPool::default(),
            )),
            _ => None,
        }
    };
    let span16 = |id: u32| node_span16(context.tokens, text, context.ast, id);
    let source_range = |id: u32| -> Option<SourceRange> {
        let span = span16(id)?;
        Some(dartq_source::SourceRange {
            offset: span.start,
            length: span.end - span.start,
        })
    };
    // The initializer wrapper unwraps to its expression row (the
    // [`initializer_expr_row`] payload walk for span-only wrappers).
    let expr = if context.ast.kind(expr) == NodeKind::VariableInitializer {
        initializer_expr_row(context, expr)?
    } else {
        expr
    };
    match context.ast.kind(expr) {
        // The fixed primitive literal types (the spec's own static types).
        NodeKind::LiteralInt => Some("int".to_owned()),
        NodeKind::LiteralDouble => Some("double".to_owned()),
        NodeKind::LiteralBool => Some("bool".to_owned()),
        NodeKind::StringLiteral => Some("String".to_owned()),
        // A bare read's flow row.
        NodeKind::Identifier => named(analysis.type_of(source_range(expr)?)),
        // An instance creation: the model's constructor face on the
        // written type name.
        NodeKind::NewExpression | NodeKind::ImplicitCreation | NodeKind::ConstExpression => {
            let type_node = child_of_kind(context.ast, expr, NodeKind::Type)?;
            named(analysis.creation_type(source_range(type_node)?))
        }
        // An invocation: the callee's static type through the bare
        // callee face.
        NodeKind::Send => {
            let callee = direct_children(context.ast, expr).into_iter().next()?;
            named(analysis.callee_type(source_range(callee)?))
        }
        // A member access: the receiver's member face on the selector.
        NodeKind::DotAccess => {
            let mut children = direct_children(context.ast, expr).into_iter();
            let receiver = children.next()?;
            let selector = children.next()?;
            let selector_span = span16(selector)?;
            let name = slice16(text, &selector_span)?;
            named(analysis.member_type(source_range(receiver)?, analysis.strings().intern(name)))
        }
        _ => None,
    }
}

/// `dart.assist.add.returnType` (`AddReturnType`): the inferred return
/// type inserted before the named function's or method's name (cursor on
/// the name). The subset posture: no returns infers `void`, one
/// `return e;` / `=> e` infers the model-namable type of `e`; several
/// returns (the official least-upper-bound walk) and async/generator
/// bodies (the `Future`/`Stream` instantiation) decline.
pub(crate) fn add_return_type(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let analysis = context.semantics()?;
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let member = enclosing_member(context.ast, node)?;
    // The subset posture: named top-level functions and class methods.
    // Constructors, factories, closures, and local functions decline.
    if matches!(
        context.ast.kind(member),
        NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::FunctionExpression
            | NodeKind::LocalFunctionDeclaration
    ) {
        return None;
    }
    // The declared name: the identifier child just before the parameter
    // list; the cursor must sit on it.
    let parameters = child_of_kind(context.ast, member, NodeKind::FormalParameters)?;
    let mut name_span = None;
    for child in direct_children(context.ast, member) {
        if child == parameters {
            break;
        }
        if context.ast.kind(child) == NodeKind::Identifier
            && let Some(span) = node_span16(context.tokens, context.text, context.ast, child)
        {
            name_span = Some(span);
        }
    }
    let name_span = name_span?;
    if !name_span.contains(&context.selection.start) {
        return None;
    }
    // Setters decline; getters insert before their `get` keyword.
    if prev_token_is(context, name_span.start, "set") {
        return None;
    }
    let insert_at = if prev_token_is(context, name_span.start, "get") {
        token_at(context.tokens, name_span.start)?
            .checked_sub(1)
            .and_then(|index| context.tokens.get(index))
            .map(|token| token.utf16_offset)?
    } else {
        name_span.start
    };
    // Already typed (the return type child sits before the parameter
    // list too).
    for child in direct_children(context.ast, member) {
        if child == parameters {
            break;
        }
        if matches!(
            context.ast.kind(child),
            NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
        ) {
            return None;
        }
    }
    let body = member_body(context.ast, member)?;
    let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
    if body_async_keyword(context, &body_span).is_some() {
        // The `Future`/`Stream` instantiation exceeds the subset; the
        // official producer wraps through the type system.
        return None;
    }
    let inferred = if context
        .tokens
        .lexeme(context.text, token_at(context.tokens, body_span.start)?)
        == "=>"
    {
        // The arrow payload is the row completing right before the body
        // (post-order adjacency; expression rows carry no child window).
        let expression = body.checked_sub(1)?;
        let expression_span = node_span16(context.tokens, context.text, context.ast, expression)?;
        if expression_span.start < body_span.start || expression_span.end > body_span.end {
            return None;
        }
        initializer_type_text(context, analysis, expression)?
    } else {
        // The block face: zero returns infers `void` (the official
        // `ReturnTypeComputer` null posture); one expression return
        // infers its type; everything else declines.
        let mut returns = Vec::new();
        let mut stack = vec![body];
        while let Some(id) = stack.pop() {
            if context.ast.kind(id) == NodeKind::ReturnStatement {
                returns.push(id);
                continue;
            }
            for child in direct_children(context.ast, id) {
                stack.push(child);
            }
        }
        match returns.as_slice() {
            [] => "void".to_owned(),
            [only] => {
                let expression = direct_children(context.ast, *only).into_iter().next()?;
                initializer_type_text(context, analysis, expression)?
            }
            _ => return None,
        }
    };
    Some(vec![(insert_at, 0, format!("{inferred} "))])
}

/// The nearest statement-list holder — `node` itself when it already is
/// one, else the first such ancestor.
pub(crate) fn statement_block_around(context: &AssistContext<'_>, node: u32) -> Option<u32> {
    if is_statement_block(context.ast, node) {
        return Some(node);
    }
    let mut current = node;
    loop {
        let parent = context.ast.parent(current)?;
        if is_statement_block(context.ast, parent) {
            return Some(parent);
        }
        current = parent;
    }
}

/// The declaration keyword of a `VariablesDeclaration`: the token right
/// before the name when it is `var`/`final`/`const`/`late` (this AST
/// excludes the keyword from the declaration's span, which starts at the
/// name), else `None` — the keyword index for the caller's lexeme reads.
pub(crate) fn declaration_keyword(
    context: &AssistContext<'_>,
    name_span: &core::ops::Range<u32>,
) -> Option<usize> {
    let name_index = token_at(context.tokens, name_span.start)?;
    let index = name_index.checked_sub(1)?;
    let end = token_end16(context.tokens, context.text, index)?;
    if end > name_span.start {
        return None;
    }
    matches!(
        context.tokens.lexeme(context.text, index),
        "var" | "final" | "const" | "late"
    )
    .then_some(index)
}

/// The local `VariablesDeclaration` statement under the cursor. The
/// declaration's span starts at the name (this AST excludes the
/// `var`/`final`/`const` keyword from it), so a cursor on the keyword
/// finds no covering row — there the declaration is the first statement
/// starting after the cursor, and only when the cursor's own token is a
/// declaration keyword.
pub(crate) fn variable_declaration_at(context: &AssistContext<'_>) -> Option<u32> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    if let Some(declaration) = ancestor_or_self_kind(
        context.tokens,
        context.text,
        context.ast,
        node,
        NodeKind::VariablesDeclaration,
    ) {
        return Some(declaration);
    }
    let keyword = token_at(context.tokens, context.selection.start)?;
    if !matches!(
        context.tokens.lexeme(context.text, keyword),
        "var" | "final" | "const"
    ) {
        return None;
    }
    let block = statement_block_around(context, node)?;
    let declaration = statement_children(context.ast, block)
        .into_iter()
        .find(|&statement| {
            context.ast.kind(statement) == NodeKind::VariablesDeclaration
                && node_span16(context.tokens, context.text, context.ast, statement)
                    .is_some_and(|span| span.start > context.selection.start)
        })?;
    context
        .ast
        .parent(declaration)
        .filter(|&parent| parent == block)?;
    Some(declaration)
}

/// `dart.assist.add.typeAnnotation` (`AddTypeAnnotation`, the local
/// variable declaration face): the model-namable initializer type
/// replaces the `var` keyword, or inserts before the name of a
/// `final`/`const` local. The other official faces (formal parameters,
/// patterns, typed literals, for-in) are later increments.
pub(crate) fn add_type_annotation_local(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let analysis = context.semantics()?;
    let declaration = variable_declaration_at(context)?;
    // A local: the declaration is a statement in a block (fields live in
    // `Fields` members, top-level fields in `TopLevelFields`).
    if context
        .ast
        .parent(declaration)
        .is_none_or(|parent| !is_statement_block(context.ast, parent))
    {
        return None;
    }
    if child_of_kind(context.ast, declaration, NodeKind::Type).is_some() {
        return None;
    }
    let mut variables = direct_children(context.ast, declaration)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier);
    let variable = variables.next()?;
    if variables.next().is_some() {
        return None;
    }
    let name = direct_children(context.ast, declaration)
        .into_iter()
        .find(|&child| context.ast.kind(child) == NodeKind::Identifier)?;
    let name_span = node_span16(context.tokens, context.text, context.ast, name)?;
    let initializer = direct_children(context.ast, variable).into_iter().next()?;
    let type_text = initializer_type_text(context, analysis, initializer)?;
    // The declaration keyword sits right before the name — the
    // declaration's own span starts there, not at the keyword.
    let keyword_index = declaration_keyword(context, &name_span)?;
    let keyword = context.tokens.get(keyword_index)?;
    match context.tokens.lexeme(context.text, keyword_index) {
        "var" => Some(vec![(
            keyword.utf16_offset,
            token_end16(context.tokens, context.text, keyword_index)? - keyword.utf16_offset,
            type_text,
        )]),
        "final" | "const" => Some(vec![(name_span.start, 0, format!("{type_text} "))]),
        _ => None,
    }
}

/// `dart.assist.convert.bodyToExpression` (`ConvertToExpressionFunctionBody`):
/// the single-statement block body rewrites to `=> expr;` (closures drop
/// the trailing semicolon; the method/declaration faces keep it). The
/// attached-comment preservation of the official producer is a later
/// increment — comments inside the converted body drop.
pub(crate) fn convert_body_to_expression(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let member = enclosing_member(context.ast, node)?;
    // Constructors decline (the official non-factory guard; factories
    // are their own member kind here).
    if context.ast.kind(member) == NodeKind::Constructor {
        return None;
    }
    let body = member_body(context.ast, member)?;
    let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
    if context
        .tokens
        .lexeme(context.text, token_at(context.tokens, body_span.start)?)
        != "{"
    {
        return None;
    }
    // Generators decline (the official `body.isGenerator` guard).
    if body_async_keyword(context, &body_span).is_some_and(|(_, generator)| generator) {
        return None;
    }
    let mut statements = statement_children(context.ast, body);
    if statements.len() != 1 {
        return None;
    }
    let only = statements.remove(0);
    // The returned expression: the `return e;` payload or the bare
    // expression statement's expression (the span-only statement's
    // payload is the row completing right before it — post-order
    // adjacency).
    let expression = match context.ast.kind(only) {
        NodeKind::ReturnStatement => direct_children(context.ast, only).into_iter().next()?,
        NodeKind::ExpressionStatement => {
            let payload = only.checked_sub(1)?;
            let payload_span = node_span16(context.tokens, context.text, context.ast, payload)?;
            let only_span = node_span16(context.tokens, context.text, context.ast, only)?;
            if payload_span.start < only_span.start || payload_span.end > only_span.end {
                return None;
            }
            payload
        }
        _ => return None,
    };
    // The assist lives before the expression — deep inside a large
    // returned expression it is a surprise (the official guard).
    let expression_span = node_span16(context.tokens, context.text, context.ast, expression)?;
    if context.selection.start >= expression_span.start {
        return None;
    }
    let expression_text = slice16(context.text, &expression_span)?;
    let mut replacement = String::new();
    if let Some((keyword, _)) = body_async_keyword(context, &body_span) {
        replacement.push_str(&slice16(context.text, &keyword)?.to_owned());
        replacement.push(' ');
    }
    replacement.push_str("=> ");
    replacement.push_str(&expression_text);
    // The semicolon: kept except on a closure's body (the official
    // `parent is FunctionExpression && parent.parent is not
    // FunctionDeclaration` shape).
    let closure = context.ast.kind(member) == NodeKind::FunctionExpression
        && context
            .ast
            .parent(member)
            .is_some_and(|parent| context.ast.kind(parent) != NodeKind::TopLevelDeclaration);
    if !closure {
        replacement.push(';');
    }
    Some(vec![(
        body_span.start,
        body_span.end - body_span.start,
        replacement,
    )])
}

/// `dart.assist.convert.bodyToAsync` (`ConvertIntoAsyncBody`): the named
/// function's or method's sync body gains `async` (fused with the
/// previous token when there is no space) and its written return type
/// wraps `Future<…>`. Closures and constructors decline (the official
/// guards).
pub(crate) fn convert_body_to_async(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let member = enclosing_member(context.ast, node)?;
    if matches!(
        context.ast.kind(member),
        NodeKind::Constructor | NodeKind::Factory | NodeKind::LocalFunctionDeclaration
    ) {
        return None;
    }
    if context.ast.kind(member) == NodeKind::FunctionExpression
        && context
            .ast
            .parent(member)
            .is_none_or(|parent| context.ast.kind(parent) != NodeKind::TopLevelDeclaration)
    {
        // A closure.
        return None;
    }
    let body = member_body(context.ast, member)?;
    let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
    // The selection lives at or before the body's first token end (the
    // official deep-body guard).
    let first = token_at(context.tokens, body_span.start)?;
    if context.selection.start > token_end16(context.tokens, context.text, first)? {
        return None;
    }
    // Already async — or a generator — declines.
    if body_async_keyword(context, &body_span).is_some() {
        return None;
    }
    let fused = first
        .checked_sub(1)
        .and_then(|index| token_end16(context.tokens, context.text, index))
        .is_some_and(|end| end == body_span.start);
    let mut edits = vec![(
        body_span.start,
        0,
        if fused {
            " async ".to_owned()
        } else {
            "async ".to_owned()
        },
    )];
    // The written return type wraps `Future<…>` (the official
    // `replaceTypeWithFuture`; an absent return type stays absent — the
    // inference fills it).
    for child in direct_children(context.ast, member) {
        if matches!(context.ast.kind(child), NodeKind::Type | NodeKind::VoidType) {
            let span = node_span16(context.tokens, context.text, context.ast, child)?;
            let written = slice16(context.text, &span)?;
            edits.push((
                span.start,
                span.end - span.start,
                format!("Future<{written}>"),
            ));
            break;
        }
    }
    Some(edits)
}

/// `dart.assist.remove.async` (`RemoveAsync`, the unnecessary variant):
/// the plain `async` keyword (no `*`) deletes off any function body.
/// The typed variants — the official `Future`-return rewrite and its
/// return-assignability walk — exceed the resolved-light seam and stay
/// out rather than approximate.
pub(crate) fn remove_async(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let mut current = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let body = loop {
        if context.ast.kind(current) == NodeKind::BlockBody {
            break current;
        }
        current = context.ast.parent(current)?;
    };
    let body_span = node_span16(context.tokens, context.text, context.ast, body)?;
    let (keyword, generator) = body_async_keyword(context, &body_span)?;
    if generator {
        return None;
    }
    // Delete the keyword plus one following character (the space) unless
    // the next token starts exactly at the keyword's end (the official
    // `keyword.next!.offset == keyword.end` posture).
    let mut end = keyword.end;
    let next = token_at(context.tokens, keyword.end).and_then(|index| context.tokens.get(index));
    if next.is_some_and(|token| token.utf16_offset > keyword.end) {
        end += 1;
    }
    Some(vec![(keyword.start, end - keyword.start, String::new())])
}

/// The climb from a covered node up to its `kind` member row, over the
/// wrapper rows this AST inserts (identifier/list/type/initializer and
/// the member-body wrappers).
pub(crate) fn climb_to_member(context: &AssistContext<'_>, node: u32, kind: NodeKind) -> Option<u32> {
    let skip = |current: u32| {
        matches!(
            context.ast.kind(current),
            NodeKind::Identifier
                | NodeKind::InitializedIdentifier
                | NodeKind::VariablesDeclaration
                | NodeKind::VariableInitializer
                | NodeKind::Type
                | NodeKind::TypeApplication
                | NodeKind::DeclarationBody
        )
    };
    let mut current = node;
    loop {
        let current_kind = context.ast.kind(current);
        if current_kind == kind {
            return Some(current);
        }
        if !skip(current) {
            return None;
        }
        current = context.ast.parent(current)?;
    }
}

/// The first `kind` member row whose keyword zone holds the selection: a
/// cursor resting on a `late`/`var`/`final`/`const` keyword covers no
/// row below the member wrappers (this AST starts member spans at the
/// name), so the member is the next one in arena order whose
/// `[cursor, span start)` gap holds only declaration keywords.
pub(crate) fn keyword_zone_member(context: &AssistContext<'_>, kind: NodeKind) -> Option<u32> {
    let keyword = token_at(context.tokens, context.selection.start)?;
    if !matches!(
        context.tokens.lexeme(context.text, keyword),
        "late" | "var" | "final" | "const"
    ) {
        return None;
    }
    let root = context.ast.root()?;
    (0..=root)
        .filter(|&id| context.ast.kind(id) == kind)
        .find(|&id| {
            node_span16(context.tokens, context.text, context.ast, id).is_some_and(|span| {
                span.start > context.selection.start
                    && keyword_zone_only(context, context.selection.start, span.start) == Some(true)
            })
        })
}

/// Whether every token in `[from16, to16)` is a declaration keyword (the
/// gap check for [`keyword_zone_member`]; token starts are monotone).
pub(crate) fn keyword_zone_only(context: &AssistContext<'_>, from16: u32, to16: u32) -> Option<bool> {
    let mut index = token_at(context.tokens, from16)?;
    loop {
        let token = context.tokens.get(index)?;
        if token.utf16_offset >= to16 {
            return Some(true);
        }
        if !matches!(
            context.tokens.lexeme(context.text, index),
            "late" | "var" | "final" | "const"
        ) {
            return Some(false);
        }
        index += 1;
    }
}

/// The expression of one `= expr` initializer wrapper: its windowed
/// child when the wrapper holds one, else the first real-span row after
/// the `=` (span-only wrappers keep the `= expr` text in their span
/// without a child window).
pub(crate) fn initializer_expr_row(context: &AssistContext<'_>, wrapper: u32) -> Option<u32> {
    if let Some(child) = direct_children(context.ast, wrapper).into_iter().next() {
        return Some(child);
    }
    let wrapper_span = node_span16(context.tokens, context.text, context.ast, wrapper)?;
    let mut index = token_at(context.tokens, wrapper_span.start)?;
    let mut eq_end = None;
    while let Some(token) = context.tokens.get(index) {
        if token.utf16_offset >= wrapper_span.end {
            break;
        }
        if context.tokens.lexeme(context.text, index) == "=" {
            eq_end = Some(token_end16(context.tokens, context.text, index)?);
            break;
        }
        index += 1;
    }
    let eq_end = eq_end?;
    span_inner_row(
        context.ast,
        context.tokens,
        context.text,
        eq_end,
        wrapper_span.end,
    )
}

/// The expression row completing inside `[from16, to16)`: the first
/// real-span arena row fully inside the range. Span-only wrappers hold
/// their payload without a child window — post-order completion puts the
/// payload row before the wrapper's own row.
pub(crate) fn span_inner_row(
    ast: &SyntaxTable,
    tokens: &TokenTable,
    text: &str,
    from16: u32,
    to16: u32,
) -> Option<u32> {
    let root = ast.root()?;
    (0..=root).find(|&id| {
        node_span16(tokens, text, ast, id)
            .is_some_and(|span| span.start >= from16 && span.end <= to16 && span.start < span.end)
    })
}

/// `dart.assist.convert.finalFieldToGetter` (`ConvertIntoGetter`): the
/// single-variable field rewrites to a getter —
/// `[Type] get name => init;` — replacing from the field's leading
/// `late`/keyword token through the declaration end (anything before —
/// `static`, metadata — stays). The declared type annotates the getter;
/// an untyped field's type is the model-namable initializer type when
/// the face is mounted, else no return type is written (the official
/// `writeType(null)` default posture). The `{0}`-parameterized official
/// title instantiates the member name; the static table title carries
/// the unparameterized spelling.
pub(crate) fn final_field_to_getter(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    // The official ancestor walk: skip the identifier/list/type wrappers
    // (plus the initializer wrapper this AST inserts). A cursor resting
    // on the field's keyword finds no covering row below the member
    // wrappers (the keyword is excluded from the `Fields` span) — there
    // the field is the first one whose keyword zone holds the cursor.
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    let fields = climb_to_member(context, node, NodeKind::Fields)
        .or_else(|| keyword_zone_member(context, NodeKind::Fields))?;
    let variables_list = child_of_kind(context.ast, fields, NodeKind::VariablesDeclaration)?;
    let mut variables = direct_children(context.ast, variables_list)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier);
    let variable = variables.next()?;
    if variables.next().is_some() {
        return None;
    }
    // The field's name and written type live at the `Fields` level (the
    // declaration holds only the initializer parts). The member's window
    // tiling ends at the declaration row, so the name is the last
    // identifier child (the adopted type row leaks in as an identifier
    // ahead of it).
    let name = direct_children(context.ast, fields)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::Identifier)
        .last()?;
    let name_span = node_span16(context.tokens, context.text, context.ast, name)?;
    if name_span.start == name_span.end {
        return None;
    }
    // The initializer expression: the variable's `= expr` wrapper (the
    // [`initializer_expr_row`] payload walk).
    let initializer = direct_children(context.ast, variable)
        .into_iter()
        .next()
        .and_then(|wrapper| initializer_expr_row(context, wrapper));
    let fields_span = node_span16(context.tokens, context.text, context.ast, fields)?;
    // The type: the written annotation, else the model-namable
    // initializer type.
    let type_text = child_of_kind(context.ast, fields, NodeKind::Type)
        .and_then(|type_node| {
            let span = node_span16(context.tokens, context.text, context.ast, type_node)?;
            Some(slice16(context.text, &span)?.to_owned())
        })
        .or_else(|| {
            let analysis = context.semantics()?;
            initializer_type_text(context, analysis, initializer?)
        });
    // The replacement's start: the field's leading keyword chain
    // (`late`/`var`/`final`/`const` tokens ending at the member's span
    // start — the span itself begins at the name), else the type, else
    // the name.
    let keyword_start = (|| {
        let mut index = token_at(context.tokens, fields_span.start)?;
        let mut start = None;
        loop {
            index = index.checked_sub(1)?;
            if token_end16(context.tokens, context.text, index)? > fields_span.start
                || !matches!(
                    context.tokens.lexeme(context.text, index),
                    "late" | "var" | "final" | "const"
                )
            {
                break;
            }
            start = Some(context.tokens.get(index)?.utf16_offset);
        }
        start
    })();
    let start_at = keyword_start
        .or_else(|| {
            child_of_kind(context.ast, fields, NodeKind::Type)
                .and_then(|type_node| {
                    node_span16(context.tokens, context.text, context.ast, type_node)
                })
                .map(|span| span.start)
        })
        .unwrap_or(name_span.start);
    let initializer_text = match initializer {
        Some(initializer) => {
            let span = node_span16(context.tokens, context.text, context.ast, initializer)?;
            slice16(context.text, &span)?.to_owned()
        }
        None => "null".to_owned(),
    };
    let name_text = slice16(context.text, &name_span)?;
    let mut replacement = String::new();
    if let Some(type_text) = type_text {
        replacement.push_str(&type_text);
        replacement.push(' ');
    }
    replacement.push_str("get ");
    replacement.push_str(&name_text);
    replacement.push_str(" => ");
    replacement.push_str(&initializer_text);
    replacement.push(';');
    Some(vec![(start_at, fields_span.end - start_at, replacement)])
}

/// `dart.assist.joinVariableDeclaration` (`JoinVariableDeclaration`): the
/// adjacent declaration/assignment pair joins. The official element
/// identity check is adjacency-exact here — the two statements are
/// adjacent block children, so no shadowing declaration can sit between
/// them and the name equality is the same variable. The assignment face
/// locates its statement by span (expression rows carry no child
/// windows); the assignment payload is the row completing right before
/// the statement.
pub(crate) fn join_variable_declaration(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let node = covering_node(
        context.tokens,
        context.text,
        context.ast,
        context.selection.start,
    )?;
    // The statement list holding the selection (both faces share it).
    let block = statement_block_around(context, node)?;
    let statements = statement_children(context.ast, block);
    // The statement the cursor sits in: the last one starting at or
    // before the cursor.
    node_span16(context.tokens, context.text, context.ast, node)?;
    let index = statements
        .iter()
        .position(|&statement| {
            node_span16(context.tokens, context.text, context.ast, statement)
                .is_some_and(|span| span.start > context.selection.start)
        })
        .unwrap_or(statements.len());
    let statement_index = index.saturating_sub(1);
    let statement = *statements.get(statement_index)?;
    let (declaration, assigned, _) = match context.ast.kind(statement) {
        // The assignment face: the payload row before the span-only
        // statement, and the official previous-element check — the
        // adjacent previous statement declares the assigned name.
        NodeKind::ExpressionStatement => {
            let assignment = statement.checked_sub(1)?;
            let assignment_span =
                node_span16(context.tokens, context.text, context.ast, assignment)?;
            let statement_span = node_span16(context.tokens, context.text, context.ast, statement)?;
            if context.ast.kind(assignment) != NodeKind::Assignment
                || assignment_span.start < statement_span.start
                || assignment_span.end > statement_span.end
            {
                return None;
            }
            let previous = *statements.get(statement_index.checked_sub(1)?)?;
            if context.ast.kind(previous) != NodeKind::VariablesDeclaration {
                return None;
            }
            (previous, statement, assignment)
        }
        // The declaration face: the next statement must assign into the
        // declared name.
        NodeKind::VariablesDeclaration => {
            let next = *statements.get(statement_index + 1)?;
            if context.ast.kind(next) != NodeKind::ExpressionStatement {
                return None;
            }
            let assignment = next.checked_sub(1)?;
            if context.ast.kind(assignment) != NodeKind::Assignment {
                return None;
            }
            (statement, next, assignment)
        }
        _ => return None,
    };
    // The declared name: written once, never initialized (the official
    // guard — an initialized declaration joins nothing).
    let (name_span, has_initializer) = declared_single(context, declaration)?;
    if has_initializer {
        return None;
    }
    // The assigned name opens the assignment statement (`name =`; the
    // assignment row's own span covers only the right-hand side) and must
    // be the declared name, assigned with a plain `=` directly on it (a
    // `a.b = …` left side fails the operator check).
    let assigned_span = node_span16(context.tokens, context.text, context.ast, assigned)?;
    let assigned_start = token_at(context.tokens, assigned_span.start)?;
    if context.tokens.lexeme(context.text, assigned_start) != slice16(context.text, &name_span)? {
        return None;
    }
    let assigned_end = token_end16(context.tokens, context.text, assigned_start)?;
    let operator = token_at(context.tokens, assigned_end)?;
    if context.tokens.lexeme(context.text, operator) != "=" {
        return None;
    }
    Some(vec![(
        name_span.end,
        context.tokens.get(operator)?.utf16_offset - name_span.end,
        " ".to_owned(),
    )])
}

/// The single declared variable of a `VariablesDeclaration` statement:
/// its name span and whether an initializer is written. The name row is
/// the declaration's `Identifier` child (the name is adopted at the
/// declaration level, not inside the `InitializedIdentifier`); an
/// initializer shows up as the variable's windowed rows.
pub(crate) fn declared_single(
    context: &AssistContext<'_>,
    declaration: u32,
) -> Option<(core::ops::Range<u32>, bool)> {
    if context.ast.kind(declaration) != NodeKind::VariablesDeclaration {
        return None;
    }
    let mut variables = direct_children(context.ast, declaration)
        .into_iter()
        .filter(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier);
    let variable = variables.next()?;
    if variables.next().is_some() {
        return None;
    }
    let name = direct_children(context.ast, declaration)
        .into_iter()
        .find(|&child| context.ast.kind(child) == NodeKind::Identifier)?;
    let name_span = node_span16(context.tokens, context.text, context.ast, name)?;
    Some((
        name_span,
        !direct_children(context.ast, variable).is_empty(),
    ))
}

/// `dart.assist.splitVariableDeclaration` (`SplitVariableDeclaration`): a
/// non-`const`/non-`final` local with one initialized variable splits at
/// the name (`;` + newline + indent + name). The official inferred-type
/// face — the `var` keyword replaced with the model-namable initializer
/// type — rides the mounted face.
pub(crate) fn split_variable_declaration(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let declaration = variable_declaration_at(context)?;
    if context
        .ast
        .parent(declaration)
        .is_none_or(|parent| !is_statement_block(context.ast, parent))
    {
        return None;
    }
    let (name_span, has_initializer) = declared_single(context, declaration)?;
    if !has_initializer {
        return None;
    }
    let variable = direct_children(context.ast, declaration)
        .into_iter()
        .find(|&child| context.ast.kind(child) == NodeKind::InitializedIdentifier)?;
    let initializer = direct_children(context.ast, variable).into_iter().next()?;
    // The declaration keyword sits right before the name — the
    // declaration's own span starts there, not at the keyword.
    let keyword_index = declaration_keyword(context, &name_span)?;
    if matches!(
        context.tokens.lexeme(context.text, keyword_index),
        "final" | "const"
    ) {
        return None;
    }
    // The caret sits between the declaration's start (the keyword, when
    // written) and the name's end.
    let declaration_start = context
        .tokens
        .get(keyword_index)
        .map(|token| token.utf16_offset)
        .unwrap_or(name_span.start);
    if context.selection.start < declaration_start || context.selection.start > name_span.end {
        return None;
    }
    let eol = eol_of(context.text);
    let indent = line_indent_of(context.text, declaration_start).to_owned();
    let name_text = slice16(context.text, &name_span)?.to_owned();
    let mut edits = Vec::new();
    // The inferred-type face: an untyped `var` local with a
    // model-namable initializer type writes the annotation in place of
    // the keyword.
    if context.tokens.lexeme(context.text, keyword_index) == "var"
        && child_of_kind(context.ast, declaration, NodeKind::Type).is_none()
        && let Some(analysis) = context.semantics()
        && let Some(type_text) = initializer_type_text(context, analysis, initializer)
    {
        let keyword = context.tokens.get(keyword_index)?;
        edits.push((
            keyword.utf16_offset,
            token_end16(context.tokens, context.text, keyword_index)? - keyword.utf16_offset,
            type_text,
        ));
    }
    edits.push((name_span.end, 0, format!(";{eol}{indent}{name_text}")));
    Some(edits)
}

// ---------------------------------------------------------------------------
// Assist producers — F5b: the surround-with family (engine-free; upstream
// Dart SDK source,
// pkg/analysis_server/lib/src/services/correction/dart/surround_with.dart)
// ---------------------------------------------------------------------------

/// The shared surround prep: the selection's fully-covered statement run
/// turned into the full-lines range, the first statement's line prefix,
/// and the re-indented range source (the shared `SurroundWith.producers`
/// preamble; the per-kind linked-edit placeholders flatten to their
/// default texts — `condition`, `v`, `init`, `item`, `iterable`, `e`,
/// `Exception` — and the official `// TODO` in the catch/finally bodies).
pub(crate) struct SurroundPrep {
    /// Full-lines range — `[line start of the first statement, start of
    /// the line after the last statement)`, UTF-16 (the official
    /// `getLinesRangeStatements`).
    pub(crate) range: core::ops::Range<u32>,
    /// The first statement's line prefix (the official `getNodePrefix`).
    indent_old: String,
    /// The range source re-indented from `indent_old` to one indent
    /// deeper, with a trailing eol ensured (the official
    /// `replaceSourceRangeIndent(..., includeLeading: true,
    /// ensureTrailingNewline: true)`).
    indented: String,
}

/// The start of the line after the line holding `offset16` (the text end
/// when `offset16` sits on the last line).
pub(crate) fn next_line_start16(text: &str, offset16: u32) -> u32 {
    let byte = utf16_offset_to_byte(text, offset16);
    match text[byte..].find('\n') {
        Some(rel) => byte_offset_to_utf16(text, byte + rel + 1),
        None => utf16_len_of(text),
    }
}

/// Whether any real token overlaps `[start16, end16)` (the official
/// `_hasTokens` gap check: the gap substring re-tokenized, whose token
/// chain skips comments, so a gap holding only a comment is token-free).
/// Token ends are monotone — tokens never overlap — so the first token
/// ending after `start16` is the only overlap candidate.
pub(crate) fn gap_has_tokens(context: &AssistContext<'_>, start16: u32, end16: u32) -> bool {
    if start16 >= end16 {
        return false;
    }
    let table = context.tokens.as_slice();
    let mut lo = 0usize;
    let mut hi = table.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let end =
            token_end16(context.tokens, context.text, mid).expect("token index stays in bounds");
        if end <= start16 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo < table.len()
        && context
            .tokens
            .get(lo)
            .is_some_and(|token| token.utf16_offset < end16)
}

/// The nodes the selection fully covers, in source order (the official
/// StatementAnalyzer outcome: the outermost covered nodes, each covering
/// its whole subtree). The arena rows below the statement level lie flat
/// in the root's window (no child windows), so the walk enumerates the
/// rows and reduces them to the maximal covered spans — inner rows drop,
/// an equal-span pair keeps the outer row (the later post-order id).
pub(crate) fn selected_nodes(context: &AssistContext<'_>) -> Option<Vec<u32>> {
    let selection = context.selection.clone();
    let root = context.ast.root()?;
    let mut covered: Vec<(u32, core::ops::Range<u32>)> = Vec::new();
    for id in 0..=root {
        // Span-less synthetic rows (recovery placeholders) hold no
        // tokens — nothing in their extent can be selected.
        let Some(span) = node_span16(context.tokens, context.text, context.ast, id) else {
            continue;
        };
        if selection.start <= span.start && span.end <= selection.end {
            covered.push((id, span));
        }
    }
    // Outermost reduction: sort by (start asc, end desc) so a container
    // precedes its containees, then keep the rows no kept row covers.
    covered.sort_by_key(|(id, span)| (span.start, core::cmp::Reverse(span.end), *id));
    let mut selected: Vec<u32> = Vec::with_capacity(covered.len());
    let mut kept: Option<core::ops::Range<u32>> = None;
    for (id, span) in covered {
        let inside = kept
            .as_ref()
            .is_some_and(|kept| kept.start <= span.start && span.end <= kept.end);
        if inside {
            // An equal span is the container (the later post-order id
            // completes after its subtree) — swap up to it.
            let equal = kept
                .as_ref()
                .is_some_and(|kept| kept.start == span.start && kept.end == span.end);
            if equal {
                let last = selected.last_mut().expect("kept implies a selected row");
                *last = id;
                kept = Some(span);
            }
            continue;
        }
        selected.push(id);
        kept = Some(span);
    }
    (!selected.is_empty()).then_some(selected)
}

/// The shared surround prep over the selection (the official guards in
/// the official order: a non-empty selection of statements in blocks,
/// edge-clean of comments and stray tokens).
pub(crate) fn surround_prep(context: &AssistContext<'_>) -> Option<SurroundPrep> {
    // A zero-length selection selects nothing (the official `covers` is
    // false for every node when the range is empty).
    if context.selection.is_empty() {
        return None;
    }
    // The fully-covered nodes of the selection.
    let selected = selected_nodes(context)?;
    // We want only statements.
    if selected
        .iter()
        .any(|&node| !is_statement_kind(context.ast.kind(node)))
    {
        return None;
    }
    // We want only statements in blocks.
    for &statement in &selected {
        let parent = context.ast.parent(statement)?;
        if !is_statement_block(context.ast, parent) {
            return None;
        }
    }
    // The selection must not begin or end inside a comment (the official
    // `visitCompilationUnit` guard).
    let comments = dartq_lints::Comments::extract(context.text, context.tokens);
    for comment in comments.all() {
        let start16 = byte_offset_to_utf16(context.text, comment.range.start as usize);
        let end16 = byte_offset_to_utf16(context.text, comment.range.end as usize);
        if (start16..end16).contains(&context.selection.start)
            || (start16 < context.selection.end && context.selection.end < end16)
        {
            return None;
        }
    }
    let first_span = node_span16(context.tokens, context.text, context.ast, selected[0])?;
    let last_span = node_span16(context.tokens, context.text, context.ast, *selected.last()?)?;
    // No real token may overlap the head or tail gap (the official
    // `_checkSelectedNodes`).
    if gap_has_tokens(context, context.selection.start, first_span.start)
        || gap_has_tokens(context, last_span.end, context.selection.end)
    {
        return None;
    }
    let range = line_start16(context.text, first_span.start)?
        ..next_line_start16(context.text, last_span.end);
    let start_byte = utf16_offset_to_byte(context.text, range.start);
    let end_byte = utf16_offset_to_byte(context.text, range.end);
    let indent_old = line_indent_of(context.text, first_span.start).to_owned();
    let indent_new = format!("{indent_old}  ");
    let indented = reindent(
        &context.text[start_byte..end_byte],
        &indent_old,
        &indent_new,
        eol_of(context.text),
        true,
    );
    Some(SurroundPrep {
        range,
        indent_old,
        indented,
    })
}

/// The composed single replacement over the full-lines range: the shared
/// `indentOld + head + eol + indented + indentOld` run plus the tail. The
/// upstream insertion+replacement groups compose into exactly one
/// replacement here (the pinned edit law).
pub(crate) fn surround_replacement(prep: &SurroundPrep, eol: &str, head: &str, tail: &str) -> String {
    let mut out = String::with_capacity(
        2 * prep.indent_old.len() + head.len() + tail.len() + 2 * eol.len() + prep.indented.len(),
    );
    out.push_str(&prep.indent_old);
    out.push_str(head);
    out.push_str(eol);
    out.push_str(&prep.indented);
    out.push_str(&prep.indent_old);
    out.push_str(tail);
    out.push_str(eol);
    out
}

/// The try/catch and try/finally shape: the shared run, then the tail
/// header, the flattened `// TODO` body at the deeper indent, and the
/// closing brace.
pub(crate) fn surround_replacement_with_todo(
    prep: &SurroundPrep,
    eol: &str,
    head: &str,
    tail: &str,
) -> String {
    // The base replacement ends with the eol the TODO line follows.
    let mut out = surround_replacement(prep, eol, head, tail);
    out.push_str(&prep.indent_old);
    out.push_str("  ");
    out.push_str("// TODO");
    out.push_str(eol);
    out.push_str(&prep.indent_old);
    out.push('}');
    out.push_str(eol);
    out
}

/// The single full-range replacement edit of one surround producer.
pub(crate) fn surround_edit(prep: &SurroundPrep, replacement: String) -> Vec<(u32, u32, String)> {
    vec![(
        prep.range.start,
        prep.range.end - prep.range.start,
        replacement,
    )]
}

/// `dart.assist.surround.block` (`_SurroundWithBlock`).
pub(crate) fn surround_with_block(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(&prep, eol, "{", "}"),
    ))
}

/// `dart.assist.surround.doWhile` (`_SurroundWithDoWhile`).
pub(crate) fn surround_with_do_while(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(&prep, eol, "do {", "} while (condition);"),
    ))
}

/// `dart.assist.surround.forEach` (`_SurroundWithFor`).
pub(crate) fn surround_with_for(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(
            &prep,
            eol,
            "for (var v = init; condition; increment) {",
            "}",
        ),
    ))
}

/// `dart.assist.surround.forIn` (`_SurroundWithForIn`).
pub(crate) fn surround_with_for_in(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(&prep, eol, "for (var item in iterable) {", "}"),
    ))
}

/// `dart.assist.surround.if` (`_SurroundWithIf`).
pub(crate) fn surround_with_if(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(&prep, eol, "if (condition) {", "}"),
    ))
}

/// `dart.assist.surround.tryCatch` (`_SurroundWithTryCatch`).
pub(crate) fn surround_with_try_catch(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement_with_todo(&prep, eol, "try {", "} on Exception catch (e) {"),
    ))
}

/// `dart.assist.surround.tryFinally` (`_SurroundWithTryFinally`).
pub(crate) fn surround_with_try_finally(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement_with_todo(&prep, eol, "try {", "} finally {"),
    ))
}

/// `dart.assist.surround.while` (`_SurroundWithWhile`).
pub(crate) fn surround_with_while(context: &AssistContext<'_>) -> Option<Vec<(u32, u32, String)>> {
    let prep = surround_prep(context)?;
    let eol = eol_of(context.text);
    Some(surround_edit(
        &prep,
        surround_replacement(&prep, eol, "while (condition) {", "}"),
    ))
}

// ---------------------------------------------------------------------------
// The 2026-10-17 fact-bound re-adjudication wave (#1883): five lint rows
// whose official producers reduce to text plus the token stream once the
// lint's own gates are read as the shape decision of record. Each rewriter
// mirrors its upstream producer's edit behavior and refuses where the
// producer would refuse — an unverifiable or unfaithful shape drops.
// ---------------------------------------------------------------------------

/// One depth-0 comma segment of the token range `[from, to)`: the half-open
/// token-index segments between the range's depth-0 commas (the `()`, `[]`,
/// `{}`, and `${}` nesting; `<` never groups here).
pub(crate) fn depth0_segments(
    tokens: &TokenTable,
    from: usize,
    to: usize,
) -> Option<Vec<core::ops::Range<usize>>> {
    let mut segments = Vec::new();
    let mut depth = 0u32;
    let mut start = from;
    for index in from..to {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                segments.push(start..index);
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    segments.push(start..to);
    Some(segments)
}

/// The statement run behind a member access at `dot`: the nearest `;`, `{`,
/// or `}` at bracket depth zero bounds an expression statement (one only
/// stands in a block). Returns the first token after the boundary — or the
/// expression-head group's own opener when the receiver opens with a
/// balanced group.
pub(crate) fn statement_head(tokens: &TokenTable, dot: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut head = 0usize;
    let mut cursor = dot;
    while cursor > 0 {
        cursor -= 1;
        match kind_of(tokens, cursor)? {
            TokenKind::CloseParen | TokenKind::CloseSquare => depth += 1,
            TokenKind::CloseCurly if depth == 0 => {
                head = cursor + 1;
                break;
            }
            TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::StringInterpolationExpression => {
                // An expression-head group: the receiver itself opens here.
                if depth == 0 {
                    head = cursor;
                    break;
                }
                depth = depth.checked_sub(1)?;
            }
            // A depth-0 `{` behind the run is a block boundary: an
            // expression-head set/map literal would have closed before the
            // member access.
            TokenKind::OpenCurly if depth == 0 => {
                head = cursor + 1;
                break;
            }
            TokenKind::OpenCurly => depth = depth.checked_sub(1)?,
            TokenKind::Semicolon if depth == 0 => {
                head = cursor + 1;
                break;
            }
            _ => {}
        }
    }
    Some(head)
}

/// The loop variable of a one-parameter closure's list spanning
/// `[open, close]`: the last depth-0 identifier before an optional trailing
/// comma. A mid-list comma (arity over one) refuses.
pub(crate) fn single_param_name(tokens: &TokenTable, open: usize, close: usize) -> Option<usize> {
    let mut name = None;
    let mut depth = 0u32;
    for index in open + 1..close {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Comma if depth == 0 => {
                if index + 1 != close {
                    return None;
                }
            }
            TokenKind::Identifier if depth == 0 => name = Some(index),
            _ => {}
        }
    }
    let name = name?;
    Some(name)
}

/// `avoid_function_literals_in_foreach_calls`: `xs.forEach((e) { … });`
/// becomes `for (var e in xs) { … }`, and `xs.forEach((e) => e);` becomes a
/// block loop carrying the statement's own indent — the official
/// `ConvertForEachToForLoop` (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_for_each_to_for_loop.dart) as one whole-statement replacement.
/// The official faces: a block body keeps its text in place with the `);`
/// tail deleted; an expression body rewraps into a block; every `return`
/// statement of the body but none of a nested function literal becomes
/// `continue;`; an async or generator body refuses; the code-style
/// `final`/written-type loop-variable faces (the analysis options' `code`
/// section) are absent; a typeless set/map literal expression body refuses
/// (the `{e}` ambiguity). The report spans the `forEach` name; the receiver
/// text is re-verified as a balanced postfix expression over the statement
/// run behind it.
pub(crate) fn for_in_from_for_each(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    if !word_at(tokens, text, first, "forEach") {
        return None;
    }
    if kind_of(tokens, first.checked_sub(1)?)? != TokenKind::Period {
        return None;
    }
    let dot = first - 1;
    let head = statement_head(tokens, dot)?;
    // `await x.forEach(…)` moves the `await` into the loop's iterable —
    // `for (var e in await x)` — so the region opens on the `await` word
    // while the receiver verification reads past it.
    let start = head;
    let recv = if word_at(tokens, text, start, "await") {
        start + 1
    } else {
        start
    };
    if recv >= dot {
        return None;
    }
    // The receiver verifies as a balanced postfix expression: an expression
    // head, a selector tail, and no depth-0 `;` or `,` between.
    {
        let mut depth = 0u32;
        for index in recv..dot {
            match kind_of(tokens, index)? {
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth = depth.checked_sub(1)?;
                }
                TokenKind::Semicolon | TokenKind::Comma if depth == 0 => return None,
                _ => {}
            }
        }
        if depth != 0 {
            return None;
        }
        if !matches!(
            kind_of(tokens, recv)?,
            TokenKind::Identifier
                | TokenKind::Keyword(_)
                | TokenKind::Int
                | TokenKind::Double
                | TokenKind::Hexadecimal
                | TokenKind::String
                | TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
        ) {
            return None;
        }
        if !matches!(
            kind_of(tokens, dot - 1)?,
            TokenKind::Identifier
                | TokenKind::Keyword(_)
                | TokenKind::CloseParen
                | TokenKind::CloseSquare
                | TokenKind::CloseCurly
                | TokenKind::String
                | TokenKind::Int
                | TokenKind::Double
                | TokenKind::Hexadecimal
                | TokenKind::Bang
        ) {
            return None;
        }
    }
    // The closure: one named parameter, then the body — async and
    // generator markers (any token that is not the body opener) refuse.
    if kind_of(tokens, first + 2)? != TokenKind::OpenParen {
        return None;
    }
    let params_close = tokens.matching_end(first + 2)?;
    let name = single_param_name(tokens, first + 2, params_close)?;
    let name = tokens.lexeme(text, name).to_owned();
    let body_open = match kind_of(tokens, params_close + 1)? {
        TokenKind::OpenCurly | TokenKind::Function => params_close + 1,
        _ => return None,
    };
    let is_block = kind_of(tokens, body_open)? == TokenKind::OpenCurly;
    // The statement's `;` behind the invocation's own `)` (the argument
    // list open sits right after the name).
    let mut semi = None;
    let mut invoke_close = None;
    {
        let mut depth = 0u32;
        let mut index = params_close + 1;
        while index < tokens.len() {
            match kind_of(tokens, index)? {
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    if depth == 0 {
                        // The invocation's own closer: the `;` follows.
                        invoke_close = Some(index);
                        if kind_of(tokens, index + 1)? == TokenKind::Semicolon {
                            semi = Some(index + 1);
                        }
                        break;
                    }
                    depth = depth.checked_sub(1)?;
                }
                TokenKind::Semicolon if depth == 0 => {
                    semi = Some(index);
                    break;
                }
                _ => {}
            }
            index += 1;
        }
    }
    let semi = semi?;
    let invoke_close = invoke_close?;
    let target = slice16(text, &utf16_span_between(text, tokens, start, dot))?.to_owned();
    let region_start16 = tokens.get(start)?.utf16_offset;
    let semi_end16 = utf16_end(tokens, text, semi)?;
    let replacement = if is_block {
        let body_close = tokens.matching_end(body_open)?;
        let mut body = String::new();
        let mut at16 = tokens.get(body_open)?.utf16_offset;
        // Every `return` statement inside the body — except one inside a
        // nested function literal — becomes `continue;` (the official
        // `_ReturnVisitor`). A brace is a closure's when the group behind
        // it is not an `if`/`for`/`while`/`switch`/`catch` header and not
        // an `else`/`try`/`do` keyword.
        let mut exempt: Vec<u32> = Vec::new();
        let mut depth = 0u32;
        let mut index = body_open + 1;
        while index < body_close {
            match kind_of(tokens, index)? {
                TokenKind::Keyword(Keyword::Return) if exempt.iter().all(|&gate| depth < gate) => {
                    let stop = depth0_find(tokens, text, index + 1, body_close, |kind, _| {
                        kind == TokenKind::Semicolon
                    })?;
                    let ret_start16 = tokens.get(index)?.utf16_offset;
                    let stop_end16 = utf16_end(tokens, text, stop)?;
                    body.push_str(slice16(text, &(at16..ret_start16))?);
                    body.push_str("continue;");
                    at16 = stop_end16;
                    index = stop + 1;
                    continue;
                }
                TokenKind::OpenCurly => {
                    let closure = match kind_of(tokens, index - 1)? {
                        TokenKind::Keyword(Keyword::Else | Keyword::Try | Keyword::Do) => false,
                        TokenKind::CloseParen => {
                            let group = matching_open(tokens, index - 1, TokenKind::CloseParen)?;
                            !matches!(
                                kind_of(tokens, group.checked_sub(1)?)?,
                                TokenKind::Keyword(
                                    Keyword::If
                                        | Keyword::For
                                        | Keyword::While
                                        | Keyword::Switch
                                        | Keyword::Catch
                                )
                            )
                        }
                        _ => true,
                    };
                    depth += 1;
                    if closure {
                        exempt.push(depth);
                    }
                }
                TokenKind::CloseCurly => {
                    if exempt.last() == Some(&depth) {
                        exempt.pop();
                    }
                    depth = depth.checked_sub(1)?;
                }
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare => {
                    depth = depth.checked_sub(1)?;
                }
                _ => {}
            }
            index += 1;
        }
        let body_end16 = utf16_end(tokens, text, body_close)?;
        body.push_str(slice16(text, &(at16..body_end16))?);
        format!("for (var {name} in {target}) {body}")
    } else {
        // The expression face: a typeless set/map literal is ambiguous with
        // a block and refuses (the official's own guard).
        let expr_start = body_open + 1;
        if kind_of(tokens, expr_start)? == TokenKind::OpenCurly {
            return None;
        }
        let start_byte = utf16_offset_to_byte(text, region_start16);
        let line_start = text[..start_byte].rfind('\n').map_or(0, |i| i + 1);
        let prefix = text.get(line_start..start_byte)?;
        if !prefix.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            return None;
        }
        let expr = slice16(
            text,
            &utf16_span_between(text, tokens, expr_start, invoke_close),
        )?;
        format!("for (var {name} in {target}) {{\n{prefix}  {expr};\n{prefix}}}")
    };
    Some((
        region_start16,
        semi_end16.checked_sub(region_start16)?,
        replacement,
    ))
}

/// `prefer_for_elements_to_map_fromiterable`: `Map.fromIterable(xs, key:
/// (k) => k.id, value: (k) => k.name)` becomes
/// `{ for (var k in xs) k.id : k.name }` — the official
/// `ConvertMapFromIterableToForLiteral`'s same-parameter leg (upstream Dart
/// SDK source, pkg/analysis_server/lib/src/services/correction/dart/
/// convert_map_from_iterable_to_for_literal.dart), the one face whose
/// composition needs no reference inventory: the different-name legs
/// re-spell the closure bodies through a parameter-reference finder and
/// are absent, as is the block-body face beyond its single-`return` shape.
/// The report spans the whole creation; the head spelling is re-verified.
pub(crate) fn map_from_iterable_for_element(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if !word_at(tokens, text, first, "Map") || kind_of(tokens, last)? != TokenKind::CloseParen {
        return None;
    }
    let open = matching_open(tokens, last, TokenKind::CloseParen)?;
    if !word_at(tokens, text, open.checked_sub(1)?, "fromIterable") {
        return None;
    }
    let segments = depth0_segments(tokens, open + 1, last)?;
    if segments.len() != 3 {
        return None;
    }
    let iterator = unparenthesized(tokens, segments[0].clone())?;
    let key = named_closure_body(tokens, text, &segments[1], "key")
        .or_else(|| named_closure_body(tokens, text, &segments[2], "key"))?;
    let value = named_closure_body(tokens, text, &segments[2], "value")
        .or_else(|| named_closure_body(tokens, text, &segments[1], "value"))?;
    // The same loop-variable name is the reference-free leg; the
    // different-name legs need the parameter-reference face.
    if key.0 != value.0 {
        return None;
    }
    let iter_text = slice16(
        text,
        &utf16_span_between(text, tokens, iterator.start, iterator.end),
    )?;
    let (start16, length16) = span_over(text, tokens, first, last)?;
    Some((
        start16,
        length16,
        format!(
            "{{ for (var {p} in {iter_text}) {k} : {v} }}",
            p = key.0,
            k = key.1,
            v = value.1,
        ),
    ))
}

/// Wraps one balanced-paren pair strip: the segment with enclosing,
/// matching `(...)` wrappers removed (the official `unParenthesized`).
pub(crate) fn unparenthesized(
    tokens: &TokenTable,
    mut segment: core::ops::Range<usize>,
) -> Option<core::ops::Range<usize>> {
    while segment.start < segment.end
        && kind_of(tokens, segment.start)? == TokenKind::OpenParen
        && kind_of(tokens, segment.end - 1)? == TokenKind::CloseParen
        && tokens.matching_end(segment.start)? == segment.end - 1
    {
        segment.start += 1;
        segment.end -= 1;
    }
    if segment.start >= segment.end {
        return None;
    }
    Some(segment)
}

/// One named argument's single-parameter closure body: `name: (p) => expr`
/// or `name: (p) { return expr; }` — the official `_extractClosure` +
/// `_extractBody`. Returns the parameter's spelling and the body
/// expression's text.
pub(crate) fn named_closure_body(
    tokens: &TokenTable,
    text: &str,
    segment: &core::ops::Range<usize>,
    name: &str,
) -> Option<(String, String)> {
    let mut start = segment.start;
    if !word_at(tokens, text, start, name) || kind_of(tokens, start + 1)? != TokenKind::Colon {
        return None;
    }
    start += 2;
    let mut end = segment.end;
    let closure = unparenthesized(tokens, start..end)?;
    start = closure.start;
    end = closure.end;
    if kind_of(tokens, start)? != TokenKind::OpenParen {
        return None;
    }
    let params_close = tokens.matching_end(start)?;
    let param = single_param_name(tokens, start, params_close)?;
    let param = tokens.lexeme(text, param).to_owned();
    let body_start = params_close + 1;
    if body_start >= end {
        return None;
    }
    let body = if kind_of(tokens, body_start)? == TokenKind::Function {
        utf16_span_between(text, tokens, body_start + 1, end)
    } else {
        // The single-`return` block face: `{ return expr; }` exactly.
        if kind_of(tokens, body_start)? != TokenKind::OpenCurly {
            return None;
        }
        let body_close = tokens.matching_end(body_start)?;
        if body_close + 1 != end {
            return None;
        }
        if !matches!(
            kind_of(tokens, body_start + 1)?,
            TokenKind::Keyword(Keyword::Return)
        ) {
            return None;
        }
        let semi = depth0_find(tokens, text, body_start + 2, body_close, |kind, _| {
            kind == TokenKind::Semicolon
        })?;
        if semi + 1 != body_close {
            return None;
        }
        utf16_span_between(text, tokens, body_start + 2, semi)
    };
    let body_text = slice16(text, &body)?.to_owned();
    Some((param, body_text))
}

/// `sort_child_properties_last`: the reported `child:`/`children:` argument
/// moves behind the last argument — the official `SortChildPropertyLast`
/// (upstream Dart SDK source, pkg/analysis_server/lib/src/services/
/// correction/dart/sort_child_property_last.dart) as one contiguous-region
/// replacement: the args after the child keep their text, the child's text
/// (its trailing comma included when the list carries one) appends at the
/// producer's insertion point, and a comment sitting on the child's line
/// ahead of the next argument rides along. The child must not be first-
/// separated from the list by anything but a comma; the widget-identity
/// gate is the lint's own decision of record.
pub(crate) fn child_property_last(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, last) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first)? != TokenKind::Identifier
        || kind_of(tokens, first + 1)? != TokenKind::Colon
    {
        return None;
    }
    // The child is never last (the lint's own gate): its comma, then the
    // argument list's close at depth 0.
    if kind_of(tokens, last + 1)? != TokenKind::Comma {
        return None;
    }
    let mut depth = 0u32;
    let mut close = None;
    for index in last + 2..tokens.len() {
        match kind_of(tokens, index)? {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                if depth == 0 {
                    close = Some(index);
                    break;
                }
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Semicolon if depth == 0 => return None,
            _ => {}
        }
    }
    let close = close?;
    if kind_of(tokens, close)? != TokenKind::CloseParen {
        return None;
    }
    // The list open: the `(` itself when the child is the first argument,
    // else the comma whose group holds the list. The child text opens at
    // the previous token's end (the official `beginToken.previous!.end`).
    let (open, child_is_first) = match kind_of(tokens, first.checked_sub(1)?)? {
        TokenKind::OpenParen => (first - 1, true),
        TokenKind::Comma => (
            matching_open(tokens, first - 1, TokenKind::CloseParen)?,
            false,
        ),
        _ => return None,
    };
    if kind_of(tokens, open)? != TokenKind::OpenParen {
        return None;
    }
    let prev = if child_is_first { open } else { first - 1 };
    let child_start16 = utf16_end(tokens, text, prev)?;
    let child_offset16 = tokens.get(first)?.utf16_offset;
    // The last argument and the list's own trailing comma.
    let mut last_arg = close - 1;
    let list_comma = kind_of(tokens, last_arg)? == TokenKind::Comma;
    if list_comma {
        last_arg -= 1;
    }
    if last_arg <= last {
        return None;
    }
    // The child text: with the list comma the child's own comma rides;
    // without it the child's trailing comma stays behind as the middle
    // args' separator. A same-line comment ahead of the next argument
    // rides too (the official's comment walk, its args-not-on-one-line
    // gate included).
    let mut child_end16 = utf16_end(tokens, text, last + 1)?;
    let comma_end_byte = token_end_byte(tokens, last + 1);
    let next_arg_byte = tokens.get(last + 2)?.byte_offset as usize;
    let child_line = line_of(text, comma_end_byte);
    if line_of(text, next_arg_byte) != child_line {
        let carried = ride_line_comments(text, comma_end_byte, next_arg_byte, child_line);
        child_end16 += u32::try_from(utf16_count(&text[comma_end_byte..carried])).ok()?;
    }
    let (region_start16, tail_start16, child_text, insertion16) = if list_comma {
        let insertion16 = utf16_end(tokens, text, close - 1)?;
        let tail_start16 = if child_is_first {
            tokens.get(last + 2)?.utf16_offset
        } else {
            child_end16
        };
        let child_text = slice16(text, &(child_start16..child_end16))?.to_owned();
        (
            if child_is_first {
                child_offset16
            } else {
                child_start16
            },
            tail_start16,
            child_text,
            insertion16,
        )
    } else {
        let last_end16 = utf16_end(tokens, text, last_arg)?;
        let mut child_text =
            slice16(text, &(child_start16..utf16_end(tokens, text, last)?))?.to_owned();
        // No list comma: a `", "`/`","` separator prefixes the child (the
        // official spacing rule over the gap before the child).
        child_text.insert_str(
            0,
            if child_start16 == child_offset16 {
                ", "
            } else {
                ","
            },
        );
        let tail_start16 = if child_is_first {
            tokens.get(last + 2)?.utf16_offset
        } else {
            last_end16
        };
        (
            if child_is_first {
                child_offset16
            } else {
                child_start16
            },
            tail_start16,
            child_text,
            last_end16,
        )
    };
    let tail = slice16(text, &(tail_start16..insertion16))?.to_owned();
    let mut replacement = tail;
    replacement.push_str(&child_text);
    let length16 = insertion16.checked_sub(region_start16)?;
    Some((region_start16, length16, replacement))
}

/// `unnecessary_underscores`: the reported multi-underscore name becomes
/// `_` — the official `ConvertToWildcardVariable.automatically`'s
/// parameter and local faces (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// convert_to_wildcard_variable.dart). The parameter face carries the
/// official owner gate: only a closure's or a `static` method's plain
/// positional parameter converts — named parameters, receiver formals, and
/// every other declaration owner refuse; the local-variable face needs no
/// references here because the rule's own unreferenced gate already held
/// (the official's assigned-only reference check passes vacuously).
pub(crate) fn wildcard_underscore(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let index = token_starting_at(tokens, input.range.start)?;
    if kind_of(tokens, index)? != TokenKind::Identifier {
        return None;
    }
    let lexeme = tokens.lexeme(text, index);
    if lexeme.len() < 2 || !lexeme.bytes().all(|byte| byte == b'_') {
        return None;
    }
    let (offset, length) = span_over(text, tokens, index, index)?;
    // The parameter face: the nearest unmatched `(` before the name, with
    // no statement boundary crossing first.
    let mut depth = 0u32;
    let mut open = None;
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        match kind_of(tokens, cursor)? {
            TokenKind::CloseParen | TokenKind::CloseSquare => depth += 1,
            TokenKind::CloseCurly if depth == 0 => break,
            TokenKind::CloseCurly => depth += 1,
            TokenKind::OpenParen if depth == 0 => {
                open = Some(cursor);
                break;
            }
            TokenKind::OpenCurly if depth == 0 => {
                // A `{` behind the name: a named-parameter group (the
                // official `node.isNamed` bail) refuses; a block body's
                // opener leaves the local face.
                if cursor > 0
                    && matches!(
                        kind_of(tokens, cursor - 1)?,
                        TokenKind::OpenParen | TokenKind::Comma
                    )
                {
                    return None;
                }
                break;
            }
            // An optional-positional group is transparent: the parameter
            // list's own `(` decides the owner gate.
            TokenKind::OpenSquare if depth == 0 => {}
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Semicolon if depth == 0 => break,
            _ => {}
        }
    }
    let Some(open) = open else {
        // The local face: the name alone re-spells.
        return Some((offset, length, "_".to_owned()));
    };
    let close = tokens.matching_end(open)?;
    if index >= close {
        return None;
    }
    // A named parameter never converts (the official `node.isNamed` bail).
    let mut inner = 0u32;
    for probe in open + 1..index {
        match kind_of(tokens, probe)? {
            TokenKind::OpenCurly if inner == 0 => return None,
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => inner += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                inner = inner.checked_sub(1)?;
            }
            _ => {}
        }
    }
    // The owner gate: an identifier before the list is a declaration head
    // — only a `static` method converts; a `Function` keyword or a generic
    // closer is a function-type parameter's list; every other shape is a
    // closure.
    match kind_of(tokens, open.checked_sub(1)?)? {
        TokenKind::Identifier => {
            if !signature_has_static(tokens, open - 1)? {
                return None;
            }
        }
        TokenKind::Keyword(Keyword::Function)
        | TokenKind::Gt
        | TokenKind::GtGt
        | TokenKind::GtGtGt => return None,
        _ => {}
    }
    Some((offset, length, "_".to_owned()))
}

/// Whether the declaration head before a parameter-list name at `name`
/// carries `static` (the official `MethodDeclaration(:var isStatic)` gate):
/// the type tail behind the name is skipped (identifier/keyword/dot/`?`/
/// `<…>` units) until a `static`, a declaration boundary, or a metadata
/// marker.
pub(crate) fn signature_has_static(tokens: &TokenTable, name: usize) -> Option<bool> {
    let mut cursor = name;
    loop {
        cursor = match cursor.checked_sub(1) {
            Some(cursor) => cursor,
            None => return Some(false),
        };
        match kind_of(tokens, cursor)? {
            TokenKind::Identifier
            | TokenKind::Period
            | TokenKind::Question
            | TokenKind::Keyword(
                Keyword::Void
                | Keyword::Dynamic
                | Keyword::Function
                | Keyword::This
                | Keyword::Super,
            ) => {}
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt => {
                cursor = angle_group_start(tokens, cursor)?;
            }
            TokenKind::Keyword(Keyword::Static) => return Some(true),
            _ => return Some(false),
        }
    }
}

/// `use_key_in_widget_constructors`: the reported constructor gains the
/// `{super.key}` parameter — the official `AddKeyToConstructors`'s
/// constructor face, super-parameter leg (upstream Dart SDK source,
/// pkg/analysis_server/lib/src/services/correction/dart/
/// add_key_to_constructors.dart): inside an empty list, after the list's
/// open, into an existing named group's head, or after the last parameter
/// with the delimiters added. The class face (the constructor synthesis
/// with its const-ability analysis) and the explicit-super-initializer
/// update leg (`_updateSuper`) are absent: a `:` after the parameter list
/// refuses, and a hit whose next token is no `(` (the class-name report)
/// refuses. The key-type resolvability gate is the lint's own decision of
/// record; the written-type (`Key key`) face of a non-super-parameter
/// language level is absent with it.
pub(crate) fn add_super_key(input: &FixInput<'_>) -> Option<(u32, u32, String)> {
    let tokens = input.tokens?;
    let text = input.text;
    let (first, _) = hit_tokens(tokens, &input.range)?;
    if kind_of(tokens, first + 1)? != TokenKind::OpenParen {
        return None;
    }
    let open = first + 1;
    let close = tokens.matching_end(open)?;
    if kind_of(tokens, close + 1)? == TokenKind::Colon {
        return None;
    }
    if open + 1 == close {
        let offset = utf16_end(tokens, text, open)?;
        return Some((offset, 0, "{super.key}".to_owned()));
    }
    let mut depth = 0u32;
    let mut group_open = None;
    let mut optional = false;
    for index in open + 1..close {
        match kind_of(tokens, index)? {
            TokenKind::OpenCurly if depth == 0 => {
                group_open = Some(index);
                break;
            }
            TokenKind::OpenSquare if depth == 0 => optional = true,
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
    if let Some(group_open) = group_open {
        let offset = utf16_end(tokens, text, group_open)?;
        return Some((offset, 0, "super.key, ".to_owned()));
    }
    if optional {
        // The official chain fixes nothing behind an optional-positional
        // group.
        return None;
    }
    let mut last = close - 1;
    if kind_of(tokens, last)? == TokenKind::Comma {
        last -= 1;
    }
    let offset = utf16_end(tokens, text, last)?;
    Some((offset, 0, ", {super.key}".to_owned()))
}
