use super::{Check, NodeKind, PayloadField, RuleSpec, same_file, slice};

const USE_CONTAINS_PROBLEM: &str = "Unnecessary use of 'indexOf' to test for containment.";
const USE_CONTAINS_CORRECTION: &str = "Try using 'contains'.";
const ALWAYS_FALSE_PROBLEM: &str =
    "Always 'false' because 'indexOf' is always greater than or equal to -1.";
const ALWAYS_TRUE_PROBLEM: &str =
    "Always 'true' because 'indexOf' is always greater than or equal to -1.";

/// Official: `pkg/linter/lib/src/rules/prefer_contains.dart:46-159` — an
/// `indexOf` compared against a small int literal with the containment
/// operators reports at the comparison. Three codes split by the matrix
/// (`_checkConstant`, :73-124): the rewrite (`==`/`!=`/`<=`/`>` at -1,
/// `>=`/`<` at 0), the always-false and always-true judgments.
///
/// The receiver type check (:143-149) reads through the shared
/// resolution-backed face ([`slice::resolved_receiver_family`]) —
/// string and collection literals decide, a bare-identifier receiver
/// resolves its written type and must land in the `Iterable`/`String`
/// family — and an opaque receiver reads the bridge static type when
/// mounted (a member receiver's declared type: `this.list.indexOf`,
/// `a.b.indexOf`); anything unresolvable stays silent like the
/// official `parentType == null` bail. `getIntValue` carries the
/// context (:66 — the official call passes it), so a same-file `const`
/// identifier answers through its constant value
/// ([`const_int_value`]); constants outside the file under-report.
/// Message: `diagnostic.g.dart`
/// `preferContainsUseContains`/`preferContainsAlwaysFalse`/`preferContainsAlwaysTrue`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("prefer_contains", &[NodeKind::BinaryExpression], check,);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op_token) = c.field_token(node, PayloadField::Operator) else {
        return;
    };
    let op = c.lexeme(op_token);
    if !matches!(op, "==" | "!=" | ">" | ">=" | "<" | "<=") {
        return;
    }
    // The comparison's operand pieces tile in the enclosing owner (the
    // binary adopts each operand's outermost subtree only), split at the
    // operator token.
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let (literal, effective_op) = {
        // Right literal first (:66-71). The `getIntValue` read carries
        // the context (a const identifier answers).
        if let Some(value) = const_int_value(c, &right, 0)
            && value <= 0
            && is_unassigned_index_of(c, node, &left)
        {
            (value, op)
        } else if let Some(value) = const_int_value(c, &left, 0)
            && value <= 0
            && is_unassigned_index_of(c, node, &right)
        {
            (value, invert(op))
        } else {
            return;
        }
    };
    let (problem, correction) = match literal {
        -1 => match effective_op {
            "==" | "!=" | "<=" | ">" => (USE_CONTAINS_PROBLEM, Some(USE_CONTAINS_CORRECTION)),
            "<" => (ALWAYS_FALSE_PROBLEM, None),
            ">=" => (ALWAYS_TRUE_PROBLEM, None),
            _ => return,
        },
        0 => match effective_op {
            ">=" | "<" => (USE_CONTAINS_PROBLEM, Some(USE_CONTAINS_CORRECTION)),
            _ => return,
        },
        v if v < -1 => match effective_op {
            "==" | "<=" | "<" => (ALWAYS_FALSE_PROBLEM, None),
            "!=" | ">=" | ">" => (ALWAYS_TRUE_PROBLEM, None),
            _ => return,
        },
        _ => return,
    };
    // The official reports at the binary expression; the container's own
    // span starts at the flattened operand (the collapse drops the
    // member-access prefix), so the run head anchors the start.
    let start = c.utf16_span(left[0]).start;
    let end = c.utf16_span(node).end;
    c.report_texts(start..end, problem, correction.unwrap_or_default(), &[]);
}

/// `TokenType.inverted` (the mirror-operand comparison, :67).
fn invert(op: &str) -> &'static str {
    match op {
        "==" => "!=",
        "!=" => "==",
        "<" => ">=",
        ">=" => "<",
        ">" => "<=",
        _ => ">",
    }
}

/// `getIntValue` over a piece run: decimal/hex int literals with
/// separators, optionally negated by a `-` prefix piece (the production
/// tiling of `-1`). Leading width-0 wrappers skip — the type-argument
/// rows (the left operand's generic-invocation tail) and the
/// `NoArguments` row (the operator's rhs anchor), all anchored at the
/// token after them.
pub(super) fn int_value(c: &Check<'_>, pieces: &[u32], start: usize) -> Option<i64> {
    let mut i = start;
    while matches!(
        pieces.get(i),
        Some(&piece)
            if matches!(
                c.kind(piece),
                NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::NoArguments
            )
    ) {
        i += 1;
    }
    // The prefix operator's handler fires after its operand parses, so
    // `-1` tiles [LiteralInt, UnaryPrefix] — the prefix leaf follows the
    // literal it negates.
    let (negative, literal) = match (pieces.get(i), pieces.get(i + 1)) {
        (Some(&p), Some(&pre))
            if c.kind(p) == NodeKind::LiteralInt
                && c.kind(pre) == NodeKind::UnaryPrefix
                && c.lexeme(c.head_token(pre)) == "-" =>
        {
            (true, p)
        }
        (Some(&p), _) if c.kind(p) == NodeKind::LiteralInt => (false, p),
        _ => return None,
    };
    let lexeme = c.lexeme(c.head_token(literal));
    let digits = lexeme.trim_start_matches("0x").trim_start_matches("0X");
    let digits: String = digits.chars().filter(|&ch| ch != '_').collect();
    i64::from_str_radix(
        &digits,
        if lexeme.starts_with("0x") || lexeme.starts_with("0X") {
            16
        } else {
            10
        },
    )
    .ok()
    .map(|v| if negative { -v } else { v })
}

/// `getIntValue` (upstream Dart SDK source, pkg/linter/lib/src/ast.dart
/// :35-42, :274-288) as `prefer_contains` calls it — with a context, so
/// a `SimpleIdentifier` operand also answers through its constant
/// value. The literal face is [`int_value`]; the identifier face
/// resolves a same-file `const` local or top-level constant's
/// initializer (identifier chains recurse with a cycle guard). A
/// static-const class field read bare, and constants living in other
/// files, under-report — the constant walk is same-file.
fn const_int_value(c: &Check<'_>, pieces: &[u32], start: usize) -> Option<i64> {
    if let Some(value) = int_value(c, pieces, start) {
        return Some(value);
    }
    // The SimpleIdentifier operand: wrappers skip first, then a lone
    // identifier that spans the whole operand (`-k` carries the negating
    // prefix leaf after it; `p.k` carries a dotted face and never
    // resolves to a bare name).
    let mut i = start;
    while matches!(
        pieces.get(i),
        Some(&piece)
            if matches!(
                c.kind(piece),
                NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::NoArguments
            )
    ) {
        i += 1;
    }
    let row = *pieces.get(i)?;
    if c.kind(row) != NodeKind::Identifier {
        return None;
    }
    let mut negated = false;
    let mut end_row = row;
    if let Some(&post) = pieces.get(i + 1)
        && c.kind(post) == NodeKind::UnaryPrefix
        && c.lexeme(c.head_token(post)) == "-"
    {
        negated = true;
        end_row = post;
    }
    // The identifier (or its `-k` wrapper) must end the operand — the
    // bare-name group's wrapper pieces ride after it; strip them.
    let Some(&tail) = slice::access_run(c, pieces).last() else {
        return None;
    };
    if c.utf16_span(end_row).end != c.utf16_span(tail).end {
        return None;
    }
    let name = c.lexeme(c.head_token(row));
    let value = const_named_value(c, row, name, &mut Vec::new())?;
    Some(if negated { -value } else { value })
}

/// The int value of the same-file `const` binding `name` at use site
/// `use_row`: a const local ([`same_file::local_const_initializer`]) or
/// a top-level `const` variable, its initializer tokens read as an int
/// literal or another constant name (cycle-guarded).
fn const_named_value(
    c: &Check<'_>,
    use_row: u32,
    name: &str,
    seen: &mut Vec<&'static str>,
) -> Option<i64> {
    // The guard needs 'static names; the lexemes live for the run.
    let spelled: &'static str = leak_spelled(name);
    if seen.contains(&spelled) {
        return None;
    }
    seen.push(spelled);
    let range = same_file::local_const_initializer(c, use_row, name)
        .or_else(|| top_level_const_initializer(c, name))?;
    const_int_tokens(c, range, seen)
}

/// The int value of an initializer token range: an optional `-`, then
/// an int literal (decimal or hex), or a bare name resolving as
/// another constant.
fn const_int_tokens(
    c: &Check<'_>,
    range: core::ops::Range<u32>,
    seen: &mut Vec<&'static str>,
) -> Option<i64> {
    let mut token = range.start;
    if token >= range.end {
        return None;
    }
    let negated = c.lexeme(token) == "-";
    if negated {
        token += 1;
        if token >= range.end {
            return None;
        }
    }
    let lexeme = c.lexeme(token);
    let hex = lexeme.starts_with("0x") || lexeme.starts_with("0X");
    let digits: String = lexeme.chars().filter(|&ch| ch != '_').collect();
    let parsed = if hex {
        digits.trim_start_matches("0x").trim_start_matches("0X")
    } else {
        &digits[..]
    };
    if let Ok(value) = i64::from_str_radix(parsed, if hex { 16 } else { 10 }) {
        return Some(if negated { -value } else { value });
    }
    if lexeme
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !lexeme.is_empty()
    {
        return const_named_value(c, 0, lexeme, seen).map(|v| if negated { -v } else { v });
    }
    None
}

/// The initializer token range of the same-file top-level `const`
/// variable `name`.
fn top_level_const_initializer(c: &Check<'_>, name: &str) -> Option<core::ops::Range<u32>> {
    let root = c.path.first().copied()?;
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::TopLevelFields
                || !crate::rules::variables_declaration_keyword(c, inner)
                    .is_some_and(|kw| c.lexeme(kw) == "const")
            {
                continue;
            }
            for decl in c.collect_direct(inner) {
                if c.kind(decl) != NodeKind::VariablesDeclaration {
                    continue;
                }
                for var in c.collect_direct(decl) {
                    if c.kind(var) == NodeKind::InitializedIdentifier
                        && crate::rules::initialized_identifier_name(c, var)
                            .is_some_and(|tok| c.lexeme(tok) == name)
                    {
                        return crate::rules::flow::exhaustive_cases::initializer_range(c, var);
                    }
                }
            }
        }
    }
    None
}

/// Extends the lookup name to `'static` for the cycle guard (the run's
/// lexemes outlive the query; the leak is bounded per distinct name).
fn leak_spelled(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

/// `_isUnassignedIndexOf` (:128-159): an `indexOf` invocation on a
/// target over the operand's piece run, not assigned to a value, with a
/// zero start index.
fn is_unassigned_index_of(c: &Check<'_>, comparison: u32, pieces: &[u32]) -> bool {
    is_unassigned_index_of_inner(c, comparison, pieces)
}

/// The unwrap recursion: each paren / `as` wrapper is a single-piece
/// run whose children the next level owns (:134-139).
fn is_unassigned_index_of_inner(c: &Check<'_>, parent: u32, pieces: &[u32]) -> bool {
    if let [only] = pieces
        && matches!(
            c.kind(*only),
            NodeKind::ParenthesizedExpression | NodeKind::AsExpression
        )
    {
        return is_unassigned_index_of_inner(c, *only, &c.collect_direct(*only));
    }
    // `invocation.parent is AssignmentExpression` (:140) — the wrapper
    // run carrying an assignment means the result is being assigned.
    if c.kind(parent) == NodeKind::Assignment
        || pieces.iter().any(|&p| c.kind(p) == NodeKind::Assignment)
    {
        return false;
    }
    // The member-invocation send: the run closes with the trailing
    // `DotAccess` marker and the invocation `Send` right before it —
    // same span (the production tiling; the receiver's own tear-off
    // `Send` precedes it, so the search anchors at the run's tail). A
    // bare `indexOf(e)` has no target and no marker (the official
    // `parentType == null` bails, :143-144).
    let Some(&marker) = pieces.last() else {
        return false;
    };
    if c.kind(marker) != NodeKind::DotAccess {
        return false;
    }
    let Some(send_pos) = pieces
        .len()
        .checked_sub(2)
        .filter(|&i| c.kind(pieces[i]) == NodeKind::Send)
    else {
        return false;
    };
    let Some((head, arguments)) = slice::send_parts(c, pieces, send_pos) else {
        return false;
    };
    if c.kind(pieces[head]) != NodeKind::Identifier
        || c.lexeme(c.head_token(pieces[head])) != "indexOf"
    {
        return false;
    }
    // The receiver type check (:143-149): `parentType == null` bails —
    // the shared typed-receiver gate decides (literals, written bare
    // names, wrapper-revealed names), a member receiver reads the
    // bridge static type when mounted (`this.list.indexOf`,
    // `a.b.indexOf`), and anything unresolvable stays silent like the
    // official unresolvable-target bail.
    if super::static_type_faces::receiver_family(
        c,
        c.semantics(),
        pieces,
        head,
        slice::ITERABLE | slice::STRING,
    ) != Some(true)
    {
        return false;
    }
    // A two-argument form must pass start index 0 (:152-155).
    if let Some(arguments) = arguments {
        let raw = c.collect_direct(arguments);
        let ranges = slice::arguments(c, &raw);
        if ranges.len() == 2 {
            let range = ranges[1].clone();
            // A named group carries its value between the name and the
            // trailing marker; a positional group is the whole range. A
            // sub-marker group (len < 2) cannot hold both a name and a
            // value — a nameless recovery `NamedArgument` reads as
            // positional instead of slicing begin>end.
            let value: &[u32] =
                if range.len() >= 2 && c.kind(raw[range.end - 1]) == NodeKind::NamedArgument {
                    &raw[range.start + 1..range.end - 1]
                } else {
                    &raw[range]
                };
            let start_index = (value.len() == 1).then(|| value[0]);
            let is_zero = start_index.is_some_and(|start| {
                c.kind(start) == NodeKind::LiteralInt && c.lexeme(c.head_token(start)) == "0"
            });
            if !is_zero {
                return false;
            }
        }
    }
    true
}
