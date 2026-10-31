use super::{Check, NodeKind, PayloadField, RuleSpec, slice};
use crate::rules::in_const_context;

const USE_IS_EMPTY_PROBLEM: &str =
    "Use 'isEmpty' instead of 'length' to test whether the collection is empty.";
const USE_IS_EMPTY_CORRECTION: &str = "Try rewriting the expression to use 'isEmpty'.";
const USE_IS_NOT_EMPTY_PROBLEM: &str =
    "Use 'isNotEmpty' instead of 'length' to test whether the collection is empty.";
const USE_IS_NOT_EMPTY_CORRECTION: &str = "Try rewriting the expression to use 'isNotEmpty'.";
const ALWAYS_FALSE_PROBLEM: &str =
    "The comparison is always 'false' because the length is always greater than or equal to 0.";
const ALWAYS_TRUE_PROBLEM: &str =
    "The comparison is always 'true' because the length is always greater than or equal to 0.";

/// Official: `pkg/linter/lib/src/rules/prefer_is_empty.dart:59-239` — a
/// `length` access compared against a small int literal reports per the
/// four-code matrix (`_check`, :111-226), mirrored verbatim including the
/// constant-on-the-left asymmetry.
///
/// The receiver type check (`implementsInterface(Iterable/Map/String)`,
/// :229-234) reads through the shared typed-receiver gate — collection/
/// string/map literals decide (a map literal reports like the official
/// `Map` face), a record literal is provably outside the family and
/// passes, a written bare-name receiver resolves (a user class
/// silences like the official non-`Iterable` type), a member receiver
/// reads the bridge static type when mounted, and an unresolvable
/// receiver bails like the official `type == null` face (the opaque
/// presumption is closed). `getIntValue` reads int literal pieces, so
/// constant expressions under-report. The const-context bails (:93-108)
/// port through [`crate::rules::in_const_context`] and the
/// const-constructor initializer face. Messages: `diagnostic.g.dart`
/// `preferIsEmptyUseIsEmpty`/`preferIsEmptyUseIsNotEmpty`/
/// `preferIsEmptyAlwaysFalse`/`preferIsEmptyAlwaysTrue`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("prefer_is_empty", &[NodeKind::BinaryExpression], check,);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The operand pieces tile in the enclosing owner, split at the
    // operator token (`slice::binary_operands`).
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let Some(op_token) = c.field_token(node, PayloadField::Operator) else {
        return;
    };
    let op = c.lexeme(op_token);
    // Right literal first (:64-71); the length access must sit on the
    // other operand (`_isLengthAccess`).
    let (value, constant_on_right, length_pieces) =
        if let Some(value) = super::prefer_contains::int_value(c, &right, 0) {
            (value, true, length_marker(c, &left).map(|_| &left))
        } else if let Some(value) = super::prefer_contains::int_value(c, &left, 0) {
            (value, false, length_marker(c, &right).map(|_| &right))
        } else {
            return;
        };
    let Some(_length_pieces) = length_pieces else {
        return;
    };
    // The const-context bails (:93-108): a const constructor initializer
    // or any constant context.
    if in_const_context(c) || in_const_constructor_initializer(c) {
        return;
    }
    let (problem, correction) = match (value, constant_on_right, op) {
        // `value == 0` (:113-146) — the official matrix branches only on
        // the operator here; the `value == 1` arms (:147-167) split on
        // the constant side.
        (0, _, "==" | "<=") | (1, true, "<") | (1, false, ">") => {
            (USE_IS_EMPTY_PROBLEM, Some(USE_IS_EMPTY_CORRECTION))
        }
        (0, _, ">" | "!=") | (1, true, ">=") | (1, false, "<=") => {
            (USE_IS_NOT_EMPTY_PROBLEM, Some(USE_IS_NOT_EMPTY_CORRECTION))
        }
        (0, _, "<") => (ALWAYS_FALSE_PROBLEM, None),
        (0, _, ">=") => (ALWAYS_TRUE_PROBLEM, None),
        // `value < 0` (:168-225).
        (v, true, "==" | "<=" | "<") if v < 0 => (ALWAYS_FALSE_PROBLEM, None),
        (v, true, "!=" | ">=" | ">") if v < 0 => (ALWAYS_TRUE_PROBLEM, None),
        (v, false, "==" | ">=" | ">") if v < 0 => (ALWAYS_FALSE_PROBLEM, None),
        (v, false, "!=" | "<=" | "<") if v < 0 => (ALWAYS_TRUE_PROBLEM, None),
        _ => {
            return;
        }
    };
    // The official span is the binary expression itself
    // (`reportAtNode(expression)`) — the whole comparison, operator and
    // both operands. The container's own covered span anchors at its
    // delta-tile operator, never the operand head, so the extent reads
    // the run bounds: the left operand's token-adjacent tail's earliest
    // head (a constructor-initializer's `x = ` target group tiles ahead
    // of the operand — the contiguous tail drops it) through the right
    // operand's last value row.
    let start = slice::operand_utf16_start(c, &left, op_token)
        .or_else(|| slice::run_utf16_start(c, &left))
        .unwrap_or_else(|| c.utf16_span(node).start);
    let end = match right.last() {
        Some(&tail) if c.kind(tail) == NodeKind::DotAccess => {
            crate::rules::utf16_token_span(c, c.head_token(tail)).end
        }
        Some(&tail) => c.utf16_span(tail).end,
        None => c.utf16_span(node).end,
    };
    c.report_texts(start..end, problem, correction.unwrap_or_default(), &[]);
}

/// The operand's `.length` access marker (`_isLengthAccess`,
/// :228-240): the `DotAccess` row naming `length` whose receiver
/// implements one of the core families (`Iterable`/`Map`/`String`,
/// :229-234). The operand reads through parentheses and `as` casts
/// (`_drillDownTo`, :188-205 — `ignoreParens: true, ignoreAs: true`):
/// the marker scan finds the access wherever the wrapper tiling
/// keeps it, so `([].length as int)` and `({1: 2}.length)` reach the
/// access face.
fn length_marker(c: &Check<'_>, pieces: &[u32]) -> Option<u32> {
    let tail = pieces.iter().rev().copied().find(|&row| {
        c.kind(row) == NodeKind::DotAccess && c.lexeme(c.head_token(row)) == "length"
    })?;
    // `_isLengthAccess` drills through parens/`as` to the access itself
    // (:188-205 — `ignoreParens: true, ignoreAs: true`), so the trailing
    // `as T`/`(…)` wrappers the production tiling keeps after the marker
    // stay the access; only a live operand row (`+`/binary tail
    // `[].length + 1`, a trailing expression piece) leaves `length`
    // mid-run and is not the access.
    let after = &pieces[pieces.iter().position(|&row| row == tail)? + 1..];
    if after.iter().any(|&row| {
        !slice::is_send_group_piece(c, row)
            && !matches!(
                c.kind(row),
                NodeKind::AsExpression
                    | NodeKind::ParenthesizedExpression
                    | NodeKind::Type
                    | NodeKind::TypeArguments
            )
    }) {
        return None;
    }
    let Some(name) = slice::access_name_index(c, pieces, tail) else {
        return None;
    };
    // A target is required (`type == null` bails, :233-234).
    if name == 0 {
        return None;
    }
    // The static type must implement one of the core families
    // (:229-234) — the shared typed-receiver gate decides (literals,
    // written bare names, wrapper-revealed names, member receivers
    // through the bridge when mounted); an unresolvable receiver
    // bails like the official `type == null` face.
    super::static_type_faces::receiver_family(
        c,
        c.semantics(),
        pieces,
        name,
        slice::ITERABLE | slice::MAP | slice::STRING,
    )
    .unwrap_or(false)
    .then_some(tail)
}

/// The constructor-initializer bail (:93-102): inside an initializer
/// whose constructor is const the comparison is exempt (a non-const
/// constructor's initializer proceeds).
fn in_const_constructor_initializer(c: &Check<'_>) -> bool {
    let mut path = c.path.iter().rev();
    while let Some(&ancestor) = path.next() {
        if matches!(
            c.kind(ancestor),
            NodeKind::FieldInitializer | NodeKind::Initializer
        ) {
            // The initializer's constructor is the enclosing declaration.
            for &outer in path {
                if c.kind(outer) == NodeKind::Constructor {
                    return c.lexeme(c.head_token(outer)) == "const";
                }
            }
            return false;
        }
    }
    false
}
