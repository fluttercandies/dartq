//! The expression-shape family: `unnecessary_new`,
//! `unnecessary_null_in_if_null_operators`, `prefer_is_not_operator`,
//! `prefer_adjacent_string_concatenation`,
//! `avoid_single_cascade_in_expression_statements`,
//! `prefer_conditional_assignment`, `prefer_if_null_operators`,
//! `prefer_null_aware_operators`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, slice, token_seq_equal};

pub(crate) mod unnecessary_new {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_new.dart:37-45` —
    /// every `new` keyword of an instance creation reports at the keyword
    /// (`reportAtToken(keyword)`). `new` is the span head (`NewExpression`
    /// is a width-0 kind: the derivable word lives in `header.first_token`,
    /// collapse map §3.5 NewExpressionEnd row + the §1 elimination note).
    /// Message: `diagnostic.g.dart` `unnecessaryNew`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_new",
        group: Group::Recommended,
        problem: "Unnecessary 'new' keyword.",
        correction: "Try removing the 'new' keyword.",
        kinds: &[NodeKind::NewExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let start = c
            .unit
            .tokens
            .get(c.head_token(node) as usize)
            .expect("span head outside the token table")
            .utf16_offset;
        c.report(start..start, &[]);
    }
}

pub(crate) mod unnecessary_null_in_if_null_operators {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_null_in_if_null_operators.dart:34-49`
    /// — `a ?? null` (or `null ?? a`) reports at the `null` literal, right
    /// operand first. The operator is payload word 0, operands the two
    /// children (collapse map §3.5 BinaryExpression row). Message:
    /// `diagnostic.g.dart` `unnecessaryNullInIfNullOperators`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_null_in_if_null_operators",
        group: Group::Recommended,
        problem: "Unnecessary use of '??' with 'null'.",
        correction: "Try removing the '??' operator and the 'null' operand.",
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(&op) = c.payload(node).first() else {
            return;
        };
        if c.lexeme(op) != "??" {
            return;
        }
        let (left, right) = {
            let kids = c.direct_children(node);
            if kids.len() < 2 {
                return;
            }
            (kids[0], kids[1])
        };
        let null_operand = if c.kind(right) == NodeKind::LiteralNull {
            Some(right)
        } else if c.kind(left) == NodeKind::LiteralNull {
            Some(left)
        } else {
            None
        };
        if let Some(operand) = null_operand {
            let range = c.utf16_span(operand);
            c.report(range, &[]);
        }
    }
}

pub(crate) mod prefer_is_not_operator {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_is_not_operator.dart:33-55`
    /// — `!(x is T)` (the `is` expression wrapped in parentheses under a
    /// `!` prefix) reports at the prefix expression. The parenthesized +
    /// prefix shape is required by the official parent chain
    /// (`parent is ParenthesizedExpression && parent.parent is
    /// PrefixExpression`, :38-54). Message: `diagnostic.g.dart`
    /// `preferIsNotOperator`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_is_not_operator",
        group: Group::Recommended,
        problem: "Use the 'is!' operator rather than negating the value of the 'is' operator.",
        correction: "Try rewriting the condition to use the 'is!' operator.",
        kinds: &[NodeKind::IsExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.notOperator != null` bails (:39-42): payload word 0 is the
        // `not` flag (collapse map §3.5 IsExpression row).
        if c.payload(node).first().copied().unwrap_or(0) != 0 {
            return;
        }
        let Some(paren) = c.ancestor(1) else {
            return;
        };
        if c.kind(paren) != NodeKind::ParenthesizedExpression {
            return;
        }
        let Some(prefix) = c.ancestor(2) else {
            return;
        };
        if c.kind(prefix) != NodeKind::UnaryPrefix {
            return;
        }
        // Prefix `!` only (`operator.type == TokenType.BANG`, :46-48). The
        // operator is the `UnaryPrefix` span head (width-0 kind, collapse
        // map §3.5 UnaryPrefixExpressionHandle row + the §1 elimination
        // note).
        if c.lexeme(c.head_token(prefix)) != "!" {
            return;
        }
        let range = c.utf16_span(prefix);
        c.report(range, &[]);
    }
}

pub(crate) mod prefer_adjacent_string_concatenation {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_adjacent_string_concatenation.dart:37-46`
    /// — `+` whose two operands are both string literals reports at the
    /// `+` (adjacent strings need no operator). Message: `diagnostic.g.dart`
    /// `preferAdjacentStringConcatenation`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_adjacent_string_concatenation",
        group: Group::Recommended,
        problem: "String literals shouldn't be concatenated by the '+' operator.",
        correction: "Try removing the operator to use adjacent strings.",
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(&op) = c.payload(node).first() else {
            return;
        };
        if c.lexeme(op) != "+" {
            return;
        }
        let string_pair = {
            let kids = c.direct_children(node);
            kids.len() >= 2
                && c.kind(kids[0]) == NodeKind::StringLiteral
                && c.kind(kids[1]) == NodeKind::StringLiteral
        };
        if !string_pair {
            return;
        }
        let start = c
            .unit
            .tokens
            .get(op as usize)
            .expect("op token outside the token table")
            .utf16_offset;
        c.report(start..start, &[]);
    }
}

pub(crate) mod avoid_single_cascade_in_expression_statements {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_single_cascade_in_expression_statements.dart:39-61`
    /// — an expression statement consisting of a single-section cascade
    /// reports at the cascade, with the equivalent `.`-operator spelled in
    /// the message (`?.` for a `?..` section, `avoid_single_cascade...dart:40-51`).
    /// Message: `diagnostic.g.dart` `avoidSingleCascadeInExpressionStatements`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_single_cascade_in_expression_statements",
        group: Group::Recommended,
        problem: "Unnecessary cascade expression.",
        correction: "Try using the operator '{0}'.",
        kinds: &[NodeKind::Cascade],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let (count, section) = {
            let kids = c.direct_children(node);
            (kids.len(), kids.first().copied())
        };
        if count != 1 {
            return;
        }
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if c.kind(parent) != NodeKind::ExpressionStatement {
            return;
        }
        let section = section.expect("count == 1 checked above");
        // The section's cascade operator decides the message argument
        // (avoid_single_cascade...dart:40-51): `?..` prints as `?.`.
        let operator = if c.kind(section) == NodeKind::CascadeAccess
            && c.payload(section).get(1).copied().unwrap_or(0) != 0
        {
            "?."
        } else {
            "."
        };
        let range = c.utf16_span(node);
        c.report(range, &[operator]);
    }
}

pub(crate) mod prefer_conditional_assignment {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, first_child, token_seq_equal};

    /// Official: `pkg/linter/lib/src/rules/prefer_conditional_assignment.dart:23-88`
    /// — an `if` without `else` whose condition is a null check `x == null`
    /// (either side null) and whose then-statement assigns to `x` reports at
    /// the `if`. The equality operand identity is the anchored token-lexeme
    /// sequence comparison (the official
    /// `canonicalElementsFromIdentifiersAreEqual` is element-based; the
    /// difference is confined to renames across scopes a lint cannot see
    /// either). Message: `diagnostic.g.dart` `preferConditionalAssignment`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_conditional_assignment",
        group: Group::Recommended,
        problem: "The 'if' statement could be replaced by a null-aware assignment.",
        correction: "Try using the '??=' operator to conditionally assign a value.",
        kinds: &[NodeKind::IfStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.elseStatement != null` bails (:70-72).
        if c.payload(node).first().copied() != Some(NONE_TOKEN) {
            return;
        }
        let Some(operand) = condition_null_operand(c, node) else {
            return;
        };
        // The then arm is the direct child after the condition wrapper
        // (collapse map §3.4: ParenthesizedCondition delimits the condition
        // segment, ThenStatementEnd D1).
        let then = c.collect_direct(node).get(1).copied();
        let Some(then) = then else {
            return;
        };
        if !assigns_to(c, then, operand) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// `_getExpressionCondition` (:48-61): the condition (unparenthesized)
    /// must be `==` with one null side; the other side is the operand.
    fn condition_null_operand(c: &Check<'_>, if_node: u32) -> Option<u32> {
        let cond_wrapper = first_child(c, if_node)?;
        let mut condition = first_child(c, cond_wrapper)?;
        while c.kind(condition) == NodeKind::ParenthesizedExpression {
            condition = first_child(c, condition)?;
        }
        if c.kind(condition) != NodeKind::BinaryExpression {
            return None;
        }
        if c.lexeme(op_token(c, condition)) != "==" {
            return None;
        }
        let kids = c.collect_direct(condition);
        if kids.len() < 2 {
            return None;
        }
        if c.kind(kids[1]) == NodeKind::LiteralNull {
            Some(kids[0])
        } else if c.kind(kids[0]) == NodeKind::LiteralNull {
            Some(kids[1])
        } else {
            None
        }
    }

    /// `_checkStatement` (:33-45): an expression statement assigning to
    /// `operand`, unwrapping single-statement blocks.
    fn assigns_to(c: &Check<'_>, statement: u32, operand: u32) -> bool {
        match c.kind(statement) {
            NodeKind::ExpressionStatement => {
                let Some(expr) = first_child(c, statement) else {
                    return false;
                };
                if c.kind(expr) != NodeKind::Assignment {
                    return false;
                }
                if c.lexeme(op_token(c, expr)) != "=" {
                    return false;
                }
                let Some(lhs) = first_child(c, expr) else {
                    return false;
                };
                token_seq_equal(c, lhs, operand)
            }
            NodeKind::Block => {
                let kids = c.collect_direct(statement);
                kids.len() == 1 && assigns_to(c, kids[0], operand)
            }
            _ => false,
        }
    }

    fn op_token(c: &Check<'_>, node: u32) -> u32 {
        c.payload(node)
            .first()
            .copied()
            .expect("binary/assignment payload carries the operator")
    }
}

pub(crate) mod prefer_if_null_operators {
    use super::{Check, Group, NodeKind, RuleSpec, token_seq_equal};

    /// Official: `pkg/linter/lib/src/rules/prefer_if_null_operators.dart:22-63`
    /// — `x == null ? a : x` (or the `!=`/then mirror) reports at the
    /// conditional when the null-check operand and the non-null arm have the
    /// same token-lexeme sequence (the official `toString()` comparison).
    /// Message: `diagnostic.g.dart` `preferIfNullOperators`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_if_null_operators",
        group: Group::Recommended,
        problem: "Use the '??' operator rather than '?:' when testing for 'null'.",
        correction: "Try rewriting the code to use '??'.",
        kinds: &[NodeKind::Conditional],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some((op, operand, arms)) = null_check_parts(c, node) else {
            return;
        };
        // `==` tests the else arm, `!=` the then arm (:49-52).
        let arm = if op == "==" { arms.1 } else { arms.0 };
        if token_seq_equal(c, arm, operand) {
            let range = c.utf16_span(node);
            c.report(range, &[]);
        }
    }

    /// The `(operator, non-null operand, (then, else))` of a null-check
    /// condition, or `None`.
    pub(super) fn null_check_parts<'a>(
        c: &'a Check<'a>,
        node: u32,
    ) -> Option<(&'a str, u32, (u32, u32))> {
        let kids = c.collect_direct(node);
        if kids.len() < 3 {
            return None;
        }
        let condition = kids[0];
        if c.kind(condition) != NodeKind::BinaryExpression {
            return None;
        }
        let op = c.lexeme(
            c.payload(condition)
                .first()
                .copied()
                .expect("binary payload"),
        );
        if op != "==" && op != "!=" {
            return None;
        }
        let operands = c.collect_direct(condition);
        if operands.len() < 2 {
            return None;
        }
        let operand = if c.kind(operands[1]) == NodeKind::LiteralNull {
            operands[0]
        } else if c.kind(operands[0]) == NodeKind::LiteralNull {
            operands[1]
        } else {
            return None;
        };
        Some((op, operand, (kids[1], kids[2])))
    }
}

pub(crate) mod prefer_null_aware_operators {
    use super::{Check, Group, NodeKind, RuleSpec, token_seq_equal};
    use dartq_scanner::TokenKind;

    /// Official: `pkg/linter/lib/src/rules/prefer_null_aware_operators.dart:22-76`
    /// — `a == null ? null : a.b` (`!=` mirrors to the then arm) reports when
    /// peeling accesses off the result expression reaches the null-check
    /// operand. The peel and the comparison are the anchored token-lexeme
    /// sequence form of the official `toString()` walk. Message:
    /// `diagnostic.g.dart` `preferNullAwareOperators`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_null_aware_operators",
        group: Group::Recommended,
        problem: "Use the null-aware operator '?.' rather than an explicit 'null' comparison.",
        correction: "Try using '?.'.",
        kinds: &[NodeKind::Conditional],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some((op, operand, arms)) = super::prefer_if_null_operators::null_check_parts(c, node)
        else {
            return;
        };
        // `x == null ? null : y` / `x != null ? y : null` (:47-60).
        let result = match op {
            "==" if c.kind(arms.0) == NodeKind::LiteralNull => arms.1,
            "!=" if c.kind(arms.1) == NodeKind::LiteralNull => arms.0,
            _ => return,
        };
        // Peel one access at a time; compare only after a peel (:61-74 — the
        // bare `a == null ? null : a` form never matches).
        let mut current = result;
        while let Some(stripped) = peel(c, current) {
            if token_seq_equal(c, stripped, operand) {
                let range = c.utf16_span(node);
                c.report(range, &[]);
                return;
            }
            current = stripped;
        }
    }

    /// One peel step (the official switch, :63-72): an access receiver, the
    /// target of a prefixed invocation, or a `!` operand.
    fn peel(c: &Check<'_>, node: u32) -> Option<u32> {
        match c.kind(node) {
            // `a.b`, `a?.b` → `a`.
            NodeKind::DotAccess | NodeKind::Qualified => {
                let kids = c.collect_direct(node);
                kids.first().copied()
            }
            // `a.b()` → the invocation target behind the callee's last dot.
            NodeKind::Send => {
                let kids = c.collect_direct(node);
                let callee = kids.iter().copied().find(|&kid| {
                    !matches!(
                        c.kind(kid),
                        NodeKind::Arguments | NodeKind::NoArguments | NodeKind::TypeApplication
                    )
                })?;
                match c.kind(callee) {
                    NodeKind::DotAccess | NodeKind::Qualified => {
                        c.collect_direct(callee).first().copied()
                    }
                    _ => None,
                }
            }
            // `a!` → `a`.
            NodeKind::UnaryPostfix => {
                let op = c.payload(node).first().copied()?;
                if c.unit.tokens.get(op as usize)?.kind != TokenKind::Bang {
                    return None;
                }
                c.collect_direct(node).first().copied()
            }
            _ => None,
        }
    }
}

pub(crate) mod unnecessary_null_aware_assignments {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{first_child, last_child};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_null_aware_assignments.dart:42-49`
    /// — `x ??= null` reports at the assignment; the write must not be a
    /// setter (`node.writeElement is SetterElement` bails, :43), i.e.
    /// property assignments (`a.b ??= null`) stay meaningful.
    ///
    /// Approximation (documented): the setter exclusion is taken from the
    /// access shape — a [`NodeKind::DotAccess`] LHS or any assignment inside
    /// a [`NodeKind::Cascade`] skips. Other exotic write elements (e.g.
    /// `x[0] ??= null`, whose write element is not a setter upstream) report,
    /// matching the official branch. Message: `diagnostic.g.dart`
    /// `unnecessaryNullAwareAssignments`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_null_aware_assignments",
        group: Group::Recommended,
        problem: "Unnecessary assignment of 'null'.",
        correction: "Try removing the assignment.",
        kinds: &[NodeKind::Assignment],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.operator.type == QUESTION_QUESTION_EQ &&
        // node.rightHandSide.isNullLiteral` (:45-46).
        if c.lexeme(c.payload(node).first().copied().expect("assignment op")) != "??=" {
            return;
        }
        let Some(rhs) = last_child(c, node) else {
            return;
        };
        if c.kind(rhs) != NodeKind::LiteralNull {
            return;
        }
        let Some(lhs) = first_child(c, node) else {
            return;
        };
        if matches!(c.kind(lhs), NodeKind::DotAccess | NodeKind::CascadeAccess) {
            return;
        }
        if c.path.iter().any(|&a| c.kind(a) == NodeKind::Cascade) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

pub(crate) mod prefer_contains {
    use super::{Check, Group, NodeKind, RuleSpec, slice};

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
    /// The receiver type check (`implementsAnyInterface([Iterable, String])`,
    /// :145-149) reads through the shared literal face
    /// ([`slice::implements_any`]) — string and collection literals decide,
    /// an opaque receiver keeps the documented presumption (a user class
    /// with an `indexOf` member over-reports). `getIntValue` reads int
    /// literal pieces (decimal/hex, separators, a `-` prefix), so constant
    /// expressions (`const k = -1`) under-report. Message: `diagnostic.g.dart`
    /// `preferContainsUseContains`/`preferContainsAlwaysFalse`/`preferContainsAlwaysTrue`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_contains",
        group: Group::Recommended,
        problem: USE_CONTAINS_PROBLEM,
        correction: USE_CONTAINS_CORRECTION,
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let op = c.lexeme(c.payload(node).first().copied().expect("binary op"));
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
            // Right literal first (:66-71).
            if let Some(value) = int_value(c, &right, 0)
                && value <= 0
                && is_unassigned_index_of(c, node, &left)
            {
                (value, op)
            } else if let Some(value) = int_value(c, &left, 0)
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
        // The receiver type check (:145-149).
        if !slice::receiver_implements_any(c, pieces, head, slice::ITERABLE | slice::STRING) {
            return false;
        }
        // A two-argument form must pass start index 0 (:152-155).
        if let Some(arguments) = arguments {
            let raw = c.collect_direct(arguments);
            let ranges = slice::arguments(c, &raw);
            if ranges.len() == 2 {
                let range = ranges[1].clone();
                // A named group carries its value between the name and the
                // trailing marker; a positional group is the whole range.
                let value: &[u32] = if c.kind(raw[range.end - 1]) == NodeKind::NamedArgument {
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
}

pub(crate) mod unnecessary_const {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{const_keyword, in_const_context};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_const.dart:38-77` — a
    /// `const` keyword on a literal / instance creation already inside a
    /// constant context reports at the keyword.
    ///
    /// The context face is the official `constantContext` walk (ported as
    /// [`crate::rules::in_const_context`] — the rule declares
    /// `canUseParsedResult`, so the walk is the whole story). The visitor's
    /// `ConstantPattern` exemptions are structural with our payload layout: a
    /// constant pattern owns the `const` keyword itself (`ConstantPattern`
    /// head-when-present, collapse map §3.7), so the wrapped expression
    /// carries none and never dispatches. Dot-shorthand const invocations
    /// (`case const .new()`) are likewise pattern-owned and skip. Message:
    /// `diagnostic.g.dart` `unnecessaryConst`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_const",
        group: Group::Recommended,
        problem: "Unnecessary 'const' keyword.",
        correction: "Try removing the keyword.",
        kinds: &[
            NodeKind::ConstExpression,
            NodeKind::ConstLiteral,
            NodeKind::ListLiteral,
            NodeKind::SetOrMapLiteral,
            NodeKind::RecordLiteral,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `keyword == null || keyword.type != CONST` bails (:57-59).
        let Some(keyword) = const_keyword(c, node) else {
            return;
        };
        // `node.inConstantContext` (:60-62).
        if !in_const_context(c) {
            return;
        }
        let start = c
            .unit
            .tokens
            .get(keyword as usize)
            .expect("const keyword outside the token table")
            .utf16_offset;
        c.report(start..start, &[]);
    }
}

pub(crate) mod prefer_is_empty {
    use super::{Check, Group, NodeKind, RuleSpec, slice};
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
    /// :229-234) reads through the shared literal face
    /// ([`slice::receiver_implements_any`]) — collection/string/map literals
    /// decide (a map literal reports like the official `Map` face), a record
    /// literal is provably outside the family and passes, and an opaque
    /// receiver keeps the documented presumption (a user `length` getter
    /// over-reports). `getIntValue` reads int literal pieces, so constant
    /// expressions under-report. The const-context bails (:93-108) port
    /// through [`crate::rules::in_const_context`] and the const-constructor
    /// initializer face. Messages: `diagnostic.g.dart`
    /// `preferIsEmptyUseIsEmpty`/`preferIsEmptyUseIsNotEmpty`/
    /// `preferIsEmptyAlwaysFalse`/`preferIsEmptyAlwaysTrue`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_is_empty",
        group: Group::Core,
        problem: USE_IS_EMPTY_PROBLEM,
        correction: USE_IS_EMPTY_CORRECTION,
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The operand pieces tile in the enclosing owner, split at the
        // operator token (`slice::binary_operands`).
        let Some((left, right)) = slice::binary_operands(c) else {
            return;
        };
        let op = c.lexeme(c.payload(node).first().copied().expect("binary op"));
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
        slice::receiver_faces(c, pieces, name)
            .is_none_or(|faces| faces & (slice::ITERABLE | slice::MAP | slice::STRING) != 0)
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
}

pub(crate) mod prefer_is_not_empty {
    use super::{Check, Group, NodeKind, RuleSpec, slice};

    /// Official: `pkg/linter/lib/src/rules/prefer_is_not_empty.dart:31-63` —
    /// `!x.isEmpty` reports at the prefix expression.
    ///
    /// The element checks (:57-62 — the property resolves to `isEmpty` and
    /// its interface declares `isNotEmpty`) read through the shared literal
    /// face: every core family carrying `isEmpty` also declares `isNotEmpty`,
    /// so the name check plus the face query answers both. An opaque
    /// receiver keeps the documented presumption (a user class with
    /// `isEmpty` but no `isNotEmpty` over-reports). Operator-container and
    /// statement owners bound the operand run exactly; a positional
    /// argument list carries no separators, so a later argument's access
    /// (`f(!x.length, y.isEmpty)`) false-fires — the documented residue.
    /// Message: `diagnostic.g.dart` `preferIsNotEmpty`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_is_not_empty",
        group: Group::Core,
        problem: "Use 'isNotEmpty' rather than negating the result of 'isEmpty'.",
        correction: "Try rewriting the expression to use 'isNotEmpty'.",
        kinds: &[NodeKind::UnaryPrefix],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `prefix.type != BANG` bails (:44-46); the operator is the width-0
        // kind's span head.
        if c.lexeme(c.head_token(node)) != "!" {
            return;
        }
        // The `!` is a childless prefix leaf emitted before its operand, so
        // the operand rows precede it as owner siblings — the receiver's
        // tear-off pieces and the trailing access marker alike. A
        // parenthesized-condition container leaks its expression pieces one
        // level up, though: the bang then opens the container (no rows
        // before it inside) and the run reads beside the container row at
        // the container's parent.
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let bang_end = c.byte_span(node).end;
        let kids = c.collect_direct(parent);
        let run: Vec<u32> = match kids.iter().position(|&kid| kid == node) {
            Some(pos) if pos > 0 => kids[..pos].to_vec(),
            _ => {
                let Some(grand) = c.ancestor(2) else {
                    return;
                };
                let owner_kids = c.collect_direct(grand);
                let Some(container_pos) = owner_kids.iter().position(|&kid| kid == parent) else {
                    return;
                };
                owner_kids[..container_pos].to_vec()
            }
        };
        let pieces: Vec<u32> = run
            .iter()
            .copied()
            .filter(|&piece| c.byte_span(piece).start >= bang_end)
            .collect();
        // `node.operand.unParenthesized` is a property access (:50-55): the
        // operand's tail is its access marker (`!(a || b)` carries none).
        let Some(marker_index) = pieces
            .iter()
            .rposition(|&p| c.kind(p) == NodeKind::DotAccess)
        else {
            return;
        };
        let marker = pieces[marker_index];
        let chain = &pieces[..=marker_index];
        // The property name is `isEmpty` (:55).
        let Some(name) = slice::access_name_index(c, chain, marker) else {
            return;
        };
        if c.lexeme(c.head_token(chain[name])) != "isEmpty" {
            return;
        }
        // The interface declares `isNotEmpty` (:57-62): every core family
        // with `isEmpty` does, so the face query decides.
        if !slice::receiver_faces(c, chain, name)
            .is_none_or(|faces| faces & (slice::ITERABLE | slice::MAP | slice::STRING) != 0)
        {
            return;
        }
        // The official reports at the prefix expression — from the `!`
        // through the member name. The completion marker's span trails one
        // token past the access (the lazy rows anchor at the following
        // token), so the name row bounds the end.
        let start = c.utf16_span(node).start;
        let end = c.utf16_span(chain[name]).end;
        c.report(start..end, &[]);
    }
}

pub(crate) mod avoid_function_literals_in_foreach_calls {
    use super::{Check, Group, NodeKind, RuleSpec, slice};

    /// Official: `pkg/linter/lib/src/rules/avoid_function_literals_in_foreach_calls.dart:31-105`
    /// — `x.forEach((e) { … })` reports at the method name.
    ///
    /// The receiver type check (`_isIterable`, :40-45) reads through the
    /// shared literal face ([`slice::receiver_implements_any`]) — collection
    /// literals decide (a map literal stays silent like the official
    /// `Iterable`-only gate), an opaque receiver keeps the documented
    /// presumption. The closure-arity stand-in remains for that opaque face:
    /// a multi-parameter closure is the `Map.forEach` shape the syntax
    /// cannot see (`map.forEach((k, v) …)` stays silent). The null-aware
    /// chain bail (:87) reads the invocation's own marker word plus any
    /// null-aware access over the receiver run — the access-chain lookups
    /// farther out under-report on exotic chains. Message: `diagnostic.g.dart`
    /// `avoidFunctionLiteralsInForeachCalls`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_function_literals_in_foreach_calls",
        group: Group::Recommended,
        problem: "Function literals shouldn't be passed to 'forEach'.",
        correction: "Try using a 'for' loop.",
        kinds: &[NodeKind::Send],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let Some((kids, pos)) = slice::owner_slice(c) else {
            return;
        };
        // The member face: a trailing `DotAccess` marker proves a target
        // (a bare `forEach(f)` has none, :82-84). A statement-position call
        // may tile without the marker row; the selector identifier from the
        // send parts stands in and the null-aware own-marker word defaults
        // to absent.
        let marker = slice::access_marker(c, &kids, pos, NodeKind::DotAccess);
        let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
            return;
        };
        let name = kids[head];
        // `node.methodName.token.value() == 'forEach'` (:82-84).
        if c.kind(name) != NodeKind::Identifier || c.lexeme(c.head_token(name)) != "forEach" {
            return;
        }
        // `arguments.isNotEmpty && arguments.first is FunctionExpression`
        // (:85-86): the argument list's subtree roots lead with the call's
        // type-argument wrapper (an empty `NoTypeVariables` row on the
        // common shape), so the first argument is the first root after it.
        let Some(arguments) = arguments else {
            return;
        };
        let raw = c.collect_direct(arguments);
        let closure = raw.iter().copied().find(|&root| {
            !matches!(
                c.kind(root),
                NodeKind::NoTypeVariables | NodeKind::TypeVariables
            )
        });
        let Some(closure) = closure.filter(|&root| c.kind(root) == NodeKind::FunctionExpression)
        else {
            return;
        };
        // The chain guards: no method chaining in the target (:31-43 — the
        // official bails unconditionally, so it runs before the receiver
        // face whose opaque presumption would over-fire the chained shape),
        // no null-aware access below (:87), not inside a cascade (:45-48).
        if has_method_chaining(c, &kids, head) {
            return;
        }
        if chain_has_null_aware(c, &kids, head, marker) {
            return;
        }
        // The receiver type check (:40-45).
        if !slice::receiver_implements_any(c, &kids, head, slice::ITERABLE) {
            return;
        }
        // The closure-arity stand-in for the opaque `Map.forEach` face: a
        // multi-parameter closure is never the flagged shape on valid code.
        // The count reads the flattened parameter rows; the empty group
        // (`()`, zero parameters — invalid for `forEach`) skips, an
        // invalid-code divergence from the official's type-only gate.
        let params = c
            .collect_direct(closure)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
        let arity = params.map(|params| {
            c.collect_direct(params)
                .iter()
                .filter(|&kid| c.kind(*kid) == NodeKind::FormalParameter)
                .count()
        });
        if arity != Some(1) {
            return;
        }
        if inside_cascade(c) {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }

    /// `containsNullAwareInvocationInChain` (:87): the invocation's own
    /// marker word, or any null-aware access over the receiver run.
    fn chain_has_null_aware(c: &Check<'_>, kids: &[u32], head: usize, marker: Option<u32>) -> bool {
        if marker.is_some_and(|marker| c.payload(marker).get(1).copied().unwrap_or(0) != 0) {
            return true;
        }
        kids[..head].iter().any(|&piece| {
            matches!(c.kind(piece), NodeKind::DotAccess | NodeKind::CascadeAccess)
                && c.payload(piece).get(1).copied().unwrap_or(0) != 0
        })
    }

    /// `_hasMethodChaining` (:31-43): the target carries an invocation — a
    /// `Send` piece in the receiver run preceded by an argument group (a
    /// chained call; the bare-name reads trail a `NoArguments`).
    fn has_method_chaining(c: &Check<'_>, kids: &[u32], head: usize) -> bool {
        kids[..head].iter().enumerate().any(|(i, &piece)| {
            c.kind(piece) == NodeKind::Send && i > 0 && c.kind(kids[i - 1]) == NodeKind::Arguments
        })
    }

    /// `_isInsideCascade` (:45-48): a cascade section owns the invocation.
    fn inside_cascade(c: &Check<'_>) -> bool {
        c.path.iter().any(|&a| c.kind(a) == NodeKind::Cascade)
    }
}

/// Same-file declaration facts for the rules that stand in for an
/// element-model face (`null_closures` type chains, `unnecessary_this`
/// shadow bail). Everything here is same-file only: a name that resolves to
/// another file stays `Unknown` and lands in the caller's documented
/// fallback family. `prefer_collection_literals` (collections.rs) reads the
/// same scan for its typedef/shadow exemptions.
pub(crate) mod same_file {
    use super::{Check, NodeKind};

    /// Collapse map §1 `ModSet` member family: static is bit 3 (`mod_bits` in
    /// the bridge; [`dartq_ast::mod_flags`] reads the header bits back).
    pub(crate) const STATIC_BIT: u16 = 1 << 3;

    /// One same-file class-like declaration: the name token, the written
    /// superclass-chain names (extends / mixin `on` / the `=` name of a
    /// mixin application), and the with + implements names.
    pub(crate) struct ClassDecl {
        name: u32,
        supers: Vec<u32>,
        clauses: Vec<u32>,
    }

    /// The unit's type declarations: class-like shapes plus typedef names
    /// (a local typedef shadows any imported name in the combined
    /// namespace).
    pub(crate) struct Decls {
        classes: Vec<ClassDecl>,
        typedefs: Vec<u32>,
    }

    /// Scans the unit once per call (top-level wrappers only — cheap, and
    /// the rules that need it run per candidate node).
    #[must_use]
    pub fn scan(c: &Check<'_>) -> Decls {
        let mut decls = Decls {
            classes: Vec::new(),
            typedefs: Vec::new(),
        };
        let Some(root) = c.path.first().copied() else {
            return decls;
        };
        for top in c.collect_direct(root) {
            // The wrapper's direct children: class-like families name
            // through a preceding Identifier sibling, so match every child
            // by kind instead of peeling a single "the declaration" child.
            for inner in c.collect_direct(top) {
                match c.kind(inner) {
                    NodeKind::Typedef => {
                        if let Some(name) = c
                            .collect_direct(inner)
                            .into_iter()
                            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                        {
                            decls.typedefs.push(c.head_token(name));
                        }
                    }
                    NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::NamedMixinApplication
                    | NodeKind::ExtensionTypeDeclaration
                    | NodeKind::EnumDeclaration => {
                        if let Some(decl) = class_decl(c, top, inner) {
                            decls.classes.push(decl);
                        }
                    }
                    _ => {}
                }
            }
        }
        decls
    }

    impl Decls {
        /// Whether a class-like declaration carries `name`.
        #[must_use]
        pub fn has_class(&self, c: &Check<'_>, name: &str) -> bool {
            self.classes.iter().any(|d| c.lexeme(d.name) == name)
        }

        /// Whether any type declaration (class-like or typedef) carries
        /// `name`.
        #[must_use]
        pub fn has_type(&self, c: &Check<'_>, name: &str) -> bool {
            self.has_class(c, name) || self.typedefs.iter().any(|&tok| c.lexeme(tok) == name)
        }
    }

    /// Whether the unit declares `name` in any namespace — types plus
    /// top-level values (functions, getters, variables). A local declaration
    /// shadows the imported face in every namespace, which is what the
    /// annotation rules (`library_annotations`, `provide_deprecation_message`)
    /// and `unnecessary_this` bail on.
    #[must_use]
    pub fn declares_name(c: &Check<'_>, decls: &Decls, name: &str) -> bool {
        if decls.has_type(c, name) {
            return true;
        }
        let Some(root) = c.path.first().copied() else {
            return false;
        };
        for top in c.collect_direct(root) {
            for inner in c.collect_direct(top) {
                match c.kind(inner) {
                    // Top-level function/getter/setter names (a written
                    // return type is a `TypeReference` identifier, filtered
                    // out here).
                    NodeKind::TopLevelMethod | NodeKind::TopLevelFields => {
                        let ctx = if c.kind(inner) == NodeKind::TopLevelMethod {
                            dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                        } else {
                            dartq_parser::IdentifierContext::TopLevelVariableDeclaration
                        } as u32;
                        if c.collect_direct(inner).into_iter().any(|kid| {
                            c.kind(kid) == NodeKind::Identifier
                                && c.payload(kid).first() == Some(&ctx)
                                && c.lexeme(c.head_token(kid)) == name
                        }) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
        false
    }

    /// How a written type name resolves over the same-file declarations.
    pub(crate) enum Verdict {
        /// The name (or a same-file supertype chain over it) lands on the
        /// core type carried in `Core` — an unshadowed written name is the
        /// library type.
        Core(&'static str),
        /// Provably not the core family: a same-file declaration on the
        /// chain shadows the core name, or the chain provably dead-ends.
        NotCore,
        /// A supertype (or the name itself) resolves outside the file — the
        /// caller's documented fallback family decides.
        Unknown,
    }

    /// Resolves `name` as an instance receiver: all written supertypes
    /// (extends / on / with / implements) walk — the official
    /// `element.allSupertypes` face (null_closures.dart:245-250). `core` is
    /// the set of written names the caller's table keys on.
    pub(crate) fn resolve<'a>(
        c: &Check<'a>,
        decls: &Decls,
        name: &'a str,
        core: &[&'static str],
    ) -> Verdict {
        let mut seen: Vec<&'a str> = Vec::new();
        walk(c, decls, name, false, core, &mut seen)
    }

    /// Resolves `name` as a creation receiver: the superclass chain only —
    /// the official `extendsClass` walk (extensions.dart:305-353).
    pub(crate) fn resolve_super<'a>(
        c: &Check<'a>,
        decls: &Decls,
        name: &'a str,
        core: &[&'static str],
    ) -> Verdict {
        let mut seen: Vec<&'a str> = Vec::new();
        walk(c, decls, name, true, core, &mut seen)
    }

    /// The exact-name verdict: the static-call face matches
    /// `element.name == function.type` with no supertype walk
    /// (null_closures.dart:224-233), so a same-file class is never the core
    /// element even when it lexically shares the name.
    #[must_use]
    pub fn direct(c: &Check<'_>, decls: &Decls, name: &str, core: &[&'static str]) -> Verdict {
        if decls.has_class(c, name) {
            return Verdict::NotCore;
        }
        verdict_of(core, name)
    }

    fn verdict_of(core: &[&'static str], name: &str) -> Verdict {
        core.iter()
            .find(|&&k| k == name)
            .map_or(Verdict::Unknown, |&k| Verdict::Core(k))
    }

    fn walk<'a>(
        c: &Check<'a>,
        decls: &Decls,
        name: &'a str,
        supers_only: bool,
        core: &[&'static str],
        seen: &mut Vec<&'a str>,
    ) -> Verdict {
        if seen.contains(&name) {
            // Cyclic same-file chains are erroneous code; provably no core
            // behind them.
            return Verdict::NotCore;
        }
        seen.push(name);
        let Some(decl) = decls.classes.iter().find(|d| c.lexeme(d.name) == name) else {
            // Not same-file: an unshadowed written core name is the library
            // type; anything else resolves outside the file.
            return verdict_of(core, name);
        };
        // A same-file declaration is never the core element — only its
        // written supertypes can reach the core family.
        let chain: Vec<u32> = if supers_only {
            decl.supers.clone()
        } else {
            decl.supers
                .iter()
                .chain(decl.clauses.iter())
                .copied()
                .collect()
        };
        let mut unknown = false;
        for token in chain {
            match walk(c, decls, c.lexeme(token), supers_only, core, seen) {
                Verdict::Core(core_name) => return Verdict::Core(core_name),
                Verdict::Unknown => unknown = true,
                Verdict::NotCore => {}
            }
        }
        if unknown {
            Verdict::Unknown
        } else {
            Verdict::NotCore
        }
    }

    /// The written type of the identifier `use` bound to `name`: the
    /// innermost local/parameter/field/top-level declaration carrying an
    /// explicit type. Untyped bindings (`var`, patterns, for-in) return
    /// `None` — the caller falls back.
    #[must_use]
    pub fn written_type<'a>(c: &Check<'a>, use_node: u32, name: &str) -> Option<&'a str> {
        let use_start = c.byte_span(use_node).start;
        let len = c.path.len();
        for i in (0..len.saturating_sub(1)).rev() {
            let ancestor = c.path[i];
            match c.kind(ancestor) {
                // The production tiling keeps a block's statements flat in
                // the owner slice — locals precede the use as pieces.
                NodeKind::Block | NodeKind::BlockBody => {
                    if let Some(t) = block_binding_type(c, ancestor, use_start, name) {
                        return Some(t);
                    }
                }
                NodeKind::FormalParameters => {
                    if let Some(t) = parameter_type(c, ancestor, name) {
                        return Some(t);
                    }
                }
                // Class-member fields and the member's own parameters: the
                // production tiling keeps both as owner-slice pieces of the
                // member. Order-independent — members scope whole.
                NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::PrimaryConstructorBody
                | NodeKind::TopLevelMethod
                | NodeKind::FunctionExpression
                | NodeKind::NamedFunctionExpression
                | NodeKind::LocalFunctionDeclaration
                    if i > 0 =>
                {
                    if let Some(params) = c
                        .collect_direct(ancestor)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                        && let Some(t) = parameter_type(c, params, name)
                    {
                        return Some(t);
                    }
                    if let Some(t) = field_type(c, c.path[i - 1], name) {
                        return Some(t);
                    }
                }
                NodeKind::CompilationUnit => {
                    if let Some(t) = top_level_field_type(c, ancestor, name) {
                        return Some(t);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The explicit type of a local declared in `block` before `use_start`.
    /// The production shape: the name identifier carries the local-variable
    /// context word inside its `VariablesDeclaration` slice, typed by the
    /// nearest preceding `Type` piece.
    fn block_binding_type<'a>(
        c: &Check<'a>,
        block: u32,
        use_start: usize,
        name: &str,
    ) -> Option<&'a str> {
        let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
        let mut written: Option<&'a str> = None;
        for kid in c.collect_direct(block) {
            match c.kind(kid) {
                NodeKind::Type => written = Some(c.lexeme(c.head_token(kid))),
                NodeKind::VariablesDeclaration => {
                    let binds = c.collect_direct(kid).into_iter().any(|var| {
                        c.kind(var) == NodeKind::Identifier
                            && c.payload(var).first() == Some(&ctx)
                            && c.lexeme(c.head_token(var)) == name
                            && c.byte_span(var).start < use_start
                    });
                    if binds {
                        return written;
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The explicit type of a formal parameter.
    fn parameter_type<'a>(c: &Check<'a>, params: u32, name: &str) -> Option<&'a str> {
        for kid in c.collect_direct(params) {
            if c.kind(kid) != NodeKind::FormalParameter {
                continue;
            }
            let named = c
                .payload(kid)
                .get(1)
                .copied()
                .filter(|&tok| tok != crate::rules::NONE_TOKEN)
                .is_some_and(|tok| c.lexeme(tok) == name);
            if !named {
                continue;
            }
            return c
                .collect_direct(kid)
                .into_iter()
                .find(|&t| c.kind(t) == NodeKind::Type)
                .map(|t| c.lexeme(c.head_token(t)));
        }
        None
    }

    /// The explicit type of an instance field declared in the owner body.
    fn field_type<'a>(c: &Check<'a>, owner: u32, name: &str) -> Option<&'a str> {
        for member in c.collect_direct(owner) {
            if c.kind(member) != NodeKind::Fields
                || dartq_ast::mod_flags(c.unit.ast, member) & STATIC_BIT != 0
            {
                continue;
            }
            if let Some(t) = fields_type(c, member, name) {
                return Some(t);
            }
        }
        None
    }

    /// The explicit type of a top-level variable.
    fn top_level_field_type<'a>(c: &Check<'a>, unit: u32, name: &str) -> Option<&'a str> {
        for top in c.collect_direct(unit) {
            for inner in c.collect_direct(top) {
                if c.kind(inner) == NodeKind::TopLevelFields
                    && let Some(t) = fields_type(c, inner, name)
                {
                    return Some(t);
                }
            }
        }
        None
    }

    /// Shared `Fields` scan: the name identifiers carry the field context
    /// (a written type is a `TypeReference` identifier), and the type head
    /// precedes each name among the children.
    fn fields_type<'a>(c: &Check<'a>, fields: u32, name: &str) -> Option<&'a str> {
        let ctx = if c.kind(fields) == NodeKind::TopLevelFields {
            dartq_parser::IdentifierContext::TopLevelVariableDeclaration as u32
        } else {
            dartq_parser::IdentifierContext::FieldDeclaration as u32
        };
        let mut written: Option<&'a str> = None;
        for kid in c.collect_direct(fields) {
            match c.kind(kid) {
                NodeKind::Type => written = Some(c.lexeme(c.head_token(kid))),
                NodeKind::Identifier
                    if c.payload(kid).first() == Some(&ctx)
                        && c.lexeme(c.head_token(kid)) == name =>
                {
                    return written;
                }
                _ => {}
            }
        }
        None
    }

    /// One class-like declaration's shape: the name token plus the clause
    /// name tokens (read off the header payload words as token bounds).
    fn class_decl(c: &Check<'_>, top: u32, inner: u32) -> Option<ClassDecl> {
        let payload = c.payload(inner);
        let none = crate::rules::NONE_TOKEN;
        let token_at = |word: usize| {
            payload
                .get(word)
                .copied()
                .unwrap_or(crate::rules::NONE_TOKEN)
        };
        // Extension types name through word 2; the other families through
        // the wrapper's first Identifier.
        let name = if c.kind(inner) == NodeKind::ExtensionTypeDeclaration {
            let tok = token_at(2);
            (tok != none).then_some(tok)?
        } else {
            let id = c
                .collect_direct(top)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)?;
            c.head_token(id)
        };
        let type_names = |lo: u32, hi: u32| -> Vec<u32> {
            c.collect_direct(inner)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
                .map(|kid| c.head_token(kid))
                .filter(|&tok| tok > lo && tok < hi)
                .collect()
        };
        let mut decl = ClassDecl {
            name,
            supers: Vec::new(),
            clauses: Vec::new(),
        };
        if c.kind(inner) == NodeKind::ExtensionTypeDeclaration {
            // The implements names are the direct type-name identifiers
            // (the representation type lives inside the primary
            // constructor's parameters).
            decl.clauses = type_names(0, u32::MAX);
            return Some(decl);
        }
        let (supers_kw, clauses) = match c.kind(inner) {
            // `class P = Q with R implements S` shares the class shape: Q
            // is the superclass.
            NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication => {
                (token_at(1), (token_at(2), token_at(3)))
            }
            // `mixin M on A with B`: the on-clause is a superclass
            // constraint; the with-clause an interface face.
            NodeKind::MixinDeclaration => (token_at(1), (token_at(2), none)),
            _ => return None,
        };
        if supers_kw != none {
            decl.supers = type_names(supers_kw, clauses.0.min(clauses.1).max(supers_kw + 1));
        }
        if clauses.0 != none {
            decl.clauses
                .extend(type_names(clauses.0, clauses.1.max(clauses.0 + 1)));
        }
        if clauses.1 != none {
            decl.clauses.extend(type_names(clauses.1, u32::MAX));
        }
        Some(decl)
    }
}

pub(crate) mod null_closures {
    use super::{Check, Group, NodeKind, RuleSpec, same_file, slice};

    /// Official: `pkg/linter/lib/src/rules/null_closures.dart` — a `null`
    /// passed in a closure position of a known core API reports at the
    /// argument. Three faces (:212-260): instance creations
    /// (`Future(null)`, `Timer.periodic(d, null)`, ...), static calls
    /// (`Future.wait(..., cleanUp: null)`, ...), and instance calls
    /// (`list.map(null)`, ...). Positions come from the fixed tables
    /// (:26-180); the report lands on the `null` literal, or on the whole
    /// named argument for the named face (:268-291).
    ///
    /// Type pinning (the :212-250 element faces over same-file facts):
    /// - creations (`extendsClass` walk, extensions.dart:305-353) resolve
    ///   the written type through the same-file superclass chain, so a user
    ///   subclass (`class MyFuture extends Future`, `MyFuture(null)`)
    ///   reports like the official and a same-file shadow (`class Future`,
    ///   then `Future(null)`) passes;
    /// - static calls match `element.name == function.type` exactly
    ///   (:224-233) — no supertype walk, the same-file class shadows;
    /// - instance calls resolve the receiver's written type (local,
    ///   parameter, field, top-level variable) through the same-file
    ///   all-supertypes chain (:245-250), and the method table filters on
    ///   the owning core class (`String.map` stays silent like the official
    ///   `_getInstanceMethod` miss).
    ///
    /// Fallback (documented over-report residue, the `prefer_contains`
    /// precedent): a receiver or type that resolves outside the file
    /// (imported class, `dynamic`, an unknown supertype) matches by written
    /// name — a user class reusing a core method name over-reports.
    /// `scheduleMicrotask` stays out: its table entry can never fire in the
    /// official either (`element.name == null`, :229-231). Message:
    /// `diagnostic.g.dart` `nullClosures`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "null_closures",
        group: Group::Recommended,
        problem: "Closure can't be 'null' because it might be invoked.",
        correction: "Try providing a non-null closure.",
        kinds: &[
            NodeKind::Send,
            NodeKind::ImplicitCreation,
            NodeKind::NewExpression,
        ],
        check,
    };

    /// The closure slots of one table entry: raw argument indexes (named
    /// arguments come last in legal Dart, so the raw index of a non-named
    /// argument is its positional index) and argument labels.
    type Slots = (&'static [usize], &'static [&'static str]);

    /// Creation face (`_constructorsWithNonNullableArguments`, :26-33);
    /// the unnamed-constructor entries key on the type name alone.
    const CONSTRUCTORS: &[((&str, Option<&str>), Slots)] = &[
        (("Future", None), (&[0], &[])),
        (("Future", Some("microtask")), (&[0], &[])),
        (("Future", Some("sync")), (&[0], &[])),
        (("Timer", None), (&[1], &[])),
        (("Timer", Some("periodic")), (&[1], &[])),
        (("List", Some("generate")), (&[1], &[])),
    ];

    /// Static face (`_staticFunctionsWithNonNullableArguments`, :172-178).
    const STATICS: &[(&str, &str, Slots)] = &[
        ("Future", "doWhile", (&[0], &[])),
        ("Future", "forEach", (&[1], &[])),
        ("Future", "wait", (&[], &["cleanUp"])),
        ("Timer", "run", (&[0], &[])),
    ];

    /// Instance face (`_instanceMethodsWithNonNullableArguments`, :35-170),
    /// keyed by method name with the core classes whose element carries it
    /// (the entry `type` values) plus the core subtypes that inherit it —
    /// the official `_getInstanceMethod` walks `allSupertypes` of the
    /// receiver's element (:245-250), so a `List` receiver reaches the
    /// `Iterable.map` entry.
    const INSTANCE_METHODS: &[(&str, &[&str], Slots)] = &[
        ("any", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
        ("complete", &["Future"], (&[0], &[])),
        ("every", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
        ("expand", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
        (
            "firstWhere",
            &["Iterable", "List", "Set", "Queue"],
            (&[0], &["orElse"]),
        ),
        (
            "forEach",
            &["Iterable", "List", "Set", "Queue", "Map"],
            (&[0], &[]),
        ),
        ("fold", &["Iterable", "List", "Set", "Queue"], (&[1], &[])),
        (
            "lastWhere",
            &["Iterable", "List", "Set", "Queue"],
            (&[0], &["orElse"]),
        ),
        ("map", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
        ("putIfAbsent", &["Map"], (&[1], &[])),
        ("reduce", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
        ("removeWhere", &["Queue", "List", "Set"], (&[0], &[])),
        ("replaceAllMapped", &["String"], (&[1], &[])),
        ("replaceFirstMapped", &["String"], (&[1], &[])),
        ("retainWhere", &["Queue", "List", "Set"], (&[0], &[])),
        (
            "singleWhere",
            &["Iterable", "List", "Set", "Queue"],
            (&[0], &["orElse"]),
        ),
        (
            "skipWhile",
            &["Iterable", "List", "Set", "Queue"],
            (&[0], &[]),
        ),
        (
            "splitMapJoin",
            &["String"],
            (&[], &["onMatch", "onNonMatch"]),
        ),
        (
            "takeWhile",
            &["Iterable", "List", "Set", "Queue"],
            (&[0], &[]),
        ),
        ("then", &["Future"], (&[0], &["onError"])),
        ("where", &["Iterable", "List", "Set", "Queue"], (&[0], &[])),
    ];

    /// The written names the tables key on (dart:async / dart:core /
    /// dart:collection members; unprefixed and same-file-unshadowed names
    /// are the library types).
    const CORE_TYPES: &[&str] = &[
        "Future", "Timer", "List", "Iterable", "Map", "Set", "Queue", "String",
    ];

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let decls = same_file::scan(c);
        // Which table matches, gated on the same-file type verdict:
        // `NotCore` bails (provably not the core API), `Unknown` falls
        // back to the written-name match (instance faces: silent).
        let face = match c.kind(node) {
            // The explicit-new shape owns its callee and argument list.
            NodeKind::NewExpression | NodeKind::ImplicitCreation => {
                let kids = c.collect_direct(node);
                let args = kids
                    .iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Arguments);
                args.zip(creation_slots(c, &decls, &kids))
            }
            // A plain send owns nothing — rebuild from the owner slice.
            NodeKind::Send => slice::owner_slice(c).and_then(|(kids, pos)| {
                let (head, args) = slice::send_parts(c, &kids, pos)?;
                // A `None` argument list is a function tear-off.
                send_slots(c, &decls, &kids, head, pos).and_then(|s| Some((args?, s)))
            }),
            _ => return,
        };
        let Some((args, (positions, named))) = face else {
            return;
        };
        report_nulls(c, args, positions, named);
    }

    /// The unnamed-creation face over an owned child list (`Future(null)`,
    /// `new Future(null)`): the callee is the identifier before the
    /// argument list — the explicit-new shape carries a
    /// `ConstructorReference` marker between them.
    fn creation_slots(c: &Check<'_>, decls: &same_file::Decls, kids: &[u32]) -> Option<Slots> {
        let args_pos = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::Arguments)?;
        let callee = kids[..args_pos]
            .iter()
            .rev()
            .find_map(|&kid| match c.kind(kid) {
                NodeKind::Identifier => Some(kid),
                // The explicit-new shape: the marker owns the callee name.
                NodeKind::ConstructorReference => c
                    .collect_direct(kid)
                    .into_iter()
                    .find(|&k| c.kind(k) == NodeKind::Identifier),
                _ => None,
            })?;
        unnamed_slots(c, decls, c.lexeme(c.head_token(callee)))
    }

    /// The unnamed-creation table hit for one written type name, gated on
    /// the same-file superclass chain (`extendsClass`, extensions.dart
    /// :305-353): a same-file class shadows its core namesake, a same-file
    /// subclass reports like the official.
    fn unnamed_slots(c: &Check<'_>, decls: &same_file::Decls, name: &str) -> Option<Slots> {
        match same_file::resolve_super(c, decls, name, CORE_TYPES) {
            same_file::Verdict::NotCore => None,
            same_file::Verdict::Core(core) => CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == core && entry.0.1.is_none())
                .map(|entry| entry.1),
            same_file::Verdict::Unknown => CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == name && entry.0.1.is_none())
                .map(|entry| entry.1),
        }
    }

    /// The send face over the owner slice with the invocation send at
    /// `pos` and its callee head at `head`: a trailing same-span
    /// `DotAccess` marker switches to the member faces.
    fn send_slots<'a>(
        c: &'a Check<'a>,
        decls: &same_file::Decls,
        kids: &[u32],
        head: usize,
        pos: usize,
    ) -> Option<Slots> {
        if c.kind(kids[head]) != NodeKind::Identifier {
            return None;
        }
        let member_face = slice::access_marker(c, kids, pos, NodeKind::DotAccess);
        match member_face {
            None => unnamed_slots(c, decls, c.lexeme(c.head_token(kids[head]))),
            Some(access) => member_slots(c, decls, kids, head, access),
        }
    }

    /// The member-access faces over the owner slice with the selector at
    /// `sel` and the `DotAccess` marker after the send: the receiver is
    /// the bare-identifier tear-off group ending just before the selector
    /// — anything else (a chained or computed receiver) bails
    /// (conservative; the official resolves it through the element
    /// model). The static and named-creation tables key on the written
    /// receiver name (`Timer.periodic` — a class reference); otherwise
    /// the instance face resolves the receiver's declared type (`xs.map`
    /// — a value), Core verdicts only.
    fn member_slots<'a>(
        c: &'a Check<'a>,
        decls: &same_file::Decls,
        kids: &[u32],
        sel: usize,
        _access: u32,
    ) -> Option<Slots> {
        let group_start = sel.checked_sub(4)?;
        if !slice::is_tear_off_group(c, kids, group_start) {
            return None;
        }
        let member = c.lexeme(c.head_token(kids[sel]));
        let written = c.lexeme(c.head_token(kids[group_start]));
        named_slots(c, decls, written, member).or_else(|| {
            let verdict = same_file::written_type(c, kids[group_start], written)
                .map_or(same_file::Verdict::NotCore, |ty| {
                    same_file::resolve(c, decls, ty, CORE_TYPES)
                });
            match verdict {
                same_file::Verdict::Core(core) => INSTANCE_METHODS
                    .iter()
                    .find(|entry| entry.0 == member && entry.1.contains(&core))
                    .map(|entry| entry.2),
                same_file::Verdict::NotCore | same_file::Verdict::Unknown => None,
            }
        })
    }

    /// `_checkNullArgForClosure` (:268-293): a `null` in a closure slot
    /// reports at the null literal, or over the whole named argument.
    /// The argument list owns its pieces: leading callee type-argument
    /// wrappers, then the arguments in order — a bare-identifier argument
    /// wraps in the four-piece tear-off group, and a named argument
    /// flattens as `name value… NamedArgument` (the marker anchors at the
    /// colon, so the name is the identifier ending right at it).
    fn report_nulls(c: &mut Check<'_>, arguments: u32, positions: &[usize], named: &[&str]) {
        let pieces = c.collect_direct(arguments);
        let mut consumed = vec![false; pieces.len()];
        for (j, &marker) in pieces.iter().enumerate() {
            if c.kind(marker) != NodeKind::NamedArgument {
                continue;
            }
            // The marker anchors at the value head: `name : value` are
            // three consecutive tokens, so the name is the identifier two
            // tokens back (`slice::named_arg_name`).
            let Some(name_idx) = slice::named_arg_name(c, &pieces, j) else {
                continue;
            };
            consumed[name_idx] = true;
            consumed[name_idx + 1..=j].fill(true);
            let value = &pieces[name_idx + 1..j];
            if value.len() == 1
                && c.kind(value[0]) == NodeKind::LiteralNull
                && named.contains(&c.lexeme(c.head_token(pieces[name_idx])))
            {
                // The marker's own span is empty (it anchors at the value
                // head) — the whole-argument range ends at the value.
                let start = c.utf16_span(pieces[name_idx]).start;
                let end = c.utf16_span(*value.last().unwrap_or(&marker)).end;
                c.report(start..end, &[]);
            }
        }
        let mut index = 0usize;
        let mut i = 0usize;
        while i < pieces.len() {
            if consumed[i] {
                i += 1;
                continue;
            }
            let tear_off = matches!(
                pieces.get(i..i + 4),
                Some([name, tyargs, no_args, send])
                    if c.kind(*name) == NodeKind::Identifier
                        && matches!(
                            c.kind(*tyargs),
                            NodeKind::NoTypeArguments | NodeKind::TypeArguments
                        )
                        && c.kind(*no_args) == NodeKind::NoArguments
                        && c.kind(*send) == NodeKind::Send
            );
            if c.kind(pieces[i]) == NodeKind::LiteralNull && positions.contains(&index) {
                let range = c.utf16_span(pieces[i]);
                c.report(range, &[]);
            }
            index += 1;
            i += if tear_off { 4 } else { 1 };
        }
    }

    /// The named-creation / static-call face keyed on the written name
    /// before the dot: the named-creation table first, then the static
    /// table (the official splits them on the target's element — a class
    /// element vs a top-level function; one written shape serves both).
    fn named_slots(
        c: &Check<'_>,
        decls: &same_file::Decls,
        class_name: &str,
        member_name: &str,
    ) -> Option<Slots> {
        let constructor = match same_file::resolve_super(c, decls, class_name, CORE_TYPES) {
            same_file::Verdict::NotCore => None,
            same_file::Verdict::Core(core) => CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == core && entry.0.1 == Some(member_name))
                .map(|entry| entry.1),
            same_file::Verdict::Unknown => CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == class_name && entry.0.1 == Some(member_name))
                .map(|entry| entry.1),
        };
        constructor.or_else(
            || match same_file::direct(c, decls, class_name, CORE_TYPES) {
                same_file::Verdict::NotCore => None,
                same_file::Verdict::Core(core) => STATICS
                    .iter()
                    .find(|entry| entry.0 == core && entry.1 == member_name)
                    .map(|entry| entry.2),
                same_file::Verdict::Unknown => STATICS
                    .iter()
                    .find(|entry| entry.0 == class_name && entry.1 == member_name)
                    .map(|entry| entry.2),
            },
        )
    }
}

pub(crate) mod unnecessary_this {
    use super::{Check, Group, NodeKind, RuleSpec, same_file};
    use crate::rules::NONE_TOKEN;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_this.dart` — a `this.`
    /// qualifier reports at the `this` keyword unless shadowing makes it
    /// necessary. Two faces (:37-45): a constructor field initializer's
    /// `this` reports unconditionally when written (:31-35 — `x = e;` is
    /// the bare form), and a `this` in a property access / method invocation
    /// reports when the unqualified name would resolve to the same member
    /// (:47-64).
    ///
    /// Decision order (the official `resolveNameInScope` + element identity,
    /// :66-99): locals, parameters, and inner declarations shadow (the
    /// `this.` stays — a scope hit that is not the member makes the
    /// qualified form necessary, :87-95); the name scope then covers the
    /// unit's own declarations AND unprefixed imports, so a same-name hit
    /// there bails; only with no scope hit (`isNone` — the member is
    /// inherited, or declared on the class itself and reachable through the
    /// instance interface) does the `this.` report.
    ///
    /// Same-file approximation: the unit's own declarations bail through
    /// [`same_file::declares_name`]; an unprefixed import bails when it can
    /// provide the name (no combinators, or the name inside `show` /
    /// outside `hide` — a combinatored import that provably excludes the
    /// name keeps reporting). The import bail under-reports the official
    /// face where the import's library does not actually export the name
    /// (the conservative family: element identity is not derivable here).
    /// Message: `diagnostic.g.dart` `unnecessaryThis`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_this",
        group: Group::Recommended,
        problem: "Unnecessary 'this.' qualifier.",
        correction: "Try removing 'this.'.",
        kinds: &[NodeKind::ThisExpression, NodeKind::Initializer],
        check,
    };

    fn check(c: &mut Check<'_>) {
        match c.kind(c.node()) {
            NodeKind::Initializer => check_field_initializer(c),
            NodeKind::ThisExpression => check_this(c),
            _ => {}
        }
    }

    /// `visitConstructorFieldInitializer` (:31-35): the written `this` on a
    /// field initializer always reports — the bare `x = e;` form exists.
    /// The `Initializer` node owns the `this` prefix (its span head; the
    /// `FieldInitializer` child begins at the `=`, collapse map §3.3 both
    /// rows).
    fn check_field_initializer(c: &mut Check<'_>) {
        if c.lexeme(c.head_token(c.node())) != "this" {
            return;
        }
        let start = c.utf16_span(c.node()).start;
        c.report(start..start, &[]);
    }

    /// `visitThisExpression` (:37-64): `this.name` accesses only. The
    /// production shape (bridge): a `this` owns no children — the access
    /// pieces tile flat into the enclosing owner as `this`, the selector,
    /// the tear-off wrappers, and the trailing `DotAccess` marker (same
    /// span, dot token + null-aware word).
    fn check_this(c: &mut Check<'_>) {
        let node = c.node();
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if c.kind(parent) == NodeKind::Initializer {
            // A constructor field initializer owns its report (:31-35).
            return;
        }
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == node) else {
            return;
        };
        let Some(&selector) = kids.get(pos + 1) else {
            return;
        };
        if c.kind(selector) != NodeKind::Identifier {
            return;
        }
        let access = kids[pos + 2..].iter().copied().find(|&kid| {
            c.kind(kid) == NodeKind::DotAccess
                && c.utf16_span(kid).start <= c.utf16_span(selector).start
                && c.utf16_span(kid).end >= c.utf16_span(selector).end
        });
        let Some(access) = access else {
            return;
        };
        // `!parent.isNullAware` (:49/:53) — the payload word is the
        // `is_null_aware` bool (collapse map §3.5 DotAccess row).
        if c.payload(access).get(1).copied().unwrap_or(0) != 0 {
            return;
        }
        let name_lexeme = c.lexeme(c.head_token(selector));
        if member_shadows(c, node, name_lexeme) {
            return;
        }
        // The name scope (:66-83): unprefixed imports and unit
        // declarations — a hit that is not the member bails.
        if import_may_shadow(c, name_lexeme) {
            return;
        }
        let decls = same_file::scan(c);
        if same_file::declares_name(c, &decls, name_lexeme) {
            return;
        }
        // No scope hit (`isNone`): the unqualified form resolves through
        // the instance interface — the enclosing class declares the member,
        // or it is inherited (:87-95 element identity).
        let start = c.utf16_span(node).start;
        c.report(start..start, &[]);
    }

    /// Whether an unprefixed import can provide `name` in the unqualified
    /// scope. A prefixed import never can; combinators narrow the face —
    /// `show` lists are authoritative (only listed names enter scope), and
    /// `hide` excludes the hidden names.
    fn import_may_shadow(c: &Check<'_>, name: &str) -> bool {
        let Some(root) = c.path.first().copied() else {
            return false;
        };
        for top in c.collect_direct(root) {
            for inner in c.collect_direct(top) {
                if c.kind(inner) != NodeKind::Import {
                    continue;
                }
                // The `as` prefix word (collapse map §3.1): present means
                // the names are reachable only prefixed.
                if c.payload(inner).first().copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN {
                    continue;
                }
                let mut shown = false;
                let mut hidden = false;
                for kid in c.collect_direct(inner) {
                    match c.kind(kid) {
                        NodeKind::Show => {
                            if combinator_lists(c, kid, name) {
                                // Authoritatively provided.
                                return true;
                            }
                            shown = true;
                        }
                        NodeKind::Hide => {
                            hidden = hidden || combinator_lists(c, kid, name);
                        }
                        _ => {}
                    }
                }
                if !shown && !hidden {
                    // An unfiltered import may export anything.
                    return true;
                }
            }
        }
        false
    }

    /// Whether a `show`/`hide` combinator lists `name`.
    fn combinator_lists(c: &Check<'_>, combinator: u32, name: &str) -> bool {
        c.collect_direct(combinator)
            .into_iter()
            .any(|kid| c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name)
    }

    /// Whether any declaration visible between the use and the enclosing
    /// member binds `name`. The walk stops at the member, after
    /// collecting its own parameters.
    fn member_shadows(c: &Check<'_>, use_node: u32, name: &str) -> bool {
        let use_start = c.byte_span(use_node).start;
        for &ancestor in c.path.iter().rev().skip(1) {
            match c.kind(ancestor) {
                // The member boundary: its parameters shadow, then stop.
                NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::TopLevelMethod
                | NodeKind::PrimaryConstructorBody => {
                    if collect_member_parameters(c, ancestor).contains(&name) {
                        return true;
                    }
                    return false;
                }
                // The production tiling keeps a block's statements flat in
                // the owner slice — locals precede the use as pieces.
                NodeKind::Block | NodeKind::BlockBody => {
                    if block_declares_before(c, ancestor, use_start, name) {
                        return true;
                    }
                }
                NodeKind::CatchClause | NodeKind::ForIn => {
                    if scope_declares(c, ancestor, name) {
                        return true;
                    }
                }
                NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression => {
                    // Parameters scope the whole body regardless of position.
                    if collect_member_parameters(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                NodeKind::LocalFunctionDeclaration => {
                    // The function name binds from its declaration on; the
                    // parameters scope the whole body.
                    let names_decl = crate::rules::member_name(c, ancestor).is_some_and(|decl| {
                        c.lexeme(c.head_token(decl)) == name && c.byte_span(decl).start < use_start
                    });
                    if names_decl || collect_member_parameters(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Local declarations of a block that precede the use. The production
    /// shape: the name identifier carries the local-variable context word
    /// inside its `VariablesDeclaration` slice; pattern bindings keep their
    /// statement node.
    fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
        let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
        for decl in c.collect_direct(block) {
            match c.kind(decl) {
                NodeKind::VariablesDeclaration => {
                    if c.collect_direct(decl).into_iter().any(|kid| {
                        c.kind(kid) == NodeKind::Identifier
                            && c.payload(kid).first() == Some(&ctx)
                            && c.lexeme(c.head_token(kid)) == name
                            && c.byte_span(kid).start < use_start
                    }) {
                        return true;
                    }
                }
                NodeKind::PatternVarDeclStatement => {
                    let window = c.children(decl);
                    for id in window.start..window.end {
                        if c.kind(id) == NodeKind::Identifier
                            && c.lexeme(c.head_token(id)) == name
                            && c.byte_span(id).start < use_start
                        {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Any name binding directly inside a catch clause / for-in header.
    fn scope_declares(c: &Check<'_>, scope: u32, name: &str) -> bool {
        let window = c.children(scope);
        for id in window.start..window.end {
            if c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == name {
                return true;
            }
        }
        false
    }

    /// The parameter names of a member or closure (the `FormalParameters`
    /// child's `FormalParameter` name words).
    fn collect_member_parameters<'a>(c: &'a Check<'a>, member: u32) -> Vec<&'a str> {
        let Some(params) = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        else {
            return Vec::new();
        };
        c.collect_direct(params)
            .into_iter()
            .filter(|&param| c.kind(param) == NodeKind::FormalParameter)
            .filter_map(|param| {
                let name = c.payload(param).get(1).copied()?;
                (name != NONE_TOKEN).then(|| c.lexeme(name))
            })
            .collect()
    }
}

pub(crate) mod prefer_interpolation_to_compose_strings {
    use super::{Check, Group, NodeKind, RuleSpec, slice};

    /// Official: `pkg/linter/lib/src/rules/prefer_interpolation_to_compose_strings.dart:32-88`
    /// — an adjacent `(literal, value)` pair in a `+` chain reports over the
    /// pair span.
    ///
    /// The left-operand String type check
    /// (`leftOperand.staticType.isDartCoreString`, :85) reads through the
    /// shared literal face ([`slice::literal_faces`]) — a string literal
    /// operand decides, a provably non-string literal (a record) passes, and
    /// an opaque operand keeps the documented presumption (a user `operator+`
    /// class over-reports). Chains report once at the chain root; the
    /// official's per-node re-visit would re-report inner pairs. The operand
    /// units are the chain's piece runs closed at expression tails; a
    /// multi-piece operand without an invocation tail (`x.y + 'a'`) splits
    /// piece-wise — a documented divergence affecting the report span only.
    /// Raw strings and both-literal pairs pass verbatim (:70-75); a
    /// `toString` invocation with arguments passes (:78-82). Message:
    /// `diagnostic.g.dart` `preferInterpolationToComposeStrings`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_interpolation_to_compose_strings",
        group: Group::Recommended,
        problem: "Use interpolation to compose strings and values.",
        correction: "Try using string interpolation to build the composite string.",
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Only the chain root flattens (the official `chainedAdditions` walk
        // re-reports from every inner node; once per chain is the anchor).
        if c.lexeme(c.payload(node).first().copied().expect("binary op")) != "+" {
            return;
        }
        let is_inner_plus = c.ancestor(1).is_some_and(|parent| {
            c.kind(parent) == NodeKind::BinaryExpression
                && c.lexeme(c.payload(parent).first().copied().expect("binary op")) == "+"
        });
        if is_inner_plus {
            return;
        }
        // The chain root's operand runs read through the adopted window
        // ([`slice::binary_operands`]); nested `+` binaries descend into
        // their own child roots.
        let Some((left, right)) = slice::binary_operands(c) else {
            return;
        };
        let mut pieces = Vec::with_capacity(left.len() + right.len());
        for &kid in &left {
            if c.kind(kid) == NodeKind::BinaryExpression
                && c.lexeme(c.payload(kid).first().copied().expect("binary op")) == "+"
            {
                chain_pieces(c, kid, &mut pieces);
            } else {
                pieces.push(kid);
            }
        }
        pieces.extend_from_slice(&right);
        let units = operand_units(c, &pieces);
        let mut skip_next = false;
        for pair in units.windows(2) {
            let (left, right) = (&pieces[pair[0].clone()], &pieces[pair[1].clone()]);
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
            // OK(#2490): a raw string on either side.
            if (left_literal && is_raw(c, left[0])) || (right_literal && is_raw(c, right[0])) {
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
            // opaque face presumes.
            let string_left = match slice::literal_faces(c, left[0]) {
                Some(faces) => faces & slice::STRING != 0,
                None => true,
            };
            if !string_left {
                continue;
            }
            // The chain root's head anchors the report (the official
            // re-reports inner pairs at the same span).
            let start = c.utf16_span(pieces[0]).start;
            let end = c.utf16_span(*right.last().unwrap_or(&right[0])).end;
            c.report(start..end, &[]);
            // The official skips the pair consuming the reported right
            // operand (:86-88).
            skip_next = true;
        }
    }

    /// The flattened `+` chain (`chainedAdditions`, :90-98): the recursion
    /// descends into left-nested `+` binaries; their operand pieces collect
    /// in order.
    fn chain_pieces(c: &Check<'_>, node: u32, out: &mut Vec<u32>) {
        for kid in c.collect_direct(node) {
            if c.kind(kid) == NodeKind::BinaryExpression
                && c.lexeme(c.payload(kid).first().copied().expect("binary op")) == "+"
            {
                chain_pieces(c, kid, out);
            } else {
                out.push(kid);
            }
        }
    }

    /// The operand units over the chain's flat pieces: a unit is the maximal
    /// piece run closed by an expression tail — a `Send`, an access/postfix
    /// marker, or a single-piece expression. The glue pieces (identifiers,
    /// type-argument wrappers, argument groups, prefixes) extend the open
    /// run; an access marker after a closed run merges into it (the marker
    /// tails the invocation it closes, `a.toString(x)`), or opens its own.
    fn operand_units(c: &Check<'_>, pieces: &[u32]) -> Vec<core::ops::Range<usize>> {
        let mut units = Vec::new();
        let mut start: Option<usize> = None;
        for (i, &piece) in pieces.iter().enumerate() {
            match c.kind(piece) {
                NodeKind::Identifier
                | NodeKind::NoTypeArguments
                | NodeKind::TypeArguments
                | NodeKind::Arguments
                | NodeKind::NoArguments
                | NodeKind::UnaryPrefix => {
                    start.get_or_insert(i);
                }
                NodeKind::DotAccess | NodeKind::CascadeAccess => match start.take() {
                    Some(from) => units.push(from..i + 1),
                    None => match units.last_mut() {
                        Some(last) => last.end = i + 1,
                        None => units.push(i..i + 1),
                    },
                },
                _ => {
                    let from = start.take().unwrap_or(i);
                    units.push(from..i + 1);
                }
            }
        }
        units
    }

    /// `StringLiteral` operand: a single-piece unit (adjacent strings tile
    /// as siblings, one unit each).
    fn is_string_literal(c: &Check<'_>, unit: &[u32]) -> bool {
        unit.len() == 1 && c.kind(unit[0]) == NodeKind::StringLiteral
    }

    /// `SimpleStringLiteral.isRaw`: the literal opens with `r`.
    fn is_raw(c: &Check<'_>, literal: u32) -> bool {
        c.lexeme(c.head_token(literal)).starts_with('r')
    }

    /// `isToStringInvocationWithArguments`: a unit carrying a `toString`
    /// call with a non-empty argument list.
    fn is_to_string_with_arguments(c: &Check<'_>, unit: &[u32]) -> bool {
        let Some(send) = unit.iter().position(|&p| c.kind(p) == NodeKind::Send) else {
            return false;
        };
        let Some((head, arguments)) = slice::send_parts(c, unit, send) else {
            return false;
        };
        if c.kind(unit[head]) != NodeKind::Identifier
            || c.lexeme(c.head_token(unit[head])) != "toString"
        {
            return false;
        }
        arguments.is_some_and(|args| !c.collect_direct(args).is_empty())
    }
}

pub(crate) mod prefer_final_fields {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// Official: `pkg/linter/lib/src/rules/prefer_final_fields.dart` — a
    /// private non-`final` field never mutated reports when it is
    /// initialized at its declaration or set by every constructor of its
    /// class (field formals and field initializers count as setting;
    /// :52-84). The mutated set is library-wide and collected before the
    /// decision (`afterLibrary`, :85-112); the report sits on the variable
    /// declaration. Message: `diagnostic.g.dart` `preferFinalFields`.
    ///
    /// Approximations (documented — the official runs on resolved elements
    /// across the whole library; this is one unit, name-level):
    /// - Mutations are bare-name writes: assignment targets (any operator)
    ///   and `++`/`--` operands. Pattern-assignment writes (`(_x,) = ...`)
    ///   and cascade-section writes (`.._x = 1`) are not scanned, so a field
    ///   mutated only there over-reports; a same-named LOCAL mutated
    ///   anywhere marks the field mutated and under-reports. Index writes
    ///   (`_x[0] = 1`) do not count — matching the official, whose
    ///   `writeElement` for them is the `[]=` method, not the field.
    /// - Private-ness is the leading underscore; the setter-override
    ///   exemption (`_overridesField`, :147-158) is element-bound and
    ///   skipped, so a field overriding a superclass setter over-reports.
    /// - The cross-unit half of the official library scan
    ///   (`_fieldsFromParameters` — primary-constructor declaring formals,
    ///   :114-131) is out of the substrate and under-reports.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_final_fields",
        group: Group::Recommended,
        problem: "The private field {0} could be 'final'.",
        correction: "Try making the field 'final'.",
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    /// One candidate: the declaration node, its name, whether the
    /// declaration carries an initializer, and the enclosing class row (the
    /// constructor set faces scan its completion window).
    struct Candidate<'a> {
        declaration: u32,
        name: &'a str,
        has_initializer: bool,
        class_row: u32,
    }

    fn check(c: &mut Check<'_>) {
        let Some(root) = c.unit.ast.root() else {
            return;
        };
        let mut candidates: Vec<Candidate<'_>> = Vec::new();
        let mut mutated: Vec<&str> = Vec::new();
        scan(c, root, None, &mut candidates, &mut mutated);
        // A field row reachable from both the class row and a body
        // container yields the candidate twice; keep one.
        candidates.sort_by_key(|candidate| candidate.declaration);
        candidates.dedup_by_key(|candidate| candidate.declaration);
        for candidate in &candidates {
            if mutated.contains(&candidate.name) {
                continue;
            }
            // The constructors of the field's own class (:62-71). The class
            // subtree occupies the contiguous row range ending at the class
            // row, so the constructor set reads from that range under every
            // tiling layout. Factories are `ConstructorDeclaration`s in the
            // official and count for the "set in every constructor"
            // question; they never set.
            let members = c.children(candidate.class_row);
            let constructors: Vec<u32> = (members.start..=candidate.class_row)
                .filter(|&id| matches!(c.kind(id), NodeKind::Constructor | NodeKind::Factory))
                .collect();
            let set = |ctor: u32| sets_field(c, ctor, candidate.name);
            let set_any = constructors.iter().copied().any(set);
            let set_all = constructors.iter().copied().all(set);
            if !(set_any && set_all) && !candidate.has_initializer {
                continue;
            }
            let range = c.utf16_span(candidate.declaration);
            c.report(range, &[candidate.name]);
        }
    }

    /// The one-pass unit walk: candidates from `Fields` declarations and
    /// mutations from bare-name writes. Enum and extension-type bodies are
    /// skipped whole (the official excludes their fields, :98/:118-121);
    /// top-level declarations are not fields and never match.
    fn scan<'a>(
        c: &Check<'a>,
        node: u32,
        class_row: Option<u32>,
        candidates: &mut Vec<Candidate<'a>>,
        mutated: &mut Vec<&'a str>,
    ) {
        match c.kind(node) {
            NodeKind::EnumDeclaration | NodeKind::ExtensionTypeDeclaration => return,
            NodeKind::ClassDeclaration => {
                // Member discovery anchors on the class row: its completion
                // window opens at the head keyword and covers every member
                // row, while the body container's window can open later and
                // the derived parent pointer can link a field row to either
                // neighbor. Walking the class row's window is correct under
                // both layouts.
                for kid in c.collect_direct(node) {
                    scan(c, kid, Some(node), candidates, mutated);
                }
                return;
            }
            NodeKind::DeclarationBody => {
                // A body container between the class row and its members
                // owns rows in either its completion window or its
                // derived-child set depending on layout; both faces are
                // cheap and the candidate set dedupes by declaration.
                let mut kids = c.collect_direct(node);
                kids.extend(crate::rules::rows_with_parent(c, node));
                for kid in kids {
                    scan(c, kid, class_row, candidates, mutated);
                }
                return;
            }
            NodeKind::Fields => {
                for declaration in c.collect_direct(node) {
                    if c.kind(declaration) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    if let Some(kw) = crate::rules::variables_declaration_keyword(c, declaration)
                        && matches!(c.lexeme(kw), "final" | "const")
                    {
                        continue;
                    }
                    let Some(class_row) = class_row else { continue };
                    for var in c.collect_direct(declaration) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        // The production row is width-0 at the name, so the
                        // name is the span head; hand-built tables keep the
                        // same head (see
                        // [`crate::rules::initialized_identifier_name`]).
                        let Some(name_token) = crate::rules::initialized_identifier_name(c, var)
                        else {
                            continue;
                        };
                        let name = c.lexeme(name_token);
                        if !name.starts_with('_') {
                            continue;
                        }
                        // The row ends past its head exactly when an
                        // initializer followed (`end_last` adopts the
                        // initializer's value end; a bare name ends at the
                        // name itself).
                        let has_initializer = c.head_token(var) != crate::rules::tail_token(c, var);
                        candidates.push(Candidate {
                            declaration: var,
                            name,
                            has_initializer,
                            class_row,
                        });
                    }
                }
            }
            NodeKind::Assignment => {
                if let Some(&lhs) = c.collect_direct(node).first()
                    && c.kind(lhs) == NodeKind::Identifier
                {
                    mutated.push(c.lexeme(c.head_token(lhs)));
                }
            }
            NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => {
                let op = c.lexeme(c.head_token(node));
                if op == "++" || op == "--" {
                    let Some(parent) = c.unit.ast.parent(node) else {
                        return;
                    };
                    let siblings = c.collect_direct(parent);
                    let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
                        return;
                    };
                    // The prefix operator parses before its operand, the
                    // postfix operator after it.
                    let operand = match c.kind(node) {
                        NodeKind::UnaryPrefix => siblings.get(pos + 1),
                        _ => pos.checked_sub(1).and_then(|i| siblings.get(i)),
                    };
                    if let Some(&operand) = operand
                        && c.kind(operand) == NodeKind::Identifier
                    {
                        mutated.push(c.lexeme(c.head_token(operand)));
                    }
                }
            }
            _ => {}
        }
        for kid in c.collect_direct(node) {
            scan(c, kid, class_row, candidates, mutated);
        }
    }

    /// Whether `ctor` sets `name`: a `this.name` field formal or a field
    /// initializer of the name (:129-146).
    fn sets_field(c: &Check<'_>, ctor: u32, name: &str) -> bool {
        let window = c.children(ctor);
        (window.start..window.end).any(|id| match c.kind(id) {
            NodeKind::FormalParameter => {
                let receiver = c.payload(id).first().copied().unwrap_or(NONE_TOKEN);
                receiver != NONE_TOKEN
                    && c.lexeme(receiver) == "this"
                    && c.payload(id)
                        .get(1)
                        .is_some_and(|&name_token| c.lexeme(name_token) == name)
            }
            NodeKind::Initializer => initializer_set_name(c, id) == Some(name),
            _ => false,
        })
    }

    /// The field name an initializer entry sets: the entry head token is the
    /// bare form's name; `this.x = e` names two tokens later.
    fn initializer_set_name<'a>(c: &'a Check<'a>, init: u32) -> Option<&'a str> {
        let head = c.head_token(init);
        let name_token = match c.lexeme(head) {
            "this" => head + 2,
            _ => head,
        };
        let token = c.unit.tokens.get(name_token as usize)?;
        (token.kind == dartq_scanner::TokenKind::Identifier).then(|| c.lexeme(name_token))
    }
}

pub(crate) mod await_only_futures {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::utf16_token_span;

    /// Official: `pkg/linter/lib/src/rules/await_only_futures.dart:38-56` —
    /// `await` on an expression whose static type is not assignable to
    /// `Future` reports at the `await` keyword, with the type as the
    /// argument (:55).
    ///
    /// Approximation (documented): the static-type face needs inference.
    /// The ported face is the syntax-knowable slice: `await` on a scalar
    /// literal (int / double / String / bool — the exact types), reading
    /// through parenthesized expressions. `await null` never reports
    /// (:39), unknown expressions (identifiers, calls, collections —
    /// inference-bound) never report, the dominant under-report face. The
    /// Dynamic/FutureOr/extension-type exemptions (:42-46) are unreachable
    /// on the ported face. Message: `diagnostic.g.dart` `awaitOnlyFutures`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "await_only_futures",
        group: Group::Core,
        problem: "Uses 'await' on an instance of '{0}', which is not a subtype of 'Future'.",
        correction: "Try removing the 'await' or changing the expression.",
        kinds: &[NodeKind::AwaitExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The awaited expression tiles inside the container's window; its
        // root is the last row (a container completes after its subtree).
        let window = c.children(node);
        if window.start >= window.end {
            return;
        }
        let mut root = window.end - 1;
        // Read through transparent `(...)` wrappers (the content is the
        // wrapper window's last row).
        while c.kind(root) == NodeKind::ParenthesizedExpression {
            let inner = c.children(root);
            if inner.start >= inner.end {
                return;
            }
            root = inner.end - 1;
        }
        let Some(type_name) = scalar_literal_type(c, root) else {
            return;
        };
        // The `await` keyword is the width-0 kind's span head.
        let range = utf16_token_span(c, c.head_token(node));
        c.report(range, &[type_name]);
    }

    /// The exact static type of a scalar literal (`int`, `double`,
    /// `String`, `bool`); `None` when the syntax carries no verdict.
    /// `await null` skips upstream (official :39).
    fn scalar_literal_type(c: &Check<'_>, row: u32) -> Option<&'static str> {
        match c.kind(row) {
            NodeKind::LiteralInt => Some("int"),
            NodeKind::LiteralDouble => Some("double"),
            NodeKind::StringLiteral => Some("String"),
            NodeKind::LiteralBool => Some("bool"),
            _ => None,
        }
    }
}

pub(crate) mod unrelated_type_equality_checks {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{slice, utf16_token_span};

    /// Official: `pkg/linter/lib/src/rules/unrelated_type_equality_checks.dart`
    /// — an `==`/`!=` comparison (a bool-typed one, :47-49) whose operand
    /// types are unrelated reports at the operator, with `(right type,
    /// left type)` (:62-66; the pattern face :69-83 needs the matched
    /// value type — unported).
    ///
    /// Approximations (documented): the operand types are the syntax-
    /// knowable slice — scalar literals (`int`, `double`, `String`,
    /// `bool`) and `this` (the enclosing class's name). Both operands must
    /// carry a face; anything else (identifiers, calls) stays silent. The
    /// comparability relation (`typesAreUnrelated`) reduces on this face
    /// to exact-name equality — no two distinct names in the ported set
    /// stand in a subtype relation (`int`/`double` are unrelated exactly
    /// as the official rule judges them; `num`/`Object` are unreachable as
    /// literal types). `null` operands skip (:53-55). A user class may
    /// override `==` returning non-`bool` — the official bool-typed gate
    /// (:47) then silences where this face reports (over-report, the
    /// user-shadowed family). Message: `diagnostic.g.dart`
    /// `unrelatedTypeEqualityChecksInExpression`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unrelated_type_equality_checks",
        group: Group::Core,
        problem: "The type of the right operand ('{0}') isn't a subtype or a supertype of the left operand ('{1}').",
        correction: "Try changing one or both of the operands.",
        kinds: &[NodeKind::BinaryExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(op) = c.payload(node).first().copied() else {
            return;
        };
        if !matches!(c.lexeme(op), "==" | "!=") {
            return;
        }
        // The operand runs of the flattened container (:52-59).
        let Some((left, right)) = slice::binary_operands(c) else {
            return;
        };
        let left_type = operand_type(c, &left).map(str::to_owned);
        let right_type = operand_type(c, &right).map(str::to_owned);
        let (Some(left_type), Some(right_type)) = (left_type, right_type) else {
            return;
        };
        if left_type == right_type {
            return; // `_comparable` on the ported face
        }
        let range = utf16_token_span(c, op);
        c.report(range, &[right_type.as_str(), left_type.as_str()]);
    }

    /// The operand's type face: a scalar literal or `this` (the enclosing
    /// class-like name); `null` literals answer `Some("Null")` so the
    /// caller's equality gate treats them as reported faces — except the
    /// official skips them outright (:53-55), enforced here by `None`
    /// being indistinguishable… they are excluded explicitly.
    fn operand_type<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
        let [root] = rows else {
            return None; // a multi-row operand is a chain — no face
        };
        match c.kind(*root) {
            NodeKind::LiteralInt => Some("int"),
            NodeKind::LiteralDouble => Some("double"),
            NodeKind::StringLiteral => Some("String"),
            NodeKind::LiteralBool => Some("bool"),
            NodeKind::LiteralNull => None, // the official skips null operands
            NodeKind::ThisExpression => {
                let class_row = c.path.iter().rev().find(|&&a| {
                    matches!(
                        c.kind(a),
                        NodeKind::ClassDeclaration
                            | NodeKind::MixinDeclaration
                            | NodeKind::EnumDeclaration
                            | NodeKind::ExtensionDeclaration
                    )
                })?;
                let name = c
                    .collect_direct(*class_row)
                    .into_iter()
                    .find(|&kid| {
                        c.kind(kid) == NodeKind::Identifier
                            && c.payload(kid).first()
                                == Some(&(dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32))
                    })?;
                Some(c.lexeme(c.head_token(name)))
            }
            _ => None,
        }
    }
}

pub(crate) mod void_checks {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{slice, type_face_of_field};
    /// Official: `pkg/linter/lib/src/rules/void_checks.dart` — a value
    /// written where `void` is expected reports (:141-173, the
    /// `_check` core). The ported face is the assignment branch
    /// (:76-84): assigning to a `void`-typed target, reported at the
    /// assignment (:156-158).
    ///
    /// Approximations (documented): the target type is the same-class
    /// field's written type, read off `this.<field>` (bare `<field>`
    /// inside instance scope over-reports under shadowing — the user-
    /// shadowed family). The right side's type is the syntax-knowable
    /// scalar-literal face; unknown right sides never report (under-
    /// report). `null` and dynamic right sides are acceptable when
    /// expecting void (:55-65) — `null` skips. The `FutureOr<void>`
    /// target branch (:158-161) ports through written `FutureOr<void>`
    /// fields with the same literal faces (dynamic/null/`Future<void>`
    /// are acceptable — the literals never are). The return, pattern,
    /// argument and instance-creation branches (:67-103, :175-187) need
    /// resolved types and stay unported. Message: `diagnostic.g.dart`
    /// `voidChecks`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "void_checks",
        group: Group::Core,
        problem: "Assignment to a variable of type 'void'.",
        correction: "Try removing the assignment or changing the type of the variable.",
        kinds: &[NodeKind::Assignment],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(op) = c.payload(node).first().copied() else {
            return;
        };
        if c.lexeme(op) != "=" {
            return;
        }
        let Some((left, right)) = slice::binary_operands(c) else {
            return;
        };
        // The target: `this.<field>` or bare `<field>` (instance scope).
        let name = field_name_of(c, &left);
        let Some(name) = name else {
            return;
        };
        let Some(face) = type_face_of_field(c, name) else {
            return;
        };
        // `isTypeAcceptableWhenExpectingVoid` (:55-65) on the literal
        // faces: `null` is acceptable, the scalar literals are not.
        let right_type = literal_null_or_scalar(c, &right);
        let Some(right_type) = right_type else {
            return;
        };
        let mut void_target = face.is_void;
        if face.is_future_or_void {
            // `isTypeAcceptableWhenExpectingFutureOrVoid` (:40-53):
            // dynamic/void-acceptable pass; the literals never do.
            void_target = true;
        }
        if !void_target {
            return;
        }
        if right_type == "Null" {
            return; // acceptable when expecting void (:57)
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// The assigned name off the left run: `this.<field>` or a bare
    /// `<field>`. The run reads through the access-run form (the access
    /// markers and the bare-name send group tile beside the chain rows).
    fn field_name_of<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
        match slice::access_run(c, rows).as_slice() {
            [single] if c.kind(*single) == NodeKind::Identifier => {
                Some(c.lexeme(c.head_token(*single)))
            }
            [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
                Some(c.lexeme(c.head_token(*name)))
            }
            _ => None,
        }
    }

    /// The right side's face: `Null` for a null literal, the scalar type
    /// for scalar literals, `None` (no verdict) otherwise.
    fn literal_null_or_scalar<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
        let run = slice::access_run(c, rows);
        let [root] = run.as_slice() else {
            return None;
        };
        match c.kind(*root) {
            NodeKind::LiteralNull => Some("Null"),
            NodeKind::LiteralInt => Some("int"),
            NodeKind::LiteralDouble => Some("double"),
            NodeKind::StringLiteral => Some("String"),
            NodeKind::LiteralBool => Some("bool"),
            _ => None,
        }
    }
}

pub(crate) mod null_check_on_nullable_type_parameter {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::utf16_token_span;

    /// Official: `pkg/linter/lib/src/rules/null_check_on_nullable_type_parameter.dart`
    /// — a `!` whose operand's static type is a nullable type parameter
    /// `T?`, in a context whose expected type is that same `T?`, reports
    /// at the operator (:55-67).
    ///
    /// Approximation (documented): the operand face is `this.<field>` or
    /// bare `<field>` where the same-class field is written `T?` with `T`
    /// a class type parameter. The context face (`getExpectedType`,
    /// unnecessary_null_checks.dart) ports for the two syntax-knowable
    /// sites: a `return` inside a method whose written return type is
    /// `T?`, and a variable initializer whose declaration is written
    /// `T?`. Other contexts (arguments, assignments) stay silent
    /// (under-report). The pattern face (:48-52 — `matchedValueType`)
    /// stays unported. Message: `diagnostic.g.dart`
    /// `nullCheckOnNullableTypeParameter`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "null_check_on_nullable_type_parameter",
        group: Group::Core,
        problem: "The null check operator shouldn't be used on a variable whose type is a potentially nullable type parameter.",
        correction: "Try explicitly testing for 'null'.",
        // The `!` tiles as the `NonNullAssert` token leaf (bridge
        // `handle_non_null_assert_expression`); `UnaryPostfix` is `++`/`--`.
        kinds: &[NodeKind::NonNullAssert],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.operator.type != BANG` bails (:56).
        if c.lexeme(c.head_token(node)) != "!" {
            return;
        }
        // The operand rows precede the leaf (the owner-slice idiom).
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == node) else {
            return;
        };
        let name = operand_field_name(c, &kids[..pos]);
        let Some(name) = name else {
            return;
        };
        if !field_is_nullable_type_parameter(c, name) {
            return;
        }
        if !context_expects_same_nullable(c, name) {
            return;
        }
        let range = utf16_token_span(c, c.head_token(node));
        c.report(range, &[]);
    }

    /// The operand's field name: `this.<field>` or bare `<field>`, read
    /// through the access-run form (the bare-name send group tiles beside
    /// the chain rows inside the statement container).
    fn operand_field_name<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
        match crate::rules::slice::access_run(c, rows).as_slice() {
            [single] if c.kind(*single) == NodeKind::Identifier => {
                Some(c.lexeme(c.head_token(*single)))
            }
            [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
                Some(c.lexeme(c.head_token(*name)))
            }
            _ => None,
        }
    }

    /// Whether the same-class field is written `T?` with `T` a type
    /// parameter of the enclosing class.
    fn field_is_nullable_type_parameter(c: &Check<'_>, name: &str) -> bool {
        let Some(class_row) = c
            .path
            .iter()
            .rev()
            .find_map(|&a| matches!(c.kind(a), NodeKind::ClassDeclaration).then_some(a))
        else {
            return false;
        };
        let type_parameters = class_type_parameter_names(c, class_row);
        let Some(face) = crate::rules::type_face_of_field(c, name) else {
            return false;
        };
        face.nullable
            && face
                .name
                .is_some_and(|t| type_parameters.iter().any(|p| *p == t))
    }

    /// The enclosing class's type parameter names. Production tiling: the
    /// `TypeVariables` row is the `ClassDeclaration`'s left sibling under
    /// the `TopLevelDeclaration` wrapper, and each name `Identifier`
    /// (context `TypeVariableDeclaration`) tiles beside its span-only
    /// `TypeVariable` row; hand-built trees nest both inside the class row.
    fn class_type_parameter_names<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<&'a str> {
        let owner = c.unit.ast.parent(class_row).unwrap_or(class_row);
        let variables = c
            .collect_direct(owner)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            .or_else(|| {
                c.collect_direct(class_row)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            });
        let Some(variables) = variables else {
            return Vec::new();
        };
        let ctx = dartq_parser::IdentifierContext::TypeVariableDeclaration as u32;
        let names: Vec<&'a str> = c
            .collect_direct(variables)
            .into_iter()
            .filter(|&kid| {
                c.kind(kid) == NodeKind::Identifier && c.payload(kid).first() == Some(&ctx)
            })
            .map(|name| c.lexeme(c.head_token(name)))
            .collect();
        if !names.is_empty() {
            return names;
        }
        c.collect_direct(variables)
            .into_iter()
            .filter(|&param| c.kind(param) == NodeKind::TypeVariable)
            .filter_map(|param| {
                c.collect_direct(param)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            })
            .map(|name| c.lexeme(c.head_token(name)))
            .collect()
    }

    /// Whether the context at the site expects the same `T?`: the
    /// nearest meaningful ancestor decides (a `return` face with the
    /// enclosing method's written return type, or a variable initializer
    /// face with the declaration's written type; a body boundary first
    /// bails).
    fn context_expects_same_nullable(c: &Check<'_>, name: &str) -> bool {
        let Some(expected) = expected_type_type_parameter(c) else {
            return false;
        };
        let Some(face) = crate::rules::type_face_of_field(c, name) else {
            return false;
        };
        face.nullable && face.name == Some(expected)
    }

    /// The context's expected type-parameter name, when written `T?`.
    fn expected_type_type_parameter<'a>(c: &'a Check<'_>) -> Option<&'a str> {
        for &ancestor in c.path.iter().rev().skip(1) {
            match c.kind(ancestor) {
                NodeKind::VariableInitializer => {
                    // `T? x = <e>!;` — the declaration's written type.
                    let decl = c.unit.ast.parent(ancestor)?;
                    let face = written_declaration_type(c, decl)?;
                    return if face.1 { Some(face.0) } else { None };
                }
                NodeKind::ReturnStatement => {
                    // `return <e>!;` — the enclosing method's written
                    // return type.
                    let function = enclosing_function(c, ancestor)?;
                    return written_return_type(c, function)
                        .filter(|face| face.1)
                        .and_then(|face| Some(face.0));
                }
                NodeKind::Block | NodeKind::BlockBody | NodeKind::ExpressionBody => {
                    return None; // a body boundary — no written context face
                }
                _ => {}
            }
        }
        None
    }

    /// The enclosing method-like row of a statement.
    fn enclosing_function(c: &Check<'_>, statement: u32) -> Option<u32> {
        let pos = c.path.iter().position(|&a| a == statement)?;
        c.path[..pos].iter().rev().copied().find(|&a| {
            matches!(
                c.kind(a),
                NodeKind::Method | NodeKind::TopLevelMethod | NodeKind::FunctionExpression
            )
        })
    }

    /// The written type sibling of a declaration row: `(name, nullable)`.
    type WrittenType<'a> = (&'a str, bool);
    fn written_declaration_type<'a>(c: &'a Check<'_>, decl: u32) -> Option<WrittenType<'a>> {
        let Some(owner) = c.unit.ast.parent(decl) else {
            return None;
        };
        let members = c.collect_direct(owner);
        let index = members.iter().position(|&m| m == decl)?;
        let type_row = index
            .checked_sub(1)
            .and_then(|i| members.get(i))
            .copied()
            .filter(|&row| matches!(c.kind(row), NodeKind::Type | NodeKind::VoidType))?;
        let name_token = c.head_token(type_row);
        let nullable =
            c.byte_span(type_row).end > crate::rules::byte_span_of_token(c, name_token).end;
        Some((c.lexeme(name_token), nullable))
    }

    /// The written return type of a method-like row.
    fn written_return_type<'a>(c: &'a Check<'_>, function: u32) -> Option<(&'a str, bool)> {
        // The member-name identifier — a written return type's own name
        // tiles before it in the completion order, so the first
        // `Identifier` is the type, not the member.
        let kids = c.collect_direct(function);
        let is_declaration_name = |kid: u32| {
            c.kind(kid) == NodeKind::Identifier
                && matches!(
                    c.payload(kid).first().copied(),
                    Some(ctx)
                        if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                            || ctx
                                == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                    as u32
                )
        };
        let name = kids
            .iter()
            .copied()
            .find(|&kid| is_declaration_name(kid))
            .or_else(|| {
                kids.iter()
                    .copied()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            })?;
        let (type_row, kind) = crate::rules::type_before(c, function, name)?;
        if kind != NodeKind::Type {
            return None;
        }
        let name_token = c.head_token(type_row);
        let nullable =
            c.byte_span(type_row).end > crate::rules::byte_span_of_token(c, name_token).end;
        Some((c.lexeme(name_token), nullable))
    }
}

pub(crate) mod implicit_call_tearoffs {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{slice, type_face_of_field};

    /// Official: `pkg/linter/lib/src/rules/implicit_call_tearoffs.dart` —
    /// the resolution-built `ImplicitCallReference` (an object with a
    /// `call` member used where a function is expected) reports at the
    /// node (:34-39).
    ///
    /// Approximation (documented): the analyzer node never exists on this
    /// substrate, so the ported face reconstructs it syntactically for the
    /// assignment/initializer site: a written `FunctionType` target and a
    /// right side of `this.<field>` (or bare `<field>` — the user-shadowed
    /// family over-reports) whose same-class field type names a same-file
    /// class declaring an instance `call` method. Argument, return and
    /// collection sites, inferred targets and cross-file `call` classes
    /// stay silent (the dominant under-report face). Message:
    /// `diagnostic.g.dart` `implicitCallTearoffs`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "implicit_call_tearoffs",
        group: Group::Core,
        problem: "Implicit tear-off of the 'call' method.",
        correction: "Try explicitly tearing off the 'call' method.",
        kinds: &[NodeKind::Assignment, NodeKind::VariableInitializer],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The context type: an assignment's left field or an
        // initializer's declared variable — both written `FunctionType`.
        let context_is_function = match c.kind(node) {
            NodeKind::VariableInitializer => {
                let decl = c.unit.ast.parent(node);
                decl.is_some_and(|decl| written_type_is_function(c, declaration_type_row(c, decl)))
            }
            NodeKind::Assignment => {
                let Some(op) = c.payload(node).first().copied() else {
                    return;
                };
                if c.lexeme(op) != "=" {
                    return;
                }
                let Some((left, _)) = slice::binary_operands(c) else {
                    return;
                };
                let left = slice::access_run(c, &left);
                let Some(name) = assigned_name(c, &left) else {
                    return;
                };
                let Some(face) = type_face_of_field(c, name) else {
                    return;
                };
                face.is_function_type
            }
            _ => false,
        };
        if !context_is_function {
            return;
        }
        // The right side: `this.<field>` or bare `<field>`, whose field
        // type names a same-file class with an instance `call` method.
        let rows = match c.kind(node) {
            NodeKind::VariableInitializer => c.collect_direct(node),
            _ => {
                let Some((_, right)) = slice::binary_operands(c) else {
                    return;
                };
                right
            }
        };
        let run = slice::access_run(c, &rows);
        let Some(field) = assigned_name(c, &run) else {
            return;
        };
        let Some(face) = type_face_of_field(c, field) else {
            return;
        };
        let Some(type_name) = face.name else {
            return;
        };
        if !same_file_class_has_call(c, type_name) {
            return;
        }
        // Report at the expression (the official node face): the access
        // run spans it in source order.
        let start = c.utf16_span(run[0]).start;
        let end = c.utf16_span(run[run.len() - 1]).end;
        c.report(start..end, &[]);
    }

    /// The assigned name off an access run: `this.<field>` or bare
    /// `<field>`.
    fn assigned_name<'a>(c: &'a Check<'_>, run: &[u32]) -> Option<&'a str> {
        match run {
            [single] if c.kind(*single) == NodeKind::Identifier => {
                Some(c.lexeme(c.head_token(*single)))
            }
            [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
                Some(c.lexeme(c.head_token(*name)))
            }
            _ => None,
        }
    }

    /// The declaration's written type row (the sibling before the
    /// declaration).
    fn declaration_type_row(c: &Check<'_>, decl: u32) -> Option<u32> {
        let owner = c.unit.ast.parent(decl)?;
        let members = c.collect_direct(owner);
        let index = members.iter().position(|&m| m == decl)?;
        index
            .checked_sub(1)
            .and_then(|i| members.get(i))
            .copied()
            .filter(|&row| matches!(c.kind(row), NodeKind::Type | NodeKind::FunctionType))
    }

    /// Whether the written row is a function type (the `FunctionType`
    /// kind — `void Function()`, `int Function(String)`).
    fn written_type_is_function(c: &Check<'_>, row: Option<u32>) -> bool {
        row.is_some_and(|row| c.kind(row) == NodeKind::FunctionType)
    }

    /// Whether the same-file class `name` declares an instance `call`
    /// method.
    fn same_file_class_has_call(c: &Check<'_>, name: &str) -> bool {
        let Some(class_row) = crate::rules::same_file_declaration(c, name) else {
            return false;
        };
        crate::rules::body_members(c, class_row)
            .into_iter()
            .any(|member| {
                c.kind(member) == NodeKind::Method
                    && crate::rules::get_set_token(c, member).is_none()
                    && crate::rules::member_is_concrete(c, member)
                    && !crate::rules::member_matches(
                        c,
                        member,
                        crate::rules::MemberQuery::Method,
                        "call",
                    )
                    .is_empty()
            })
    }
}

pub(crate) mod invalid_runtime_check_with_js_interop_types {
    use super::{Check, Group, NodeKind, RuleSpec};

    const DART_IS_JS_PROBLEM: &str = "Runtime check between '{0}' and '{1}' checks whether a Dart value is a JS interop type, which might not be platform-consistent.";
    const DART_IS_JS_CORRECTION: &str =
        "Try using 'isA' from 'dart:js_interop' to check if the Dart value is a JS interop type.";
    const DART_AS_JS_PROBLEM: &str = "Cast from '{0}' to '{1}' casts a Dart value to a JS interop type, which might not be platform-consistent.";
    const DART_AS_JS_CORRECTION: &str = "Try using conversion methods from 'dart:js_interop' to convert between Dart types and JS interop types.";

    /// Official:
    /// `pkg/linter/lib/src/rules/invalid_runtime_check_with_js_interop_types.dart`
    /// — a runtime type test involving JS interop types that is not
    /// statically guaranteed reports (:360-398, eight diagnostic
    /// variants; the visit faces :400-437).
    ///
    /// Approximation (documented): the interop classification
    /// (`_isJsInteropType`, :43-72 — extension types, `@staticInterop`,
    /// SDK web libraries) reduces on the syntax face to same-file classes
    /// annotated `@JS`, in a file importing `dart:js_interop`. The ported
    /// face is the dart-is/as-js slice: `this is JSClass` / `this as
    /// JSClass` (the left type is the enclosing Dart class), reported at
    /// the expression with `(dart type, js type)` (:400-437). Other left
    /// expressions (no written type), cross-file interop types, the
    /// catch-clause face and the js-side variants stay silent (the
    /// dominant under-report face). Message: `diagnostic.g.dart`
    /// `invalidRuntimeCheckWithJsInteropTypesDart{Is,As}Js`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "invalid_runtime_check_with_js_interop_types",
        group: Group::Recommended,
        problem: DART_IS_JS_PROBLEM,
        correction: DART_IS_JS_CORRECTION,
        kinds: &[NodeKind::IsExpression, NodeKind::AsExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let is_check = c.kind(node) == NodeKind::IsExpression;
        // The left operand rows precede the operator container; the tested
        // type tiles inside it.
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == node) else {
            return;
        };
        // Left face: a lone `this` (the enclosing Dart class) immediately
        // before the operator container — an arrow body's signature rows
        // tile before it in the member window, so only the trailing `this`
        // is the tested expression and everything earlier must be
        // signature.
        let left: Vec<u32> = kids[..pos]
            .iter()
            .copied()
            .filter(|&row| !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified))
            .collect();
        let Some(&root) = left.last() else {
            return;
        };
        if c.kind(root) != NodeKind::ThisExpression {
            return;
        }
        if left[..left.len() - 1]
            .iter()
            .any(|&row| !is_signature_row(c, row))
        {
            return;
        }
        let Some(dart_type) = enclosing_class_name(c) else {
            return;
        };
        // The tested type: the `Type` row naming a same-file `@JS` class,
        // in a file importing `dart:js_interop`. The type tiles inside the
        // operator container (the bridge begins it at the operator, before
        // the type parses); the sibling face — the rows completing before
        // the container seals — is accepted too. The head token after the
        // operator keyword tells the tested type from the member's return
        // type (token ids are source-ordered).
        let op_head = c.head_token(node);
        let mut candidates = c.collect_direct(node);
        candidates.extend_from_slice(&kids[..pos]);
        let Some(type_row) = candidates
            .into_iter()
            .rev()
            .find(|&row| c.kind(row) == NodeKind::Type && c.head_token(row) > op_head)
        else {
            return;
        };
        let js_type = c.lexeme(c.head_token(type_row));
        if !imports_js_interop(c) || !same_file_js_class(c, js_type) {
            return;
        }
        let (dart_type, js_type) = (dart_type.to_owned(), js_type.to_owned());
        let range = c.utf16_span(node);
        if is_check {
            c.report_texts(
                range,
                DART_IS_JS_PROBLEM,
                DART_IS_JS_CORRECTION,
                &[dart_type.as_str(), js_type.as_str()],
            );
        } else {
            c.report_texts(
                range,
                DART_AS_JS_PROBLEM,
                DART_AS_JS_CORRECTION,
                &[dart_type.as_str(), js_type.as_str()],
            );
        }
    }

    /// The enclosing class-like declaration's name (the wrapper-aware
    /// read — the production name is the row's left sibling).
    fn enclosing_class_name<'a>(c: &'a Check<'_>) -> Option<&'a str> {
        let class_row = c.path.iter().rev().find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::ExtensionDeclaration
            )
        })?;
        crate::rules::class_declaration_name(c, *class_row)
    }

    /// The member-signature rows that tile before an arrow body's
    /// expression (the written return type, the name, the parameter list,
    /// their wrappers — and the childless `ExpressionBody` handle the
    /// expression rows trail).
    fn is_signature_row(c: &Check<'_>, row: u32) -> bool {
        matches!(
            c.kind(row),
            NodeKind::Type
                | NodeKind::VoidType
                | NodeKind::FunctionType
                | NodeKind::Identifier
                | NodeKind::FormalParameters
                | NodeKind::OptionalFormalParameters
                | NodeKind::TypeVariables
                | NodeKind::OperatorName
                | NodeKind::NoTypeArguments
                | NodeKind::TypeArguments
                | NodeKind::ExpressionBody
        )
    }

    /// Whether the unit imports `dart:js_interop`.
    fn imports_js_interop(c: &Check<'_>) -> bool {
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        // The import directive tiles inside its `TopLevelDeclaration`
        // wrapper; a root-direct Import is accepted too should a shape ever
        // leave it bare.
        let has_uri = |c: &Check<'_>, row: u32| {
            c.kind(row) == NodeKind::Import
                && c.collect_direct(row).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::StringLiteral && {
                        let span = c.byte_span(kid);
                        let text = &c.unit.source[span.start..span.end];
                        text.trim_matches(|q| q == '\'' || q == '"') == "dart:js_interop"
                    }
                })
        };
        c.collect_direct(root).into_iter().any(|row| {
            has_uri(c, row)
                || (c.kind(row) == NodeKind::TopLevelDeclaration
                    && c.collect_direct(row).into_iter().any(|kid| has_uri(c, kid)))
        })
    }

    /// Whether `name` is a same-file class carrying a `@JS` annotation
    /// (the annotation star precedes the top-level wrapper at the root).
    fn same_file_js_class(c: &Check<'_>, name: &str) -> bool {
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        let tops = c.collect_direct(root);
        for (i, &top) in tops.iter().enumerate() {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            let named = c.collect_direct(top).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.payload(kid).first()
                        == Some(
                            &(dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                                as u32),
                        )
                    && c.lexeme(c.head_token(kid)) == name
            });
            if !named {
                continue;
            }
            // The class's annotation star tiles as the wrapper's left
            // sibling at the root (the annotation seals before the wrapper
            // opens), or inside the wrapper when the wrapper opened first —
            // read both faces.
            let mut stars: Vec<u32> = Vec::new();
            if i > 0 && c.kind(tops[i - 1]) == NodeKind::MetadataStar {
                stars.push(tops[i - 1]);
            }
            for kid in c.collect_direct(top) {
                if c.kind(kid) == NodeKind::MetadataStar {
                    stars.push(kid);
                }
            }
            let annotated = stars.into_iter().any(|star| {
                c.collect_direct(star).into_iter().any(|meta| {
                    c.kind(meta) == NodeKind::Metadata
                        && c.collect_direct(meta).into_iter().any(|piece| {
                            c.kind(piece) == NodeKind::Identifier
                                && c.lexeme(c.head_token(piece)) == "JS"
                        })
                })
            });
            if annotated {
                return true;
            }
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    /// `test/rules/unnecessary_new_test.dart` `test_new` — the keyword
    /// reports, zero-width at its start.
    #[test]
    fn unnecessary_new_reports_the_keyword() {
        let source = "new B();";
        // new0 B1 (2 )3 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::NewExpression, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);

                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::NewExpression, 4);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_new"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'new' keyword.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// Implicit creation (`test_implicit`) never dispatches.
    #[test]
    fn unnecessary_new_implicit_creation_passes() {
        let source = "B();";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.leaf_span(NodeKind::Send, 0, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_new")).is_empty());
    }

    /// `test/rules/unnecessary_null_in_if_null_operators_test.dart`
    /// `test_rhsNull` — the right `null` reports first.
    #[test]
    fn if_null_right_null_reports() {
        let source = "var a = b ?? null;";
        // var0 a1 =2 b3 ??4 null5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, tok(source, t, "var", 0));
                b.begin(NodeKind::InitializedIdentifier, 1);

                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::BinaryExpression, 3);
                b.patch(0, tok(source, t, "??", 0));
                identifier(b, 3, 4);
                b.leaf(NodeKind::LiteralNull, 5);
                b.end(NodeKind::BinaryExpression, 6);
                b.end(NodeKind::VariableInitializer, 6);
                b.end(NodeKind::InitializedIdentifier, 6);
                b.end(NodeKind::VariablesDeclaration, 7);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_null_in_if_null_operators"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary use of '??' with 'null'.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// The left `null` reports when the right operand is a value
    /// (`test_lhsNull`).
    #[test]
    fn if_null_left_null_reports() {
        let source = "null ?? b;";
        // null0 ??1 b2 ;3
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "??", 0));
                b.leaf(NodeKind::LiteralNull, 0);
                identifier(b, 2, 3);
                b.end(NodeKind::BinaryExpression, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_null_in_if_null_operators"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    #[test]
    fn if_null_without_null_passes() {
        let source = "b ?? c;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "??", 0));
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::BinaryExpression, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_null_in_if_null_operators"))
                .is_empty()
        );
    }

    /// `test/rules/prefer_is_not_operator_test.dart` `test_notIs` — the
    /// whole prefix expression reports.
    #[test]
    fn prefer_is_not_reports_the_prefix() {
        let source = "!(a is int);";
        // !0 (1 a2 is3 int4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::UnaryPrefix, 0);
                b.begin(NodeKind::ParenthesizedExpression, 1);
                b.begin(NodeKind::IsExpression, 1);
                b.patch(0, 0); // `is`, not `is!`
                identifier(b, 2, 3);
                b.leaf_span(NodeKind::Type, 4, 5);
                b.end(NodeKind::IsExpression, 5);
                b.end(NodeKind::ParenthesizedExpression, 6);
                b.end(NodeKind::UnaryPrefix, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_is_not_operator"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the 'is!' operator rather than negating the value of the 'is' operator."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// `is!` under a `!` already uses the operator (`test_isNot`).
    #[test]
    fn prefer_is_not_bare_is_bang_passes() {
        let source = "!(a is! int);";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::UnaryPrefix, 0);
                b.begin(NodeKind::ParenthesizedExpression, 1);
                b.begin(NodeKind::IsExpression, 1);
                b.patch(0, 1); // `is!`
                identifier(b, 2, 3);
                b.leaf_span(NodeKind::Type, 4, 5);
                b.end(NodeKind::IsExpression, 5);
                b.end(NodeKind::ParenthesizedExpression, 6);
                b.end(NodeKind::UnaryPrefix, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(f.run(&one_rule("prefer_is_not_operator")).is_empty());
    }

    /// The parenthesized-`!` chain is required (prefer_is_not_operator.dart:
    /// 38-54) — a bare `is` never dispatches.
    #[test]
    fn prefer_is_not_unwrapped_is_passes() {
        let source = "a is int;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::IsExpression, 0);
                b.patch(0, 0);
                identifier(b, 0, 1);
                b.leaf_span(NodeKind::Type, 2, 3);
                b.end(NodeKind::IsExpression, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(f.run(&one_rule("prefer_is_not_operator")).is_empty());
    }

    /// `test/rules/prefer_adjacent_string_concatenation_test.dart`
    /// `test_adjacentStrings`.
    #[test]
    fn adjacent_strings_reports_the_plus() {
        let source = "'a' + 'b';";
        // 'a'0 +1 'b'2 ;3
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "+", 0));
                b.leaf_span(NodeKind::StringLiteral, 0, 1);
                b.leaf_span(NodeKind::StringLiteral, 2, 3);
                b.end(NodeKind::BinaryExpression, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        let lints = f.run(&one_rule("prefer_adjacent_string_concatenation"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "String literals shouldn't be concatenated by the '+' operator."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    #[test]
    fn adjacent_strings_mixed_operands_passes() {
        let source = "'a' + b;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "+", 0));
                b.leaf_span(NodeKind::StringLiteral, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::BinaryExpression, 3);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_adjacent_string_concatenation"))
                .is_empty()
        );
    }

    /// `test/rules/avoid_single_cascade_in_expression_statements_test.dart`
    /// `test_oneCascade` — message spells the `.` operator.
    #[test]
    fn single_cascade_reports_with_dot() {
        let source = "a..b();";
        // a0 ..1 b2 (3 )4 ;5 — the target precedes the cascade node; the
        // cascade's children are its sections (collapse map §3.5 CascadeEnd).
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 2);
                b.patch(0, 2);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::Arguments, 3);

                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Send, 5);
                b.end(NodeKind::CascadeAccess, 5);
                b.end(NodeKind::Cascade, 5);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("avoid_single_cascade_in_expression_statements"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary cascade expression.");
        assert_eq!(
            lints[0].correction,
            Some("Try using the operator '.'.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// A `?..` section prints `?.` (avoid_single_cascade...dart:40-51).
    #[test]
    fn single_null_aware_cascade_suggests_qmark_dot() {
        let source = "a?..b();";
        // a0 ?..1 b2 (3 )4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 2);
                b.patch(0, 2);
                b.patch(1, 1);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::Arguments, 3);

                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Send, 5);
                b.end(NodeKind::CascadeAccess, 5);
                b.end(NodeKind::Cascade, 5);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("avoid_single_cascade_in_expression_statements"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].correction,
            Some("Try using the operator '?.'.".to_owned())
        );
    }

    #[test]
    fn multi_section_cascade_passes() {
        let source = "a..b()..c();";
        // a0 ..1 b2 (3 )4 ..5 c6 (7 )8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 2);
                b.patch(0, 2);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::Arguments, 3);

                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::Send, 5);
                b.end(NodeKind::CascadeAccess, 5);
                b.begin(NodeKind::CascadeAccess, 5);
                b.patch(0, 5);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 6);
                b.begin(NodeKind::Arguments, 7);

                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::CascadeAccess, 9);
                b.end(NodeKind::Cascade, 9);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
        assert!(
            f.run(&one_rule("avoid_single_cascade_in_expression_statements"))
                .is_empty()
        );
    }

    // —— prefer_conditional_assignment ———————————————————————————————————

    /// `test/rules/prefer_conditional_assignment_test.dart` `test_ifNullAssign`.
    #[test]
    fn conditional_assignment_reports_the_if() {
        let source = "var x;\nif (x == null) x = 1;";
        // var0 x1 ;2 if3 (4 x5 ==6 null7 )8 x9 =10 111 ;12
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0);
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.end(NodeKind::InitializedIdentifier, 2);
                b.end(NodeKind::VariablesDeclaration, 3);
                b.begin(NodeKind::IfStatement, 3);
                b.patch(0, NONE); // no else
                b.begin(NodeKind::ParenthesizedCondition, 4);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::BinaryExpression, 5);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 5, 6);
                b.leaf(NodeKind::LiteralNull, 7);
                b.end(NodeKind::BinaryExpression, 8);
                b.end(NodeKind::ParenthesizedCondition, 9);
                b.begin(NodeKind::ExpressionStatement, 9);
                b.begin(NodeKind::Assignment, 9);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 9, 10);
                b.leaf(NodeKind::LiteralInt, 11);
                b.end(NodeKind::Assignment, 12);
                b.end(NodeKind::ExpressionStatement, 13);
                b.end(NodeKind::IfStatement, 13);
            }),
        );
        let lints = f.run(&one_rule("prefer_conditional_assignment"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The 'if' statement could be replaced by a null-aware assignment."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// An else arm disqualifies (:70-72).
    #[test]
    fn conditional_assignment_with_else_passes() {
        let source = "if (x == null) x = 1; else ;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, tok(source, t, "else", 0));
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::BinaryExpression, 2);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 2, 3);
                b.leaf(NodeKind::LiteralNull, 4);
                b.end(NodeKind::BinaryExpression, 5);
                b.end(NodeKind::ParenthesizedCondition, 6);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.begin(NodeKind::Assignment, 6);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 6, 7);
                b.leaf(NodeKind::LiteralInt, 8);
                b.end(NodeKind::Assignment, 9);
                b.end(NodeKind::ExpressionStatement, 10);
                b.leaf(NodeKind::EmptyStatement, 11);
                b.end(NodeKind::IfStatement, 12);
            }),
        );
        assert!(f.run(&one_rule("prefer_conditional_assignment")).is_empty());
    }

    /// A non-assignment then arm passes.
    #[test]
    fn conditional_assignment_other_then_passes() {
        let source = "if (x == null) y = 1;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::BinaryExpression, 2);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 2, 3);
                b.leaf(NodeKind::LiteralNull, 4);
                b.end(NodeKind::BinaryExpression, 5);
                b.end(NodeKind::ParenthesizedCondition, 6);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.begin(NodeKind::Assignment, 6);
                b.patch(0, tok(source, t, "=", 0));
                identifier(b, 6, 7);
                b.leaf(NodeKind::LiteralInt, 8);
                b.end(NodeKind::Assignment, 9);
                b.end(NodeKind::ExpressionStatement, 10);
                b.end(NodeKind::IfStatement, 10);
            }),
        );
        assert!(f.run(&one_rule("prefer_conditional_assignment")).is_empty());
    }

    // —— prefer_if_null_operators ————————————————————————————————————————

    /// `test/rules/prefer_if_null_operators_test.dart` `test_usedElse`.
    #[test]
    fn if_null_reports_the_else_mirror() {
        let source = "a == null ? 1 : a;";
        // a0 ==1 null2 ?3 14 :5 a6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 3);
                b.patch(0, tok(source, t, "?", 0));
                b.patch(1, tok(source, t, ":", 0));
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::BinaryExpression, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                identifier(b, 6, 7);
                b.end(NodeKind::Conditional, 7);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("prefer_if_null_operators"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the '??' operator rather than '?:' when testing for 'null'."
        );
    }

    /// The `!=` form tests the then arm (`test_usedThen`).
    #[test]
    fn if_null_reports_the_then_mirror() {
        let source = "a != null ? a : 1;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 3);
                b.patch(0, tok(source, t, "?", 0));
                b.patch(1, tok(source, t, ":", 0));
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "!=", 0));
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::BinaryExpression, 3);
                identifier(b, 4, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::Conditional, 7);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("prefer_if_null_operators"));
        assert_eq!(lints.len(), 1);
    }

    /// Divergent arms pass (`test_different`).
    #[test]
    fn if_null_divergent_arms_pass() {
        let source = "a == null ? 1 : b;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 3);
                b.patch(0, tok(source, t, "?", 0));
                b.patch(1, tok(source, t, ":", 0));
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::BinaryExpression, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                identifier(b, 6, 7);
                b.end(NodeKind::Conditional, 7);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        assert!(f.run(&one_rule("prefer_if_null_operators")).is_empty());
    }

    // —— prefer_null_aware_operators —————————————————————————————————————

    /// `x == null ? null : x.b` reports (`test_prefixedIdentifier` family).
    #[test]
    fn null_aware_reports_the_access_form() {
        let source = "a.b == null ? null : a.b.c;";
        // a0 .1 b2 ==3 null4 ?5 null6 :7 a8 .9 b10 .11 c12 ;13
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 5);
                b.patch(0, tok(source, t, "?", 0));
                b.patch(1, tok(source, t, ":", 0));
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "==", 0));
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.leaf(NodeKind::LiteralNull, 4);
                b.end(NodeKind::BinaryExpression, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.begin(NodeKind::DotAccess, 8);
                b.patch(0, 9);
                b.patch(1, 0);
                b.begin(NodeKind::DotAccess, 8);
                b.patch(0, 9);
                b.patch(1, 0);
                identifier(b, 8, 9);
                identifier(b, 10, 11);
                b.end(NodeKind::DotAccess, 11);
                identifier(b, 12, 13);
                b.end(NodeKind::DotAccess, 13);
                b.end(NodeKind::Conditional, 13);
                b.end(NodeKind::ExpressionStatement, 14);
            }),
        );
        let lints = f.run(&one_rule("prefer_null_aware_operators"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use the null-aware operator '?.' rather than an explicit 'null' comparison."
        );
    }

    /// A result with no peelable access passes (`a == null ? null : a`).
    #[test]
    fn null_aware_bare_identifier_passes() {
        let source = "a == null ? null : a;";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 3);
                b.patch(0, tok(source, t, "?", 0));
                b.patch(1, tok(source, t, ":", 0));
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "==", 0));
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::BinaryExpression, 3);
                b.leaf(NodeKind::LiteralNull, 4);
                identifier(b, 6, 7);
                b.end(NodeKind::Conditional, 7);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        assert!(f.run(&one_rule("prefer_null_aware_operators")).is_empty());
    }

    /// The `!=` mirror reports on the then arm (`test_notEqual`).
    #[test]
    fn null_aware_not_equal_reports() {
        // The official walk peels the result before the comparison
        // (:76-93), so the matching shape is `a != null ? a.b : null` —
        // `a.b != null ? a.b : null` peels `a.b` to `a` and never matches.
        let source = "a != null ? a.b : null;";
        // a0 !=1 null2 ?3 a4 .5 b6 :7 null8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Conditional, 3);
                b.patch(0, 3);
                b.patch(1, 7);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::BinaryExpression, 3);
                b.begin(NodeKind::DotAccess, 4);
                b.patch(0, 5);
                b.patch(1, 0);
                identifier(b, 4, 5);
                identifier(b, 6, 7);
                b.end(NodeKind::DotAccess, 7);
                b.leaf(NodeKind::LiteralNull, 8);
                b.end(NodeKind::Conditional, 9);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
        let lints = f.run(&one_rule("prefer_null_aware_operators"));
        assert_eq!(lints.len(), 1);
    }

    // —— unnecessary_null_aware_assignments ———————————————————————————————

    /// `test/rules/unnecessary_null_aware_assignments_test.dart` — `x ??=
    /// null` reports at the assignment.
    #[test]
    fn null_aware_assignment_reports() {
        let source = "x ??= null;";
        // x0 ??=1 null2 ;3
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Assignment, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::Assignment, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_null_aware_assignments"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary assignment of 'null'.");
        assert_eq!(
            lints[0].correction,
            Some("Try removing the assignment.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// A property target (`a.b ??= null`) and a non-null RHS pass.
    #[test]
    fn null_aware_property_and_non_null_pass() {
        let property = "a.b ??= null;";
        // a0 .1 b2 ??=3 null4 ;5
        let f = Fixture::build(
            property,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Assignment, 0);
                b.patch(0, 3);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 4);
                b.leaf(NodeKind::LiteralNull, 4);
                b.end(NodeKind::Assignment, 6);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_null_aware_assignments"))
                .is_empty()
        );

        let int_rhs = "x ??= 1;";
        // x0 ??=1 12 ;3
        let f = Fixture::build(
            int_rhs,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Assignment, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                b.leaf(NodeKind::LiteralInt, 2);
                b.end(NodeKind::Assignment, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_null_aware_assignments"))
                .is_empty()
        );
    }

    // —— prefer_contains ——————————————————————————————————————————————————

    /// `test/rules/prefer_contains_test.dart` `test_contains` — `indexOf ==
    /// -1` reports with the rewrite template. Production-bridge fixture.
    #[test]
    fn contains_reports_index_of_equals_minus_one() {
        let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) == -1; }");
        let lints = f.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unnecessary use of 'indexOf' to test for containment."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try using 'contains'.".to_owned())
        );
        // The operand occurrence: "xs"#0 is the formal parameter's token.
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "xs", 1))
        );
    }

    /// The always-false/always-true judgments carry their own texts and no
    /// correction; `>= 0` rewrites.
    #[test]
    fn contains_message_matrix() {
        let always_false = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) < -1; }");
        let lints = always_false.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Always 'false' because 'indexOf' is always greater than or equal to -1."
        );
        assert_eq!(lints[0].correction, None);

        let always_true = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) >= -1; }");
        let lints = always_true.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Always 'true' because 'indexOf' is always greater than or equal to -1."
        );

        let rewrite = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) >= 0; }");
        let lints = rewrite.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unnecessary use of 'indexOf' to test for containment."
        );
    }

    /// A left-hand literal compares with the inverted operator.
    #[test]
    fn contains_left_literal_inverts_the_operator() {
        let f = Fixture::parse("void f(List<int> xs) { -1 != xs.indexOf(1); }");
        let lints = f.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Unnecessary use of 'indexOf' to test for containment."
        );
    }

    /// An assigned `indexOf`, a bare untargeted invocation, and a non-zero
    /// start index pass (`_isUnassignedIndexOf`, :144-171).
    #[test]
    fn contains_assigned_and_bare_invocations_pass() {
        let f = Fixture::parse("void f(List<int> xs) { (next = xs.indexOf('{')) != -1; }");
        assert!(f.run(&one_rule("prefer_contains")).is_empty());

        let f = Fixture::parse("void f() { var e = 1; indexOf(e) == -1; }");
        assert!(f.run(&one_rule("prefer_contains")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1, 1) == -1; }");
        assert!(f.run(&one_rule("prefer_contains")).is_empty());
    }

    /// The restored receiver face (:145-149): a string literal receiver
    /// reports (the `String` half), a map literal receiver stays silent
    /// (`Map` implements neither `Iterable` nor `String` — the recon's
    /// receiver-check divergence, closed).
    #[test]
    fn contains_receiver_face_decides_literal_receivers() {
        let f = Fixture::parse("void f() { var x = 'abc'.indexOf('a') != -1; }");
        assert_eq!(f.run(&one_rule("prefer_contains")).len(), 1, "String face");

        let f = Fixture::parse("void f() { var x = ({'k': 1}).indexOf('a') == -1; }");
        assert!(
            f.run(&one_rule("prefer_contains")).is_empty(),
            "a map literal is outside [Iterable, String]"
        );

        // An opaque receiver keeps the documented presumption.
        let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) == -1; }");
        assert_eq!(f.run(&one_rule("prefer_contains")).len(), 1);
    }

    // —— unnecessary_const ————————————————————————————————————————————————

    /// The official `constantContext` face: a const-marked literal under a
    /// const variable declaration reports (`test_constConstructor` family —
    /// `const c = [!const!] C()`).
    #[test]
    fn unnecessary_const_reports_inside_const_declaration() {
        let source = "const x = [const A()];";
        // const0 x1 =2 [3 const4 A5 (6 )7 ]8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::ListLiteral, 3);
                b.patch(0, NONE);
                b.begin(NodeKind::ConstExpression, 4);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::ConstExpression, 8);
                b.end(NodeKind::ListLiteral, 9);
                b.end(NodeKind::VariableInitializer, 9);
                b.end(NodeKind::InitializedIdentifier, 9);
                b.end(NodeKind::VariablesDeclaration, 10);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_const"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'const' keyword.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// A const expression under a non-const declaration stays
    /// (`final x = const [1]` — the official `VariableDeclarationList` branch
    /// stops the walk without the keyword).
    #[test]
    fn unnecessary_const_final_declaration_passes() {
        let source = "final x = const [1];";
        // final0 x1 =2 const3 [4 15 ]6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::ConstExpression, 3);
                b.begin(NodeKind::ListLiteral, 4);
                b.patch(0, NONE);
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::ListLiteral, 7);
                b.end(NodeKind::ConstExpression, 7);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_const")).is_empty());
    }

    /// A const-marked literal with no const ancestor stays
    /// (`final x = const [1]` again exercises the list-literal dispatch
    /// through the payload keyword).
    #[test]
    fn unnecessary_const_list_literal_keyword_face() {
        let source = "final x = const [1];";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::ListLiteral, 3);
                b.patch(0, tok(source, t, "const", 0));
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::ListLiteral, 7);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_const")).is_empty());
    }

    // —— prefer_is_empty ———————————————————————————————————————————————————

    /// `test/rules/prefer_is_empty_test.dart` `test_prefersIsEmptyOverLength0`
    /// — `x.length == 0` reports with the isEmpty rewrite. Production-bridge
    /// fixture.
    #[test]
    fn prefer_is_empty_reports_length_eq_zero() {
        let f = Fixture::parse("void f(List<int> xs) { xs.length == 0; }");
        let lints = f.run(&one_rule("prefer_is_empty"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'isEmpty' instead of 'length' to test whether the collection is empty."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try rewriting the expression to use 'isEmpty'.".to_owned())
        );
        assert_eq!(
            lints[0].range.start,
            // The operand occurrence: "xs"#0 is the formal parameter's token.
            utf16_at(&f, tok(&f.source, &f.tokens, "xs", 1))
        );
    }

    /// A value outside the matrix passes (`== 2`); the always-true face
    /// carries its own text and no correction (`x.length >= -1`).
    #[test]
    fn prefer_is_empty_matrix_faces() {
        let f = Fixture::parse("void f(List<int> xs) { xs.length == 2; }");
        assert!(f.run(&one_rule("prefer_is_empty")).is_empty());

        let f = Fixture::parse("void f(List<int> xs) { xs.length >= -1; }");
        let lints = f.run(&one_rule("prefer_is_empty"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The comparison is always 'true' because the length is always greater than or equal to 0."
        );
        assert_eq!(lints[0].correction, None);
    }

    /// The restored receiver face (:229-234): a record literal is provably
    /// outside `Iterable`/`Map`/`String` and passes (the recon's
    /// receiver-check divergence, closed), while a map literal reports like
    /// the official `Map` face and an opaque receiver keeps the presumption.
    #[test]
    fn prefer_is_empty_receiver_face_decides_literal_receivers() {
        let f = Fixture::parse("void f() { if ((1,).length >= 1) {} }");
        assert!(
            f.run(&one_rule("prefer_is_empty")).is_empty(),
            "a record is not Iterable/Map/String"
        );

        let f = Fixture::parse("void f() { if (({'k': 1}).length >= 1) {} }");
        assert_eq!(f.run(&one_rule("prefer_is_empty")).len(), 1, "Map face");

        let f = Fixture::parse("void f(List<int> xs) { if (xs.length >= 1) {} }");
        assert_eq!(f.run(&one_rule("prefer_is_empty")).len(), 1);
    }

    // —— prefer_is_not_empty ———————————————————————————————————————————————

    /// `test/rules/prefer_is_not_empty_test.dart` `test_notIsEmpty` — the
    /// negated `isEmpty` reports at the prefix. Production-bridge fixture.
    #[test]
    fn prefer_is_not_empty_reports_bang_is_empty() {
        let f = Fixture::parse("void f(List<int> xs) { if (!xs.isEmpty) {} }");
        let lints = f.run(&one_rule("prefer_is_not_empty"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'isNotEmpty' rather than negating the result of 'isEmpty'."
        );
        let bang = tok(&f.source, &f.tokens, "!", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, bang));
        let is_empty = tok(&f.source, &f.tokens, "isEmpty", 0);
        assert_eq!(lints[0].range.end, utf16_at(&f, is_empty) + 7);
    }

    /// A negated `isNotEmpty` (or any other property) passes.
    #[test]
    fn prefer_is_not_empty_other_property_passes() {
        let f = Fixture::parse("void f(List<int> xs) { if (!xs.isNotEmpty) {} }");
        assert!(f.run(&one_rule("prefer_is_not_empty")).is_empty());

        let f = Fixture::parse("void f(bool a, bool b) { if (!(a || b)) {} }");
        assert!(f.run(&one_rule("prefer_is_not_empty")).is_empty());
    }

    /// A string-literal receiver carries `isNotEmpty` on `String` and
    /// reports through the face query (:57-62).
    #[test]
    fn prefer_is_not_empty_literal_receiver_reports() {
        let f = Fixture::parse("void f() { if (!'abc'.isEmpty) {} }");
        assert_eq!(f.run(&one_rule("prefer_is_not_empty")).len(), 1);
    }

    // —— avoid_function_literals_in_foreach_calls ——————————————————————————

    /// `test/rules/avoid_function_literals_in_foreach_calls_test.dart`
    /// `test forEach with function literal` — the method name reports.
    /// Production-bridge fixture.
    #[test]
    fn foreach_literal_reports_the_method_name() {
        let f = Fixture::parse("void f(List<int> xs) { xs.forEach((e) {}); }");
        let lints = f.run(&one_rule("avoid_function_literals_in_foreach_calls"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Function literals shouldn't be passed to 'forEach'."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try using a 'for' loop.".to_owned())
        );
        let name = tok(&f.source, &f.tokens, "forEach", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
        assert_eq!(lints[0].range.end, utf16_at(&f, name) + 7);
    }

    /// The `Map.forEach` face — a two-parameter closure — passes through the
    /// arity gate, a cascade section passes through the cascade guard, and a
    /// null-aware chain passes through the marker word (:87).
    #[test]
    fn foreach_two_params_cascade_and_null_aware_pass() {
        let f = Fixture::parse("void f(Map<String, int> map) { map.forEach((k, v) {}); }");
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );

        let f = Fixture::parse("void f(List<int> xs) { xs..forEach((e) {}); }");
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );

        let f = Fixture::parse("void f(List<int> xs) { xs?.forEach((e) {}); }");
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );
    }

    /// A chained target passes (`_hasMethodChaining`, :31-43), and the
    /// restored receiver face keeps a map literal silent like the official
    /// `Iterable`-only gate (:40-45).
    #[test]
    fn foreach_chaining_and_receiver_face_pass() {
        let f = Fixture::parse("void f(List<int> xs, int g(int e)) { xs.map(g).forEach((e) {}); }");
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );

        let f = Fixture::parse("void f() { ({'k': 1}).forEach((e) {}); }");
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty(),
            "a map literal is outside Iterable"
        );

        // An opaque receiver keeps the documented presumption.
        let f = Fixture::parse("void f(List<int> xs) { xs.forEach((e) { print(e); }); }");
        assert_eq!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .len(),
            1
        );
    }

    // —— prefer_interpolation_to_compose_strings ———————————————————————————

    /// `name + ' (id)'` reports over the pair span. Production-bridge
    /// fixture: the bare-name operand tiles as the four-piece tear-off
    /// group, so the report span starts at `name`.
    #[test]
    fn interpolation_reports_literal_plus_value() {
        let f = Fixture::parse("void f(String name) { var s = name + ' (id)'; }");
        let lints = f.run(&one_rule("prefer_interpolation_to_compose_strings"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use interpolation to compose strings and values."
        );
        // The operand occurrence: "name"#0 is the formal parameter's token.
        let name = tok(&f.source, &f.tokens, "name", 1);
        let raw = tok(&f.source, &f.tokens, "' (id)'", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, name));
        assert_eq!(lints[0].range.end, utf16_at(&f, raw) + 7);
    }

    /// Both literals and a raw string pass (:70-75).
    #[test]
    fn interpolation_both_literals_and_raw_pass() {
        let f = Fixture::parse("void f() { var s = 'a' + 'b'; }");
        assert!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .is_empty()
        );

        let f = Fixture::parse("void f() { var s = 'a' + r'b'; }");
        assert!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .is_empty()
        );
    }

    /// A `toString` invocation with arguments passes (sdk#52610), the chain
    /// reports once at the root, and the left-operand String face (:85)
    /// decides literal receivers — a string literal reports.
    #[test]
    fn interpolation_to_string_gate_and_chain_root() {
        let f = Fixture::parse("void f(A a, int x) { var s = a.toString(x) + 'foo'; }");
        assert!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .is_empty()
        );

        let f = Fixture::parse("void f(String a, String b) { var s = a + b + 'c'; }");
        assert_eq!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .len(),
            1,
            "an opaque chain reports once, at the literal pair"
        );

        let f = Fixture::parse("void f() { var s = 'a' + f(); }");
        assert_eq!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .len(),
            1,
            "the left literal is dart:core String"
        );
    }

    // —— null_closures ——————————————————————————————————————————————————————

    /// `Future(null)` — a `null` in the unnamed-constructor closure slot
    /// reports at the null (`test/rules/null_closures_test.dart`
    /// `test_Future`).
    #[test]
    fn null_closures_reports_a_null_future_callback() {
        let f = Fixture::parse("void f() { Future(null); }");
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Closure can't be 'null' because it might be invoked."
        );
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
        );
    }

    /// `new Future(null)` reports the same way through the explicit-new
    /// shape.
    #[test]
    fn null_closures_reports_an_explicit_new_creation() {
        let f = Fixture::parse("void f() { new Future(null); }");
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
        );
    }

    /// `Future.wait(fs, cleanUp: null)` — the named slot reports over the
    /// whole named argument (`test_nullFromFuture_wait`).
    #[test]
    fn null_closures_reports_a_named_null_over_the_argument() {
        let f = Fixture::parse("void f() { Future.wait(fs, cleanUp: null); }");
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "cleanUp", 0))
        );
        let null_start = utf16_at(&f, tok(&f.source, &f.tokens, "null", 0));
        assert_eq!(lints[0].range.end, null_start + 4);
    }

    /// `Timer.periodic(d, null)` — the named-creation face, positional
    /// slot 1 (real bridge shape: the `Qualified` callee).
    #[test]
    fn null_closures_reports_a_periodic_timer_callback() {
        let f = Fixture::parse("void f() { Timer.periodic(d, null); }");
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
        );
    }

    /// A non-null closure passes, and a bare `map(null)` never matches —
    /// the official bails with a null target (null_closures.dart:222).
    #[test]
    fn null_closures_non_null_and_bare_calls_pass() {
        let f = Fixture::parse("void f(List<int> list, void g(int e)) { list.map(g); }");
        assert!(f.run(&one_rule("null_closures")).is_empty());

        // A bare unknown callee is no creation (`map(null)`).
        let f = Fixture::parse("void f() { map(null); }");
        assert!(f.run(&one_rule("null_closures")).is_empty());
    }

    /// `Future.doWhile(null)` — the static face, positional slot 0 (the
    /// official `Future.doWhile` entry, null_closures.dart:174).
    #[test]
    fn null_closures_reports_a_static_do_while_callback() {
        let f = Fixture::parse("void f() { Future.doWhile(null); }");
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
        );
    }

    /// A same-file subclass of a core creation type reports through the
    /// `extendsClass` chain (extensions.dart:305-353), and a same-file
    /// shadow of the core name passes (the creation resolves to the user
    /// class, whose element is not the library type).
    #[test]
    fn null_closures_user_subclass_creation_reports_and_shadow_passes() {
        let f = Fixture::parse("class MyFuture extends Future {}\nvoid f() { MyFuture(null); }");
        assert_eq!(
            f.run(&one_rule("null_closures")).len(),
            1,
            "MyFuture extends Future: the closure slot reports"
        );

        let f = Fixture::parse("class Future {}\nvoid f() { Future(null); }");
        assert!(
            f.run(&one_rule("null_closures")).is_empty(),
            "a same-file `Future` is not the dart:async element"
        );
    }

    /// The instance face resolves the receiver's written type through the
    /// same-file chain: an `Iterable` receiver reports, a `String` receiver
    /// stays silent (the official `_getInstanceMethod` finds no `map` on
    /// `String`), a user class with its own `map` stays silent, and a user
    /// subclass of `List` reports (the `allSupertypes` walk reaches
    /// `Iterable.map`).
    #[test]
    fn null_closures_instance_receiver_type_pinning() {
        let reports = [
            (
                "void f(Iterable<int> xs) { xs.map(null); }",
                "Iterable receiver",
            ),
            (
                "void f(List<int> l) { l.map(null); }",
                "List receiver (supertype walk)",
            ),
            (
                "class MyList extends List {}\nvoid f(MyList l) { l.map(null); }",
                "user subclass of List",
            ),
        ];
        for (source, why) in reports {
            let f = Fixture::parse(source);
            assert_eq!(f.run(&one_rule("null_closures")).len(), 1, "{why}");
        }

        let silent = [
            ("void f(String s) { s.map(null); }", "String has no map"),
            (
                "class Foo { void map(void cb(int e)) {} }\nvoid f(Foo foo) { foo.map(null); }",
                "user class with its own map",
            ),
        ];
        for (source, why) in silent {
            let f = Fixture::parse(source);
            assert!(f.run(&one_rule("null_closures")).is_empty(), "{why}");
        }
    }

    // —— unnecessary_this ————————————————————————————————————————————————————

    /// `this.x` in an instance method over a same-class field reports
    /// zero-width at the `this` (`test/rules/unnecessary_this_test.dart`
    /// `test_UnnecessaryThis`); the inherited-member face reports the same
    /// way (`isNone` — no scope hit). Production-bridge fixtures: the field
    /// and method names ride the collapse-map contexts.
    #[test]
    fn unnecessary_this_reports_a_field_access() {
        let f = Fixture::parse("class C { int x = 0; int f() { return this.x; } }");
        let lints = f.run(&one_rule("unnecessary_this"));
        assert_eq!(lints.len(), 1, "same-class field: the `this.` reports");
        assert_eq!(lints[0].message, "Unnecessary 'this.' qualifier.");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "this", 0))
        );

        // Inherited member — no declaration anywhere in the file.
        let f =
            Fixture::parse("class A { void f() {} }\nclass B extends A { void g() { this.f(); } }");
        assert_eq!(
            f.run(&one_rule("unnecessary_this")).len(),
            1,
            "inherited member: no scope hit, the `this.` reports"
        );
    }

    /// An unprefixed import can provide the member name in the unqualified
    /// scope (:87-95 bail): no report. `show`/`hide` combinators narrow the
    /// face — a name provably excluded keeps reporting; a prefixed import
    /// never enters the scope.
    #[test]
    fn unnecessary_this_import_bails() {
        let bailed = "import 'other.dart';\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }";
        let f = Fixture::parse(bailed);
        assert!(
            f.run(&one_rule("unnecessary_this")).is_empty(),
            "the unprefixed import may provide `f` — the official bails"
        );

        let shown_away = "import 'other.dart' show Other;\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }";
        let f = Fixture::parse(shown_away);
        assert_eq!(
            f.run(&one_rule("unnecessary_this")).len(),
            1,
            "`show Other` provably excludes `f`"
        );

        let hidden = "import 'other.dart' hide f;\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }";
        let f = Fixture::parse(hidden);
        assert_eq!(
            f.run(&one_rule("unnecessary_this")).len(),
            1,
            "`hide f` provably excludes `f`"
        );

        let prefixed = "import 'other.dart' as other;\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }";
        let f = Fixture::parse(prefixed);
        assert_eq!(
            f.run(&one_rule("unnecessary_this")).len(),
            1,
            "a prefixed import never enters the unqualified scope"
        );
    }

    /// A same-name unit declaration shadows the inherited form (:87-95 —
    /// the resolution finds a different element): no report.
    #[test]
    fn unnecessary_this_top_level_name_bails() {
        let f = Fixture::parse(
            "void f() {}\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }",
        );
        assert!(f.run(&one_rule("unnecessary_this")).is_empty());
    }

    /// A parameter of the enclosing member shadows — the `this.` stays
    /// (unnecessary_this.dart:87-95, `isRequestedName` branch).
    #[test]
    fn unnecessary_this_parameter_shadow_passes() {
        let f = Fixture::parse("class C { int x = 0; int f(int x) { return this.x; } }");
        assert!(
            f.run(&one_rule("unnecessary_this")).is_empty(),
            "the parameter shadows the field — the qualified form is required"
        );
    }

    /// A local declared before the use in the same block shadows.
    #[test]
    fn unnecessary_this_local_shadow_passes() {
        let f = Fixture::parse("class C { int x = 0; int f() { int x = 1; return this.x; } }");
        assert!(
            f.run(&one_rule("unnecessary_this")).is_empty(),
            "the local shadows the field"
        );
    }

    /// The written `this` on a constructor field initializer always reports
    /// (unnecessary_this.dart:31-35); the bare `x = 1;` form passes.
    #[test]
    fn unnecessary_this_reports_an_initializer_this_and_bare_passes() {
        let f = Fixture::parse("class C { int x = 0; C() : this.x = 1; }");
        let lints = f.run(&one_rule("unnecessary_this"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "this", 0))
        );

        let f = Fixture::parse("class C { int x = 0; C() : x = 1; }");
        assert!(f.run(&one_rule("unnecessary_this")).is_empty());
    }

    // —— prefer_final_fields ——————————————————————————————————————————————
    // Fixtures over the collapse-map §3.3 contracts (`Fields` >
    // `VariablesDeclaration` > `InitializedIdentifier` > `VariableInitializer`,
    // `Constructor` > `FormalParameters`/`Initializers`).

    /// `class C { int _x = 0; }` — the candidate fixture; `extra` appends
    /// unit-level siblings (mutation statements, more constructors).
    fn field_fixture(
        extra: impl FnOnce(&mut dartq_ast::AstBuilder, &dartq_scanner::TokenTable),
    ) -> Fixture {
        Fixture::build("class C {\n  int _x = 0;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 8);
            b.end(NodeKind::ClassDeclaration, 8);
            b.end(NodeKind::TopLevelDeclaration, 9);
            extra(b, t);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        })
    }

    fn field_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("prefer_final_fields"))
    }

    /// Through the production bridge: a private initialized field never
    /// mutated reports at the variable declaration (the candidate walk
    /// reads the real `Fields` tiling).
    #[test]
    fn final_fields_reports_on_real_parse() {
        let f = Fixture::parse("class C {\n  int _x = 0;\n}\n");
        let lints = field_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "The private field _x could be 'final'.");
    }

    /// An assignment statement over the field name, as a unit-level sibling.
    fn assignment_statement(
        b: &mut dartq_ast::AstBuilder,
        name_at: u32,
        eq_at: u32,
        value_at: u32,
    ) {
        b.begin(NodeKind::ExpressionStatement, name_at);
        b.begin(NodeKind::Assignment, name_at);
        b.patch(0, eq_at);
        identifier(b, name_at, name_at + 1);
        b.leaf(NodeKind::LiteralInt, value_at);
        b.end(NodeKind::Assignment, value_at + 1);
        b.end(NodeKind::ExpressionStatement, value_at + 2);
    }

    /// An initialized private field reports on its declaration
    /// (prefer_final_fields_test.dart `initialized`).
    #[test]
    fn initialized_private_field_reports() {
        let f = field_fixture(|_b, _t| {});
        let lints = field_hits(&f);
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "The private field _x could be 'final'.");
        assert_eq!(
            lints[0].correction,
            Some("Try making the field 'final'.".to_owned())
        );
        // The report covers the declaration `_x = 0` (bytes 16..22).
        assert_eq!(lints[0].range.start, 16);
        assert_eq!(lints[0].range.end, 22);
    }

    /// An assignment anywhere marks the field mutated (`mutated`).
    #[test]
    fn assignment_blocks() {
        // The statement sits past the class: `_x14 =15 116 ;17`.
        let source = "class C {\n  int _x = 0;\n}\n_x = 1;";
        let f = Fixture::build(source, |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 8);
            b.end(NodeKind::ClassDeclaration, 8);
            b.end(NodeKind::TopLevelDeclaration, 9);
            assignment_statement(b, 9, 10, 11);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
    }

    /// `++` marks the field mutated too (`incrementAndDecrement`) — the
    /// prefix operator parses before its operand.
    #[test]
    fn increment_blocks() {
        // c0..{8 | ++9 _x10 ;11
        let f = Fixture::build("class C {\n  int _x = 0;\n}\n++_x;", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 8);
            b.end(NodeKind::ClassDeclaration, 8);
            b.end(NodeKind::TopLevelDeclaration, 9);
            b.begin(NodeKind::ExpressionStatement, 9);
            b.leaf(NodeKind::UnaryPrefix, 9);
            identifier(b, 10, 11);
            b.end(NodeKind::ExpressionStatement, 12);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
    }

    /// A field formal in every constructor counts as setting
    /// (`fieldFormalInParameter`).
    #[test]
    fn field_formal_set_reports() {
        // c0 l1 a2 s3 s4 ␣5 C6 {7 int8 _x9 ;10 C11 (12 this13 .14 _x15 )16 ;17 }18
        let source = "class C {\n  int _x;\n  C(this._x);\n}\n";
        let f = Fixture::build(source, |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.begin(NodeKind::Constructor, 6);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 7);
            b.patch(3, NONE);
            identifier(b, 6, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::FormalParameter, 8);
            b.patch(0, 8);
            b.patch(1, 10);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(
                4,
                dartq_parser::FormalParameterKind::RequiredPositional as u32,
            );
            b.leaf(NodeKind::NoType, 10);
            b.end(NodeKind::FormalParameter, 11);
            b.end(NodeKind::FormalParameters, 11);
            b.leaf(NodeKind::EmptyBody, 12);
            b.end(NodeKind::Constructor, 13);
            b.end(NodeKind::DeclarationBody, 13);
            b.end(NodeKind::ClassDeclaration, 14);
            b.end(NodeKind::TopLevelDeclaration, 14);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert_eq!(field_hits(&f).len(), 1);
    }

    /// A field initializer in every constructor counts as setting
    /// (`fieldInitializer`) — the bare and `this.`-prefixed entry forms.
    #[test]
    fn field_initializer_set_reports() {
        // Bare form: `C() : _x = 0;` — c0..C6 (7 )8 :9 _x10 =11 012 ;13 }14
        let bare = Fixture::build("class C {\n  int _x;\n  C() : _x = 0;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.begin(NodeKind::Constructor, 6);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 7);
            b.patch(3, 9);
            identifier(b, 6, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.begin(NodeKind::Initializers, 9);
            b.begin(NodeKind::Initializer, 10);
            b.begin(NodeKind::FieldInitializer, 11);
            b.patch(0, 11);
            b.leaf(NodeKind::LiteralInt, 12);
            b.end(NodeKind::FieldInitializer, 13);
            b.end(NodeKind::Initializer, 13);
            b.end(NodeKind::Initializers, 13);
            b.leaf(NodeKind::EmptyBody, 13);
            b.end(NodeKind::Constructor, 14);
            b.end(NodeKind::DeclarationBody, 14);
            b.end(NodeKind::ClassDeclaration, 15);
            b.end(NodeKind::TopLevelDeclaration, 15);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert_eq!(field_hits(&bare).len(), 1);
        // `this.` form: the entry head is the `this` keyword; the name sits
        // two tokens later.
        let this_form =
            Fixture::build("class C {\n  int _x;\n  C() : this._x = 0;\n}\n", |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::Fields, 6);
                b.begin(NodeKind::Constructor, 6);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 7);
                b.patch(3, 9);
                identifier(b, 6, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::Initializers, 9);
                b.begin(NodeKind::Initializer, 10);
                b.begin(NodeKind::FieldInitializer, 13);
                b.patch(0, 13);
                b.leaf(NodeKind::LiteralInt, 15);
                b.end(NodeKind::FieldInitializer, 16);
                b.end(NodeKind::Initializer, 16);
                b.end(NodeKind::Initializers, 16);
                b.leaf(NodeKind::EmptyBody, 16);
                b.end(NodeKind::Constructor, 17);
                b.end(NodeKind::DeclarationBody, 17);
                b.end(NodeKind::ClassDeclaration, 18);
                b.end(NodeKind::TopLevelDeclaration, 18);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            });
        assert_eq!(field_hits(&this_form).len(), 1);
    }

    /// Set in some constructors only — never reports (`notPassedInAll`).
    #[test]
    fn set_in_some_constructors_only_passes() {
        // c0..{7 int8 _x9 ;10 C11 (12 this13 .14 _x15 )16 ;17 C18 .19 n20
        // (21 )22 ;23 }24
        let f = Fixture::build(
            "class C {\n  int _x;\n  C(this._x);\n  C.n();\n}\n",
            |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.end(NodeKind::InitializedIdentifier, 5);
                b.end(NodeKind::VariablesDeclaration, 6);
                b.end(NodeKind::Fields, 6);
                // The setting constructor: `C(this._x);`.
                b.begin(NodeKind::Constructor, 6);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 7);
                b.patch(3, NONE);
                identifier(b, 6, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::FormalParameter, 8);
                b.patch(0, 8);
                b.patch(1, 10);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(
                    4,
                    dartq_parser::FormalParameterKind::RequiredPositional as u32,
                );
                b.leaf(NodeKind::NoType, 10);
                b.end(NodeKind::FormalParameter, 11);
                b.end(NodeKind::FormalParameters, 11);
                b.leaf(NodeKind::EmptyBody, 12);
                b.end(NodeKind::Constructor, 13);
                // The unset named constructor: `C.n();`.
                b.begin(NodeKind::Constructor, 13);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, 15);
                b.patch(2, 16);
                b.patch(3, NONE);
                identifier(b, 13, 14);
                b.begin(NodeKind::FormalParameters, 16);
                b.end(NodeKind::FormalParameters, 18);
                b.leaf(NodeKind::EmptyBody, 19);
                b.end(NodeKind::Constructor, 20);
                b.end(NodeKind::DeclarationBody, 20);
                b.end(NodeKind::ClassDeclaration, 21);
                b.end(NodeKind::TopLevelDeclaration, 21);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        );
        assert!(field_hits(&f).is_empty());
    }

    /// Public fields, `final`/`const` fields, unset unassigned fields, and
    /// top-level variables never report.
    #[test]
    fn non_candidates_pass() {
        // Public name.
        let f = Fixture::build("class C {\n  int x = 0;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.end(NodeKind::DeclarationBody, 8);
            b.end(NodeKind::ClassDeclaration, 8);
            b.end(NodeKind::TopLevelDeclaration, 9);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
        // `final` keyword on the declaration.
        let f = Fixture::build("class C {\n  final int _x = 0;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, 3);
            b.leaf_span(NodeKind::Type, 4, 5);
            b.begin(NodeKind::InitializedIdentifier, 5);
            identifier(b, 5, 6);
            b.begin(NodeKind::VariableInitializer, 6);
            b.leaf(NodeKind::LiteralInt, 7);
            b.end(NodeKind::VariableInitializer, 8);
            b.end(NodeKind::InitializedIdentifier, 8);
            b.end(NodeKind::VariablesDeclaration, 9);
            b.end(NodeKind::Fields, 9);
            b.end(NodeKind::DeclarationBody, 9);
            b.end(NodeKind::ClassDeclaration, 9);
            b.end(NodeKind::TopLevelDeclaration, 10);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
        // Unset, never assigned.
        let f = Fixture::build("class C {\n  int _x;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.end(NodeKind::DeclarationBody, 6);
            b.end(NodeKind::ClassDeclaration, 6);
            b.end(NodeKind::TopLevelDeclaration, 7);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
        // Top-level declarations are not fields.
        let f = Fixture::build("var _x = 0;\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 0);
            identifier(b, 0, 1);
            b.begin(NodeKind::VariableInitializer, 2);
            b.leaf(NodeKind::LiteralInt, 3);
            b.end(NodeKind::VariableInitializer, 4);
            b.end(NodeKind::InitializedIdentifier, 4);
            b.end(NodeKind::VariablesDeclaration, 5);
            b.end(NodeKind::TopLevelFields, 5);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
    }

    /// An assignment inside a constructor body is still a mutation.
    #[test]
    fn constructor_body_assignment_blocks() {
        // c0..{7 int8 _x9 ;10 C11 (12 )13 {14 _x15 =16 017 ;18 }19 }20
        let f = Fixture::build(
            "class C {\n  int _x = 0;\n  C() { _x = 0; }\n}\n",
            |b, t| {
                b.begin(NodeKind::CompilationUnit, 0);
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Fields, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
                b.begin(NodeKind::VariablesDeclaration, 3);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                b.begin(NodeKind::InitializedIdentifier, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::VariableInitializer, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::VariableInitializer, 7);
                b.end(NodeKind::InitializedIdentifier, 7);
                b.end(NodeKind::VariablesDeclaration, 8);
                b.end(NodeKind::Fields, 8);
                b.begin(NodeKind::Constructor, 8);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 9);
                b.patch(3, 11);
                identifier(b, 8, 9);
                b.begin(NodeKind::FormalParameters, 9);
                b.end(NodeKind::FormalParameters, 11);
                b.begin(NodeKind::BlockBody, 11);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 11);
                b.begin(NodeKind::ExpressionStatement, 12);
                b.begin(NodeKind::Assignment, 12);
                b.patch(0, 13);
                identifier(b, 12, 13);
                b.leaf(NodeKind::LiteralInt, 14);
                b.end(NodeKind::Assignment, 15);
                b.end(NodeKind::ExpressionStatement, 16);
                b.end(NodeKind::Block, 16);
                b.end(NodeKind::BlockBody, 17);
                b.end(NodeKind::Constructor, 18);
                b.end(NodeKind::DeclarationBody, 18);
                b.end(NodeKind::ClassDeclaration, 19);
                b.end(NodeKind::TopLevelDeclaration, 19);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        );
        assert!(field_hits(&f).is_empty());
    }

    /// Enum fields stay out (`PreferFinalFieldsEnum`) — the walk skips the
    /// enum declaration whole.
    #[test]
    fn enum_fields_pass() {
        let f = Fixture::build("enum E { a }\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::EnumDeclaration, 0);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.patch(2, NONE);
            identifier(b, 1, 2);
            b.begin(NodeKind::EnumBody, 3);
            b.begin(NodeKind::Fields, 4);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 4);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.end(NodeKind::EnumBody, 7);
            b.end(NodeKind::EnumDeclaration, 8);
            b.end(NodeKind::CompilationUnit, t.len() as u32);
        });
        assert!(field_hits(&f).is_empty());
    }

    // ---- #586 standard-set completions (production-bridge fixtures) ----

    /// `await` on scalar literals reports at the keyword with the exact
    /// type; `null` (:39) and inference-bound expressions stay silent.
    #[test]
    fn await_only_futures_scalar_literals_report() {
        let f = Fixture::parse(
            "void f() async {\n  await 1;\n  await 'x';\n  await (2.5);\n  await null;\n  await g();\n}\nvoid g() {}",
        );
        let lints = f.run(&one_rule("await_only_futures"));
        assert_eq!(lints.len(), 3, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Uses 'await' on an instance of 'int', which is not a subtype of 'Future'."
        );
        assert_eq!(
            lints[1].message,
            "Uses 'await' on an instance of 'String', which is not a subtype of 'Future'."
        );
        assert_eq!(
            lints[2].message,
            "Uses 'await' on an instance of 'double', which is not a subtype of 'Future'."
        );
    }

    /// `==`/`!=` on unrelated scalar literal faces reports at the
    /// operator with `(right, left)`; null operands (:53-55) and equal
    /// faces stay silent.
    #[test]
    fn unrelated_type_equality_reports_mismatched_scalars() {
        let f = Fixture::parse(
            "void f() {\n  if (1 == 'a') {}\n  if (1 != true) {}\n  if (1 == 2) {}\n  if (null == 1) {}\n}",
        );
        let lints = f.run(&one_rule("unrelated_type_equality_checks"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The type of the right operand ('String') isn't a subtype or a supertype of the left operand ('int')."
        );
        assert_eq!(
            lints[1].message,
            "The type of the right operand ('bool') isn't a subtype or a supertype of the left operand ('int')."
        );
    }

    /// Assigning a non-null scalar to a `void` (or `FutureOr<void>`)
    /// field reports; `null` is acceptable when expecting void (:57).
    #[test]
    fn void_checks_reports_void_field_assignment() {
        let f = Fixture::parse(
            "import 'dart:async';\nclass C {\n  void v;\n  FutureOr<void> fv;\n  int n;\n  void set_it() {\n    this.v = 1;\n    this.v = null;\n    this.fv = 'x';\n    this.fv = null;\n    this.n = 1;\n  }\n}",
        );
        let lints = f.run(&one_rule("void_checks"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(lints[0].message, "Assignment to a variable of type 'void'.");
        assert_eq!(lints[1].message, "Assignment to a variable of type 'void'.");
    }

    /// A `==` comparison is not an assignment face — no void_checks
    /// report (the coordinator boundary case).
    #[test]
    fn void_checks_equality_operand_stays_silent() {
        let f = Fixture::parse("class C {\n  void v;\n  bool eq(int x) => x == 1;\n}");
        assert!(f.run(&one_rule("void_checks")).is_empty());
    }

    /// `!` on a `T?` field in a `T?` context reports at the operator;
    /// a different context type and no-`!` reads stay silent.
    #[test]
    fn null_check_on_nullable_type_parameter_reports_in_matching_context() {
        let f = Fixture::parse(
            "class C<T> {\n  T? f;\n  T? m() { return this.f!; }\n  T? n() { return f!; }\n  T? ok() { return this.f; }\n  int? other() { return this.f!; }\n}",
        );
        let lints = f.run(&one_rule("null_check_on_nullable_type_parameter"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The null check operator shouldn't be used on a variable whose type is a potentially nullable type parameter."
        );
    }

    /// A `FunctionType` target assigned a `call`-bearing object reports
    /// the implicit tear-off; explicit `.call` and plain values stay
    /// silent.
    #[test]
    fn implicit_call_tearoffs_reports_function_typed_target() {
        let f = Fixture::parse(
            "class Callable {\n  void call() {}\n}\nclass Plain {\n  void hello() {}\n}\nclass C {\n  Callable obj = Callable();\n  Plain p = Plain();\n  void Function() f;\n  void setup() {\n    this.f = this.obj;\n    this.f = this.p;\n    this.f = this.obj.call;\n    this.f = 1;\n  }\n}",
        );
        let lints = f.run(&one_rule("implicit_call_tearoffs"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Implicit tear-off of the 'call' method.");
    }

    /// `this is JSClass` / `this as JSClass` with a same-file `@JS`
    /// class under a `dart:js_interop` import reports; non-interop
    /// types, other receivers and files without the import stay silent.
    #[test]
    fn invalid_runtime_check_reports_dart_vs_js() {
        let f = Fixture::parse(
            "import 'dart:js_interop';\n\n@JS\nclass JS {\n  external JS();\n}\n\nclass Dart {\n  bool check() => this is JS;\n  Object cast() => this as JS;\n  bool plain(Object o) => o is JS;\n  bool self() => this is Dart;\n}",
        );
        let lints = f.run(&one_rule("invalid_runtime_check_with_js_interop_types"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Runtime check between 'Dart' and 'JS' checks whether a Dart value is a JS interop type, which might not be platform-consistent."
        );
        assert_eq!(
            lints[1].message,
            "Cast from 'Dart' to 'JS' casts a Dart value to a JS interop type, which might not be platform-consistent."
        );
    }

    /// Without `dart:js_interop` the same syntax is not interop-classified.
    #[test]
    fn invalid_runtime_check_requires_js_interop_import() {
        let f =
            Fixture::parse("@JS\nclass JS {}\n\nclass Dart {\n  bool check() => this is JS;\n}");
        assert!(
            f.run(&one_rule("invalid_runtime_check_with_js_interop_types"))
                .is_empty()
        );
    }
}
