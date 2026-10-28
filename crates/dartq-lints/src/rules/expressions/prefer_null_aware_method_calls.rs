use super::{Check, NodeKind, RuleSpec, query_site, slice};
use crate::rules::NONE_TOKEN;
use dartq_semantic::{Binding, BindingKind};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_null_aware_method_calls.dart`
/// — a `x != null` condition (a conditional expression whose else arm
/// is `null`, or an `if` without `else`) whose branch is one
/// expression statement invoking `x!()` reports at the invocation
/// (`_Visitor` :33-57 + `_check` :59-70): the `!` postfix must sit
/// directly on the condition's left operand (the `toSource()`
/// equality) and the invocation must be the bare postfix form — a
/// selector invocation (`x!.m()`) or a parenthesized operand never
/// matches, and the single-statement block unwrap (:51-55) is the
/// only branch shape past a bare expression statement.
///
/// dartq faces over the production tiling: the dispatch rows are
/// [`NodeKind::Conditional`] and [`NodeKind::IfStatement`]; the
/// condition is the `!=`-over-a-trailing-`null` binary — recovered from
/// the owner window for the conditional (the parser opens the
/// Conditional only at the `?`, so the condition rows sit immediately
/// before its slot; the shared
/// [`super::prefer_if_null_operators::null_check_parts`] face) and the
/// adopted [`NodeKind::ParenthesizedCondition`] child for the `if` —
/// with the operand read as the `toSource()` token window from the
/// binary's head to the operator. The branch is the then arm's flat
/// piece run: the whole-extent `Send` leaf anchors the invocation at
/// the run's tail, the argument list sits before it (the `!`-then-`(`
/// gap marker skipped), the [`NodeKind::NonNullAssert`] leaf before the
/// operand rows, and the operand rows keep only the real chain pieces
/// (`Identifier`/`DotAccess`) for the semantic faces. The `if` branch
/// peels the single-statement block — exactly one statement, the
/// trailing `ExpressionStatement` leaf — and reads the invocation rows
/// tiled before it.
///
/// The semantic face decides (the wave posture: no face, no report —
/// a semantic rule enabled without the mounted `Analysis` is a
/// driver-configuration error surfaced at the driver). The operand's
/// head identifier read answers twice. Resolution: the member face
/// (`member_of`) must resolve `p.m` on the receiver's same-library
/// container for the member shape, and the binding face (`binding_of`)
/// must resolve a bare `f` to a `Decl`/`Local`/`Member` target (a
/// prefixed or type-parameter target is not a plain callable) for the
/// bare shape. Nullability (family (a)): a definitely-non-null bare
/// read makes the null test provably dead and the suggestion vacuous.
/// Approximations (documented, under-report directions): an
/// unresolvable receiver (`dynamic`, unpublished member rows) and a
/// multi-dot chain (the face resolves through the head container's
/// member table) stay silent where the shape-only upstream reports —
/// the dominant typed-receiver corpus resolves — and a dead-check
/// bare shape stays silent for the same reason. Message:
/// `diagnostic.g.dart` `preferNullAwareMethodCalls`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_null_aware_method_calls",
    &[NodeKind::Conditional, NodeKind::IfStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    let Some((condition, run)) = (match c.kind(node) {
        NodeKind::Conditional => conditional_face(c, node),
        NodeKind::IfStatement => if_face(c, node),
        _ => None,
    }) else {
        return;
    };
    // The invocation face over the branch run: the whole-extent `Send`
    // leaf anchors the invocation at the run's tail, the argument list
    // sits before it, the `!`-then-`(` gap marker before that, and the
    // `NonNullAssert` leaf before the operand rows.
    let Some((&send, rest)) = run.split_last() else {
        return;
    };
    if c.kind(send) != NodeKind::Send {
        return;
    }
    let Some((&arguments, callee)) = rest.split_last() else {
        return;
    };
    if !matches!(
        c.kind(arguments),
        NodeKind::Arguments | NodeKind::NoArguments
    ) {
        return;
    }
    let callee = match callee.split_last() {
        Some((&marker, callee))
            if matches!(
                c.kind(marker),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            ) =>
        {
            callee
        }
        _ => callee,
    };
    let Some((&bang, operand_rows)) = callee.split_last() else {
        return;
    };
    if c.kind(bang) != NodeKind::NonNullAssert {
        return;
    }
    // The chain rows keep the access-shape faces: the bare-name group's
    // markers (`NoTypeArguments`/`NoArguments`/the group's own `Send`)
    // drop.
    let chain: Vec<u32> = operand_rows
        .iter()
        .copied()
        .filter(|&row| matches!(c.kind(row), NodeKind::Identifier | NodeKind::DotAccess))
        .collect();
    // The `toSource()` equality: the `!` operand's token window against
    // the condition operand's. The operand head is the first operand
    // row's head — `send`'s own range opens at `!` (the send was
    // emitted over `bang`'s span by the argument-list selector), not
    // at the operand.
    let operand_window = c.head_token(operand_rows[0])..c.head_token(bang);
    if !same_tokens(c, operand_window, condition) {
        return;
    }
    // The semantic faces answer at the operand's head identifier read.
    let Some(&head) = chain.first() else {
        return;
    };
    if c.kind(head) != NodeKind::Identifier {
        return;
    }
    let site = query_site(c, head);
    let fires = match chain.split_last() {
        // `p.m!()`: the trailing `DotAccess` marker carries the
        // selector right after its payload dot (`token_range`'s last
        // token is the swallowed `!` follower, not the selector); the
        // member face resolves the same-library member on the
        // receiver's container.
        Some((&marker, _)) if c.kind(marker) == NodeKind::DotAccess => {
            let Some(selector) = c.payload(marker).first().map(|&dot| dot + 1) else {
                return;
            };
            let name = analysis.strings().intern(c.lexeme(selector));
            analysis.member_of(site, name).is_some()
        }
        // `f!()`: the bare binding face — a plain `Decl`/`Local`/
        // `Member` target — plus the dead-check face (family (a)): a
        // definitely-non-null read cannot make the test succeed.
        _ => {
            if chain.len() != 1 {
                return;
            }
            if analysis.is_definitely_non_null(site) {
                return;
            }
            matches!(
                analysis.binding_of(site),
                Some(Binding {
                    kind: BindingKind::Decl | BindingKind::Local | BindingKind::Member,
                    ..
                })
            )
        }
    };
    if fires {
        // The official reports at the invocation (`x!()`) — the send
        // leaf's own span opens at `!` (the send was emitted over the
        // bang's span), so the range opens at the operand's head.
        let range = c.utf16_span(operand_rows[0]).start..c.utf16_span(send).end;
        c.report(range, &[]);
    }
}

/// The conditional face (`visitConditionalExpression` :33-41): the
/// then arm is the invocation run, the else arm is the `null` literal,
/// and the condition is the `!=`-over-trailing-`null` binary recovered
/// from the owner window.
fn conditional_face(c: &Check<'_>, node: u32) -> Option<(core::ops::Range<u32>, Vec<u32>)> {
    let (op, binary, operand, _condition, (then_run, else_run)) =
        super::prefer_if_null_operators::null_check_parts(c, node)?;
    if op != "!=" {
        return None;
    }
    // `node.elseExpression is NullLiteral` (:38) — the exact single
    // literal row.
    if !super::prefer_if_null_operators::is_null_literal_run(c, &else_run) {
        return None;
    }
    // `condition.rightOperand is NullLiteral` (:37): the binary's
    // adopted right tail.
    let rows = c.collect_direct(binary);
    let (&rhs, _) = rows.split_last()?;
    if c.kind(rhs) != NodeKind::LiteralNull {
        return None;
    }
    let op_token = c.payload(binary).first().copied()?;
    // The `leftOperand.toSource()` window opens at the condition's
    // first token — the operand run's earliest head, never the
    // binary's covered start (a member operand's covered span opens at
    // the selector: `p.m != null` opens at `m`).
    let head = slice::run_start_token(c, &operand)?;
    Some((head..op_token, then_run))
}

/// The `if` face (`visitIfStatement` :43-57): no `else` clause; the
/// condition is the adopted `ParenthesizedCondition` child, and the
/// branch is one expression statement — a bare statement or the
/// single-statement block unwrap — whose invocation rows tile before
/// the statement leaf.
fn if_face(c: &Check<'_>, node: u32) -> Option<(core::ops::Range<u32>, Vec<u32>)> {
    // `elseKeyword == null` (:49).
    if c.payload(node).first().copied() != Some(NONE_TOKEN) {
        return None;
    }
    let kids = c.collect_direct(node);
    let condition_pos = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)?;
    // The adopted condition: the window's tail row, peeling paren
    // containers down to the null-check binary — the production window
    // carries the operand pieces beside the binary, and a wrapped
    // binary sits behind its `ParenthesizedExpression` row.
    let mut condition = *c.collect_direct(kids[condition_pos]).last()?;
    while c.kind(condition) == NodeKind::ParenthesizedExpression {
        condition = *c.collect_direct(condition).last()?;
    }
    let branch_run = then_branch_run(c, &kids[condition_pos + 1..])?;
    let window = condition_window(c, condition)?;
    Some((window, branch_run))
}

/// The then branch's expression-statement run: the bare statement's
/// rows between the condition and its trailing `ExpressionStatement`
/// leaf, or the single-statement block's rows before its only
/// statement leaf (:51-55).
pub(super) fn then_branch_run(c: &Check<'_>, rest: &[u32]) -> Option<Vec<u32>> {
    let (&last, rows) = rest.split_last()?;
    if c.kind(last) == NodeKind::ExpressionStatement {
        // The production leaf seals empty and its pieces tile flat
        // beside it; a hand-built tree nests the expression inside —
        // union both faces.
        let mut run = rows.to_vec();
        run.extend(c.collect_direct(last));
        return Some(run);
    }
    if c.kind(last) == NodeKind::Block {
        let statements = c.collect_direct(last);
        let (&leaf, inner) = statements.split_last()?;
        // Exactly one statement — the trailing expression-statement
        // leaf; any other statement row is a second statement (or a
        // non-expression one) and disqualifies the shape.
        if c.kind(leaf) == NodeKind::ExpressionStatement
            && !inner
                .iter()
                .any(|&row| crate::rules::is_statement_row(c, row))
        {
            return Some(inner.to_vec());
        }
    }
    None
}

/// The condition operand's token window — the `!=`-over-trailing-`null`
/// binary's left operand (`condition.leftOperand.toSource()`): the
/// operand runs from the binary's head to the operator.
fn condition_window(c: &Check<'_>, condition: u32) -> Option<core::ops::Range<u32>> {
    if c.kind(condition) != NodeKind::BinaryExpression {
        return None;
    }
    let op = c.payload_word(condition, 0)?;
    if c.lexeme(op) != "!=" {
        return None;
    }
    let rows = c.collect_direct(condition);
    let (&rhs, _) = rows.split_last()?;
    if c.kind(rhs) != NodeKind::LiteralNull {
        return None;
    }
    // The window opens at the left operand's first token — the
    // operand run's earliest head bounded at the operator (the
    // owner-side rows can carry enclosing strays), never the
    // binary's covered start (a member operand opens at the
    // selector: `p.m != null` opens at `m`).
    let (left, _) = slice::binary_operands_at(c, condition)?;
    Some(slice::operand_start_token(c, &left, op)?..op)
}

/// The `toSource()` equality at the token-lexeme level: equal-length
/// windows carrying the same lexeme sequence are the same source.
fn same_tokens(c: &Check<'_>, a: core::ops::Range<u32>, b: core::ops::Range<u32>) -> bool {
    a.len() == b.len() && a.zip(b).all(|(x, y)| c.lexeme(x) == c.lexeme(y))
}
