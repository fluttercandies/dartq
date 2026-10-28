use dartq_semantic::model::Ty;

use super::{Check, NodeKind, RuleSpec, query_site, slice, static_type_faces as faces, value_run_end};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_dynamic_calls.dart` — a method call
/// or property access whose target carries the `dynamic` static type
/// reports: compound-assignment reads (:55-67, `??=` exempt), binary
/// user-definable operators over a `dynamic` left operand (:70-88,
/// `==`/`!=` exempt), invocation functions typed `dynamic` or bare
/// `Function` (:91-93, :174-181), member invocations over a `dynamic`
/// receiver (:101-132, the `noSuchMethod`/`toString` exemptions and
/// the explicit-cast bail), `!` never counts (:136-139), prefix and
/// postfix operators over a `dynamic` operand (:134-155, :183-195),
/// and property accesses over a `dynamic` target (:144-164, the
/// `hashCode`/`noSuchMethod`/`runtimeType`/`toString` exemptions).
///
/// dartq faces over the production tiling: the semantic faces decide —
/// the read-type face at the operand's head identifier, the
/// variable-read face for bare invocations, and the getter/field face
/// for a single-name receiver (the receiver token must sit directly
/// before the selector's dot; a method's tear-off is a typed function
/// type and never answers). Approximations (documented,
/// under-report): multi-piece receivers (`getD().m()`, `(d).m()`,
/// chain receivers beyond the head) and index expressions stay
/// silent, the `.call` receiver special-casing (:119-129) stays
/// silent, declared-function and getter bare calls stay silent (no
/// read-row face), and a member invocation on an unresolvable
/// receiver keeps only the head-read verdict. Message:
/// `diagnostic.g.dart` `avoidDynamicCalls`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_dynamic_calls",
    &[
        NodeKind::Assignment,
        NodeKind::BinaryExpression,
        NodeKind::Send,
        NodeKind::DotAccess,
        NodeKind::UnaryPrefix,
        NodeKind::UnaryPostfix,
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
        _ => {}
    }
}

/// The invocation faces (:91-132): a member receiver's `dynamic` read
/// reports at the receiver (:116); otherwise the invocation function's
/// tear-off type decides (:130) — a bare invocation reports at the
/// selector, a member invocation over the receiver-through-selector
/// window.
fn send_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
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
    // The receiver face (:116): one exact name read as `dynamic`.
    let receiver = member
        .then(|| faces::single_receiver(c, &kids[..head], c.head_token(selector_row)))
        .flatten();
    if let Some(receiver) = receiver
        && analysis.type_of(query_site(c, receiver)) == Some(Ty::Dynamic)
    {
        let range = utf16_token_span(c, c.head_token(receiver));
        c.report(range, &[]);
        return;
    }
    // The tear-off face (:130): `a.m` / `m` typed `dynamic` or bare
    // `Function`. The tear-off type is the member's own type — a
    // getter's or field's declared type (a method's tear-off is a
    // typed function type, never `dynamic`/`Function`) — and the bare
    // name's flow read row (a variable read; a declared function's
    // name carries no read row and cannot answer).
    let tear_off = if member {
        receiver.and_then(|receiver| {
            let name = analysis
                .strings()
                .intern(c.lexeme(c.head_token(selector_row)));
            let member_face = analysis.member_of(query_site(c, receiver), name)?;
            match member_face.entry.kind {
                dartq_semantic::model::MemberKind::Getter
                | dartq_semantic::model::MemberKind::Field => {
                    analysis.member_type(query_site(c, receiver), name)
                }
                _ => None,
            }
        })
    } else if arguments.is_some() {
        // A bare invocation; the plain name reference group (no
        // arguments) is not a call site (:91-93 dispatches
        // FunctionExpressionInvocation).
        analysis.type_of(query_site(c, selector_row))
    } else {
        None
    };
    if tear_off.is_some_and(|ty| faces::is_dynamic_or_function(analysis, ty)) {
        let range = if member {
            let receiver_token = receiver.map_or(c.head_token(selector_row), |receiver| {
                c.head_token(receiver)
            });
            utf16_token_span(c, receiver_token).start
                ..utf16_token_span(c, c.head_token(selector_row)).end
        } else {
            utf16_token_span(c, c.head_token(selector_row))
        };
        c.report(range, &[]);
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

/// The property-access face (:144-164, the `PrefixedIdentifier` and
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
    let Some(receiver) = faces::single_receiver(c, &before, selector) else {
        return;
    };
    if analysis.type_of(query_site(c, receiver)) == Some(Ty::Dynamic) {
        let range = utf16_token_span(c, c.head_token(receiver));
        c.report(range, &[]);
    }
}

/// The compound-assignment read face (:55-67): a compound operator
/// (`=`, `??=` exempt) over a `dynamic` read reports at the
/// assignment.
fn assignment_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some(&op) = c.payload(c.node()).first() else {
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
/// exempt) over a `dynamic` left operand reports at the operator
/// expression; the right operand is an implicit cast, not a call.
fn binary_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some(&op) = c.payload(c.node()).first() else {
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
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    if faces::is_explicit_cast(c, &left) {
        return;
    }
    if faces::expr_type(c, analysis, &left) == Some(Ty::Dynamic) {
        // `reportAtNode(node)` spans the whole binary expression; the
        // container row opens at the operator's tile anchor, so the
        // extent runs from the left operand run's first token to the
        // right run's last value row.
        let Some(start) = slice::operand_utf16_start(c, &left, op) else {
            return;
        };
        c.report(start..value_run_end(c, &right, c.node()), &[]);
    }
}

/// The prefix and postfix faces (:134-155): a `dynamic` operand
/// reports at the operator expression (the postfix `!` never
/// dispatches here — it is its own token-leaf kind).
fn unary_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let operand = slice::statement_run(c, &kids[..pos]);
    if faces::is_explicit_cast(c, &operand) {
        return;
    }
    if faces::expr_type(c, analysis, &operand) == Some(Ty::Dynamic) {
        // `reportAtNode(node)` spans the whole operator expression; a
        // postfix row anchors at its operator token (`x++` opens at
        // `++`), so the extent runs from the operand run's first
        // token — bounded at the operator, since the owner-side run
        // can carry enclosing strays. A prefix row already opens at
        // its operator.
        let start = if c.kind(c.node()) == NodeKind::UnaryPostfix {
            let floor = c.unit.ast.token_range(c.node()).end - 1;
            let Some(start) = slice::operand_utf16_start(c, &operand, floor) else {
                return;
            };
            start
        } else {
            c.utf16_span(c.node()).start
        };
        c.report(start..c.utf16_span(c.node()).end, &[]);
    }
}
