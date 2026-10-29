//! The missing-switch-cases family — dartq's equivalent of upstream's
//! `AddMissingSwitchCases` producer (upstream Dart SDK source,
//! pkg/analysis_server/lib/src/services/correction/dart/add_missing_switch_cases.dart;
//! fix kind `dart.fix.add.missingSwitchCases`, registration
//! pkg/analysis_server/lib/src/services/correction/fix_internal.dart —
//! `nonExhaustiveSwitchStatement` / `nonExhaustiveSwitchExpression`).
//! The diagnostic's correction slot carries the first missing pattern
//! text the witness engine rendered (the `{a2}` placeholder of the
//! oracle's `correctionMessage` — "Try adding a default case or cases
//! that match '<pattern>'." / "Try adding a wildcard pattern or cases
//! that match '<pattern>'."); this family un-renders that text out of
//! the message and writes one case arm per reported pattern through the
//! upstream `insertCaseClauseAtEnd` placement (upstream Dart SDK source,
//! pkg/analyzer_plugin/lib/src/utilities/change_builder/change_builder_dart.dart).
//!
//! Three upstream legs are deliberately out: the multi-pattern list
//! (dartq's witness is a single pattern, so every fix writes one arm),
//! the `null`-sorted-last reorder (a single pattern has no order), and
//! the `needsDefault` arm (its gate reads inaccessible enum members and
//! `src/` types through resolved elements — element facts the fix pass
//! does not carry; a pattern the correction cannot spell never reaches
//! the slot, so the arm is only reachable upstream). `missing_enum_
//! constant_in_switch` has no dartq emission; its upstream producer
//! (`AddMissingEnumCaseClauses`, fix_internal.dart) is a different
//! registration and stays out.

use super::*;

/// The switch-construct face a non-exhaustive code reports on —
/// `statement` writes `case <pattern>:` arms, `expression` writes
/// `<pattern> => throw UnimplementedError(),` arms (the upstream
/// `_switchStatement` / `_switchExpression` emitters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchFace {
    Statement,
    Expression,
}

/// The missing-switch-cases arm of an emitted semantic code, or `None`
/// for a code with no `AddMissingSwitchCases` producer.
#[must_use]
pub(crate) fn switch_case_arm(code: &str) -> Option<SwitchFace> {
    match code {
        "non_exhaustive_switch_statement" => Some(SwitchFace::Statement),
        "non_exhaustive_switch_expression" => Some(SwitchFace::Expression),
        _ => None,
    }
}

/// The missing pattern the diagnostic's correction slot names: the text
/// between the `'{a2}'` quotes of the oracle's correction message
/// ("…cases that match '<pattern>'."). Anything that does not carry the
/// shape — a hand-built row, a different correction family — yields no
/// fix rather than fabricate an arm.
fn missing_pattern(diagnostic: &Diagnostic) -> Option<&str> {
    let correction = diagnostic.correction.as_deref()?;
    let end = correction.rfind('\'')?;
    let start = correction[..end].rfind('\'')?;
    let pattern = &correction[start + 1..end];
    if pattern.is_empty() {
        return None;
    }
    Some(pattern)
}

/// The `switch ( … ) { … }` token frame of the reported `switch`
/// keyword — `(keyword, leftBracket, rightBracket, rightParenthesis)` —
/// or `None` when the span does not open on a switch the token stream
/// still carries. The diagnostic anchors the `switch` keyword;
/// upstream's producer walks `SwitchStatement`/`SwitchExpression` node
/// fields, which this face mirrors over the paired token groups.
fn switch_frame(tokens: &TokenTable, offset: u32) -> Option<(usize, usize, usize, usize)> {
    let keyword = token_starting_at(tokens, offset)?;
    if kind_of(tokens, keyword)? != TokenKind::Keyword(Keyword::Switch) {
        return None;
    }
    let open_paren = keyword + 1;
    let right_paren = skip_group(tokens, open_paren, TokenKind::OpenParen)?;
    let left_bracket = right_paren + 1;
    let right_bracket = skip_group(tokens, left_bracket, TokenKind::OpenCurly)?;
    Some((keyword, left_bracket, right_bracket, right_paren))
}

/// The fix's inserted text and insertion point, the verbatim
/// `insertCaseClauseAtEnd` decision: a single-line body inserts at the
/// left bracket's end (a newline, the arm lines, then the switch line's
/// prefix to re-seat the closing `}`); a multi-line body inserts at the
/// start of the closing bracket's line (the arm lines only — the line's
/// own indentation then leads the `}`). Brackets a recovery parse
/// synthesized (`is_synthetic`) take the upstream synthetic arms: a
/// synthetic left bracket also writes `" {"` at the right parenthesis's
/// end; a synthetic right bracket appends `"}"`.
fn insertion(
    text: &str,
    tokens: &TokenTable,
    face: SwitchFace,
    pattern: &str,
    frame: (usize, usize, usize, usize),
) -> Option<(u32, u32, String)> {
    let (keyword, left_bracket, right_bracket, right_paren) = frame;
    let left = tokens.get(left_bracket)?;
    let right = tokens.get(right_bracket)?;
    let same_line =
        line_of(text, left.byte_offset as usize) == line_of(text, right.byte_offset as usize);
    // The switch line's leading whitespace — `linePrefix(switchKeyword)`
    // upstream; the arm lines ride it one `oneIndent` (two spaces) deep.
    let keyword_byte = tokens.get(keyword)?.byte_offset as usize;
    let line_start = text[..keyword_byte].rfind('\n').map_or(0, |i| i + 1);
    let line_indent = &text[line_start..line_indent_end(text, line_start)];
    let mut replacement = String::new();
    if left.is_synthetic() {
        replacement.push_str(" {");
    }
    if same_line {
        replacement.push('\n');
    }
    write_arm(&mut replacement, face, line_indent, pattern);
    if same_line {
        replacement.push_str(line_indent);
    }
    if right.is_synthetic() {
        replacement.push('}');
    }
    let byte = if same_line {
        if left.is_synthetic() {
            let paren = tokens.get(right_paren)?;
            paren.byte_offset as usize + paren.byte_len as usize
        } else {
            left.byte_offset as usize + left.byte_len as usize
        }
    } else {
        // `getOffsetOfLine(rightBracket.line - 1)` upstream (1-based line
        // to 0-based index): the bracket's OWN line start. Not
        // `line_start_before` — its walk-back guard skips a terminator
        // directly preceding the byte, which a `}` first on its line
        // (`};`) is, stranding the anchor on the previous line.
        text[..right.byte_offset as usize]
            .rfind('\n')
            .map_or(0, |i| i + 1)
    };
    Some((byte_offset_to_utf16(text, byte), 0, replacement))
}

/// One arm's lines at `lineIndent + oneIndent` — the upstream
/// `_switchStatement` / `_switchExpression` `write`/`writeln` sequence
/// verbatim (two-space `oneIndent`, the `// TODO:` line, the throw).
fn write_arm(replacement: &mut String, face: SwitchFace, line_indent: &str, pattern: &str) {
    let arm = format!("{line_indent}  ");
    match face {
        SwitchFace::Statement => {
            replacement.push_str(&arm);
            replacement.push_str("case ");
            replacement.push_str(pattern);
            replacement.push_str(":\n");
            replacement.push_str(&arm);
            replacement.push_str("  // TODO: Handle this case.\n");
            replacement.push_str(&arm);
            replacement.push_str("  throw UnimplementedError();\n");
        }
        SwitchFace::Expression => {
            replacement.push_str(&arm);
            replacement.push_str("// TODO: Handle this case.\n");
            replacement.push_str(&arm);
            replacement.push_str(pattern);
            replacement.push_str(" => throw UnimplementedError(),\n");
        }
    }
}

/// The missing-switch-cases producer: one `RuleFix` per report the
/// correction slot still carries a pattern for, at the upstream
/// `insertCaseClauseAtEnd` position.
#[must_use]
pub(crate) fn from_switch_cases(
    diagnostics: &[Diagnostic],
    text: &str,
    tokens: &TokenTable,
    lines: &LineIndex,
    fuse: &PlannerFuse,
) -> Vec<RuleFix> {
    let mut rows = Vec::new();
    for diagnostic in diagnostics {
        if !fuse.step() {
            break;
        }
        let Some(face) = switch_case_arm(diagnostic.code.name) else {
            continue;
        };
        let Some(pattern) = missing_pattern(diagnostic) else {
            continue;
        };
        let Some(frame) = switch_frame(tokens, diagnostic.range.offset) else {
            continue;
        };
        let Some((offset, length, replacement)) = insertion(text, tokens, face, pattern, frame)
        else {
            continue;
        };
        rows.push(RuleFix {
            rule: diagnostic.code.name,
            // The upstream fix kind's message (fix.dart:
            // `addMissingSwitchCases` — 'Add missing switch cases').
            message: "Add missing switch cases".to_owned(),
            source: BUILTIN_SEMANTIC_SOURCE,
            edit: anchored_edit(text, lines, offset, length, &replacement),
            target: None,
            role: FixRole::Primary,
            diag: Some(diagnostic.range),
            group: rows.len() as u32,
        });
    }
    rows
}
