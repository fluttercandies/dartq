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
    fn int_value(c: &Check<'_>, node: u32) -> Option<i64> {
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
}
