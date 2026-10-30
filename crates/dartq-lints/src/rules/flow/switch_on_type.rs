use dartq_ast::NodeKind;
use dartq_scanner::TokenKind;

use super::{Check, PayloadField, RuleSpec};
use crate::rules::NONE_TOKEN;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/switch_on_type.dart` — a switch
/// statement/expression whose scrutinee's static type is assignable
/// to `Type` reports once at the scrutinee expression node
/// (`visitSwitchExpression`/`visitSwitchStatement` :53-60 →
/// `_processExpression` :71-126): the recursion faces are the
/// then/else arms of a conditional, both operands of a `+` (string
/// building), a nested switch expression's case values, and a string
/// interpolation's segments; the leaf faces are a `.runtimeType`
/// tail, a bare `Type`-written name, a type parameter, and a type
/// literal — a `.toString()` call walks its target (:111-112). The
/// report pins the scrutinee expression's own text span.
///
/// Gate: the official registration bails without the patterns feature
/// (:34) — the same [`crate::engine::Features::patterns`] bit.
///
/// Faces over the syntax substrate: both switch heads wrap the
/// scrutinee in a parenthesized-condition container
/// (`ensure_parenthesized_condition`), so the head walk unwraps it —
/// the adopted-begin tiling leaks some expression rows ahead of the
/// container and keeps the rest inside it. The report span is
/// token-anchored: the first expression row opens it and the token
/// before the container's `)` closes it, the exact expression text
/// the official `reportAtNode` pins. The written-type face reads
/// same-file bindings (`Type t` reports, `String s` and inferred
/// locals stay silent, the nullability-sensitive `isAssignableTo`
/// posture); the implicit-`this` `runtimeType` face is name-level (a
/// same-file override returning a non-`Type` over-reports); the
/// type-literal face reuses the `type_literal_in_constant_pattern`
/// channel (same-file type or an unshadowed built-in; a prefixed
/// spelling under-reports). The string-interpolation faces ride the
/// token layer (`_processInterpolation` :128-149): the segments
/// produce no AST rows, so the head's token run is scanned — a
/// depth-0 `$name` segment resolves through the identifier face and a
/// `${expr}` segment re-enters the token walk (conditional arms,
/// nested switch values, `+` operands, the leaf chain). Message:
/// `diagnostic.g.dart` `switchOnType`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "switch_on_type",
    &[NodeKind::SwitchStatement, NodeKind::SwitchExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    // The registration bail (:34).
    if !c.unit.features.patterns {
        return;
    }
    let Some((rows, open)) = scrutinee_head(c) else {
        return;
    };
    if rows.is_empty() {
        return;
    }
    // The row faces first; the interpolation segments (no AST rows) ride
    // the token layer. Both answer the same single scrutinee report.
    let row_face = type_scrutinee(c, &rows);
    let token_face = !row_face && interpolation_scrutinee(c, &rows, open);
    if row_face || token_face {
        report_scrutinee(c, &rows, open);
    }
}

/// One report per switch, at the scrutinee expression
/// (`reportAtNode(node.expression)`): the token-anchored span — the
/// token right after the wrapping `(` opens it and the token before
/// the matching `)` closes it, the exact expression text the official
/// report pins. The row span is the fallback: the adopted-begin tiling
/// can leak the head's leading pieces ahead of the container, so a row
/// read can start inside the expression (a nested switch's head row
/// anchored at its scrutinee, not the `switch` keyword).
fn report_scrutinee(c: &mut Check<'_>, rows: &[u32], open: Option<u32>) {
    let span = open
        .and_then(|open| {
            let close = c.unit.tokens.matching_end(open as usize)?;
            let first = u32::try_from(open as usize + 1).ok()?;
            let last = u32::try_from(close.checked_sub(1)?).ok()?;
            (close > open as usize + 1)
                .then(|| crate::rules::utf16_token_span(c, first).start)
                .zip(Some(crate::rules::utf16_token_span(c, last).end))
                .map(|(start, end)| start..end)
        })
        .unwrap_or_else(|| {
            let start = c.utf16_span(rows[0]).start;
            let end = c.utf16_span(rows[rows.len() - 1]).end;
            start..end
        });
    c.report(span, &[]);
}

/// The scrutinee piece run with the head container unwrapped, plus the
/// `(` token of the wrapping container when present. A `case` clause
/// (`switch (x case ...)`) is not the lint's face.
fn scrutinee_head(c: &Check<'_>) -> Option<(Vec<u32>, Option<u32>)> {
    let node = c.node();
    let kids = c.collect_direct(node);
    match c.kind(node) {
        NodeKind::SwitchStatement => {
            let position = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)?;
            let container = kids[position];
            if c.field(container, PayloadField::CaseKeyword)
                .is_some_and(|case| case != NONE_TOKEN)
            {
                return None;
            }
            let mut rows: Vec<u32> = kids[..position].to_vec();
            rows.extend(c.children(container));
            Some((rows, Some(c.head_token(container))))
        }
        NodeKind::SwitchExpression => {
            let position = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock)?;
            // The head's parenthesized-condition container carries the
            // scrutinee rows its tiling did not leak ahead of it; unwrap
            // it the way the statement face does.
            let mut rows: Vec<u32> = Vec::new();
            let mut open = None;
            for &kid in &kids[..position] {
                if c.kind(kid) == NodeKind::ParenthesizedCondition {
                    rows.extend(c.children(kid));
                    open = Some(c.head_token(kid));
                } else {
                    rows.push(kid);
                }
            }
            Some((rows, open))
        }
        _ => None,
    }
}

/// The official `_processExpression` walk (:71-121): the first
/// `Type`-valued leaf reports once.
fn type_scrutinee(c: &Check<'_>, rows: &[u32]) -> bool {
    let Some(position) = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::SwitchExpression)
    else {
        return walk(c, rows);
    };
    // A nested switch expression: its case values walk (:86-93). The
    // value rows follow the arrow payload token.
    let block = c
        .collect_direct(rows[position])
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock);
    let Some(block) = block else {
        return false;
    };
    for case in c.collect_direct(block) {
        if c.kind(case) != NodeKind::SwitchExpressionCase {
            continue;
        }
        let Some(arrow) = c.field(case, PayloadField::Arrow) else {
            continue;
        };
        if arrow == NONE_TOKEN {
            continue;
        }
        let arrow_byte = token_byte(c, arrow);
        let value_rows: Vec<u32> = c
            .children(case)
            .filter(|&row| c.byte_span(row).start > arrow_byte)
            .collect();
        if type_scrutinee(c, &value_rows) {
            return true;
        }
    }
    false
}

fn walk(c: &Check<'_>, rows: &[u32]) -> bool {
    // The conditional face: the arms decide, the condition is ignored
    // (:78-84).
    if let Some(position) = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::Conditional)
    {
        let arms: Vec<u32> = c.children(rows[position]).collect();
        return walk(c, &arms);
    }
    // The `+` concatenation face: both operand runs walk (:95-104).
    if let Some(position) = rows.iter().rposition(|&row| {
        c.kind(row) == NodeKind::BinaryExpression
            && c.lexeme(c.field(row, PayloadField::Operator).unwrap_or(NONE_TOKEN)) == "+"
    }) {
        let operator = c
            .field(rows[position], PayloadField::Operator)
            .unwrap_or(NONE_TOKEN);
        let operator_byte = token_byte(c, operator);
        let split = rows.partition_point(|&row| c.byte_span(row).end <= operator_byte);
        // The container row spans the operator, so it falls in neither
        // operand side: the right walk takes only the rows beginning
        // after the operator byte. Each half is therefore strictly
        // smaller than the walked run (the triggering row is in the
        // run but in neither half) — structural progress, no re-walk
        // of the same slice.
        let right: Vec<u32> = rows[split..]
            .iter()
            .copied()
            .filter(|&row| c.byte_span(row).start > operator_byte)
            .collect();
        return walk(c, &rows[..split]) || walk(c, &right);
    }
    // The invocation face: a `.toString()` selector walks its target
    // (:111-112); a bare name is the identifier face itself. A
    // `NoArguments` send is a tear-off, not the official
    // `MethodInvocation` face — the outermost selector's property
    // type decides (the static-type read of :97-107), i.e. the
    // access-chain face below (`n.runtimeType.toString` is silent,
    // `test_runtimeTypeToString_noCall`).
    if let Some(position) = rows.iter().rposition(|&row| c.kind(row) == NodeKind::Send) {
        let Some(mut head) = position.checked_sub(1) else {
            return false;
        };
        match c.kind(rows[head]) {
            NodeKind::Arguments => head -= 1,
            NodeKind::NoArguments => return access_chain_face(c, rows),
            _ => return false,
        }
        loop {
            match c.kind(rows[head]) {
                NodeKind::TypeArguments | NodeKind::NoTypeArguments => head -= 1,
                NodeKind::Identifier | NodeKind::DotAccess => break,
                _ => return false,
            }
        }
        let lexeme = c.lexeme(c.head_token(rows[head]));
        if lexeme == "toString" {
            return head > 0 && walk(c, &rows[..head]);
        }
        // A bare invocation has no real target (:111 — `realTarget?`).
        if c.kind(rows[head]) != NodeKind::Identifier {
            return false;
        }
        return name_is_type(c, rows[head], lexeme);
    }
    // The access-chain face: only the outermost selector names the
    // expression's type (`n.runtimeType` reports,
    // `n.runtimeType.toString` does not).
    access_chain_face(c, rows)
}

/// The static-type read's access-chain posture (:97-107): the
/// outermost selector names the expression's type.
fn access_chain_face(c: &Check<'_>, rows: &[u32]) -> bool {
    if let Some(&tail) = rows
        .iter()
        .rev()
        .find(|&row| c.kind(*row) == NodeKind::DotAccess)
    {
        return c.lexeme(c.head_token(tail)) == "runtimeType";
    }
    // The bare-name face: the run's root identifier.
    if let Some(&name) = rows
        .iter()
        .rev()
        .find(|&row| c.kind(*row) == NodeKind::Identifier)
    {
        return name_is_type(c, name, c.lexeme(c.head_token(name)));
    }
    false
}

/// The `Type`-valued name faces: a written `Type` binding first (the
/// shadow-correct same-file walk), then the implicit-`this`
/// `runtimeType` face, a type parameter, and the type-literal
/// channel.
fn name_is_type(c: &Check<'_>, use_row: u32, lexeme: &str) -> bool {
    if crate::rules::expressions::same_file::binding(c, use_row, lexeme).is_some() {
        return crate::rules::written_type_face(c, use_row, lexeme)
            .is_some_and(|face| face.name == Some("Type") && !face.nullable);
    }
    // `runtimeType` on implicit `this` (:94-99 — the corpus
    // `runtimeType`/`this.runtimeType` faces); a same-file override
    // returning a non-`Type` over-reports.
    if lexeme == "runtimeType" {
        return true;
    }
    if is_enclosing_type_parameter(c, lexeme) {
        return true;
    }
    crate::rules::expressions::same_file::declares_type(c, lexeme)
        || super::type_literal_in_constant_pattern::is_builtin_type_name(lexeme)
}

/// Whether the name is a type parameter of an enclosing declaration
/// (`switch (T)`, :336-343 — the type-literal face).
fn is_enclosing_type_parameter(c: &Check<'_>, name: &str) -> bool {
    for &ancestor in c.path.iter().rev().skip(1) {
        let variables = c
            .collect_direct(ancestor)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
        let Some(variables) = variables else {
            continue;
        };
        for variable in c.collect_direct(variables) {
            if c.kind(variable) == NodeKind::TypeVariable
                && c.lexeme(c.head_token(variable)) == name
            {
                return true;
            }
        }
    }
    false
}

/// The string-interpolation faces (`_processInterpolation` :128-149):
/// the segments of a scrutinee interpolation produce no AST rows, so
/// the head's token run is scanned. A depth-0 `$name` segment resolves
/// through the identifier face, a `${expr}` segment re-enters the
/// token walk; segments buried in nested groups stay silent the way
/// the official expression-shape read leaves a parenthesized operand
/// unanswered. `open` is the wrapping container's `(`.
fn interpolation_scrutinee(c: &Check<'_>, rows: &[u32], open: Option<u32>) -> bool {
    let Some(open) = open else {
        return false;
    };
    // The token run is `(open+1 .. close)`: the container's `(` anchors
    // the walk regardless of which expression rows the tiling leaked —
    // the row set only supplies the binding anchor.
    if c.lexeme(open) != "(" {
        return false;
    }
    let Some(close) = c.unit.tokens.matching_end(open as usize) else {
        return false;
    };
    let Ok(close) = u32::try_from(close) else {
        return false;
    };
    token_walk(c, open + 1, close, rows[0])
}

/// The token-run mirror of `_processExpression` (:76-126) over
/// `[start, end)`: the depth-0 structure decides — interpolation
/// segments, the conditional split, a nested switch expression's case
/// values, the `+` split — then the leaf access chain. `anchor` is the
/// row the name faces read bindings against (the head's first
/// expression row; every relevant binding precedes it).
fn token_walk(c: &Check<'_>, start: u32, end: u32, anchor: u32) -> bool {
    if start >= end {
        return false;
    }
    let tokens = c.unit.tokens;
    // The interpolation segments first (:80-82): a `$name` segment is
    // the `$` marker plus the name token; a `${expr}` segment's group
    // re-enters the walk. The depth-0 scan keeps a segment buried in a
    // nested group silent — the official read answers only the
    // expression shapes themselves.
    let mut depth: u32 = 0;
    let mut i = start;
    while i < end {
        match tokens.get(i as usize).map_or(TokenKind::Eof, |t| t.kind) {
            TokenKind::StringInterpolationIdentifier if depth == 0 => {
                if i + 1 < end && name_is_type(c, anchor, c.lexeme(i + 1)) {
                    return true;
                }
                i += 2;
                continue;
            }
            TokenKind::StringInterpolationExpression if depth == 0 => {
                let Some(close) = tokens.matching_end(i as usize) else {
                    return false;
                };
                let Ok(close) = u32::try_from(close) else {
                    return false;
                };
                if close >= end {
                    return false;
                }
                if token_walk(c, i + 1, close, anchor) {
                    return true;
                }
                i = close + 1;
                continue;
            }
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        i += 1;
    }
    // The depth-0 structure scan: the conditional's `?`/`:`, the `+`
    // operands, a nested switch expression's head.
    let mut depth: u32 = 0;
    let mut question = None;
    let mut colon = None;
    let mut plus = None;
    let mut nested = None;
    let mut i = start;
    while i < end {
        match tokens.get(i as usize).map_or(TokenKind::Eof, |t| t.kind) {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            TokenKind::Question if depth == 0 => question = Some(i),
            TokenKind::Colon if depth == 0 && question.is_some() && colon.is_none() => {
                colon = Some(i);
            }
            TokenKind::Plus if depth == 0 => plus = Some(i),
            TokenKind::Keyword(_) if depth == 0 && c.lexeme(i) == "switch" => nested = Some(i),
            _ => {}
        }
        i += 1;
    }
    // The conditional face (:83-89): the arms decide, the condition is
    // ignored.
    if let (Some(q), Some(colon)) = (question, colon)
        && colon > q
    {
        return token_walk(c, start, q, anchor) || token_walk(c, colon + 1, end, anchor);
    }
    // A nested switch expression's case values walk (:91-98): the value
    // run after each depth-0 `=>` inside the head's block.
    if let Some(switch_kw) = nested
        && let Some(values) = nested_switch_values(c, switch_kw, end)
    {
        return values
            .into_iter()
            .any(|range| token_walk(c, range.start, range.end, anchor));
    }
    // The `+` concatenation face (:100-109): both operand runs walk.
    if let Some(plus) = plus {
        return token_walk(c, start, plus, anchor) || token_walk(c, plus + 1, end, anchor);
    }
    token_leaf(c, start, end, anchor)
}

/// The case-value runs of the switch-expression block that follows the
/// `switch (` group opening at `switch_kw`: each depth-0 `=>` inside
/// the block opens a value run ending at the next `,` (or the block's
/// `}`).
fn nested_switch_values(
    c: &Check<'_>,
    switch_kw: u32,
    end: u32,
) -> Option<Vec<core::ops::Range<u32>>> {
    let tokens = c.unit.tokens;
    let open = switch_kw.checked_add(1)?;
    (c.lexeme(open) == "(").then_some(())?;
    let close = u32::try_from(tokens.matching_end(open as usize)?).ok()?;
    let block = close.checked_add(1)?;
    (c.lexeme(block) == "{").then_some(())?;
    let block_close = u32::try_from(tokens.matching_end(block as usize)?).ok()?;
    let block_close = block_close.min(end);
    let mut values = Vec::new();
    let mut depth: u32 = 0;
    let mut value: Option<u32> = None;
    let mut i = block + 1;
    while i < block_close {
        match tokens.get(i as usize).map_or(TokenKind::Eof, |t| t.kind) {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            // `=>` lexes as one token in expression position.
            TokenKind::Function if depth == 0 => value = Some(i + 1),
            TokenKind::Comma if depth == 0 && value.is_some() => {
                let start = value.take().expect("checked above");
                values.push(start..i);
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(start) = value {
        values.push(start..block_close);
    }
    Some(values)
}

/// The leaf face over a token run (:111-119): a trailing invocation
/// group whose callee is `.toString` walks its target; the outermost
/// top-level selector names the expression's type (`runtimeType`
/// reports, other selectors and tear-offs stay silent); a bare name is
/// the identifier face.
fn token_leaf(c: &Check<'_>, start: u32, end: u32, anchor: u32) -> bool {
    let tokens = c.unit.tokens;
    let tail = end - 1;
    // A trailing balanced group: `name(args)` (a call) or `(...)`
    // (parenthesized — the official expression read answers no arm and
    // stays silent).
    if c.lexeme(tail) == ")" {
        let Some(group_open) = group_open_before(c, start, tail) else {
            return false;
        };
        if group_open == start {
            return false;
        }
        let head = group_open - 1;
        if head > start && c.lexeme(head - 1) == "." && c.lexeme(head) == "toString" {
            return token_walk(c, start, head - 1, anchor);
        }
        return false;
    }
    // The outermost top-level `.` selector.
    let mut depth: u32 = 0;
    let mut dot = None;
    let mut i = start;
    while i < end {
        match tokens.get(i as usize).map_or(TokenKind::Eof, |t| t.kind) {
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth = depth.saturating_sub(1);
            }
            TokenKind::Period if depth == 0 => dot = Some(i),
            _ => {}
        }
        i += 1;
    }
    if let Some(dot) = dot
        && dot + 1 < end
        && tokens
            .get(dot as usize + 1)
            .is_some_and(|t| t.kind == TokenKind::Identifier)
    {
        // `a.runtimeType` reports; `a.b` and `a.toString` tear-offs name
        // no `Type` on this read.
        return c.lexeme(dot + 1) == "runtimeType";
    }
    // The bare-name face: the run is one identifier (`switch (T)`,
    // `switch (int)`); any other shape answers no official arm.
    (end == start + 1)
        && tokens
            .get(start as usize)
            .is_some_and(|t| t.kind == TokenKind::Identifier)
        && name_is_type(c, anchor, c.lexeme(start))
}

/// The matching group opener at or after `start` for the closer at
/// `close` (a backward depth scan; an unbalanced run answers `None`).
fn group_open_before(c: &Check<'_>, start: u32, close: u32) -> Option<u32> {
    let tokens = c.unit.tokens;
    let mut depth: u32 = 0;
    let mut i = close;
    while i > start {
        i -= 1;
        match tokens.get(i as usize).map_or(TokenKind::Eof, |t| t.kind) {
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                depth += 1;
            }
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn token_byte(c: &Check<'_>, token: u32) -> usize {
    c.unit
        .tokens
        .get(token as usize)
        .map_or(usize::MAX, |token| token.byte_offset as usize)
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    fn run(source: &str) -> Vec<crate::engine::Lint> {
        Fixture::parse(source).run(&one_rule("switch_on_type"))
    }

    /// `test_variableType` — a `Type`-written parameter reports.
    #[test]
    fn type_written_parameter_reports() {
        let source = "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Avoid switch statements on a 'Type'.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try using pattern matching on a variable instead.")
        );
        assert_eq!(
            // The report pins the scrutinee expression
            // (`reportAtNode(node.expression)`) — the `t` inside the
            // switch head, not the parameter spelling.
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "t", 1))
        );
    }

    /// `test_runtimeType` / `test_insideClass_implicitThis` /
    /// `test_insideClass_withThis` — the `.runtimeType` faces.
    #[test]
    fn runtime_type_faces_report() {
        assert_eq!(
            run("void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("class A {\n  void m() {\n    switch (runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("class A {\n  void m() {\n    switch (this.runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
            1
        );
    }

    /// `test_runtimeTypeToString` / `test_stringAddition` — the
    /// `.toString()` walk.
    #[test]
    fn to_string_calls_walk_their_target() {
        assert_eq!(
            run("void f(num n) {\n  switch (n.runtimeType.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
        assert_eq!(
            run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
            1
        );
    }

    /// The `+` concatenation face walks both operand runs and
    /// terminates: the container row spans the operator and belongs to
    /// neither operand side (regression pin — the walked run used to
    /// re-enter its own slice and overflow the stack).
    #[test]
    fn plus_operand_runs_walk_and_terminate() {
        // The left operand is the `Type` leaf.
        assert_eq!(
            run("void f(Type t) {\n  switch (t.toString() + '') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
        // The right operand is the `Type` leaf (the crash shape: the
        // post-operator run held the container row).
        assert_eq!(
            run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
        // Neither operand is `Type`-valued — silent.
        assert!(
            run("void f(num n) {\n  switch (n.toString() + '') {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// `test_runtimeTypeToString_noCall` — the tear-off's outermost
    /// selector is `toString`, not `Type`.
    #[test]
    fn tear_off_access_chain_is_silent() {
        assert!(
            run("void f(num n) {\n  switch (n.runtimeType.toString) {\n    case function:\n      break;\n    default:\n      break;\n  }\n}\n\nvoid function() {}\n")
                .is_empty()
        );
    }

    /// `test_other` / `test_dynamic` / `test_variable_typeToString` —
    /// non-`Type` scrutinees stay silent.
    #[test]
    fn non_type_scrutinees_are_silent() {
        assert!(
            run("void f(num i) {\n  switch (i) {\n    case int _:\n      break;\n    case double _:\n      break;\n  }\n}\n")
                .is_empty()
        );
        assert!(run("void foo(dynamic a) {\n  switch (a) {}\n}\n").is_empty());
        assert!(
            run("void f(Object? o) {\n  final type = o.runtimeType.toString();\n  switch (type) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// `test_functionToString` / `test_functionToString_prefixed` —
    /// bare and prefixed `toString` invocations have no `Type`
    /// target.
    #[test]
    fn bare_to_string_is_silent() {
        assert!(
            run("void f() {\n  switch (toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                .is_empty()
        );
        assert!(
            run("import '' as self;\n\nvoid f() {\n  switch (self.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                .is_empty()
        );
    }

    /// `test_conditionalBoth` — the arms walk.
    #[test]
    fn conditional_arm_reports() {
        assert_eq!(
            run("void f(Type t) {\n  switch (1 == 1 ? t : 'other') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// `test_nestedSwitch` — the nested switch expression's case
    /// values walk.
    #[test]
    fn nested_switch_expression_reports() {
        assert_eq!(
            run("void f(Type t, Object o) {\n  switch (switch (o) { _ => t }) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// `test_typeParameter` — a bare type parameter is a type
    /// literal.
    #[test]
    fn type_parameter_reports() {
        assert_eq!(
            run("void f<T>() {\n  switch (T) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                .len(),
            1
        );
    }

    /// The switch-expression face reports on its scrutinee.
    #[test]
    fn switch_expression_scrutinee_reports() {
        assert_eq!(
            run("void f(Type t) {\n  (switch (t) { _ => null });\n}\n").len(),
            1
        );
    }

    /// The gate (:34): with the feature off the rule is silent.
    #[test]
    fn patterns_feature_off_is_silent() {
        let mut f = Fixture::parse(
            "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n",
        );
        f.features.patterns = false;
        assert!(f.run(&one_rule("switch_on_type")).is_empty());
    }

    /// A `String`-written name shadows the built-in reading
    /// (`switch (int)` with a local `int` binding stays consistent
    /// with the official element read).
    #[test]
    fn value_binding_shadows_the_type_reading() {
        assert!(
            run("void f() {\n  var int = 3;\n  switch (int) {\n    case 3:\n      break;\n    default:\n      break;\n  }\n}\n")
                .is_empty()
        );
    }

    /// The report pins the scrutinee EXPRESSION text — the container's
    /// window rows run into the head's `)`, the token-anchored close
    /// does not (`test_runtimeType`: `[!i.runtimeType!]`).
    #[test]
    fn report_pins_the_scrutinee_run() {
        let source = "void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "i", 1))
        );
        // `i.runtimeType` — 13 UTF-16 units, the `)` excluded.
        assert_eq!(lints[0].range.end - lints[0].range.start, 13);
    }

    /// `test_variableType` — a bare `Type` scrutinee pins exactly its
    /// name (`[!t!]`, one UTF-16 unit): the token-anchored close lands
    /// on the token before the container's `)`, never the `)` itself.
    #[test]
    fn bare_name_report_excludes_the_paren() {
        let source = "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "t", 1))
        );
        assert_eq!(lints[0].range.end - lints[0].range.start, 1);
    }

    /// `test_nestedSwitch` — the nested switch expression IS the
    /// scrutinee expression the official `reportAtNode` pins
    /// (`[!switch (o) {_ => t}!]` — 18 UTF-16 units from the inner
    /// `switch` keyword).
    #[test]
    fn nested_switch_report_pins_the_expression() {
        let source = "void f(Type t, Object o) {\n  switch (switch (o) {_ => t}) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "switch", 1))
        );
        // `switch (o) {_ => t}` — 18 UTF-16 units.
        assert_eq!(lints[0].range.end - lints[0].range.start, 18);
    }

    /// `SwitchExpressionOnTypeTest` — the expression head's container
    /// unwraps like the statement head: the variable face, the
    /// conditional arms, the `+` operands, and the `.runtimeType` access
    /// chain all decide inside `(switch (...) {`.
    #[test]
    fn switch_expression_unwrap_faces_report() {
        let expr = |scrutinee: &str| {
            format!(
                "void f(Type t, num i) {{\n  (switch ({scrutinee}) {{\n    'type: int' => null,\n    _ => null,\n  }});\n}}\n"
            )
        };
        // test_variableType
        assert_eq!(run(&expr("t")).len(), 1, "t");
        // test_runtimeType
        assert_eq!(run(&expr("i.runtimeType")).len(), 1, "i.runtimeType");
        // test_conditionalBoth / test_conditionalElse
        assert_eq!(run(&expr("1 == 1 ? t : '$t'")).len(), 1, "cond both");
        assert_eq!(run(&expr("1 == 1 ? 'other' : '$t'")).len(), 1, "cond else");
        // test_binaryExpression
        assert_eq!(run(&expr("'' + '$t'")).len(), 1, "binary");
        // test_runtimeTypeToString
        assert_eq!(run(&expr("i.runtimeType.toString()")).len(), 1, "toString");
        // test_nestedSwitch
        assert_eq!(run(&expr("switch (i) { _ => t }")).len(), 1, "nested");
    }

    /// `test_stringInterpolation*` — the interpolation segments ride the
    /// token layer on both switch heads: a `$t` segment and every
    /// `${...}` shape (a conditional arm, a nested interpolation, a
    /// nested switch's value, a `.toString()` target) report; a segment
    /// buried in a parenthesized operand stays silent the way the
    /// official expression read leaves the shape unanswered.
    #[test]
    fn interpolation_faces_report() {
        let cases: [(&str, &str, usize); 8] = [
            // (label, scrutinee, expected count)
            ("statement", "'type: $t'", 1),
            ("expression", "'type: $t'", 1),
            ("inner conditional", "'type: ${1 == 1 ? '$t' : 'other'}'", 1),
            ("inner interpolation", "'type: ${'inner string $t'}'", 1),
            ("inner switch", "'type: ${switch (1) { _ => '$t', }}'", 1),
            ("inner toString", "'type: ${t.toString()}'", 1),
            ("silent string", "'type: $s'", 0),
            ("buried segment", "('$t')", 0),
        ];
        for (label, scrutinee, expected) in cases {
            let statement = format!(
                "void f(Type t, String s) {{\n  switch ({scrutinee}) {{\n    case 'type: int':\n      break;\n    default:\n      break;\n  }}\n}}\n"
            );
            assert_eq!(run(&statement).len(), expected, "{label}");
            let expression = format!(
                "void f(Type t, String s) {{\n  (switch ({scrutinee}) {{\n    'type: int' => null,\n    _ => null,\n  }});\n}}\n"
            );
            assert_eq!(run(&expression).len(), expected, "{label} (expression)");
        }
    }

    /// The interpolation report pins the whole scrutinee expression
    /// (`[!'' + '$t'!]` — the string-literal operands included).
    #[test]
    fn interpolation_report_pins_the_expression() {
        let source = "void f(Type t) {\n  switch ('' + '$t') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("switch_on_type"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(source, &f.tokens, "''", 0))
        );
        // `'' + '$t'` — 9 UTF-16 units, the `)` excluded.
        assert_eq!(lints[0].range.end - lints[0].range.start, 9);
    }
}
