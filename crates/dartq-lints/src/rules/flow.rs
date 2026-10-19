//! The flow-control family: `curly_braces_in_flow_control_structures`,
//! `control_flow_in_finally`, `no_duplicate_case_values`,
//! `use_rethrow_when_possible`, `avoid_returning_null_for_void`,
//! `use_test_throws_matchers`.

use dartq_ast::NodeKind;

use crate::engine::{Check, narrow_u32};
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, last_child, utf16_token_span};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// Official: `pkg/linter/lib/src/rules/curly_braces_in_flow_control_structures.dart:36-95`
/// — control-flow bodies must be blocks, except a single-line `if` without
/// `else` (`if (c) return;` on one line is allowed, :59-70). Message:
/// `diagnostic.g.dart` `curlyBracesInFlowControlStructures` with the
/// construct spelled into `{0}`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "curly_braces_in_flow_control_structures",
    group: Group::Core,
    problem: "Statements in {0} should be enclosed in a block.",
    correction: "Try wrapping the statement in a block.",
    kinds: &[
        NodeKind::IfStatement,
        NodeKind::WhileStatement,
        NodeKind::ForStatement,
        NodeKind::ForIn,
        NodeKind::DoWhileStatement,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::WhileStatement => {
            // Body is the last child (the condition is the first,
            // the `WhileStatement` layout).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a while");
            }
        }
        NodeKind::ForStatement | NodeKind::ForIn => {
            // Body is the last child after the for-parts (:82-84); a for-in
            // seals the same group shape (`ForInParts` then the body).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a for");
            }
        }
        NodeKind::DoWhileStatement => {
            // Body rows start at the token after `do` (the children segment
            // before the `while (...)` condition); the statement leaf seals
            // after its spilled inner fragments, so the widest row at the
            // body's head token is the body statement.
            if let Some(body) = widest_row_at(c, node, c.head_token(node) + 1) {
                check_body(c, body, "a do");
            }
        }
        NodeKind::IfStatement => check_if(c, node),
        _ => {}
    }
}

/// The window row starting at `token` that seals last: the statement row
/// among the fragments tiled at the same head (the production tiling spills
/// an arm body's inner expression rows into the owner window, all anchored
/// at the body's first token).
fn widest_row_at(c: &Check<'_>, node: u32, token: u32) -> Option<u32> {
    let start = utf16_token_span(c, token).start;
    let mut best: Option<u32> = None;
    let mut best_end = 0;
    for kid in c.children(node) {
        let span = c.utf16_span(kid);
        if span.start != start {
            continue;
        }
        if best.is_none_or(|_| span.end >= best_end) {
            best = Some(kid);
            best_end = span.end;
        }
    }
    best
}

/// `_check` (curly_braces...dart:93-95): a non-block body reports.
fn check_body(c: &mut Check<'_>, body: u32, where_: &str) {
    if c.kind(body) != NodeKind::Block {
        let range = c.utf16_span(body);
        c.report(range, &[where_]);
    }
}

fn check_if(c: &mut Check<'_>, node: u32) {
    let else_present = c.payload(node).first().copied() != Some(NONE_TOKEN);
    // The arms are selected structurally, never by fixed child index: a
    // production parse tiles the condition into fragment rows ahead of the
    // arms (the head `Identifier`, the `NoTypeArguments`/`NoArguments`
    // sentinel leaves, the adopted `ParenthesizedCondition`), so positional
    // indexing lands on those fragments. The else arm — when present — is
    // the last child; the then arm is the geometric selection of
    // [`then_arm`]; without an `else` the then arm is the last child, as
    // the while/for bodies are.
    let Some(last) = last_child(c, node) else {
        return;
    };
    if !else_present {
        check_if_without_else(c, node, last);
        return;
    }
    if let Some(then) = then_arm(c, node, last) {
        check_body(c, then, "an if");
    }
    // An else-if chain arm is skipped (`elseStatement is! IfStatement`,
    // :71-77).
    if c.kind(last) != NodeKind::IfStatement {
        check_body(c, last, "an if");
    }
}

/// The then arm of an if with an `else`: the production tiling spills the
/// arms' inner expression rows into the statement window around the
/// `ExpressionStatement` leaves, so arena adjacency (the row before the
/// else subtree) can land on the else side's spilled head. The selection is
/// geometric instead: among the window rows closing before the `else`
/// keyword (payload word 0), the one sealing last is the then arm — the
/// arm's own statement leaf seals after its inner fragments. `None` when
/// the else subtree opens at the statement window's head (no recovered then
/// arm).
fn then_arm(c: &Check<'_>, if_stmt: u32, else_stmt: u32) -> Option<u32> {
    let else_token = c.payload(if_stmt).first().copied()?;
    let else_start = utf16_token_span(c, else_token).start;
    let mut best: Option<u32> = None;
    let mut best_end = 0;
    for kid in c.children(if_stmt) {
        if kid == else_stmt {
            continue;
        }
        let span = c.utf16_span(kid);
        if span.start >= else_start || span.end > else_start {
            continue;
        }
        if best.is_none_or(|_| span.end >= best_end) {
            best = Some(kid);
            best_end = span.end;
        }
    }
    best
}

/// The no-`else` face (:55-69): an if that IS an outer if's else branch
/// reports unconditionally (the outer chain already requires blocks);
/// otherwise the single-line form is allowed and only a line-spanning arm
/// reports.
fn check_if_without_else(c: &mut Check<'_>, node: u32, then: u32) {
    if let Some(parent) = c.ancestor(1)
        && c.kind(parent) == NodeKind::IfStatement
        && c.payload(parent).first().copied() != Some(NONE_TOKEN)
        && last_child(c, parent) == Some(node)
    {
        check_body(c, then, "an if");
        return;
    }
    // Single-line form allowed (:62-69): report only when the statement
    // spans lines (`lineOf(if) != lineOf(then.end)`).
    if c.kind(then) == NodeKind::Block {
        return;
    }
    let if_line = c
        .unit
        .lines
        .line_col_utf16(c.unit.source, c.utf16_span(node).start)
        .0;
    let then_end = c.utf16_span(then).end;
    let then_line = c.unit.lines.line_col_utf16(c.unit.source, then_end).0;
    if if_line != then_line {
        let range = c.utf16_span(then);
        c.report(range, &["an if"]);
    }
}

// Per-rule modules (mechanical split 2026-10-19; registry paths unchanged).
pub(crate) mod control_flow_in_finally;
pub(crate) mod no_duplicate_case_values;
pub(crate) mod use_rethrow_when_possible;
pub(crate) mod avoid_returning_null_for_void;
pub(crate) mod type_literal_in_constant_pattern;
pub(crate) mod exhaustive_cases;
pub(crate) mod always_put_control_body_on_new_line;
pub(crate) mod avoid_catches_without_on_clauses;
pub(crate) mod require_trailing_commas;
pub(crate) mod use_test_throws_matchers;
#[cfg(test)]
mod tests;
/// The Dart-3.0 migration family: legacy `case` heads whose expression
/// changes meaning (or errors) under patterns.
pub(crate) mod invalid_case_patterns;
/// The pattern-shape family: a variable pattern whose explicit field name
/// spells the variable twice.
pub(crate) mod simplify_variable_pattern;
/// The type-scrutinee family: a switch whose scrutinee is a `Type` value.
pub(crate) mod switch_on_type;
/// The enum-switch family: a `default` clause on a switch whose scrutinee
/// names an enum type.
pub(crate) mod no_default_cases;
/// The join family: an assignment statement a following `return` makes
/// redundant.
pub(crate) mod join_return_with_assignment;
/// The catch family: an `on` clause catching the dart:core `Error`
/// family.
pub(crate) mod avoid_catching_errors;
/// The W5 (#1124) approximation pins over real parser trees (the rule
/// dispatches [`NodeKind::TryStatement`] — production parses throughout).
#[cfg(test)]
mod w5_approx_pins;
pub(crate) mod use_build_context_synchronously;
#[cfg(test)]
mod use_build_context_synchronously_tests;
