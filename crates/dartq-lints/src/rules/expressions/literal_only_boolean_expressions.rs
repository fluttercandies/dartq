use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, query_site};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/literal_only_boolean_expressions.dart`
/// — a `do`/`for`/`if`/`while` condition (or a `when` guard) composed
/// only of literals reports at the statement (`_onlyLiterals`
/// :14-41 + the visitor :69-113): literals count, a string literal
/// counts iff its computed constant value is non-`null` (:23), a `!`
/// prefix recurses into its operand, a binary recurses into both
/// operands (`??` into the left alone), and an `is` recurses unless
/// the type resolves to a type parameter (:36). `while (true)` passes
/// (:101-105, dart-lang/linter#453), and an `if` with a case clause
/// passes whole (:86).
///
/// dartq faces over the production tiling: the dispatch rows are the
/// condition containers — [`NodeKind::ParenthesizedCondition`] under the
/// `if`/`while`/`do` statements (the upstream direct-condition faces),
/// [`NodeKind::ForParts`] for the `for` face (the condition rows sit
/// between the two separator payload tokens), and
/// [`NodeKind::SwitchCaseWhenClause`] for the guard. The operand walk
/// runs over piece runs: a trailing `UnaryPrefix` piece is its operand's
/// postfix notation (the handler fires after the operand parses), and a
/// binary's operand runs split at the operator token over its own
/// window. The string constant-value gate (:23) reads the literal's
/// interpolation rows: a plain literal (or an adjacent-strings run) is
/// valued, and an interpolated literal is valued iff every
/// interpolation expression is itself literal-only — anything else
/// stays silent (the conservative `computeConstantValue` direction).
/// The case-clause skip (:86) reads the clause keyword the condition
/// row's payload carries. The `is` type face (:36) resolves through the
/// semantic model — a declared container (same file or imported)
/// recurses — with the unshadowed core names and same-file types as the
/// name-table face; a type-parameter or prefixed type stays silent (the
/// type-parameter bail's conservative face — the model publishes no
/// type-position binding, so a type parameter is undecidable from an
/// unresolvable name and both keep silent). Message: `diagnostic.g.dart`
/// `literalOnlyBooleanExpressions`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "literal_only_boolean_expressions",
    group: Group::Standalone,
    problem: "The Boolean expression has a constant value.",
    correction: "Try changing the expression.",
    kinds: &[
        NodeKind::ParenthesizedCondition,
        NodeKind::ForParts,
        NodeKind::SwitchCaseWhenClause,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    match c.kind(node) {
        NodeKind::ParenthesizedCondition => {
            let Some(parent) = c.ancestor(1) else {
                return;
            };
            if !matches!(
                c.kind(parent),
                NodeKind::IfStatement | NodeKind::WhileStatement | NodeKind::DoWhileStatement
            ) {
                return;
            }
            // The case-clause skip (:86): `if (e case p)` carries the
            // clause keyword in the condition row's payload word 0.
            if c.payload(node).first().is_some_and(|&kw| kw != NONE_TOKEN) {
                return;
            }
            let run = c.collect_direct(node);
            // `while (true) { }` passes (:101-105).
            if c.kind(parent) == NodeKind::WhileStatement
                && let [only] = run[..]
                && c.kind(only) == NodeKind::LiteralBool
                && c.lexeme(c.head_token(only)) == "true"
            {
                return;
            }
            if only_literals(c, analysis, &run) {
                let range = c.utf16_span(parent);
                c.report(range, &[]);
            }
        }
        NodeKind::ForParts => {
            let run = for_parts_condition(c, node);
            if only_literals(c, analysis, &run) {
                let span = match c.ancestor(1) {
                    Some(parent) if c.kind(parent) == NodeKind::ForStatement => {
                        c.utf16_span(parent)
                    }
                    _ => c.utf16_span(node),
                };
                c.report(span, &[]);
            }
        }
        NodeKind::SwitchCaseWhenClause => {
            let run = c.collect_direct(node);
            if only_literals(c, analysis, &run) {
                let range = c.utf16_span(node);
                c.report(range, &[]);
            }
        }
        _ => {}
    }
}

/// The `_onlyLiterals` walk (:14-41) over a piece run. A trailing
/// `UnaryPrefix` piece is its operand's prefix marker — under the
/// production tiling the operand completes after the marker's owner
/// seals, so the operand is the run's tail when the marker closes the
/// run, and otherwise the node the marker's next token-sibling
/// expression resolves to (see [`unary_operand`]); a run of string
/// literals (the adjacent-strings face — upstream `AdjacentStrings`
/// is a `StringLiteral`) decides through the constant-value gate; a
/// single row dispatches by kind; anything longer stays silent.
fn only_literals(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) -> bool {
    // A lone `UnaryPrefix` marker: the operand completed outside its
    // owner's sealed window — resolve through the marker's
    // token-sibling scan before the trailing-marker peel below would
    // drop the marker to an empty run.
    if let &[only] = run
        && c.kind(only) == NodeKind::UnaryPrefix
    {
        return unary_operand(c, only)
            .is_some_and(|operand| only_literals(c, analysis, &[operand]));
    }
    let run = match run.split_last() {
        Some((last, head)) if c.kind(*last) == NodeKind::UnaryPrefix => head,
        _ => run,
    };
    if !run.is_empty()
        && run
            .iter()
            .all(|&row| c.kind(row) == NodeKind::StringLiteral)
    {
        return run.iter().all(|&row| string_valued(c, analysis, row));
    }
    let &[only] = run else {
        return false;
    };
    match c.kind(only) {
        NodeKind::LiteralBool
        | NodeKind::LiteralDouble
        | NodeKind::LiteralInt
        | NodeKind::LiteralNull
        | NodeKind::SymbolLiteral
        | NodeKind::ListLiteral
        // `[]`/`{}` are `Literal`s upstream (`is! StringLiteral` → true).
        | NodeKind::SetOrMapLiteral => true,
        NodeKind::StringLiteral => string_valued(c, analysis, only),
        // `const []` parses as a const-tagged literal — the content
        // decides (the upstream `ListLiteral` face).
        NodeKind::ConstLiteral => only_literals(c, analysis, &c.collect_direct(only)),
        NodeKind::ParenthesizedExpression => {
            only_literals(c, analysis, &c.collect_direct(only))
        }
        NodeKind::BinaryExpression => {
            let Some((left, right)) = binary_operand_runs(c, only) else {
                return false;
            };
            let op = c
                .payload(only)
                .first()
                .map(|&op| c.lexeme(op))
                .unwrap_or_default();
            // `??` recurses into the left operand alone (:23-25).
            if op == "??" {
                return only_literals(c, analysis, &left);
            }
            only_literals(c, analysis, &left) && only_literals(c, analysis, &right)
        }
        NodeKind::IsExpression => {
            // The type-parameter bail (:27-30): an unresolvable type
            // name stays silent; a resolved container, a core name, or
            // a same-file type recurses into the operand.
            let kids = c.collect_direct(only);
            let Some(type_row) = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Type)
            else {
                return false;
            };
            if !is_type_resolves(c, analysis, type_row) {
                return false;
            }
            // The operand escapes the sealed `is` window — under the
            // adopted-begin tiling the left operand completes before
            // the `is` marker opens, so it tiles as an earlier sibling
            // in the enclosing statement chain (the `unary_operand`
            // idiom), never inside the `IsExpression` row itself.
            let Some(operand) = is_operand(c, only) else {
                return false;
            };
            only_literals(c, analysis, &[operand])
        }
        _ => false,
    }
}

/// The string constant-value gate (:23): a literal is valued iff its
/// computed constant is non-`null`. A plain literal always is; an
/// interpolated literal is valued iff every interpolation expression is
/// itself literal-only (the conservative `computeConstantValue`
/// direction — an unverifiable interpolation keeps the whole condition
/// silent).
fn string_valued(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, literal: u32) -> bool {
    // The interpolation rows tile in the literal's construction window
    // (D4: the literal's token span anchors at its head).
    let kids = c.collect_direct(literal);
    if kids.is_empty() {
        return true;
    }
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    for row in kids {
        current.push(row);
        // An invocation interpolation's root send covers its whole
        // piece run — that boundary closes the group. A bare literal or
        // identifier interpolation (`'${1}'`) carries no send and keeps
        // its single piece open until the literal's own end.
        if c.kind(row) == NodeKind::Send {
            let span = c.byte_span(row);
            if current
                .iter()
                .all(|&part| c.byte_span(part).end <= span.end)
            {
                let mut group = std::mem::take(&mut current);
                // Strip the interpolation's own completion pair.
                while let Some(&last) = group.last()
                    && matches!(
                        c.kind(last),
                        NodeKind::Send
                            | NodeKind::NoArguments
                            | NodeKind::NoTypeArguments
                            | NodeKind::TypeArguments
                            | NodeKind::DotAccess
                    )
                {
                    group.pop();
                }
                groups.push(group);
            }
        }
    }
    // A trailing non-send group is a bare expression interpolation —
    // its own face decides (a `Send`-less literal is verifiable).
    if !current.is_empty() {
        groups.push(std::mem::take(&mut current));
    }
    groups
        .into_iter()
        .all(|group| !group.is_empty() && only_literals(c, analysis, &group))
}

/// Whether the `is` type name resolves to a non-type-parameter type
/// (:36 — `element is TypeParameterElement` bails): the semantic
/// container face first (same-file and imported classes, enums, mixins,
/// extensions), then the unshadowed core names and the same-file types.
fn is_type_resolves(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    type_row: u32,
) -> bool {
    if analysis
        .resolve_constructor(query_site(c, type_row), None)
        .is_some()
    {
        return true;
    }
    let name = c.lexeme(c.head_token(type_row));
    if matches!(
        name,
        "bool"
            | "double"
            | "int"
            | "num"
            | "Object"
            | "String"
            | "dynamic"
            | "Never"
            | "Null"
            | "Function"
            | "Type"
            | "Symbol"
            | "Iterable"
            | "List"
            | "Map"
            | "Set"
    ) {
        return true;
    }
    super::same_file::declares_type(c, name)
}

/// The operand runs of a binary row, split at the operator token over
/// the row's own direct children (the `slice::binary_operands` shape).
/// Direct children only — a nested interpolation row inside a string
/// literal stays inside its literal's subtree and never pollutes the
/// operand split.
fn binary_operand_runs(c: &Check<'_>, binary: u32) -> Option<(Vec<u32>, Vec<u32>)> {
    let op_token = c.payload(binary).first().copied()?;
    let op_start = c.unit.tokens.get(op_token as usize)?.byte_offset as usize;
    let rows = c.collect_direct(binary);
    let split = rows.partition_point(|&row| c.byte_span(row).end <= op_start);
    if split == 0 || split == rows.len() {
        return None;
    }
    Some((rows[..split].to_vec(), rows[split..].to_vec()))
}

/// The left operand of an `is`/`as` test: under the adopted-begin
/// tiling the operand completes before the `is` marker's owner opens,
/// so it escapes to the enclosing chain — an earlier sibling whose span
/// ends at the test row's head token (the `unary_operand` idiom, matched
/// by span-end rather than head-order).
fn is_operand(c: &Check<'_>, is_row: u32) -> Option<u32> {
    let is_head = c.head_token(is_row);
    let is_start = c.unit.tokens.get(is_head as usize)?.byte_offset as usize;
    let mut level = is_row;
    loop {
        let parent = c.unit.ast.parent(level)?;
        // The operand is the expression sibling ending closest before
        // the `is` marker's own start (whitespace separates them).
        let mut best: Option<(usize, u32)> = None;
        for sibling in c.collect_direct(parent) {
            if sibling == level || sibling == is_row {
                continue;
            }
            let span = c.byte_span(sibling);
            if span.end <= is_start && best.is_none_or(|(e, _)| span.end > e) {
                best = Some((span.end, sibling));
            }
        }
        if let Some((_, row)) = best {
            return Some(row);
        }
        level = parent;
    }
}

/// The operand of a lone `UnaryPrefix` marker (`!e`, `-e`, `~e`): under
/// the production tiling the marker is a leaf piece while its operand
/// completes into the enclosing window — `( ! true )` tiles
/// `UnaryPrefix` inside `ParenthesizedCondition` and the `LiteralBool`
/// as its parent's earlier sibling (the adopted-begin face: the
/// operand's head token follows the marker's, but its row id can sit
/// before the marker's owner). The walk scans the enclosing chain's
/// children for the first expression row whose head token follows the
/// marker's — a single row, never a run.
fn unary_operand(c: &Check<'_>, marker: u32) -> Option<u32> {
    let marker_head = c.head_token(marker);
    let mut level = marker;
    loop {
        let parent = c.unit.ast.parent(level)?;
        for sibling in c.collect_direct(parent) {
            if sibling != level && c.head_token(sibling) > marker_head {
                return Some(sibling);
            }
        }
        level = parent;
    }
}

/// The condition rows of a `for` parts row: the pieces between the two
/// separator payload tokens (`lparen/left_sep/right_sep` words). The
/// `ForParts` leaf is childless — the condition tiles as the
/// `ForStatement`'s own children, so the walk scans the parent's
/// direct rows for the pieces whose byte spans sit inside the
/// separator window.
fn for_parts_condition(c: &Check<'_>, parts: u32) -> Vec<u32> {
    let payload = c.payload(parts);
    let Some(&left_sep) = payload.get(1) else {
        return Vec::new();
    };
    let Some(&right_sep) = payload.get(2) else {
        return Vec::new();
    };
    let (Some(left_end), Some(right_start)) = (
        c.unit
            .tokens
            .get(left_sep as usize)
            .map(|t| (t.byte_offset + t.byte_len) as usize),
        c.unit
            .tokens
            .get(right_sep as usize)
            .map(|t| t.byte_offset as usize),
    ) else {
        return Vec::new();
    };
    let Some(parent) = c.unit.ast.parent(parts) else {
        return Vec::new();
    };
    let inside: Vec<u32> = c
        .collect_direct(parent)
        .into_iter()
        .filter(|&row| {
            let span = c.byte_span(row);
            span.start >= left_end && span.end <= right_start
        })
        .collect();
    // The parser emits the condition's `ExpressionStatement` wrapper
    // beside the operand itself (same operand span, trailing `;` — the
    // completion-order window puts both at the `ForStatement` level).
    // The operand is the semantic piece; drop the statement-width
    // wrapper when it strictly contains another window row's span.
    inside
        .iter()
        .copied()
        .filter(|&row| {
            if c.kind(row) != NodeKind::ExpressionStatement {
                return true;
            }
            let span = c.byte_span(row);
            !inside.iter().any(|&other| {
                if other == row {
                    return false;
                }
                let o = c.byte_span(other);
                o.start >= span.start
                    && o.end <= span.end
                    && (o.start > span.start || o.end < span.end)
            })
        })
        .collect()
}
