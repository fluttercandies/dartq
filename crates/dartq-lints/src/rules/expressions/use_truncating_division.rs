use dartq_semantic::model::Ty;

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, static_type_faces as faces,
};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/use_truncating_division.dart` — a `/`
/// division whose operands are both `int` (:40-44, the `/` element
/// from dart:core :48-52) with a parenthesized parent (:54-55) whose
/// outermost parentheses are wrapped by a `toInt()` invocation with no
/// arguments (:57-67) reports: `(a / b).toInt()` should be `a ~/ b`.
///
/// dartq faces over the production tiling: the dispatched
/// `BinaryExpression` is the division (payload operator `/`); the
/// operand faces are the single stripped identifier reads or the
/// piece-run types; the wrapper face is the direct
/// [`NodeKind::ParenthesizedExpression`] parent, climbed to its
/// outermost wrapper, followed by the `[toInt, (), Send]` invocation
/// tail. The report sits on the invocation send (the official
/// grand-parent node, :66). Approximations (documented,
/// under-report): a non-`int`, unresolved, or compound operand, a
/// user-declared `/` (the dart:core element face), and a
/// non-parenthesized division stay silent.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "use_truncating_division",
    &[NodeKind::BinaryExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        return;
    };
    // The division operator (:37).
    let Some(op) = c.field(c.node(), PayloadField::Operator) else {
        return;
    };
    if c.lexeme(op) != "/" {
        return;
    }
    // Both operands are `int` (:40-44).
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    if !operand_is_int(c, analysis, &left) || !operand_is_int(c, analysis, &right) {
        return;
    }
    // The parenthesized wrapper (:54-55), climbed to the outermost.
    let Some(mut outer) = c.ancestor(1) else {
        return;
    };
    if c.kind(outer) != NodeKind::ParenthesizedExpression {
        return;
    }
    while let Some(up) = c.unit.ast.parent(outer) {
        if c.kind(up) != NodeKind::ParenthesizedExpression {
            break;
        }
        outer = up;
    }
    // The `toInt()` invocation tail (:57-67): the outermost wrapper's
    // owner window carries the selector, the empty argument list, and
    // the invocation send.
    let Some(owner) = c.unit.ast.parent(outer) else {
        return;
    };
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == outer) else {
        return;
    };
    // The member-send tail tiles [selector, type-argument wrapper,
    // arguments, send, access marker] — peel the wrapper and the
    // marker the same way [`slice::send_parts`] reads a callee run.
    let [selector, tail @ ..] = &kids[pos + 1..] else {
        return;
    };
    if c.kind(*selector) != NodeKind::Identifier || c.lexeme(c.head_token(*selector)) != "toInt" {
        return;
    }
    let tail = match tail {
        [head @ .., last] if c.kind(*last) == NodeKind::DotAccess => head,
        _ => tail,
    };
    let tail = match tail {
        [wrapper, rest @ ..]
            if matches!(
                c.kind(*wrapper),
                NodeKind::NoTypeArguments | NodeKind::TypeArguments
            ) =>
        {
            rest
        }
        _ => tail,
    };
    if tail.len() < 2 {
        return;
    }
    let [arguments, send] = std::array::from_fn::<_, 2, _>(|i| tail[i]);
    let empty = match c.kind(arguments) {
        NodeKind::Arguments => slice::arguments(c, &c.collect_direct(arguments)).is_empty(),
        NodeKind::NoArguments => true,
        _ => false,
    };
    if !empty || c.kind(send) != NodeKind::Send {
        return;
    }
    c.report(c.utf16_span(send), &[]);
}

/// The operand's `int` face: a single stripped identifier read, or the
/// piece-run type face; anything else under-reports.
fn operand_is_int(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) -> bool {
    let is_int = |ty: Option<Ty>| {
        matches!(ty, Some(Ty::Named { name, nullable: false, .. })
            if analysis.strings().resolve(name) == "int")
    };
    let stripped = slice::access_run(c, run);
    if let [only] = stripped[..]
        && c.kind(only) == NodeKind::Identifier
    {
        return is_int(analysis.type_of(query_site(c, only)));
    }
    is_int(faces::expr_type(c, analysis, run))
}
