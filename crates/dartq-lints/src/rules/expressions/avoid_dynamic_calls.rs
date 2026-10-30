use dartq_semantic::model::Ty;

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, static_type_faces as faces,
    value_run_end,
};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_dynamic_calls.dart` — a method call
/// or property access whose target carries the `dynamic` static type
/// reports: compound-assignment reads (:55-67, `??=` exempt), binary
/// user-definable operators over a `dynamic` left operand (:70-88,
/// `==`/`!=` exempt, reported at the left operand), function-expression
/// invocations whose function is `dynamic` or bare `Function`
/// (:91-93), index expressions over a `dynamic` target (:94-96),
/// member invocations over a `dynamic` receiver (:101-132, the
/// `noSuchMethod`/`toString` exemptions, the explicit-cast bail, and
/// the `.call` special case — `a.call()` reports at `call` when the
/// receiver is a function type), `!` never counts (:136-139), prefix
/// and postfix operators (:140-155, the operand reports for plain
/// operators, the whole expression for compounds), property accesses
/// over a `dynamic` target (:156-164, the `hashCode`/`noSuchMethod`/
/// `runtimeType`/`toString` exemptions), and cascade sections over a
/// `dynamic` target (:166-181).
///
/// dartq faces over the production tiling: the semantic faces decide —
/// the member faces read the receiver row directly before the selector
/// (an identifier, a `!` row, a parenthesized expression — the
/// upstream `realTarget` extent), the bare-invocation face reads the
/// selector's flow type, and the function-expression-invocation face
/// reads the piece before the argument list. Index, cascade, and the
/// prefix/postfix faces read the operand run's static type.
/// Approximations (documented, under-report): multi-piece receivers
/// beyond the head row stay silent (the documented chain under-report),
/// and a compound operand's read-type fallback reports whole only for
/// `++`/`--` (the census-compound shape). Message: `diagnostic.g.dart`
/// `avoidDynamicCalls`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_dynamic_calls",
    &[
        NodeKind::Assignment,
        NodeKind::BinaryExpression,
        NodeKind::Send,
        NodeKind::DotAccess,
        NodeKind::UnaryPrefix,
        NodeKind::UnaryPostfix,
        NodeKind::Indexed,
        NodeKind::Cascade,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::Send => send_face(c, analysis),
        NodeKind::DotAccess => access_face(c, analysis),
        NodeKind::Assignment => assignment_face(c, analysis),
        NodeKind::BinaryExpression => binary_face(c, analysis),
        NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => unary_face(c, analysis),
        NodeKind::Indexed => index_face(c, analysis),
        NodeKind::Cascade => cascade_face(c, analysis),
        _ => {}
    }
}

/// The receiver expression row directly before the selector's dot: the
/// last non-marker piece — an identifier, a `!` row, or another
/// expression root (the upstream `realTarget`) — within a one-or-two
/// token gap of the selector (`.x`, `?.x`, `!.x`, `..x`).
fn chain_receiver(c: &Check<'_>, run: &[u32], selector: u32) -> Option<u32> {
    let run = slice::access_run(c, run);
    let sel_span = utf16_token_span(c, selector);
    // Drop the selector's own tail: the selector identifier rides its
    // send group, whose pieces all open at the selector token.
    let mut stripped = &run[..];
    while let Some(&last) = stripped.last() {
        if c.unit.ast.token_range(last).start >= selector {
            stripped = &stripped[..stripped.len() - 1];
            continue;
        }
        break;
    }
    let receiver = *stripped.last()?;
    if matches!(
        c.kind(receiver),
        NodeKind::Send
            | NodeKind::NoArguments
            | NodeKind::Arguments
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
    ) {
        return None;
    }
    let span = c.utf16_span(receiver);
    let gap = sel_span.start.checked_sub(span.end)?;
    (gap == 1 || gap == 2).then_some(receiver)
}

/// The invocation faces (:91-132): a member receiver's `dynamic` read
/// reports at the receiver (:116); otherwise a `call` selector over a
/// function-typed receiver reports at the selector (:119-129); a bare
/// invocation reports at the selector; a function-expression
/// invocation reports at its function (:91-93).
fn send_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some(&args) = pos.checked_sub(1).and_then(|i| kids.get(i)) else {
        return;
    };
    if !matches!(c.kind(args), NodeKind::Arguments | NodeKind::NoArguments) {
        return;
    }
    // The function-expression invocation face (:91-93): the piece
    // before the argument list is the invoked function itself when it
    // is not a bare selector (`(f)()`, `(g1 ?? g2)()`).
    if c.kind(args) == NodeKind::Arguments {
        let Some(mut fi) = pos.checked_sub(2) else {
            return;
        };
        while fi > 0
            && matches!(
                c.kind(kids[fi]),
                NodeKind::TypeArguments | NodeKind::NoTypeArguments
            )
        {
            fi -= 1;
        }
        let function = kids[fi];
        if c.kind(function) != NodeKind::Identifier {
            let ty = analysis.type_of(query_site(c, function));
            if ty.is_some_and(|ty| faces::is_dynamic_or_function(analysis, ty)) {
                c.report(c.utf16_span(function), &[]);
            }
            return;
        }
    }
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    let selector_row = kids[head];
    let member = slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some();
    // The every-object exemptions (:103-114) need a receiver.
    if member {
        let name = c.lexeme(c.head_token(selector_row));
        let arg_count =
            arguments.map_or(0, |args| slice::arguments(c, &c.collect_direct(args)).len());
        let no_named = arguments.is_some_and(|args| single_positional(c, args, arg_count == 1));
        if (name == "noSuchMethod" && arg_count == 1 && no_named)
            || (name == "toString" && arg_count == 0)
            // The property-read tile's own Send dispatch (no argument
            // list — a getter read, not an invocation): the
            // every-object property faces (:144-150).
            || (arguments.is_none()
                && matches!(
                    name,
                    "hashCode" | "runtimeType" | "toString" | "noSuchMethod"
                ))
        {
            return;
        }
    }
    let before = &kids[..head];
    let selector = c.head_token(selector_row);
    let receiver = member
        .then(|| chain_receiver(c, before, selector))
        .flatten();
    if let Some(receiver) = receiver
        && analysis.type_of(query_site(c, receiver)) == Some(Ty::Dynamic)
    {
        // The explicit-cast bail (:115).
        if faces::is_explicit_cast(c, before) {
            return;
        }
        c.report(c.utf16_span(receiver), &[]);
        return;
    }
    // The `.call` special case (:119-129): `a.call()` is `a()` — the
    // receiver's own function type decides, and the report lands on
    // the `call` selector.
    if member
        && c.lexeme(selector) == "call"
        && let Some(receiver) = receiver
        && !faces::is_explicit_cast(c, before)
        && faces::is_dynamic_or_function(
            analysis,
            analysis
                .type_of(query_site(c, receiver))
                .unwrap_or(Ty::Unknown),
        )
    {
        c.report(utf16_token_span(c, selector), &[]);
        return;
    }
    // The bare-invocation face (:174-181): the selector's flow type is
    // `dynamic` or bare `Function` (`function();`).
    if arguments.is_some()
        && !faces::is_explicit_cast(c, before)
        && faces::is_dynamic_or_function(
            analysis,
            analysis
                .type_of(query_site(c, selector_row))
                .unwrap_or(Ty::Unknown),
        )
    {
        c.report(utf16_token_span(c, selector), &[]);
    }
}

/// Whether the argument list is exactly one positional argument (the
/// `noSuchMethod` exemption's first-argument shape, :104-109).
fn single_positional(c: &Check<'_>, arguments: u32, expected: bool) -> bool {
    if !expected {
        return false;
    }
    let raw = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &raw);
    let [range] = &ranges[..] else {
        return false;
    };
    c.kind(raw[range.end - 1]) != NodeKind::NamedArgument
}

/// The property-access face (:156-164, the `PrefixedIdentifier` and
/// `PropertyAccess` dispatch): a `dynamic` target over a non-exempt
/// member reports at the target. The invocation's own trailing
/// whole-access marker dispatches here too — the piece directly before
/// it is the invocation send, whose face already owns the verdict.
fn access_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    if pos > 0 && c.kind(kids[pos - 1]) == NodeKind::Send {
        return;
    }
    let Some(selector) = c.unit.ast.token_range(kids[pos]).next_back() else {
        return;
    };
    if matches!(
        c.lexeme(selector),
        "hashCode" | "noSuchMethod" | "runtimeType" | "toString"
    ) {
        return;
    }
    let before = slice::statement_run(c, &kids[..pos]);
    if faces::is_explicit_cast(c, &before) {
        return;
    }
    let Some(receiver) = chain_receiver(c, &before, selector) else {
        return;
    };
    if analysis.type_of(query_site(c, receiver)) == Some(Ty::Dynamic) {
        c.report(c.utf16_span(receiver), &[]);
    }
}

/// The compound-assignment read face (:55-67): a compound operator
/// (`=`, `??=` exempt) over a `dynamic` read reports at the
/// assignment.
fn assignment_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some(op) = c.field(c.node(), PayloadField::Operator) else {
        return;
    };
    if matches!(c.lexeme(op), "=" | "??=") {
        return;
    }
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let run = slice::access_run(c, &left);
    let [only] = run[..] else {
        return;
    };
    if c.kind(only) != NodeKind::Identifier {
        return;
    }
    if analysis.type_of(query_site(c, only)) == Some(Ty::Dynamic) {
        // `reportAtNode(node)` spans the whole assignment; the
        // container row opens at the operator's tile anchor, so the
        // extent runs from the left operand run's first token to the
        // right run's last value row.
        let Some(start) = slice::operand_utf16_start(c, &left, op) else {
            return;
        };
        c.report(start..value_run_end(c, &right, c.node()), &[]);
    }
}

/// The binary face (:70-88): a user-definable operator (`==`/`!=`
/// exempt) over a `dynamic` left operand reports at the LEFT OPERAND
/// (`reportAtNode(node.leftOperand)`); the right operand is an
/// implicit cast, not a call.
fn binary_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some(op) = c.field(c.node(), PayloadField::Operator) else {
        return;
    };
    let operator = c.lexeme(op);
    if matches!(operator, "==" | "!=")
        || !matches!(
            operator,
            "+" | "-"
                | "*"
                | "/"
                | "%"
                | "~/"
                | "<<"
                | ">>"
                | ">>>"
                | "&"
                | "|"
                | "^"
                | "<"
                | "<="
                | ">"
                | ">="
        )
    {
        return;
    }
    let Some((left, _right)) = slice::binary_operands(c) else {
        return;
    };
    if faces::is_explicit_cast(c, &left) {
        return;
    }
    if faces::expr_type(c, analysis, &left) == Some(Ty::Dynamic) {
        let Some(start) = slice::operand_utf16_start(c, &left, op) else {
            return;
        };
        let end = value_run_end(c, &left, *left.last().unwrap_or(&c.node()));
        c.report(start..end, &[]);
    }
}

/// The prefix and postfix faces (:140-155): a `dynamic` operand
/// reports at the operand for plain operators (`-a`), at the whole
/// expression for compounds (`++a` — the compound read-type fallback,
/// :150-155); the postfix `!` never dispatches here (its own
/// token-leaf kind).
fn unary_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let operand = slice::statement_run(c, &kids[..pos]);
    if operand.is_empty() || faces::is_explicit_cast(c, &operand) {
        return;
    }
    if faces::expr_type(c, analysis, &operand) != Some(Ty::Dynamic) {
        return;
    }
    let compound = matches!(c.lexeme(c.head_token(c.node())), "++" | "--");
    let last = *operand.last().unwrap();
    let (start, end) = if compound {
        // `reportAtNode(root)` — the whole operator expression: a
        // prefix row (`++a`) opens at its operator, a postfix row
        // (`a++`) opens at the operator too with the operand tiled
        // before it — the extent runs from the operand run's first
        // row to the node's own span end.
        (c.utf16_span(operand[0]).start, c.utf16_span(c.node()).end)
    } else {
        // `reportAtNode(operand)`.
        let Some(start) = slice::operand_utf16_start(
            c,
            &operand,
            c.unit.ast.token_range(c.node()).next_back().unwrap_or(0),
        ) else {
            return;
        };
        (start, value_run_end(c, &operand, last))
    };
    c.report(start..end, &[]);
}

/// The index face (:94-96): a `dynamic` target reports at the target
/// (`reportAtNode(node.realTarget)`) — `a[1]` and the `a[1] = v`
/// assignment target alike.
fn index_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    // The target run sits before the index's own `[` (the first
    // depth-zero bracket from the row's start).
    let mut depth = 0i32;
    let mut bracket = None;
    for token in c.unit.ast.token_range(node) {
        match c.lexeme(token) {
            "[" => {
                if depth == 0 {
                    bracket = Some(token);
                    break;
                }
            }
            "(" | "{" => depth += 1,
            ")" | "}" => depth -= 1,
            _ => {}
        }
    }
    let Some(bracket) = bracket else {
        return;
    };
    let bracket_start = crate::rules::byte_span_of_token(c, bracket).start;
    let before: Vec<u32> = kids[..pos]
        .iter()
        .copied()
        .filter(|&kid| c.byte_span(kid).end <= bracket_start)
        .collect();
    let run = slice::statement_run(c, &before);
    if run.is_empty() || faces::is_explicit_cast(c, &run) {
        return;
    }
    if faces::expr_type(c, analysis, &run) == Some(Ty::Dynamic) {
        let start = c.utf16_span(run[0]).start;
        let end = value_run_end(c, &run, *run.last().unwrap());
        c.report(start..end, &[]);
    }
}

/// The cascade face (:166-181): a `dynamic` cascade target reports at
/// the target once (`reportAtNode(node.target)`) — `a..b()` and
/// `a..b` alike; the subsequent sections ride the same target.
fn cascade_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    // The target run sits before the first depth-zero `..`.
    let mut depth = 0i32;
    let mut dots = None;
    for token in c.unit.ast.token_range(node) {
        match c.lexeme(token) {
            ".." | "?.." => {
                if depth == 0 {
                    dots = Some(token);
                    break;
                }
            }
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            _ => {}
        }
    }
    let Some(dots) = dots else {
        return;
    };
    let dots_start = crate::rules::byte_span_of_token(c, dots).start;
    let before: Vec<u32> = kids[..pos]
        .iter()
        .copied()
        .filter(|&kid| c.byte_span(kid).end <= dots_start)
        .collect();
    let run = slice::statement_run(c, &before);
    if run.is_empty() || faces::is_explicit_cast(c, &run) {
        return;
    }
    if faces::expr_type(c, analysis, &run) == Some(Ty::Dynamic) {
        let start = c.utf16_span(run[0]).start;
        let end = value_run_end(c, &run, *run.last().unwrap());
        c.report(start..end, &[]);
    }
}
