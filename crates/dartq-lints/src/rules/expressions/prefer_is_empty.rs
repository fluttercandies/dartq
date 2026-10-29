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
    let (value, constant_on_right, length_access) = {
        // Right literal first (:64-71).
        if let Some(value) = super::prefer_contains::int_value(c, &right, 0) {
            (value, true, is_length_access(c, &left))
        } else if let Some(value) = super::prefer_contains::int_value(c, &left, 0) {
            (value, false, is_length_access(c, &right))
        } else {
            return;
        }
    };
    if !length_access {
        return;
    }
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
    // The container's own span starts at the flattened operand (the
    // collapse drops the member-access prefix), so the run head anchors
    // the start.
    let start = c.utf16_span(left[0]).start;
    let end = c.utf16_span(node).end;
    c.report_texts(start..end, problem, correction.unwrap_or_default(), &[]);
}

/// `_isLengthAccess` (:228-240): a `length` property access on a target
/// over the operand's piece run — the trailing access marker names
/// `length` and the receiver implements one of the core families
/// (`Iterable`/`Map`/`String`, :229-234).
fn is_length_access(c: &Check<'_>, pieces: &[u32]) -> bool {
    let Some(&tail) = pieces.last() else {
        return false;
    };
    if c.kind(tail) != NodeKind::DotAccess || c.lexeme(c.head_token(tail)) != "length" {
        return false;
    }
    let Some(name) = slice::access_name_index(c, pieces, tail) else {
        return false;
    };
    // A target is required (`type == null` bails, :233-234).
    if name == 0 {
        return false;
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
