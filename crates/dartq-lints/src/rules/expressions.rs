//! The expression-shape family: `unnecessary_new`,
//! `unnecessary_null_in_if_null_operators`, `prefer_is_not_operator`,
//! `prefer_adjacent_string_concatenation`,
//! `avoid_single_cascade_in_expression_statements`,
//! `prefer_conditional_assignment`, `prefer_if_null_operators`,
//! `prefer_null_aware_operators`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, token_seq_equal};

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
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::first_child;

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
    /// Approximations (documented): the receiver type check
    /// (`implementsAnyInterface([Iterable, String])`, :145-149) is dropped —
    /// any `indexOf` invocation on a target reports, so a user class with an
    /// `indexOf` method over-reports; `getIntValue` reads int literals
    /// (decimal/hex, separators, a negation prefix) only, so constant
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
        let operands = c.collect_direct(node);
        if operands.len() < 2 {
            return;
        }
        let (literal, effective_op) = {
            let right = int_value(c, operands[1]);
            if right.is_some_and(|v| v <= 0) && is_unassigned_index_of(c, node, operands[0]) {
                (right.expect("checked Some"), op)
            } else {
                let left = int_value(c, operands[0]);
                if left.is_some_and(|v| v <= 0) && is_unassigned_index_of(c, node, operands[1]) {
                    (left.expect("checked Some"), invert(op))
                } else {
                    return;
                }
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
        let range = c.utf16_span(node);
        c.report_texts(range, problem, correction.unwrap_or_default(), &[]);
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

    /// `getIntValue` restricted to int literals: decimal/hex with
    /// separators, optionally negated by a prefix `-` (the source form of
    /// `-1` in an expression).
    pub(super) fn int_value(c: &Check<'_>, node: u32) -> Option<i64> {
        let (negative, literal) = if c.kind(node) == NodeKind::UnaryPrefix {
            // `UnaryPrefix` is a width-0 kind; the operator is the span
            // head (the `prefer_is_not_operator` precedent).
            if c.lexeme(c.head_token(node)) != "-" {
                return None;
            }
            (true, first_child(c, node)?)
        } else {
            (false, node)
        };
        if c.kind(literal) != NodeKind::LiteralInt {
            return None;
        }
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
    /// target, not the RHS of an assignment, with a zero start index.
    fn is_unassigned_index_of(c: &Check<'_>, comparison: u32, expression: u32) -> bool {
        is_unassigned_index_of_inner(c, comparison, expression).unwrap_or(false)
    }

    fn is_unassigned_index_of_inner(
        c: &Check<'_>,
        comparison: u32,
        expression: u32,
    ) -> Option<bool> {
        // Unwrap parens and `as` expressions, tracking the invocation's
        // parent (:148-152).
        let mut current = expression;
        let mut parent = comparison;
        while matches!(
            c.kind(current),
            NodeKind::ParenthesizedExpression | NodeKind::AsExpression
        ) {
            parent = current;
            current = first_child(c, current)?;
        }
        if c.kind(current) != NodeKind::Send {
            return Some(false);
        }
        // `invocation.parent is AssignmentExpression` (:140).
        if c.kind(parent) == NodeKind::Assignment {
            return Some(false);
        }
        let kids = c.collect_direct(current);
        let args_pos = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments))?;
        let callee = *kids.get(args_pos.checked_sub(1)?)?;
        let args = c.collect_direct(kids[args_pos]);
        // The method name is the callee's identifier (`x.indexOf` — a
        // `DotAccess`; a bare `indexOf` has no target and the official
        // `parentType == null` bails, :143-144).
        let (name, target) = match c.kind(callee) {
            NodeKind::DotAccess | NodeKind::Qualified => {
                let access = c.collect_direct(callee);
                let name = access.last().copied()?;
                if c.kind(name) != NodeKind::Identifier {
                    return Some(false);
                }
                (name, access.first().copied())
            }
            _ => return Some(false),
        };
        if c.lexeme(c.head_token(name)) != "indexOf" || target.is_none() {
            return Some(false);
        }
        // A two-argument form must pass start index 0 (:152-155).
        if args.len() == 2 {
            let start = if c.kind(args[1]) == NodeKind::NamedArgument {
                c.collect_direct(args[1]).get(1).copied()?
            } else {
                args[1]
            };
            if c.kind(start) != NodeKind::LiteralInt || c.lexeme(c.head_token(start)) != "0" {
                return Some(false);
            }
        }
        Some(true)
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
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{first_child, in_const_context};

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
    /// Approximations (documented): the receiver type check
    /// (`implementsInterface(Iterable/Map/String)`, :229-234) is dropped —
    /// the `prefer_contains` precedent, so a user `length` getter
    /// over-reports; `getIntValue` reads int literals only, so constant
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
        let operands = c.collect_direct(node);
        if operands.len() < 2 {
            return;
        }
        let op = c.lexeme(c.payload(node).first().copied().expect("binary op"));
        let (value, constant_on_right, length_access) = {
            // Right literal first (:64-71).
            match super::prefer_contains::int_value(c, operands[1]) {
                Some(value) => (value, true, is_length_access(c, operands[0])),
                None => match super::prefer_contains::int_value(c, operands[0]) {
                    Some(value) => (value, false, is_length_access(c, operands[1])),
                    None => return,
                },
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
            _ => return,
        };
        let range = c.utf16_span(node);
        c.report_texts(range, problem, correction.unwrap_or_default(), &[]);
    }

    /// `_isLengthAccess` (:228-240): a `length` property access on a target,
    /// through parens and `as`.
    fn is_length_access(c: &Check<'_>, operand: u32) -> bool {
        let mut current = operand;
        while matches!(
            c.kind(current),
            NodeKind::ParenthesizedExpression | NodeKind::AsExpression
        ) {
            let Some(inner) = first_child(c, current) else {
                return false;
            };
            current = inner;
        }
        if c.kind(current) != NodeKind::DotAccess {
            return false;
        }
        let access = c.collect_direct(current);
        let (Some(target), Some(name)) = (access.first().copied(), access.last().copied()) else {
            return false;
        };
        c.kind(name) == NodeKind::Identifier
            && c.lexeme(c.head_token(name)) == "length"
            && target != name
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
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::first_child;

    /// Official: `pkg/linter/lib/src/rules/prefer_is_not_empty.dart:31-63` —
    /// `!x.isEmpty` reports at the prefix expression.
    ///
    /// Approximation (documented): the element checks (the property resolves
    /// to `isEmpty` and its interface declares `isNotEmpty`, :57-62) are
    /// taken from the accessed name — a user class with `isEmpty` but no
    /// `isNotEmpty` over-reports. Message: `diagnostic.g.dart`
    /// `preferIsNotEmpty`.
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
        // `node.operand.unParenthesized` (:48).
        let Some(operand) = first_child(c, node) else {
            return;
        };
        let mut operand = operand;
        while c.kind(operand) == NodeKind::ParenthesizedExpression {
            let Some(inner) = first_child(c, operand) else {
                return;
            };
            operand = inner;
        }
        // `PropertyAccess`/`PrefixedIdentifier` (:50-55) — a `DotAccess`
        // whose last child names `isEmpty`.
        if c.kind(operand) != NodeKind::DotAccess {
            return;
        }
        let Some(name) = c.collect_direct(operand).last().copied() else {
            return;
        };
        if c.kind(name) != NodeKind::Identifier || c.lexeme(c.head_token(name)) != "isEmpty" {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

pub(crate) mod avoid_function_literals_in_foreach_calls {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_function_literals_in_foreach_calls.dart:31-105`
    /// — `x.forEach((e) { … })` reports at the method name.
    ///
    /// Approximations (documented): the receiver type check
    /// (`_isIterable`, :44-45) is dropped — the `prefer_contains` precedent,
    /// so a user class with a one-parameter `forEach` over-reports. The
    /// closure-arity gate stands in for the type gate's `Map.forEach` face:
    /// a valid `Iterable.forEach` closure has exactly one parameter, and a
    /// multi-parameter closure (the `map.forEach((k, v) …)` shape) skips.
    /// The null-aware-chain bail (:87) reads the null-aware bits along the
    /// access chain; `containsNullAwareInvocationInChain` farther out
    /// under-reports on exotic chains. Message: `diagnostic.g.dart`
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
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments))
        else {
            return;
        };
        // `node.methodName.token.value() == 'forEach'` with a target
        // (:82-84).
        let Some(callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        let name = match c.kind(*callee) {
            NodeKind::DotAccess | NodeKind::Qualified => {
                let access = c.collect_direct(*callee);
                match access.last().copied() {
                    Some(name)
                        if c.kind(name) == NodeKind::Identifier
                            && c.lexeme(c.head_token(name)) == "forEach" =>
                    {
                        name
                    }
                    _ => return,
                }
            }
            _ => return,
        };
        // `arguments.isNotEmpty && arguments.first is FunctionExpression`
        // (:85-86) — a named first argument is not a literal.
        let args = c.collect_direct(kids[args_pos]);
        let Some(&first) = args.first() else {
            return;
        };
        if c.kind(first) != NodeKind::FunctionExpression {
            return;
        }
        // The closure-arity stand-in for the type gate: a multi-parameter
        // closure is the `Map.forEach` face (a type error on a real
        // Iterable), never the flagged shape.
        let closure_params = c
            .collect_direct(first)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .map(|params| c.collect_direct(params).len());
        if closure_params != Some(1) {
            return;
        }
        // The chain guards: no null-aware access below (:87), no method
        // chaining in the target (:31-43), not inside a cascade (:45-48).
        if chain_has_null_aware(c) || has_method_chaining(c, *callee) || inside_cascade(c) {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }

    /// `containsNullAwareInvocationInChain` (:87): any `?.`/`?..` on the
    /// chain this invocation belongs to — the access below and the
    /// invocation/access spine above, up to the nearest boundary.
    fn chain_has_null_aware(c: &Check<'_>) -> bool {
        for &ancestor in c.path.iter().rev() {
            match c.kind(ancestor) {
                NodeKind::DotAccess | NodeKind::CascadeAccess => {
                    if c.payload(ancestor).get(1).copied().unwrap_or(0) != 0 {
                        return true;
                    }
                }
                NodeKind::Send | NodeKind::Arguments => {}
                // A statement (or anything else) ends the chain.
                _ => return false,
            }
        }
        false
    }

    /// `_hasMethodChaining` (:31-43): the target chain contains a method
    /// invocation.
    fn has_method_chaining(c: &Check<'_>, callee: u32) -> bool {
        let mut target = c.collect_direct(callee).first().copied();
        while let Some(current) = target {
            match c.kind(current) {
                NodeKind::Send => return true,
                // PrefixedIdentifier → prefix; PropertyAccess → target.
                NodeKind::DotAccess | NodeKind::Qualified => {
                    target = c.collect_direct(current).first().copied();
                }
                _ => return false,
            }
        }
        false
    }

    /// `_isInsideCascade` (:45-48): the nearest statement-or-cascade
    /// ancestor is a cascade.
    fn inside_cascade(c: &Check<'_>) -> bool {
        for &ancestor in c.path.iter().rev() {
            match c.kind(ancestor) {
                NodeKind::Cascade => return true,
                NodeKind::ExpressionStatement
                | NodeKind::ReturnStatement
                | NodeKind::IfStatement
                | NodeKind::WhileStatement
                | NodeKind::DoWhileStatement
                | NodeKind::ForStatement
                | NodeKind::ForIn
                | NodeKind::YieldStatement
                | NodeKind::LabeledStatement
                | NodeKind::SwitchCase => return false,
                _ => {}
            }
        }
        false
    }
}

pub(crate) mod null_closures {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/null_closures.dart` — a `null`
    /// passed in a closure position of a known core API reports at the
    /// argument. Three faces (:212-260): instance creations
    /// (`Future(null)`, `Timer.periodic(d, null)`, ...), static calls
    /// (`Future.wait(..., cleanUp: null)`, ...), and instance calls
    /// (`list.map(null)`, ...). Positions come from the fixed tables
    /// (:26-180); the report lands on the `null` literal, or on the whole
    /// named argument for the named face (:286-291).
    ///
    /// Approximations (documented): the resolved-element matching
    /// (`element is ClassElement`, `target.staticType` + supertype walk,
    /// :222-250) is replaced by written-name matching. Instance creations
    /// match the written type name, so a user subclass of `Future` named
    /// otherwise under-reports (a rare legal family); static and instance
    /// calls match the trailing identifier(s), so a user class or prefix
    /// reusing a core name (`myObj.map(null)` on a nullable-closure
    /// parameter) over-reports — the #353 `prefer_contains` receiver-name
    /// precedent. Bare top-level calls (`map(null)`) never match — the
    /// official bails with a null target (:222). `const Future(...)` shapes
    /// are out (these types have no const constructors — erroneous code
    /// only). Message: `diagnostic.g.dart` `nullClosures`.
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
        ("Future", "doWhile", (&[1], &[])),
        ("Future", "forEach", (&[1], &[])),
        ("Future", "wait", (&[], &["cleanUp"])),
        ("Timer", "run", (&[0], &[])),
    ];

    /// Instance face (`_instanceMethodsWithNonNullableArguments`, :35-170),
    /// keyed by method name.
    const INSTANCE_METHODS: &[(&str, Slots)] = &[
        ("any", (&[0], &[])),
        ("complete", (&[0], &[])),
        ("every", (&[0], &[])),
        ("expand", (&[0], &[])),
        ("firstWhere", (&[0], &["orElse"])),
        ("forEach", (&[0], &[])),
        ("fold", (&[1], &[])),
        ("lastWhere", (&[0], &["orElse"])),
        ("map", (&[0], &[])),
        ("putIfAbsent", (&[1], &[])),
        ("reduce", (&[0], &[])),
        ("removeWhere", (&[0], &[])),
        ("replaceAllMapped", (&[1], &[])),
        ("replaceFirstMapped", (&[1], &[])),
        ("retainWhere", (&[0], &[])),
        ("singleWhere", (&[0], &["orElse"])),
        ("skipWhile", (&[0], &[])),
        ("splitMapJoin", (&[], &["onMatch", "onNonMatch"])),
        ("takeWhile", (&[0], &[])),
        ("then", (&[0], &["onError"])),
        ("where", (&[0], &[])),
    ];

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The arguments child and the identifier chain written before it
        // (`a.Future`, `List.generate`, `x.map`, prefixed forms included).
        let kids = c.collect_direct(node);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments | NodeKind::NoArguments))
        else {
            return;
        };
        let mut chain: Vec<&str> = Vec::new();
        for &callee in &kids[..args_pos] {
            // A bare identifier callee is a leaf: its children window holds
            // only descendants, so test the node itself first.
            if c.kind(callee) == NodeKind::Identifier {
                chain.push(c.lexeme(c.head_token(callee)));
                continue;
            }
            let window = c.children(callee);
            for id in window.start..window.end {
                if c.kind(id) == NodeKind::Identifier {
                    chain.push(c.lexeme(c.head_token(id)));
                }
            }
        }
        let Some(member_name) = chain.last() else {
            return;
        };
        let member_name = *member_name;
        // Which table matches: the single-name form is an unnamed
        // constructor; a pair is a named constructor, a static call, or an
        // instance call (the bare-call form never matches — the official
        // bails on a null target, :222).
        let slots: Option<Slots> = if chain.len() == 1 {
            CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == member_name && entry.0.1.is_none())
                .map(|entry| entry.1)
        } else {
            let class_name = chain[chain.len() - 2];
            let constructor = CONSTRUCTORS
                .iter()
                .find(|entry| entry.0.0 == class_name && entry.0.1 == Some(member_name))
                .map(|entry| entry.1);
            let function = STATICS
                .iter()
                .find(|entry| entry.0 == class_name && entry.1 == member_name)
                .map(|entry| entry.2);
            let instance = INSTANCE_METHODS
                .iter()
                .find(|entry| entry.0 == member_name)
                .map(|entry| entry.1);
            constructor.or(function).or(instance)
        };
        let Some((positions, named)) = slots else {
            return;
        };
        // `_checkNullArgForClosure` (:268-293): a `null` in a closure slot
        // reports at the null literal, or at the whole named argument.
        for (i, &arg) in c.collect_direct(kids[args_pos]).iter().enumerate() {
            if c.kind(arg) == NodeKind::NamedArgument {
                let arg_kids = c.collect_direct(arg);
                let Some(&name) = arg_kids.first() else {
                    continue;
                };
                if c.kind(name) != NodeKind::Identifier
                    || !named.contains(&c.lexeme(c.head_token(name)))
                {
                    continue;
                }
                let is_null = arg_kids
                    .get(1)
                    .is_some_and(|&value| c.kind(value) == NodeKind::LiteralNull);
                if is_null {
                    let range = c.utf16_span(arg);
                    c.report(range, &[]);
                }
            } else if c.kind(arg) == NodeKind::LiteralNull && positions.contains(&i) {
                let range = c.utf16_span(arg);
                c.report(range, &[]);
            }
        }
    }
}

pub(crate) mod unnecessary_this {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// Official: `pkg/linter/lib/src/rules/unnecessary_this.dart` — a `this.`
    /// qualifier reports at the `this` keyword unless shadowing makes it
    /// necessary. Two faces (:37-45): a constructor field initializer's
    /// `this` reports unconditionally when written (:31-35 — `x = e;` is
    /// the bare form), and a `this` in a property access / method invocation
    /// reports when the unqualified name would resolve to the same member
    /// (:47-64).
    ///
    /// Approximation (documented): the scope resolution
    /// (`resolveNameInScope` + element identity, :66-99) is replaced by a
    /// syntax-level shadow index. Locals, parameters, catch parameters,
    /// loop variables, closure parameters, and local functions declared
    /// before the use shadow (the `this.` stays); the enclosing member's
    /// parameters shadow. When the enclosing class body declares an
    /// instance member of the name, the unqualified form resolves to it and
    /// the `this.` reports; otherwise the unit's top-level names make the
    /// official conservative bail (:87-95) and the face skips. The unseen
    /// over-report face is an inherited member plus a same-named name
    /// imported from another library with an explicit `this.` — the
    /// official bails on the import, the lexical face reports (narrow legal
    /// family, the `prefer_contains` receiver-face precedent). Message:
    /// `diagnostic.g.dart` `unnecessaryThis`.
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

    /// `visitThisExpression` (:37-64): `this.name` accesses only.
    fn check_this(c: &mut Check<'_>) {
        let node = c.node();
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if c.kind(parent) != NodeKind::DotAccess {
            return;
        }
        // `!parent.isNullAware` (:49/:53).
        if c.payload(parent).get(1).copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN {
            return;
        }
        let kids = c.collect_direct(parent);
        if kids.first() != Some(&node) {
            return;
        }
        let Some(&name) = kids.last() else {
            return;
        };
        if c.kind(name) != NodeKind::Identifier {
            return;
        }
        let name_lexeme = c.lexeme(c.head_token(name));
        if member_shadows(c, node, name_lexeme) {
            return;
        }
        if !owner_declares_instance_member(c, name_lexeme)
            && unit_declares_top_level_name(c, name_lexeme)
        {
            // The official conservative bail (:87-95): a same-name visible
            // in the outer scopes while the class has no instance member
            // might shadow the inherited form — no report.
            return;
        }
        let start = c.utf16_span(node).start;
        c.report(start..start, &[]);
    }

    /// Whether any declaration visible between the use and the enclosing
    /// instance member binds `name`. The walk stops at the member, after
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
                NodeKind::Block => {
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

    /// Local declarations of a block that precede the use: variables
    /// (`VariablesDeclaration`) and pattern bindings.
    fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
        for decl in c.collect_direct(block) {
            match c.kind(decl) {
                NodeKind::VariablesDeclaration => {
                    for var in c.collect_direct(decl) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        if let Some(var_name) = crate::rules::member_name(c, var)
                            && c.lexeme(c.head_token(var_name)) == name
                            && c.byte_span(var_name).start < use_start
                        {
                            return true;
                        }
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

    /// Whether the enclosing class body declares an instance member named
    /// `name` (a non-static field or a non-static method/getter/setter).
    fn owner_declares_instance_member(c: &Check<'_>, name: &str) -> bool {
        // The member's owner: the parent of the enclosing member on the path.
        let Some(member_index) = c.path.iter().rposition(|&node| {
            matches!(
                c.kind(node),
                NodeKind::Method | NodeKind::Constructor | NodeKind::PrimaryConstructorBody
            )
        }) else {
            return false;
        };
        let Some(&owner) = member_index.checked_sub(1).and_then(|i| c.path.get(i)) else {
            return false;
        };
        c.collect_direct(owner)
            .into_iter()
            .any(|member| match c.kind(member) {
                NodeKind::Fields => {
                    c.payload(member).first()
                        == Some(&(dartq_parser::MemberKind::NonStaticField as u32))
                        && field_names(c, member).contains(&name)
                }
                NodeKind::Method => {
                    let kind = c.payload(member).first().copied();
                    let instance = matches!(
                        kind,
                        Some(k) if k == dartq_parser::MemberKind::NonStaticMethod as u32
                            || k == dartq_parser::MemberKind::ExtensionNonStaticMethod as u32
                            || k == dartq_parser::MemberKind::ExtensionTypeNonStaticMethod as u32
                    );
                    instance
                        && crate::rules::member_name(c, member)
                            .is_some_and(|member_node| c.lexeme(c.head_token(member_node)) == name)
                }
                _ => false,
            })
    }

    /// The field names of one `Fields` member.
    fn field_names<'a>(c: &'a Check<'_>, fields: u32) -> Vec<&'a str> {
        c.collect_direct(fields)
            .into_iter()
            .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
            .flat_map(|decl| c.collect_direct(decl))
            .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
            .filter_map(|var| crate::rules::member_name(c, var))
            .map(|name| c.lexeme(c.head_token(name)))
            .collect()
    }

    /// Whether the compilation unit declares any top-level name `name`
    /// (any namespace — values and types both make the official
    /// resolution find a different element than the member, :87-95).
    fn unit_declares_top_level_name(c: &Check<'_>, name: &str) -> bool {
        let Some(root) = c.path.first().copied() else {
            return false;
        };
        for top in c.collect_direct(root) {
            match c.kind(top) {
                NodeKind::TopLevelMethod | NodeKind::TopLevelDeclaration
                    if crate::rules::member_name(c, top)
                        .is_some_and(|n| c.lexeme(c.head_token(n)) == name) =>
                {
                    return true;
                }
                NodeKind::TopLevelFields => {
                    for decl in c.collect_direct(top) {
                        if c.kind(decl) == NodeKind::VariablesDeclaration
                            && c.collect_direct(decl)
                                .into_iter()
                                .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                                .filter_map(|var| crate::rules::member_name(c, var))
                                .any(|n| c.lexeme(c.head_token(n)) == name)
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
}

pub(crate) mod prefer_interpolation_to_compose_strings {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/prefer_interpolation_to_compose_strings.dart:32-88`
    /// — an adjacent `(literal, value)` pair in a `+` chain reports over the
    /// pair span.
    ///
    /// Approximations (documented): the left-operand String type check
    /// (`leftOperand.staticType.isDartCoreString`, :85) is approximated by
    /// the chain-face guarantees — with a literal on either side and valid
    /// code, the left operand is a string unless a user `operator+` class
    /// shadows (which over-reports). Chains report once at the chain root;
    /// the official's per-node re-visit would re-report inner pairs.
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
        let chain = flatten(c, node);
        let mut skip_next = false;
        for pair in chain.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if skip_next {
                skip_next = false;
                continue;
            }
            let left_literal = c.kind(left) == NodeKind::StringLiteral;
            let right_literal = c.kind(right) == NodeKind::StringLiteral;
            // OK(#735): neither side is a literal.
            if !left_literal && !right_literal {
                continue;
            }
            // OK(#2490): a raw string on either side.
            if (left_literal && is_raw(c, left)) || (right_literal && is_raw(c, right)) {
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
            // `leftOperand.staticType?.isDartCoreString` — true for a
            // literal, presumed for the non-literal face (the documented
            // operator+ divergence).
            let start = c.utf16_span(left).start;
            let end = c.utf16_span(right).end;
            c.report(start..end, &[]);
            // The official skips the pair consuming the reported right
            // operand (:86-88).
            skip_next = true;
        }
    }

    /// The flattened `+` chain (`chainedAdditions`, :90-98).
    fn flatten(c: &Check<'_>, node: u32) -> Vec<u32> {
        let kids = c.collect_direct(node);
        let mut chain = Vec::new();
        let left = kids.first().copied();
        if let Some(left) = left {
            if c.kind(left) == NodeKind::BinaryExpression
                && c.lexeme(c.payload(left).first().copied().expect("binary op")) == "+"
            {
                chain = flatten(c, left);
            } else {
                chain.push(left);
            }
        }
        if let Some(right) = kids.get(1).copied() {
            chain.push(right);
        }
        chain
    }

    /// `SimpleStringLiteral.isRaw`: the literal opens with `r`.
    fn is_raw(c: &Check<'_>, literal: u32) -> bool {
        c.lexeme(c.head_token(literal)).starts_with('r')
    }

    /// `isToStringInvocationWithArguments`: a `toString` call with a
    /// non-empty argument list.
    fn is_to_string_with_arguments(c: &Check<'_>, node: u32) -> bool {
        if c.kind(node) != NodeKind::Send {
            return false;
        }
        let kids = c.collect_direct(node);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments))
        else {
            return false;
        };
        if c.collect_direct(kids[args_pos]).is_empty() {
            return false;
        }
        let Some(callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return false;
        };
        match c.kind(*callee) {
            NodeKind::Identifier => c.lexeme(c.head_token(*callee)) == "toString",
            NodeKind::DotAccess | NodeKind::Qualified => c
                .collect_direct(*callee)
                .last()
                .filter(|&&name| c.kind(name) == NodeKind::Identifier)
                .is_some_and(|&name| c.lexeme(c.head_token(name)) == "toString"),
            _ => false,
        }
    }
}

pub(crate) mod prefer_final_fields {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, member_name};

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
    /// declaration carries an initializer, and the enclosing
    /// `DeclarationBody` (the constructor set faces scan its constructors).
    struct Candidate<'a> {
        declaration: u32,
        name: &'a str,
        has_initializer: bool,
        class_body: u32,
    }

    fn check(c: &mut Check<'_>) {
        let Some(root) = c.unit.ast.root() else {
            return;
        };
        let mut candidates: Vec<Candidate<'_>> = Vec::new();
        let mut mutated: Vec<&str> = Vec::new();
        scan(c, root, None, &mut candidates, &mut mutated);
        for candidate in &candidates {
            if mutated.contains(&candidate.name) {
                continue;
            }
            // The constructors of the field's own class (:62-71). Factories
            // are `ConstructorDeclaration`s in the official and count for
            // the "set in every constructor" question; they never set.
            let constructors: Vec<u32> = c
                .collect_direct(candidate.class_body)
                .into_iter()
                .filter(|&member| {
                    matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory)
                })
                .collect();
            let set =
                |ctor: u32| sets_field(c, ctor, candidate.name);
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
        class_body: Option<u32>,
        candidates: &mut Vec<Candidate<'a>>,
        mutated: &mut Vec<&'a str>,
    ) {
        match c.kind(node) {
            NodeKind::EnumDeclaration | NodeKind::ExtensionTypeDeclaration => return,
            NodeKind::DeclarationBody => {
                for kid in c.collect_direct(node) {
                    scan(c, kid, Some(node), candidates, mutated);
                }
                return;
            }
            NodeKind::Fields => {
                for declaration in c.collect_direct(node) {
                    if c.kind(declaration) != NodeKind::VariablesDeclaration {
                        continue;
                    }
                    let keyword = c
                        .payload(declaration)
                        .first()
                        .copied()
                        .unwrap_or(NONE_TOKEN);
                    if keyword != NONE_TOKEN && matches!(c.lexeme(keyword), "final" | "const") {
                        continue;
                    }
                    let Some(body) = class_body else { continue };
                    for var in c.collect_direct(declaration) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        let Some(name) = member_name(c, var) else { continue };
                        let name = c.lexeme(c.head_token(name));
                        if !name.starts_with('_') {
                            continue;
                        }
                        let has_initializer = c
                            .collect_direct(var)
                            .iter()
                            .any(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
                        candidates.push(Candidate {
                            declaration: var,
                            name,
                            has_initializer,
                            class_body: body,
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
            scan(c, kid, class_body, candidates, mutated);
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
                    && c
                        .payload(id)
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
    /// -1` reports with the rewrite template.
    #[test]
    fn contains_reports_index_of_equals_minus_one() {
        let source = "list.indexOf(e) == -1;";
        // list0 .1 indexOf2 (3 e4 )5 ==6 -7 18 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 6);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 6);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.begin(NodeKind::UnaryPrefix, 7);
                b.leaf(NodeKind::LiteralInt, 8);
                b.end(NodeKind::UnaryPrefix, 10);
                b.end(NodeKind::BinaryExpression, 10);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
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
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
    }

    /// The always-false/always-true judgments carry their own texts and no
    /// correction; `>= 0` rewrites.
    #[test]
    fn contains_message_matrix() {
        let build = |source: &str, literal: u32, negated: bool| {
            Fixture::build(
                source,
                unit_of(|b, _t| {
                    b.begin(NodeKind::ExpressionStatement, 0);
                    b.begin(NodeKind::BinaryExpression, 0);
                    b.patch(0, literal);
                    b.begin(NodeKind::Send, 0);
                    b.begin(NodeKind::DotAccess, 0);
                    b.patch(0, 1);
                    b.patch(1, 0);
                    identifier(b, 0, 1);
                    identifier(b, 2, 3);
                    b.end(NodeKind::DotAccess, 6);
                    b.begin(NodeKind::Arguments, 3);
                    identifier(b, 4, 5);
                    b.end(NodeKind::Arguments, 6);
                    b.end(NodeKind::Send, 6);
                    if negated {
                        b.begin(NodeKind::UnaryPrefix, literal + 1);
                        b.leaf(NodeKind::LiteralInt, literal + 2);
                        b.end(NodeKind::UnaryPrefix, literal + 4);
                    } else {
                        b.leaf(NodeKind::LiteralInt, literal + 1);
                    }
                    // Negated: `-` literal+1, digit literal+2, `;` literal+3
                    // (exclusive end literal+4); plain: digit literal+1,
                    // `;` literal+2 (exclusive end literal+3).
                    let end = if negated { literal + 4 } else { literal + 3 };
                    b.end(NodeKind::BinaryExpression, end);
                    b.end(NodeKind::ExpressionStatement, end);
                }),
            )
        };
        // list0 .1 indexOf2 (3 e4 )5 <6 -7 18 ;9
        let always_false = build("list.indexOf(e) < -1;", 6, true);
        let lints = always_false.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Always 'false' because 'indexOf' is always greater than or equal to -1."
        );
        assert_eq!(lints[0].correction, None);

        // list0 .1 indexOf2 (3 e4 )5 >=6 -7 18 ;9
        let always_true = build("list.indexOf(e) >= -1;", 6, true);
        let lints = always_true.run(&one_rule("prefer_contains"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Always 'true' because 'indexOf' is always greater than or equal to -1."
        );

        // list0 .1 indexOf2 (3 e4 )5 >=6 07 ;8
        let rewrite = build("list.indexOf(e) >= 0;", 6, false);
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
        let source = "-1 != list.indexOf(e);";
        // -0 11 !=2 list3 .4 indexOf5 (6 e7 )8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 2);
                b.begin(NodeKind::UnaryPrefix, 0);
                b.leaf(NodeKind::LiteralInt, 1);
                b.end(NodeKind::UnaryPrefix, 3);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::DotAccess, 3);
                b.patch(0, 4);
                b.patch(1, 0);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.end(NodeKind::DotAccess, 9);
                b.begin(NodeKind::Arguments, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::BinaryExpression, 10);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
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
        let assigned = "(next = list.indexOf('{')) != -1;";
        // (0 next1 =2 list3 .4 indexOf5 (6 '7 )8 )9 !=10 -11 112 ;13
        let f = Fixture::build(
            assigned,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 10);
                b.begin(NodeKind::ParenthesizedExpression, 0);
                b.begin(NodeKind::Assignment, 1);
                b.patch(0, 2);
                identifier(b, 1, 2);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::DotAccess, 3);
                b.patch(0, 4);
                b.patch(1, 0);
                identifier(b, 3, 4);
                identifier(b, 5, 6);
                b.end(NodeKind::DotAccess, 9);
                b.begin(NodeKind::Arguments, 6);
                b.leaf_span(NodeKind::StringLiteral, 7, 8);
                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::Assignment, 9);
                b.end(NodeKind::ParenthesizedExpression, 10);
                b.begin(NodeKind::UnaryPrefix, 11);
                b.leaf(NodeKind::LiteralInt, 12);
                b.end(NodeKind::UnaryPrefix, 14);
                b.end(NodeKind::BinaryExpression, 14);
                b.end(NodeKind::ExpressionStatement, 14);
            }),
        );
        assert!(f.run(&one_rule("prefer_contains")).is_empty());

        let bare = "indexOf(e) == -1;";
        // indexOf0 (1 e2 )3 ==4 -5 16 ;7
        let f = Fixture::build(
            bare,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 4);
                b.begin(NodeKind::Send, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Arguments, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::Send, 4);
                b.begin(NodeKind::UnaryPrefix, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::UnaryPrefix, 8);
                b.end(NodeKind::BinaryExpression, 8);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        assert!(f.run(&one_rule("prefer_contains")).is_empty());

        let started = "list.indexOf(e, 1) == -1;";
        // list0 .1 indexOf2 (3 e4 ,5 16 )7 ==8 -9 110 ;11
        let f = Fixture::build(
            started,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 8);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 7);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.begin(NodeKind::UnaryPrefix, 9);
                b.leaf(NodeKind::LiteralInt, 10);
                b.end(NodeKind::UnaryPrefix, 12);
                b.end(NodeKind::BinaryExpression, 12);
                b.end(NodeKind::ExpressionStatement, 12);
            }),
        );
        assert!(f.run(&one_rule("prefer_contains")).is_empty());
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
    /// — `x.length == 0` reports with the isEmpty rewrite.
    #[test]
    fn prefer_is_empty_reports_length_eq_zero() {
        let source = "x.length == 0;";
        // x0 .1 length2 ==3 04 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 3);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::BinaryExpression, 6);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
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
    }

    /// A value outside the matrix passes (`== 2`); the always-true face
    /// carries its own text and no correction (`x.length >= -1`).
    #[test]
    fn prefer_is_empty_matrix_faces() {
        let plain = "x.length == 2;";
        let f = Fixture::build(
            plain,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 3);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::BinaryExpression, 6);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        assert!(f.run(&one_rule("prefer_is_empty")).is_empty());

        let source = "x.length >= -1;";
        // x0 .1 length2 >=3 -4 15 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, 3);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.begin(NodeKind::UnaryPrefix, 4);
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::UnaryPrefix, 7);
                b.end(NodeKind::BinaryExpression, 7);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_is_empty"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The comparison is always 'true' because the length is always greater than or equal to 0."
        );
        assert_eq!(lints[0].correction, None);
    }

    // —— prefer_is_not_empty ———————————————————————————————————————————————

    /// `test/rules/prefer_is_not_empty_test.dart` `test_notIsEmpty` — the
    /// negated `isEmpty` reports at the prefix.
    #[test]
    fn prefer_is_not_empty_reports_bang_is_empty() {
        let source = "!x.isEmpty;";
        // !0 x1 .2 isEmpty3 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::UnaryPrefix, 0);
                b.begin(NodeKind::DotAccess, 1);
                b.patch(0, 2);
                b.patch(1, 0);
                identifier(b, 1, 2);
                identifier(b, 3, 4);
                b.end(NodeKind::DotAccess, 4);
                b.end(NodeKind::UnaryPrefix, 5);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        let lints = f.run(&one_rule("prefer_is_not_empty"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'isNotEmpty' rather than negating the result of 'isEmpty'."
        );
    }

    /// A negated `isNotEmpty` (or any other property) passes.
    #[test]
    fn prefer_is_not_empty_other_property_passes() {
        let source = "!x.isNotEmpty;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::UnaryPrefix, 0);
                b.begin(NodeKind::DotAccess, 1);
                b.patch(0, 2);
                b.patch(1, 0);
                identifier(b, 1, 2);
                identifier(b, 3, 4);
                b.end(NodeKind::DotAccess, 4);
                b.end(NodeKind::UnaryPrefix, 5);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        assert!(f.run(&one_rule("prefer_is_not_empty")).is_empty());
    }

    // —— avoid_function_literals_in_foreach_calls ——————————————————————————

    /// `test/rules/avoid_function_literals_in_foreach_calls_test.dart`
    /// `test forEach with function literal` — the method name reports.
    #[test]
    fn foreach_literal_reports_the_method_name() {
        let source = "x.forEach((e) {});";
        // x0 .1 forEach2 (3 e4 )5 {6 }7 )8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.begin(NodeKind::Arguments, 3);
                b.begin(NodeKind::FunctionExpression, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 4);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 4, 5);
                b.end(NodeKind::FormalParameter, 6);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::BlockBody, 6);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::BlockBody, 8);
                b.end(NodeKind::FunctionExpression, 8);
                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
        let lints = f.run(&one_rule("avoid_function_literals_in_foreach_calls"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Function literals shouldn't be passed to 'forEach'."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    /// The `Map.forEach` face — a two-parameter closure — passes through the
    /// arity gate, and a cascade section passes through the cascade guard.
    #[test]
    fn foreach_two_params_and_cascade_pass() {
        let source = "map.forEach((k, v) {});";
        // map0 .1 forEach2 (3 k4 ,5 v6 )7 {8 }9 )10 ;11
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.begin(NodeKind::Arguments, 3);
                b.begin(NodeKind::FunctionExpression, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 4);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 4, 5);
                b.end(NodeKind::FormalParameter, 5);
                b.begin(NodeKind::FormalParameter, 6);
                b.patch(0, NONE);
                b.patch(1, 6);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 6, 7);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::BlockBody, 8);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 8);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::BlockBody, 10);
                b.end(NodeKind::FunctionExpression, 10);
                b.end(NodeKind::Arguments, 11);
                b.end(NodeKind::Send, 11);
                b.end(NodeKind::ExpressionStatement, 12);
            }),
        );
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );

        let cascade = "x..forEach((e) {});";
        // x0 ..1 forEach2 (3 e4 )5 {6 }7 )8 ;9
        let f = Fixture::build(
            cascade,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 2);
                b.patch(0, 2);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 3);
                b.begin(NodeKind::Arguments, 3);
                b.begin(NodeKind::FunctionExpression, 3);
                b.begin(NodeKind::FormalParameters, 3);
                b.begin(NodeKind::FormalParameter, 3);
                b.patch(0, NONE);
                b.patch(1, 4);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 4, 5);
                b.end(NodeKind::FormalParameter, 6);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::BlockBody, 6);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::BlockBody, 8);
                b.end(NodeKind::FunctionExpression, 8);
                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::CascadeAccess, 9);
                b.end(NodeKind::Cascade, 9);
                b.end(NodeKind::ExpressionStatement, 10);
            }),
        );
        assert!(
            f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
                .is_empty()
        );
    }

    // —— prefer_interpolation_to_compose_strings ———————————————————————————

    /// `'a' + b` reports over the pair span.
    #[test]
    fn interpolation_reports_literal_plus_value() {
        let source = "'a' + b;";
        // 'a'0 +1 b2 ;3
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "+", 0));
                b.leaf_span(NodeKind::StringLiteral, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::BinaryExpression, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        let lints = f.run(&one_rule("prefer_interpolation_to_compose_strings"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use interpolation to compose strings and values."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 0));
        assert_eq!(lints[0].range.end, utf16_at(&f, 3));
    }

    /// Both literals and a raw string pass (:70-75).
    #[test]
    fn interpolation_both_literals_and_raw_pass() {
        let source = "'a' + 'b';";
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(source, t, "+", 0));
                b.leaf_span(NodeKind::StringLiteral, 0, 1);
                b.leaf_span(NodeKind::StringLiteral, 2, 3);
                b.end(NodeKind::BinaryExpression, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .is_empty()
        );

        let raw = "'a' + r'b';";
        // 'a'0 +1 r'b'2 ;3
        let f = Fixture::build(
            raw,
            unit_of(|b, t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::BinaryExpression, 0);
                b.patch(0, tok(raw, t, "+", 0));
                b.leaf_span(NodeKind::StringLiteral, 0, 1);
                b.leaf_span(NodeKind::StringLiteral, 2, 3);
                b.end(NodeKind::BinaryExpression, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_interpolation_to_compose_strings"))
                .is_empty()
        );
    }

    // —— null_closures ——————————————————————————————————————————————————————

    /// `Future(null)` — a `null` in the unnamed-constructor closure slot
    /// reports at the null (`test/rules/null_closures_test.dart`
    /// `test_Future`).
    #[test]
    fn null_closures_reports_a_null_future_callback() {
        let source = "Future(null);";
        // Future0 (1 null2 )3 ;4
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ImplicitCreation, 0);
                b.patch(0, NONE);
                identifier(b, 0, 1);
                b.begin(NodeKind::Arguments, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::ImplicitCreation, 4);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Closure can't be 'null' because it might be invoked."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    /// `new Future(null)` reports the same way through the explicit-new
    /// shape.
    #[test]
    fn null_closures_reports_an_explicit_new_creation() {
        let source = "new Future(null);";
        // new0 Future1 (2 null3 )4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::NewExpression, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::Arguments, 2);
                b.leaf(NodeKind::LiteralNull, 3);
                b.end(NodeKind::Arguments, 5);
                b.end(NodeKind::NewExpression, 5);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 3));
    }

    /// `Future.wait(fs, cleanUp: null)` — the named slot reports over the
    /// whole named argument (`test_nullFromFuture_wait`).
    #[test]
    fn null_closures_reports_a_named_null_over_the_argument() {
        let source = "Future.wait(fs, cleanUp: null);";
        // Future0 .1 wait2 (3 fs4 ,5 cleanUp6 :7 null8 )9 ;10
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.begin(NodeKind::NamedArgument, 6);
                b.patch(0, 7);
                identifier(b, 6, 7);
                b.leaf(NodeKind::LiteralNull, 8);
                b.end(NodeKind::NamedArgument, 9);
                b.end(NodeKind::Arguments, 10);
                b.end(NodeKind::Send, 10);
                b.end(NodeKind::ExpressionStatement, 11);
            }),
        );
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
        assert_eq!(lints[0].range.end, utf16_at(&f, 9));
    }

    /// `Timer.periodic(d, null)` — the instance-creation named-constructor
    /// face, positional slot 1.
    #[test]
    fn null_closures_reports_a_periodic_timer_callback() {
        let source = "Timer.periodic(d, null);";
        // Timer0 .1 periodic2 (3 d4 ,5 null6 )7 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, NONE);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::ExpressionStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("null_closures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    }

    /// A non-null closure passes, and a bare `map(null)` never matches —
    /// the official bails with a null target (null_closures.dart:222).
    #[test]
    fn null_closures_non_null_and_bare_calls_pass() {
        let non_null = "list.map(f);";
        // list0 .1 map2 (3 f4 )5 ;6
        let f = Fixture::build(
            non_null,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, NONE);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(f.run(&one_rule("null_closures")).is_empty());

        let bare = "map(null);";
        // map0 (1 null2 )3 ;4
        let f = Fixture::build(
            bare,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ImplicitCreation, 0);
                b.patch(0, NONE);
                identifier(b, 0, 1);
                b.begin(NodeKind::Arguments, 1);
                b.leaf(NodeKind::LiteralNull, 2);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::ImplicitCreation, 4);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        assert!(f.run(&one_rule("null_closures")).is_empty());
    }

    // —— unnecessary_this ————————————————————————————————————————————————————

    /// `this.x` in an instance method over a same-class field reports
    /// zero-width at the `this` (`test/rules/unnecessary_this_test.dart`
    /// `test_UnnecessaryThis`).
    #[test]
    fn unnecessary_this_reports_a_field_access() {
        let source = "class C { int x = 0; int f() { return this.x; } }";
        // class0 C1 {2 int3 x4 =5 06 ;7 int8 f9 (10 )11 {12 return13 this14 .15 x16 ;17 }18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
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
                b.begin(NodeKind::Method, 8);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 10);
                b.patch(3, NONE);
                identifier(b, 9, 10);
                b.begin(NodeKind::FormalParameters, 10);
                b.end(NodeKind::FormalParameters, 12);
                b.begin(NodeKind::BlockBody, 12);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::ReturnStatement, 13);
                b.begin(NodeKind::DotAccess, 14);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 14);
                identifier(b, 16, 17);
                b.end(NodeKind::DotAccess, 17);
                b.end(NodeKind::ReturnStatement, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::BlockBody, 19);
                b.end(NodeKind::Method, 19);
                b.end(NodeKind::DeclarationBody, 20);
                b.end(NodeKind::ClassDeclaration, 20);
                b.end(NodeKind::TopLevelDeclaration, 20);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_this"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary 'this.' qualifier.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 14));
    }

    /// A parameter of the enclosing member shadows — the `this.` stays
    /// (unnecessary_this.dart:87-95, `isRequestedName` branch).
    #[test]
    fn unnecessary_this_parameter_shadow_passes() {
        let source = "class C { int f(int x) { return this.x; } }";
        // class0 C1 {2 int3 f4 (5 int6 x7 )8 {9 return10 this11 .12 x13 ;14 }15 }16
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 5);
                b.patch(3, NONE);
                b.leaf_span(NodeKind::Type, 3, 4);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.begin(NodeKind::FormalParameter, 6);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.leaf_span(NodeKind::Type, 6, 7);
                b.end(NodeKind::FormalParameter, 8);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::BlockBody, 9);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::ReturnStatement, 10);
                b.begin(NodeKind::DotAccess, 11);
                b.patch(0, 12);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 11);
                identifier(b, 13, 14);
                b.end(NodeKind::DotAccess, 14);
                b.end(NodeKind::ReturnStatement, 15);
                b.end(NodeKind::Block, 16);
                b.end(NodeKind::BlockBody, 16);
                b.end(NodeKind::Method, 16);
                b.end(NodeKind::DeclarationBody, 17);
                b.end(NodeKind::ClassDeclaration, 17);
                b.end(NodeKind::TopLevelDeclaration, 17);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_this")).is_empty());
    }

    /// A local declared before the use in the same block shadows.
    #[test]
    fn unnecessary_this_local_shadow_passes() {
        let source = "class C { int f() { int x = 1; return this.x; } }";
        // class0 C1 {2 int3 f4 (5 )6 {7 int8 x9 =10 111 ;12 return13 this14 .15 x16 ;17 }18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelDeclaration, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Method, 2);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, 5);
                b.patch(3, NONE);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::BlockBody, 7);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::VariablesDeclaration, 8);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 8, 9);
                b.begin(NodeKind::InitializedIdentifier, 9);
                identifier(b, 9, 10);
                b.begin(NodeKind::VariableInitializer, 10);
                b.leaf(NodeKind::LiteralInt, 11);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ReturnStatement, 13);
                b.begin(NodeKind::DotAccess, 14);
                b.patch(0, 15);
                b.patch(1, NONE);
                b.leaf(NodeKind::ThisExpression, 14);
                identifier(b, 16, 17);
                b.end(NodeKind::DotAccess, 17);
                b.end(NodeKind::ReturnStatement, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::BlockBody, 19);
                b.end(NodeKind::Method, 19);
                b.end(NodeKind::DeclarationBody, 20);
                b.end(NodeKind::ClassDeclaration, 20);
                b.end(NodeKind::TopLevelDeclaration, 20);
            }),
        );
        assert!(f.run(&one_rule("unnecessary_this")).is_empty());
    }

    /// The written `this` on a constructor field initializer always reports
    /// (unnecessary_this.dart:31-35); the bare `x = 1;` form passes.
    #[test]
    fn unnecessary_this_reports_an_initializer_this_and_bare_passes() {
        let this_form = "class C { int x = 0; C() : this.x = 1; }";
        // class0 C1 {2 int3 x4 =5 06 ;7 C8 (9 )10 :11 this12 .13 x14 =15 116 ;17 }18
        let f = Fixture::build(
            this_form,
            unit_of(|b, _t| {
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
                b.begin(NodeKind::Initializers, 11);
                b.begin(NodeKind::Initializer, 12);
                b.begin(NodeKind::FieldInitializer, 15);
                b.patch(0, 15);
                b.leaf(NodeKind::LiteralInt, 16);
                b.end(NodeKind::FieldInitializer, 17);
                b.end(NodeKind::Initializer, 17);
                b.end(NodeKind::Initializers, 18);
                b.leaf(NodeKind::EmptyBody, 17);
                b.end(NodeKind::Constructor, 18);
                b.end(NodeKind::DeclarationBody, 19);
                b.end(NodeKind::ClassDeclaration, 19);
                b.end(NodeKind::TopLevelDeclaration, 19);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_this"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 12));

        let bare_form = "class C { int x = 0; C() : x = 1; }";
        // class0 C1 {2 int3 x4 =5 06 ;7 C8 (9 )10 :11 x12 =13 114 ;15 }16
        let f = Fixture::build(
            bare_form,
            unit_of(|b, _t| {
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
                b.begin(NodeKind::Initializers, 11);
                b.begin(NodeKind::Initializer, 12);
                b.begin(NodeKind::FieldInitializer, 13);
                b.patch(0, 13);
                b.leaf(NodeKind::LiteralInt, 14);
                b.end(NodeKind::FieldInitializer, 15);
                b.end(NodeKind::Initializer, 15);
                b.end(NodeKind::Initializers, 16);
                b.leaf(NodeKind::EmptyBody, 15);
                b.end(NodeKind::Constructor, 16);
                b.end(NodeKind::DeclarationBody, 17);
                b.end(NodeKind::ClassDeclaration, 17);
                b.end(NodeKind::TopLevelDeclaration, 17);
            }),
        );
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
        Fixture::build(
            "class C {\n  int _x = 0;\n}\n",
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
                b.end(NodeKind::DeclarationBody, 8);
                b.end(NodeKind::ClassDeclaration, 8);
                b.end(NodeKind::TopLevelDeclaration, 9);
                extra(b, t);
                b.end(NodeKind::CompilationUnit, t.len() as u32);
            },
        )
    }

    fn field_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
        f.run(&one_rule("prefer_final_fields"))
    }

    /// An assignment statement over the field name, as a unit-level sibling.
    fn assignment_statement(b: &mut dartq_ast::AstBuilder, name_at: u32, eq_at: u32, value_at: u32) {
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
            b.patch(4, dartq_parser::FormalParameterKind::RequiredPositional as u32);
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
        let this_form = Fixture::build("class C {\n  int _x;\n  C() : this._x = 0;\n}\n", |b, t| {
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
                b.patch(4, dartq_parser::FormalParameterKind::RequiredPositional as u32);
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
}
