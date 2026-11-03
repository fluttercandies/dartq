use dartq_scanner::TokenKind;

use super::{Check, NodeKind, PayloadField, RuleSpec, same_file, slice};

/// Official: `pkg/linter/lib/src/rules/prefer_interpolation_to_compose_strings.dart:32-88`
/// — an adjacent `(literal, value)` pair in a `+` chain reports over the
/// pair span (`reportAtOffset(leftOperand.offset, rightOperand.end -
/// leftOperand.offset)`).
///
/// The left-operand String type check
/// (`leftOperand.staticType.isDartCoreString`, :85) reads through the
/// shared literal face ([`slice::literal_faces`]) — a string literal
/// operand decides, a provably non-string literal (a record) passes,
/// and an opaque operand answers at its same-file written type (a
/// non-`String` binding is the user type — #792's
/// `test_variableNotString`), an untyped local's string-literal
/// initializer answering `String` first (the #1135 declaration-side
/// gate), the remaining untyped face presuming the user type when the
/// same file declares any `operator +` and the string presumption
/// otherwise. The chain flattens once at the root (`chainedAdditions`,
/// :90-98; the official's per-node re-visit would re-report inner
/// pairs), and the operands anchor on the chain's `+` operators: each
/// operand spans from the previous operator's end to the next one, and
/// the tail operand closes at the first depth-0 statement/argument
/// boundary after it (the production tiling spills the enclosing
/// declaration's tail pieces beside the operand run, so the boundary
/// token is what the official's `rightOperand.end` corresponds to).
/// Raw simple strings and both-literal pairs pass verbatim (:70-75); a
/// `toString` invocation with arguments passes (:78-82). Message:
/// `diagnostic.g.dart` `preferInterpolationToComposeStrings`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_interpolation_to_compose_strings",
    &[NodeKind::BinaryExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // Only the chain root flattens (the official `chainedAdditions` walk
    // re-reports from every inner node; once per chain is the anchor).
    let root_op = c.field_token(node, PayloadField::Operator);
    if root_op.map(|op| c.lexeme(op)) != Some("+") {
        return;
    }
    let is_inner_plus = c.ancestor(1).is_some_and(|parent| {
        c.kind(parent) == NodeKind::BinaryExpression
            && c.field_token(parent, PayloadField::Operator)
                .is_none_or(|op| c.lexeme(op) == "+")
    });
    if is_inner_plus {
        return;
    }
    // The chain root's operand runs read through the adopted window
    // ([`slice::binary_operands`]); nested `+` binaries descend into
    // their own child roots. The operators anchor the official's
    // operands: each `+`-separated sub-expression spans from the
    // previous operator's end to the next one (or the expression tail).
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let Some(root_op) = c.field_token(node, PayloadField::Operator) else {
        return;
    };
    // An enclosing assignment's `=` divides its own lhs pieces from the
    // chain operands in the spilled run: the official's leftOperand is
    // the chain operand alone (`buffer = buffer + 'a'` reports at the
    // second `buffer`).
    let left = match assignment_divider(c, root_op) {
        Some(divider) => {
            let divider_end = c.utf16_end_of_token(divider).unwrap_or(0);
            left.into_iter()
                .filter(|&piece| c.utf16_span(piece).start > divider_end)
                .collect::<Vec<_>>()
        }
        None => left,
    };
    // The chain's operands: the spilled pieces. The tiling seals ONE
    // binary row per chain, so the nested operators exist only as
    // tokens — the depth-0 `+` tokens across the run's token span (the
    // glue rows straddle the operator byte, their spans swallow it, so
    // a per-gap scan would miss them). Bracketed and interpolation
    // regions are skipped through the table's begin→close pairing.
    let mut run = left.clone();
    run.extend_from_slice(&right);
    let span_first = run
        .iter()
        .map(|&piece| c.unit.ast.token_range(piece).start)
        .min()
        .unwrap_or(0);
    let span_last = run
        .iter()
        .map(|&piece| c.unit.ast.token_range(piece).end)
        .max()
        .unwrap_or(0) as usize;
    let mut ops: Vec<u32> = Vec::new();
    let mut token = span_first as usize;
    while token < span_last {
        let is_begin = c
            .token(token as u32)
            .is_some_and(|t| t.kind.is_begin_group());
        if is_begin {
            token = c.unit.tokens.matching_end(token).unwrap_or(span_last);
            continue;
        }
        if c.lexeme(token as u32) == "+" {
            ops.push(token as u32);
        }
        token += 1;
    }
    let mut pieces = run;
    // The chain's own nested binary rows ride the run as containers —
    // their operand pieces already sit beside them — so the operand
    // faces read the leaf rows only.
    pieces.retain(|&piece| c.kind(piece) != NodeKind::BinaryExpression);
    let starts: Vec<u32> = ops
        .iter()
        .map(|&op| c.utf16_end_of_token(op).unwrap_or(0))
        .collect();
    let n = starts.len() + 1;
    // Operand pieces group by their start against the operator
    // positions (no operand carries a top-level `+`).
    let mut groups: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &piece in &pieces {
        let start = c.utf16_span(piece).start;
        let index = starts.iter().filter(|&&s| s <= start).count();
        if let Some(group) = groups.get_mut(index) {
            group.push(piece);
        }
    }
    // The width-0 glue wrappers (`NoTypeArguments`/`NoArguments`) ride
    // beside their operand row and carry no value of their own; the
    // operand faces read the semantic rows only.
    for group in &mut groups {
        group.retain(|&piece| {
            !matches!(
                c.kind(piece),
                NodeKind::NoTypeArguments | NodeKind::NoArguments
            )
        });
    }
    // The tail operand's end reads off the expression tail: the first
    // depth-0 statement/argument boundary after the last operator (the
    // production tiling spills the enclosing declaration's tail pieces
    // beside the operand run). Mid-chain operands end at the token
    // right before their operator.
    let tail_end = match (ops.last().copied(), pieces.last().copied()) {
        (Some(op), Some(last)) => chain_operand_end(c, op, c.utf16_span(last).end),
        _ => 0,
    };
    let mut ends: Vec<u32> = Vec::with_capacity(n);
    for k in 0..n {
        ends.push(if k + 1 == n {
            tail_end
        } else {
            c.utf16_end_of_token(ops[k] - 1).unwrap_or(tail_end)
        });
    }
    let mut skip_next = false;
    for k in 0..n.saturating_sub(1) {
        let left = &groups[k];
        let right = &groups[k + 1];
        if skip_next {
            skip_next = false;
            continue;
        }
        let left_literal = is_string_literal(c, left);
        let right_literal = is_string_literal(c, right);
        // OK(#735): neither side is a literal.
        if !left_literal && !right_literal {
            continue;
        }
        // OK(#2490): a raw simple string on either side (an adjacent
        // chain is `StringLiteral` but not `SimpleStringLiteral`, so
        // the exemption does not apply to it).
        if (left_literal && left.len() == 1 && is_raw(c, left[0]))
            || (right_literal && right.len() == 1 && is_raw(c, right[0]))
        {
            continue;
        }
        // OK: both literals (prefer_adjacent_string_concatenation's face).
        if left_literal && right_literal {
            continue;
        }
        // OK(sdk#52610): a `toString` invocation with arguments.
        if is_to_string_with_arguments(c, left) || is_to_string_with_arguments(c, right) {
            continue;
        }
        // `leftOperand.staticType?.isDartCoreString` (:85): a string
        // literal decides, a provably non-string literal passes, the
        // opaque face answers at the same-file written type. The literal
        // row (not the run head) is what carries the type: an
        // interpolation operand's head is one of its content rows.
        let Some(&left_head) = left.first() else {
            continue;
        };
        let string_left =
            match slice::literal_faces(c, string_literal_row(c, left).unwrap_or(left_head)) {
                Some(faces) => faces & slice::STRING != 0,
                None => opaque_left_is_string(c, left),
            };
        if !string_left {
            continue;
        }
        // The official reports the pair span: the left operand's start
        // through the right operand's end (`reportAtOffset(
        // leftOperand.offset, rightOperand.end - leftOperand.offset)`).
        // The left operand's opening token answers: the literal row when
        // it spans the operand, the interpolation's leading segment when
        // the run opens inside one (that segment carries no row).
        let start = match (string_literal_row(c, left), interpolation_opener(c, left)) {
            (Some(row), _) => c.utf16_span(row).start,
            (None, Some(opener)) => crate::rules::utf16_token_span(c, opener).start,
            (None, None) => match left.first() {
                Some(&piece) => c.utf16_span(piece).start,
                None => continue,
            },
        };
        let end = ends[k + 1];
        if end <= start {
            continue;
        }
        c.report(start..end, &[]);
        // The official skips the pair consuming the reported right
        // operand (:86-88).
        skip_next = true;
    }
}

/// The enclosing assignment's divider: scanning back from `op` at
/// bracket depth 0, the first `=`-family operator before a statement
/// boundary (`None` when the chain owns its whole run).
fn assignment_divider(c: &Check<'_>, op: u32) -> Option<u32> {
    let mut depth = 0i32;
    let mut i = op;
    while i > 0 {
        i -= 1;
        if c.token(i).is_none() {
            return None;
        }
        match c.lexeme(i) {
            ")" | "]" | "}" => {
                if depth == 0 && c.lexeme(i) == "}" {
                    return None;
                }
                depth += 1;
            }
            "(" | "[" | "{" => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            ";" if depth == 0 => return None,
            lex if depth == 0 => {
                if lex == "=" || (lex.ends_with('=') && !matches!(lex, "==" | "!=" | "<=" | ">=")) {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The operand tail after `op`: the first depth-0 statement/argument
/// boundary (`;`, `,`, `)`, `]`, `}`) closes the expression — the end
/// is the token before it; an unterminated run falls back to `fallback`.
fn chain_operand_end(c: &Check<'_>, op: u32, fallback: u32) -> u32 {
    let mut depth: i32 = 0;
    let mut previous = op;
    let mut index = op + 1;
    while c.token(index).is_some() {
        match c.lexeme(index) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                if depth == 0 {
                    return c.utf16_end_of_token(previous).unwrap_or(fallback);
                }
                depth -= 1;
            }
            ";" | "," => {
                if depth == 0 {
                    return c.utf16_end_of_token(previous).unwrap_or(fallback);
                }
            }
            _ => {}
        }
        previous = index;
        index += 1;
    }
    fallback
}

/// The unit's UTF-16 extent: the union of its pieces' spans.
fn unit_span(c: &Check<'_>, unit: &[u32]) -> Option<core::ops::Range<u32>> {
    let mut span: Option<core::ops::Range<u32>> = None;
    for &piece in unit {
        let piece_span = c.utf16_span(piece);
        span = Some(match span {
            None => piece_span,
            Some(prev) => prev.start.min(piece_span.start)..prev.end.max(piece_span.end),
        });
    }
    span
}

/// The `StringLiteral` row covering the whole operand. A simple literal
/// is its own row; an interpolation literal (`'${…}'`) is a
/// `StringLiteral` too — upstream's `StringLiteral` supertype covers
/// `StringInterpolation` — but its content rows tile beside it, so the
/// kind check has to read the row that spans the operand rather than
/// every piece. A nested literal (an argument's `'x'`, a literal
/// element) covers only part of the operand and never answers here.
fn string_literal_row(c: &Check<'_>, unit: &[u32]) -> Option<u32> {
    let span = unit_span(c, unit)?;
    unit.iter()
        .copied()
        .find(|&piece| c.kind(piece) == NodeKind::StringLiteral && c.utf16_span(piece) == span)
}

/// The operand's interpolation opener (`'${`/`"${`): the literal's
/// leading string segment opens the operand and a `$`/`${` marker follows
/// it — upstream's `StringInterpolation` is a `StringLiteral` in the
/// official hierarchy, so this face answers the `leftOperand is
/// StringLiteral` test and carries the pair's opening offset. A call's
/// argument literal (`foo('a')`) opens at the call, never at a string
/// segment.
fn interpolation_opener(c: &Check<'_>, unit: &[u32]) -> Option<u32> {
    /// A literal segment token (`'…`, `"…`, `r'…`, `r"…`).
    fn is_segment(lexeme: &str) -> bool {
        lexeme.starts_with('\'')
            || lexeme.starts_with('"')
            || lexeme.starts_with("r'")
            || lexeme.starts_with("r\"")
    }
    // Read the token stream at the operand's own leftmost byte: a row's
    // head token is not a position anchor here (a literal's tail segment
    // row anchors back at the literal's opening quote).
    let leftmost = *unit.iter().min_by_key(|&&piece| c.byte_span(piece).start)?;
    let byte = c.byte_span(leftmost).start;
    let opener = u32::try_from(
        c.unit
            .tokens
            .as_slice()
            .partition_point(|token| (token.byte_offset as usize) < byte),
    )
    .ok()?;
    let follows = opener
        .checked_add(1)
        .filter(|&next| (next as usize) < c.unit.tokens.len());
    (is_segment(c.lexeme(opener))
        && follows.is_some_and(|next| matches!(c.lexeme(next), "{" | "${" | "$")))
    .then_some(opener)
}

fn is_string_literal(c: &Check<'_>, unit: &[u32]) -> bool {
    string_literal_row(c, unit).is_some() || interpolation_opener(c, unit).is_some()
}

/// `SimpleStringLiteral.isRaw`: the literal opens with `r`.
fn is_raw(c: &Check<'_>, literal: u32) -> bool {
    c.lexeme(c.head_token(literal)).starts_with('r')
}

/// `isToStringInvocationWithArguments`: a unit carrying a `toString`
/// call with a non-empty argument list — every send group answers (the
/// receiver access's own glue send is not the invocation).
fn is_to_string_with_arguments(c: &Check<'_>, unit: &[u32]) -> bool {
    unit.iter().enumerate().any(|(pos, &piece)| {
        c.kind(piece) == NodeKind::Send
            && slice::send_parts(c, unit, pos).is_some_and(|(head, arguments)| {
                c.kind(unit[head]) == NodeKind::Identifier
                    && c.lexeme(c.head_token(unit[head])) == "toString"
                    && arguments.is_some_and(|args| !c.collect_direct(args).is_empty())
            })
    })
}

/// The opaque left operand's `isDartCoreString` face (:74): the
/// official reads the operand's static type. The same-file written
/// face: a binding with a written non-`String` type is the user type
/// (silent — #792), a written `String` reports, an untyped local whose
/// initializer is a plain string-literal run is dart:core `String`
/// (adjudication #1135's declaration-side gate — the file's
/// `operator +` never fires on a `String` receiver), and the remaining
/// untyped or unbound face presumes the user type when the same file
/// declares any `operator +` (a user-type `+` makes the operand's
/// static type that user type, never dart:core `String`), the string
/// presumption otherwise.
fn opaque_left_is_string(c: &Check<'_>, unit: &[u32]) -> bool {
    if let Some(root) = unit
        .iter()
        .copied()
        .find(|&p| c.kind(p) == NodeKind::Identifier)
    {
        let name = c.lexeme(c.head_token(root));
        match same_file::written_type(c, root, name) {
            Some("String") => return true,
            Some(_) => return false,
            None => {
                if inferred_string_local(c, root) {
                    return true;
                }
            }
        }
    }
    !same_file_declares_plus(c)
}

/// Whether `row`'s name binds to an untyped local declared before the
/// use whose initializer is a plain string-literal run (a sole literal
/// or an adjacent chain — static type non-nullable `String`). The same
/// innermost-block walk the strings module's inferred-local face
/// runs; a written type never reaches here.
fn inferred_string_local(c: &Check<'_>, row: u32) -> bool {
    let name = c.lexeme(c.head_token(row));
    let use_start = c.byte_span(row).start;
    for i in (0..c.path.len().saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        if !matches!(c.kind(ancestor), NodeKind::Block | NodeKind::BlockBody) {
            continue;
        }
        for kid in c.collect_direct(ancestor) {
            if c.kind(kid) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c
                .collect_direct(kid)
                .into_iter()
                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            {
                let binds = crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|tok| c.lexeme(tok) == name)
                    && c.byte_span(var).start < use_start;
                if binds {
                    let Some(range) =
                        crate::rules::flow::exhaustive_cases::initializer_range(c, var)
                    else {
                        return false;
                    };
                    return range.start < range.end
                        && (range.start..range.end).all(|token| {
                            c.unit
                                .tokens
                                .get(token as usize)
                                .is_some_and(|t| t.kind == TokenKind::String)
                        });
                }
            }
        }
    }
    false
}

/// Whether the same file declares an `operator +` on a user type — a
/// class, mixin, enum, or extension type whose `+` is a member
/// declaration. Extensions are exempt: an extension `+` never changes
/// the receiver's static type (`'a' + x` stays `String` and reports).
fn same_file_declares_plus(c: &Check<'_>) -> bool {
    let Some(&root) = c.path.first() else {
        return false;
    };
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if matches!(
                c.kind(inner),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            ) && class_declares_plus(c, inner)
            {
                return true;
            }
        }
    }
    false
}

/// Whether the declaration's members carry an `operator +`: the
/// [`NodeKind::OperatorName`] marker tiles beside its `Method` in the
/// declaration body's completion window (the production bridge keeps
/// it out of the method's own children), so the window scan reads the
/// marker's `Name` word directly.
fn class_declares_plus(c: &Check<'_>, decl: u32) -> bool {
    for member in c.children(decl) {
        if c.kind(member) == NodeKind::OperatorName
            && c.lexeme(
                c.field(member, PayloadField::Name)
                    .unwrap_or(crate::rules::NONE_TOKEN),
            ) == "+"
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod p3769_pins {
    use crate::testsupport::{Fixture, one_rule};

    fn spans(source: &str) -> Vec<(u32, u32)> {
        let f = Fixture::parse(source);
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .into_iter()
            .map(|lint| (lint.range.start, lint.range.end))
            .collect()
    }

    /// `test_stringLiteral_variableString#0` — `3:11|9` — the pair span
    /// covers the left literal through the right operand (the
    /// production tiling's spilled declaration tail stays out).
    #[test]
    fn pair_span_covers_both_operands() {
        let source = "final a = 'foo';\nfinal b = 'bar' + a;\n";
        let line3 = source.find("'bar'").unwrap() as u32;
        assert_eq!(spans(source), vec![(line3, line3 + 9)]);
    }

    /// `test_stringLiteral_toStringInvocation#0` — `2:11|20` — the
    /// `7.toString()` operand runs to its closing paren.
    #[test]
    fn invocation_operand_extends_to_its_arguments() {
        let source = "final b = 'foo' + 7.toString();\n";
        let start = source.find("'foo'").unwrap() as u32;
        assert_eq!(spans(source), vec![(start, start + 20)]);
    }

    /// `test_stringLiteral_toStringInvocation_withArguments#0` — a
    /// `toString` invocation with arguments on either side passes
    /// (sdk#52610) — through the operator-anchored operand read.
    #[test]
    fn to_string_with_arguments_stays_silent() {
        assert!(spans("final b = 'foo' + a.toString(x: 0);\n").is_empty());
        assert!(spans("final b = a.toString(x: 0) + 'foo';\n").is_empty());
    }

    /// `doc_bad_0` — `2:10|7` — one lint at the first pair; the
    /// reported right operand skips the next pair (`i++`, :86-88).
    #[test]
    fn doc_pair_reports_once_and_skips_the_consumed_pair() {
        let source = "String f(String s) {\n  return '(' + s + ')';\n}\n";
        let start = source.find("'('").unwrap() as u32;
        assert_eq!(spans(source), vec![(start, start + 7)]);
    }

    /// `doc_bad_1` (the corpus wraps the doc example in a function) —
    /// each literal pair reports at its own pair span and the consumed
    /// pair skips: `6:10|23` + `6:36|22`.
    #[test]
    fn chained_pairs_report_at_their_own_spans() {
        let source = "class Person {\n  String name = '', city = '';\n}\nString f() {\n  var person = Person();\n  return 'Hello, ' + person.name + ' from ' + person.city + '.';\n}\n";
        let first = source.find("'Hello, '").unwrap() as u32;
        let second = source.find("' from '").unwrap() as u32;
        assert_eq!(
            spans(source),
            vec![(first, first + 23), (second, second + 22)]
        );
    }

    /// `test_stringLiteral_variableString_insideInterpolation#0` — the
    /// inner chain reports inside the interpolation and the outer pair
    /// reports over the whole interpolation literal.
    #[test]
    fn interpolation_content_and_outer_chain_both_report() {
        let source = "final a = 'foo';\nfinal b = '${'bar' + a}' + a;\n";
        let outer = source.find("'${'").unwrap() as u32;
        let inner = source.find("'bar'").unwrap() as u32;
        let mut got = spans(source);
        got.sort_unstable();
        assert_eq!(got, vec![(outer, outer + 18), (inner, inner + 9)]);
    }

    /// `test_variableString_variableString_stringLiteral#0` —
    /// `3:15|9` — a non-literal leading pair stays silent and the
    /// literal pair reports at its own span.
    #[test]
    fn non_literal_leading_pair_stays_silent() {
        let source = "final a = 'foo';\nfinal c = a + a + 'bar';\n";
        let start = source.find("'bar'").unwrap() as u32;
        assert_eq!(spans(source), vec![(start - 4, start + 5)]);
    }

    /// `test_variableNotString#0` — a user-type `+` keeps the lint
    /// silent (#792); both-literal and raw pairs pass verbatim.
    #[test]
    fn non_string_and_literal_faces_stay_silent() {
        assert!(spans("class A {\n  A operator +(String other) => this;\n}\nvoid f(A a) {\n  a + ' ';\n}\n").is_empty());
        assert!(spans("final a = 'foo' + 'bar';\n").is_empty());
        assert!(spans("final a = 'foo';\nfinal b = r'bar' + a;\n").is_empty());
        assert!(spans("final a = 'foo';\nfinal b = a + r'bar';\n").is_empty());
    }
}
