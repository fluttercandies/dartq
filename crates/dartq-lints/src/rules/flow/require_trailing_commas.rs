use dartq_parser::AssertKind;

use super::{Check, Group, NodeKind, RuleSpec, last_child, narrow_u32};

/// Official: `pkg/linter/lib/src/rules/require_trailing_commas.dart` —
/// every multi-line argument, parameter, assert, and collection-literal
/// group ends with a trailing comma. Registration is version-gated: the
/// rule registers only BELOW the 3.7 tall-style milestone (:52-56,
/// `languageVersion >= language37` returns before any registration —
/// from 3.7 `dart format` enforces the commas itself), pinned in
/// [`crate::registry::VERSION_GATES`].
///
/// Ported faces (:58-121): argument lists, assert statements and
/// initializers (one [`NodeKind::Assert`] kind here — the upstream
/// unregistered expression position skips by its kind word), formal
/// parameter lists (the report pins the optional group's closer,
/// :100-115; extension-type primary constructors skip, :89-96), and
/// list/set-or-map literals. `_checkTrailingComma` (:124-146): a
/// trailing comma after the last element exits, so does a group whose
/// delimiters share a line; among the exceptions only a MULTILINE last
/// element qualifies (:137-139) — a block-bodied function literal, a
/// string literal, an anonymous function call whose argument parens
/// share a line, or a collection literal. The upstream `isNamed`
/// formal-parameter gate (:135) is subsumed: no exception class is a
/// formal parameter, so a named parameter never qualifies either way.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "require_trailing_commas",
    group: Group::Standalone,
    problem: "Missing a required trailing comma.",
    correction: "Try adding a trailing comma.",
    kinds: &[
        NodeKind::Arguments,
        NodeKind::Assert,
        NodeKind::FormalParameters,
        NodeKind::ListLiteral,
        NodeKind::SetOrMapLiteral,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let span = c.unit.ast.token_range(node);
    match c.kind(node) {
        NodeKind::Arguments => {
            // Named arguments contribute a `name:` handle after their
            // value row; the last argument's value is the last
            // non-handle child. An empty group skips (:61-63).
            let kids = c.collect_direct(node);
            let Some(last) = kids
                .into_iter()
                .rev()
                .find(|&kid| c.kind(kid) != NodeKind::NamedArgument)
            else {
                return;
            };
            check_group(c, span.start, span.end - 1, last, span.end - 1);
        }
        NodeKind::Assert => {
            // Upstream registers the statement and initializer visitors
            // only (:58-71); the expression position (a recovery shape)
            // is outside the lint. Payload word 0 is the kind, word 1
            // the open paren (the `Assert` payload).
            let kind = c.payload(node).first().copied();
            if kind != Some(AssertKind::Statement as u32)
                && kind != Some(AssertKind::Initializer as u32)
            {
                return;
            }
            let Some(last) = last_child(c, node) else {
                return;
            };
            let open = c
                .payload(node)
                .get(1)
                .copied()
                .filter(|&t| t != super::NONE_TOKEN)
                .unwrap_or(span.start);
            check_group(c, open, span.end - 1, last, span.end - 1);
        }
        NodeKind::FormalParameters => {
            // Extension-type primary constructors skip (:89-96).
            if c.ancestor(1)
                .is_some_and(|a| c.kind(a) == NodeKind::PrimaryConstructor)
                && c.ancestor(2)
                    .is_some_and(|a| c.kind(a) == NodeKind::ExtensionTypeDeclaration)
            {
                return;
            }
            let kids = c.collect_direct(node);
            let Some(&last) = kids.last() else {
                return; // `f()` — the empty list skips (:79).
            };
            if c.kind(last) == NodeKind::OptionalFormalParameters {
                // The last parameter sits in the optional group; the
                // report pins the group's own closer (:100-115,
                // `rightDelimiter ?? rightParenthesis`).
                let Some(param) = last_child(c, last) else {
                    return; // an empty group is an empty list.
                };
                let group_span = c.unit.ast.token_range(last);
                check_group(c, span.start, span.end - 1, param, group_span.end - 1);
            } else {
                check_group(c, span.start, span.end - 1, last, span.end - 1);
            }
        }
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => {
            let Some(last) = last_child(c, node).or_else(|| {
                // production: the container seals childless; its
                // elements leak into the owner run before it — the
                // last row before the handle is the last element's
                // tail row.
                let parent = c.unit.ast.parent(node)?;
                let kids = c.collect_direct(parent);
                let pos = kids.iter().position(|&kid| kid == node)?;
                kids.get(pos.checked_sub(1)?).copied()
            }) else {
                return;
            };
            check_group(c, span.start, span.end - 1, last, span.end - 1);
        }
        _ => {}
    }
}

/// The official `_checkTrailingComma` (:124-146) with the report token
/// resolved by the face (`errorToken ?? closingToken`).
fn check_group(c: &mut Check<'_>, open: u32, close: u32, last: u32, error: u32) {
    // A trailing comma already ends the group (:130).
    let next = c.unit.ast.token_range(last).end;
    if c.unit
        .tokens
        .get(next as usize)
        .is_some_and(|_| c.lexeme(next) == ",")
    {
        return;
    }
    // A group whose delimiters share a line needs no comma (:132-136).
    let (Some(open_bytes), Some(close_bytes)) = (token_bytes(c, open), token_bytes(c, close))
    else {
        return;
    };
    if line_of(c, open_bytes.start) == line_of(c, close_bytes.end) {
        return;
    }
    if exception(c, last) {
        return;
    }
    c.report(crate::rules::utf16_token_span(c, error), &[]);
}

/// The official `_shouldAllowTrailingCommaException` (:148-171): a
/// multiline last element that is a block-bodied function literal, a
/// string literal, an anonymous function call with one-line argument
/// parens, or a collection literal.
fn exception(c: &Check<'_>, last: u32) -> bool {
    // The invocation's send leaf tiles only its own tail (`}()`), so
    // its node span lies on the whole-expression multiline face — the
    // arm measures the expression's full extent itself.
    if c.kind(last) == NodeKind::Send {
        return anonymous_call_on_one_line(c, last);
    }
    // A last element that fits on one line never qualifies (:137-139).
    let last_span = c.byte_span(last);
    if line_of(c, narrow_u32(last_span.start)) == line_of(c, narrow_u32(last_span.end)) {
        return false;
    }
    match c.kind(last) {
        // Function literal with a block body (:141-143).
        NodeKind::FunctionExpression => c
            .collect_direct(last)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::BlockBody),
        // A (multiline) string literal (:145-147), or a set, map or
        // list literal (:159-160).
        NodeKind::StringLiteral | NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => true,
        _ => false,
    }
}

/// The anonymous-function-call exception (:149-157): a send whose
/// function is the preceding function expression, whose own extent
/// (function head through call tail) spans lines, and whose argument
/// parens share a line. The send leaf adopts the function
/// expression's tail token as its span head (the `}` before the call
/// parens), so the function pin compares tails, not heads.
fn anonymous_call_on_one_line(c: &Check<'_>, send: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == send) else {
        return false;
    };
    // Arena order: the function expression completes, its argument
    // list completes (the bare-identifier send group's type-argument
    // sentinel may ride between them), the send leaf seals last.
    let Some(&args) = pos.checked_sub(1).and_then(|i| kids.get(i)) else {
        return false;
    };
    if c.kind(args) != NodeKind::Arguments {
        return false;
    }
    if pos < 2 {
        return false;
    }
    let mut head_pos = pos - 2;
    while matches!(
        c.kind(kids[head_pos]),
        NodeKind::TypeArguments | NodeKind::NoTypeArguments
    ) {
        if head_pos == 0 {
            return false;
        }
        head_pos -= 1;
    }
    let head = kids[head_pos];
    if c.kind(head) != NodeKind::FunctionExpression {
        return false;
    }
    // The send leaf adopts the function expression's tail token as
    // its own span head (the `}` before the call parens) — the head
    // pin is the function's tail, not the send's own `(`.
    if crate::rules::tail_token(c, head) != c.head_token(send) {
        return false;
    }
    // The whole invocation — function head through call tail — must
    // span lines (:137-139), measured over the expression's full
    // extent since the send leaf carries only its own tail.
    let whole = c.byte_span(head).start..c.byte_span(send).end;
    if line_of(c, narrow_u32(whole.start)) == line_of(c, narrow_u32(whole.end)) {
        return false;
    }
    let args_span = c.unit.ast.token_range(args);
    let (Some(first_bytes), Some(last_bytes)) = (
        token_bytes(c, args_span.start),
        token_bytes(c, args_span.end - 1),
    ) else {
        return false;
    };
    line_of(c, first_bytes.start) == line_of(c, last_bytes.end)
}

fn token_bytes(c: &Check<'_>, token: u32) -> Option<core::ops::Range<u32>> {
    c.token(token)
        .map(|t| t.byte_offset..t.byte_offset + t.byte_len)
}

fn line_of(c: &Check<'_>, byte: u32) -> u32 {
    c.unit.lines.byte_to_line_col(byte).0
}
