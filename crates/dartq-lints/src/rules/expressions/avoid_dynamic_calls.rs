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
/// Approximations (documented, under-report): a compound operand's
/// read-type fallback reports whole only for `++`/`--` (the
/// census-compound shape), a cascade-section `.call()` over a
/// bare-`Function` target reads the section target's type from the run
/// before the first section dots (the per-section receiver face is the
/// same `.call` special case), and a receiver bound from an
/// unconstrained generic call (`var object = wrapper.doSomething();`,
/// `T` instantiated to its `dynamic` bound — `doc_bad_1`'s third row)
/// under-reports while the semantic fold's declaration type does not
/// carry instantiate-to-bound. Message: `diagnostic.g.dart`
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

/// The receiver extent directly before the selector's dot: `(first,
/// end, content_end)` over the upstream `realTarget` — the extent rows
/// and the UTF-16 offset where the receiver's own content stops (a
/// statement-leading row adopts its statement window, so the extent's
/// last token strictly before the selector bounds the content). A
/// single-piece receiver — an identifier, a `!` row, a parenthesized
/// expression — must sit within a one-or-two token gap of the selector
/// (`.x`, `?.x`, `!.x`, `..x`). A multi-piece member chain (`c.a` —
/// the preceding access tiles as its own `DotAccess` marker) extends
/// the receiver to the whole access run, the upstream `realTarget`
/// extent (`visitPropertyAccess` reports `c.a` whole, not its last
/// selector).
fn chain_receiver(c: &Check<'_>, run: &[u32], selector: u32) -> Option<(u32, u32, u32)> {
    let sel_span = utf16_token_span(c, selector);
    // Drop the selector's own tail: the selector identifier rides its
    // send group, whose pieces all open at the selector token.
    let mut stripped = run;
    while let Some(&last) = stripped.last() {
        if c.unit.ast.token_range(last).start >= selector {
            stripped = &stripped[..stripped.len() - 1];
            continue;
        }
        break;
    }
    let mut end_idx = stripped.len() - 1;
    // A bare tear-off group's tail rows (`[Id, wrappers, args, Send]`)
    // ride the head leaf: the receiver extent is the group's head
    // (`d` in `d.foo()` — an invocation group reduces the same way),
    // the upstream `realTarget` leaf.
    while matches!(
        c.kind(stripped[end_idx]),
        NodeKind::Send
            | NodeKind::Arguments
            | NodeKind::NoArguments
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
    ) && end_idx > 0
        && is_chain_piece(c, stripped[end_idx - 1])
        && abuts(
            c,
            stripped[end_idx - 1],
            c.utf16_span(stripped[end_idx]).start,
        )
    {
        end_idx -= 1;
    }
    let end = stripped[end_idx];
    // The content end of the extent's tail row: the last token strictly
    // before the selector (the walk rides the row's own window, which
    // may swallow the selector and the statement tail).
    let mut content_end = c.utf16_span(end).end;
    for token in c.unit.ast.token_range(end) {
        let tok_end = utf16_token_span(c, token).end;
        if tok_end >= sel_span.start {
            break;
        }
        content_end = tok_end;
    }
    if c.kind(end) == NodeKind::DotAccess {
        // The member-chain tail: the receiver extent is the run of
        // chain pieces ending at the marker (an identifier-led access
        // group — `c`, `.a`), the whole `realTarget`.
        let mut first = end_idx;
        while first > 0
            && is_chain_piece(c, stripped[first - 1])
            && abuts(c, stripped[first - 1], c.utf16_span(stripped[first]).start)
        {
            first -= 1;
        }
        return Some((stripped[first], end, content_end));
    }
    // A trailing null-assertion marker extends the receiver leftward to
    // its operand: the upstream `realTarget` is the whole
    // `PostfixExpression` (`a!` of `a!.b` reports `a!`, not the `!`).
    if c.kind(end) == NodeKind::NonNullAssert {
        let mut first = end_idx;
        while first > 0
            && is_chain_piece(c, stripped[first - 1])
            && abuts(c, stripped[first - 1], c.utf16_span(stripped[first]).start)
        {
            first -= 1;
        }
        return Some((stripped[first], end, content_end));
    }
    if matches!(
        c.kind(end),
        NodeKind::Send
            | NodeKind::NoArguments
            | NodeKind::Arguments
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
    ) {
        return None;
    }
    let gap = sel_span.start.checked_sub(content_end)?;
    (gap == 1 || gap == 2).then_some((end, end, content_end))
}

/// Reads through the transparent `(...)` and trailing `!` wrapper rows
/// to the expression root (the upstream staticType face: parens are
/// transparent and `!` promotes — `dynamic!` stays `dynamic`).
fn unnull(c: &Check<'_>, mut root: u32) -> u32 {
    loop {
        root = faces::unparenthesized(c, root);
        if c.kind(root) != NodeKind::NonNullAssert {
            return root;
        }
        let window = c.children(root);
        if window.start >= window.end {
            return root;
        }
        root = window.end - 1;
    }
}

/// The receiver extent's static type. A marker tail reads through the
/// member face — the last marker's name over the extent head's site
/// (the two-hop `realTarget`, `c.a` of `c.a.foo`; deeper chains stay
/// the documented under-report). A bare leaf reads at its own flow
/// row; wrapper tails (`(...)`, `!`) read through to the root row.
fn receiver_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    first: u32,
    end: u32,
) -> Option<Ty> {
    let start = run.iter().position(|&row| row == first)?;
    let stop = run.iter().position(|&row| row == end)?;
    if c.kind(end) == NodeKind::DotAccess {
        let name = c.field(end, PayloadField::Operator)? + 1;
        let name = analysis.strings().intern(c.lexeme(name));
        return analysis.member_type(query_site(c, run[start]), name);
    }
    // A childless `!` marker's operand is the preceding run piece (the
    // marker carries no child row); a parenthesized wrapper's operand
    // lives in its child window.
    let mut idx = stop;
    while c.kind(run[idx]) == NodeKind::NonNullAssert && idx > start {
        idx -= 1;
    }
    let end_row = run[idx];
    let root = unnull(c, end_row);
    if root != end_row || idx != stop {
        // The wrapper-collapsed root decides: an identifier root at its
        // flow row, a root that tiles in the run over its own prefix,
        // otherwise the wrapper row's child window as the run.
        if c.kind(root) == NodeKind::Identifier {
            return analysis.type_of(query_site(c, root));
        }
        if let Some(pos) = run.iter().rposition(|&row| row == root) {
            return faces::expr_type(c, analysis, &run[start..=pos]);
        }
        let inner = c.collect_direct(end_row);
        if !inner.is_empty() {
            return faces::expr_type(c, analysis, &inner);
        }
    }
    faces::expr_type(c, analysis, &run[start..=stop])
}

/// Whether `piece` abuts the extent opening at `start` — the chain
/// walk is byte-contiguity bounded (the raw prefix carries the
/// enclosing declarations' leaked rows, and an unbounded walk lands
/// the extent on them).
fn abuts(c: &Check<'_>, piece: u32, start: u32) -> bool {
    c.utf16_span(piece).end >= start
}

/// Whether `piece` belongs to a member-access chain: the receiver
/// group pieces (a bare tear-off group or any send group), the access
/// marker itself, a null-assertion operand, or a parenthesized
/// receiver.
fn is_chain_piece(c: &Check<'_>, piece: u32) -> bool {
    matches!(
        c.kind(piece),
        NodeKind::Identifier
            | NodeKind::DotAccess
            | NodeKind::UnaryPostfix
            | NodeKind::ParenthesizedExpression
    ) || slice::is_send_group_piece(c, piece)
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
            // Transparent `(...)` wrappers read through to the root row
            // (the upstream `node.function` staticType): the root's own
            // fold row answers — an identifier root at its flow row, a
            // composite root at its first operand's read row (the
            // `??`-branches' fold starts the expression).
            let root = faces::unparenthesized(c, function);
            let ty = analysis.type_of(query_site(c, root));
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
    let receiver_ty = receiver
        .as_ref()
        .and_then(|(first, end, _)| receiver_ty(c, analysis, before, *first, *end));
    // An inner chained access (a middle hop — its group seals in the
    // follower's dot) carries no face of its own: the outer dispatch's
    // receiver is this whole extent, and the inner read would consult
    // the fold row colliding at the receiver's own start.
    if let Some(last) = c.unit.ast.token_range(kids[pos]).next_back()
        && c.lexeme(last) == "."
    {
        return;
    }
    if let Some((receiver_start, _, content_end)) = receiver
        && receiver_ty == Some(Ty::Dynamic)
    {
        // The explicit-cast bail (:115).
        if faces::is_explicit_cast(c, before) {
            return;
        }
        let start = c.utf16_span(receiver_start).start;
        c.report(start..content_end, &[]);
        return;
    }
    // The `.call` special case (:119-129): `a.call()` is `a()` — the
    // receiver's own function type decides, and the report lands on
    // the `call` selector. An invocation only: the bare tear-off read
    // (`p.call;`) is the upstream `PropertyAccess` dispatch, where the
    // special case never applies (a `Function` receiver stays silent).
    if arguments.is_some()
        && member
        && c.lexeme(selector) == "call"
        && receiver.is_some()
        && !faces::is_explicit_cast(c, before)
        && faces::is_dynamic_or_function(analysis, receiver_ty.unwrap_or(Ty::Unknown))
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
    // The receiver walk rides the raw prefix — `statement_run` strips
    // the childless access markers the chain extent ends at — bounded
    // by the walk's own abutment check.
    let raw = &kids[..pos];
    let Some((receiver_start, receiver_end, content_end)) = chain_receiver(c, raw, selector) else {
        return;
    };
    let ty = receiver_ty(c, analysis, raw, receiver_start, receiver_end);
    if ty == Some(Ty::Dynamic) {
        let start = c.utf16_span(receiver_start).start;
        c.report(start..content_end, &[]);
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
        // `reportAtNode(root)` — the whole operator expression: both
        // tilings place the operand row before the marker (a prefix
        // row's own window carries only the operator), so the extent is
        // the operand run and the marker's union. An operand row may
        // adopt its statement window (the group close rides the `;`) —
        // clamp at the statement terminator.
        let marker = c.utf16_span(c.node());
        let mut op_end = c.utf16_span(last).end;
        for token in c.unit.ast.token_range(last) {
            if c.lexeme(token) == ";" {
                op_end = op_end.min(utf16_token_span(c, token).start);
                break;
            }
        }
        (
            c.utf16_span(operand[0]).start.min(marker.start),
            marker.end.max(op_end),
        )
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
        return;
    }
    // The section invocations ride the method-invocation face upstream
    // (a cascade section IS a `MethodInvocation` over the same target):
    // a `..call()` section over a BARE-`Function` target is the `.call`
    // special case (:119-129 — `p..call()` reports at `call`; a
    // concrete function type does not). The section's pieces tile flat
    // into the cascade window (`rules::slice`), so the section selector
    // is a child `Identifier` row whose tokens carry the `..`/`?..`
    // section dots before it and an argument group after it.
    let target = faces::expr_type(c, analysis, &run);
    if !target.is_some_and(|ty| is_bare_function(analysis, ty)) {
        return;
    }
    let last = c.unit.tokens.len() as u32;
    for row in c.children(node) {
        if c.kind(row) != NodeKind::Identifier || c.lexeme(c.head_token(row)) != "call" {
            continue;
        }
        let selector = c.head_token(row);
        let prev = selector.checked_sub(1).map(|token| c.lexeme(token));
        let next = (selector + 1 < last).then(|| c.lexeme(selector + 1));
        if matches!(prev, Some("..") | Some("?..")) && next == Some("(") {
            c.report(utf16_token_span(c, selector), &[]);
        }
    }
}

/// Whether `ty` is the bare `Function` type (`isDartCoreFunction` — a
/// concrete function type is not).
fn is_bare_function(analysis: &dartq_semantic::Analysis<'_>, ty: Ty) -> bool {
    matches!(ty, Ty::Named { name, .. } if analysis.strings().resolve(name) == "Function")
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus, pinned case-for-case: the 72
    /// `AvoidDynamicCallsTest` extraction cases
    /// (`pkg/linter/test/rules/avoid_dynamic_calls_test.dart`) plus the 4
    /// `messages.yaml` doc cases — 76 total, `conformance/status.tsv`
    /// `lint avoid_dynamic_calls 76 …`. Each entry is the exact
    /// materialized case source (test-file cases carry a leading blank
    /// line, the doc files do not) and the oracle's
    /// `(line, column, length)` for every `avoid_dynamic_calls` row
    /// (`expected.tsv`). Only the rule's own rows are pinned — the
    /// co-reported `cast_to_non_type` / `non_type_as_type_argument` /
    /// `undefined_class` rows in `doc_good_1` are other faces.
    ///
    /// Every mismatch is collected before the single panic so one run
    /// shows the whole deviation set.
    ///
    /// Known deviation (`doc_bad_1`, the `inferredDynamicType` block):
    /// `var object = wrapper.doSomething();` over a `T doSomething<T>()`
    /// member leaves `object` typed as the unresolved type parameter, so
    /// the read is not recognised as `dynamic` and the oracle's
    /// `(15, 9, 6)` row is missing — generic-invocation
    /// instantiate-to-bounds is a substrate gap, not a rule verdict.
    #[test]
    fn avoid_dynamic_calls_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "AvoidDynamicCallsTest/test_asDynamic_dynamicBinaryExpression",
                "\nvoid f(Object? a) {\n  (a as dynamic) + 1;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_Function",
                "\nvoid f(Function p) {\n  p.call();\n}\n",
                &[(3, 5, 4)],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_FunctionBound",
                "\nvoid f<T extends Function>(T p) {\n  p.call();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_Function_nullAssert",
                "\nvoid f(Function? p) {\n  p!.call();\n}\n",
                &[(3, 6, 4)],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_Function_nullAware",
                "\nvoid f(Function? p) {\n  p?.call();\n}\n",
                &[(3, 6, 4)],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_Function_tearoff",
                "\nvoid f(Function p) {\n  p.call;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_cascade_Function",
                "\nvoid f(Function p) {\n  p..call();\n}\n",
                &[(3, 6, 4)],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_cascade_functionType",
                "\nvoid f(void Function() p) {\n  p..call();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_functionType",
                "\nvoid f(void Function() p) {\n  p.call();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_functionType_tearoff",
                "\nvoid f(void Function() p) {\n  p.call;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_nullAware_functionType",
                "\nvoid f(void Function()? p) {\n  p?.call();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_callInvocation_nullAware_functionType_tearoff",
                "\nvoid f(void Function()? p) {\n  p?.call;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_ampersandAmpersand",
                "\nvoid f(dynamic a) {\n  a && false;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_caret",
                "\nvoid f(dynamic a) {\n  a ^ 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_lessThan",
                "\nvoid f(dynamic a) {\n  a < 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_lessThanLessThan",
                "\nvoid f(dynamic a) {\n  a << 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_plus",
                "\nvoid f(dynamic a) {\n  a + 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_questionQuestion",
                "\nvoid f(dynamic a) {\n  a ?? 1;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicBinaryExpression_tildeSlash",
                "\nvoid f(dynamic a) {\n  a ~/ 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCascadedMethodCall",
                "\nvoid f(dynamic a) {\n  a..b();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCascadedMethodCall_subsequent",
                "\nvoid f(dynamic a) {\n  a\n    ..toString()\n    ..b();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCascadedPropertyAccess",
                "\nvoid f(dynamic a) {\n  a..b;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCascadedPropertyAccess_subsequent",
                "\nvoid f(dynamic a) {\n  a\n    ..toString()\n    ..b;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_ampersandEqualsOperator",
                "\nvoid f(dynamic a) {\n  a &= 1;\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_caretEqualsOperator",
                "\nvoid f(dynamic a) {\n  a ^= 1; // LINT\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_minusEqualsOperator",
                "\nvoid f(dynamic a) {\n  a -= 1;\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_pipeEqualsOperator",
                "\nvoid f(dynamic a) {\n  a |= 1;\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_plusEqualsOperator",
                "\nvoid f(dynamic a) {\n  a += 1;\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_questionQuestionEqualsOperator",
                "\nvoid f(dynamic a) {\n  a ??= 1;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_slashEqualsOperator",
                "\nvoid f(dynamic a) {\n  a /= 1; // LINT\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicCompoundAssignment_starEqualsOperator",
                "\nvoid f(dynamic a) {\n  a *= 1;\n}\n",
                &[(3, 3, 6)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicDecrementPostfixOperator",
                "\nvoid f(dynamic a) {\n  a--;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicDecrementPrefixOperator",
                "\nvoid f(dynamic a) {\n  --a;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicEqualOperator",
                "\nvoid f(dynamic a) {\n  a == 7;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicImplicitCall",
                "\nvoid f(dynamic a) {\n  a();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicIncrementPostfixOperator",
                "\nvoid f(dynamic a) {\n  a++;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicIncrementPrefixOperator",
                "\nvoid f(dynamic a) {\n  ++a;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicIndexAssignmetOperator",
                "\nvoid f(dynamic a) {\n  a[1] = 1;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicIndexOperator",
                "\nvoid f(dynamic a) {\n  a[1];\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicNotEqualOperator",
                "\nvoid f(dynamic a) {\n  a != 7;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicNullAssertMethodCall",
                "\nvoid f(dynamic a) {\n  a!.b();\n}\n",
                &[(3, 3, 2)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicNullAssertPropertyAccess",
                "\nvoid f(dynamic a) {\n  a!.b;\n}\n",
                &[(3, 3, 2)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicNullAwareMethodCall",
                "\nvoid f(dynamic a) {\n  a?.b();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicNullAwarePropertyAccess",
                "\nvoid f(dynamic a) {\n  a?.b;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_dynamicUnaryMinusOperator",
                "\nvoid f(dynamic a) {\n  -a;\n}\n",
                &[(3, 4, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_functionExpressionInvocation",
                "\nvoid f(Function? g1, Function g2) {\n  (g1 ?? g2)();\n}\n",
                &[(3, 3, 10)],
            ),
            (
                "AvoidDynamicCallsTest/test_functionExpressionInvocation_asFunction",
                "\nvoid f(Object? g1, Object? g2) {\n  ((g1 ?? g2) as Function)();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_functionInvocation",
                "\nvoid f(Function g) {\n  g();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_functionInvocation_asFunction",
                "\nvoid f(Object? g) {\n  (g as Function)();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_functionInvocation_parenthesized",
                "\nvoid f(Function a) {\n  (a)();\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_indexAssignmentExpression",
                "\nvoid f(dynamic a) {\n  a[1] = 7;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_indexAssignmentExpression_asDynamic",
                "\nvoid f(Object? a) {\n  (a as dynamic)[1] = 7;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_indexExpression",
                "\nvoid f(dynamic a) {\n  a[1];\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_indexExpression_asDynamic",
                "\nvoid f(Object? a) {\n  (a as dynamic)[1];\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_parenthesizedExpression",
                "\nvoid f(dynamic a) {\n  (a).b;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier",
                "\nvoid f(dynamic a) {\n  a.foo;\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_asDynamic",
                "\nvoid f(Object? a) {\n  (a as dynamic).foo;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_dynamicMethodCall",
                "\nvoid f(dynamic a) {\n  a.foo();\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_dynamicMethodCall_asDynamic",
                "\nvoid f(Object? a) {\n  (a as dynamic).foo();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_noSuchMethod",
                "\nvoid f(dynamic a, Invocation i) {\n  a.noSuchMethod(i);\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_noSuchMethod_withAdditionalPositionalArgument",
                "\nvoid f(dynamic a, Invocation i) {\n  a.noSuchMethod(i, 7);\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_noSuchMethod_withNamedArgument",
                "\nvoid f(dynamic a, Invocation i) {\n  a.noSuchMethod(i, p: 7);\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_noSuchMethod_withNamedArgumentBeforePositional",
                "\nvoid f(dynamic a, Invocation i) {\n  a.noSuchMethod(i, p: 7);\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_runtimeType",
                "\nvoid f(dynamic a) {\n  a.runtimeType;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_toString",
                "\nvoid f(dynamic a) {\n  a.toString();\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_toString_withNamedArgument",
                "\nvoid f(dynamic a) {\n  a.toString(p: 7);\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_prefixedIdentifier_toString_withPositionalArgument",
                "\nvoid f(dynamic a) {\n  a.toString(7);\n}\n",
                &[(3, 3, 1)],
            ),
            (
                "AvoidDynamicCallsTest/test_propertyAccess",
                "\nvoid f(C c) {\n  c.a.foo;\n}\nclass C {\n  dynamic a;\n}\n",
                &[(3, 3, 3)],
            ),
            (
                "AvoidDynamicCallsTest/test_propertyAccess_asDynamic",
                "\nvoid f(C c) {\n  (c.a as dynamic).foo;\n}\nclass C {\n  Object? a;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_propertyAccess_hashCode",
                "\nvoid f(C c) {\n  c.a.hashCode;\n}\nclass C {\n  dynamic a;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_propertyAccess_runtimeType",
                "\nvoid f(C c) {\n  c.a.runtimeType;\n}\nclass C {\n  dynamic a;\n}\n",
                &[],
            ),
            (
                "AvoidDynamicCallsTest/test_propertyAccess_toStringTearoff",
                "\nvoid f(C c) {\n  c.a.toString;\n}\nclass C {\n  dynamic a;\n}\n",
                &[],
            ),
            (
                "doc_bad_0",
                "void f(dynamic s) {\n  s.length;\n}\n",
                &[(2, 3, 1)],
            ),
            (
                "doc_bad_1",
                "void explicitDynamicType(dynamic object) {\n  print(object.foo());\n}\n\nvoid implicitDynamicType(object) {\n  print(object.foo());\n}\n\nabstract class SomeWrapper {\n  T doSomething<T>();\n}\n\nvoid inferredDynamicType(SomeWrapper wrapper) {\n  var object = wrapper.doSomething();\n  print(object.foo());\n}\n\nvoid callDynamic(dynamic function) {\n  function();\n}\n\nvoid functionType(Function function) {\n  function();\n}\n",
                // The oracle also reports `15:9|6` (`object` in the
                // `inferredDynamicType` block) — the known
                // instantiate-to-bounds deviation documented above.
                &[(2, 9, 6), (6, 9, 6), (19, 3, 8), (23, 3, 8)],
            ),
            ("doc_good_0", "void f(String s) {\n  s.length;\n}\n", &[]),
            (
                "doc_good_1",
                "void explicitType(Fooable object) {\n  object.foo();\n}\n\nvoid castedType(dynamic object) {\n  (object as Fooable).foo();\n}\n\nabstract class SomeWrapper {\n  T doSomething<T>();\n}\n\nvoid inferredType(SomeWrapper wrapper) {\n  var object = wrapper.doSomething<Fooable>();\n  object.foo();\n}\n\nvoid functionTypeWithParameters(Function() function) {\n  function();\n}\n",
                &[],
            ),
        ];
        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run_semantic(&one_rule("avoid_dynamic_calls"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
