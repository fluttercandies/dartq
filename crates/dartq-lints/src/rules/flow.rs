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

pub(crate) mod control_flow_in_finally {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec, last_child};

    /// Official: `pkg/linter/lib/src/rules/control_flow_in_finally.dart:24-107`
    /// — `break`/`continue`/`return` inside a `finally` block reports, unless
    /// the statement sits inside a closure within the block (its jump binds
    /// locally), or — for targeted `break`/`continue` — the resolved target
    /// sits within the try statement (the jump stays local to it). Targets
    /// resolve through the official label machinery
    /// (`_lookupBreakOrContinueTarget`, `scope_analyzer.dart:1042-1093`): a
    /// labeled jump through the lexical label scope — the nearest enclosing
    /// labeled statement or switch member declaring the label, where a
    /// statement's label binds the statement after the labels
    /// (`visitLabeledStatement`, :631-639) — an untargeted jump through the
    /// break/continue context chain (the nearest enclosing loop; `continue`
    /// skips switches, `scope.dart:333-341`; the chain resets at every
    /// function body, :224). Message: `diagnostic.g.dart`
    /// `controlFlowInFinally`.
    ///
    /// Approximation (documented): the switch's member-label scope walk
    /// reads the members over the production tiling (the same rows the
    /// `exhaustive_cases` member walk collects); a label declared inside a
    /// case BODY (a labeled statement within a member) scopes normally.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "control_flow_in_finally",
        group: Group::Recommended,
        problem: "Use of '{0}' in a 'finally' clause.",
        correction: "Try restructuring the code.",
        kinds: &[
            NodeKind::Break,
            NodeKind::Continue,
            NodeKind::ReturnStatement,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let node_idx = c.path.len() - 1;
        let kind = match c.kind(node) {
            NodeKind::Break => "break",
            NodeKind::Continue => "continue",
            NodeKind::ReturnStatement => "return",
            _ => return,
        };
        // `node.thisOrAncestorOfType<TryStatement>()` (:45-47).
        let Some(try_idx) = c
            .path
            .iter()
            .rposition(|n| c.kind(*n) == NodeKind::TryStatement)
        else {
            return;
        };
        let try_node = c.path[try_idx];
        // `finallyBlock == null` bails (:48-54): payload word 0 is the
        // `finally` token (`TryStatementEnd`).
        if c.payload(try_node).first() == Some(&NONE_TOKEN) {
            return;
        }
        // The node must be inside the finally block — the try's last child
        // (body block, catch clauses, then the finally block).
        let Some(finally_block) = last_child(c, try_node) else {
            return;
        };
        if c.kind(finally_block) != NodeKind::Block
            || c.path.get(try_idx + 1) != Some(&finally_block)
        {
            return;
        }
        // The switch-member tiling overlaps windows (each member's window
        // starts at the previous member's arena row), which spills the
        // member's body rows both at the switch-block level and under the
        // member row the next member's window adopts — the try's subtree
        // can dispatch twice. Keep only the canonical (`ast.parent`)
        // descent; the spilled one bails.
        if c.unit.ast.parent(try_node) != c.path.get(try_idx - 1).copied() {
            return;
        }
        let report = match c.kind(node) {
            NodeKind::Break | NodeKind::Continue => match jump_target(c, node, node_idx) {
                // `enablerNode = target.thisOrAncestorMatching(n ==
                // tryStatement)` (:79-91): the target's own ancestor chain
                // containing the try keeps the jump local —
                // `thisOrAncestorMatching` includes the target itself, so a
                // label bound onto the try (`a: try …`) also counts as
                // within. Outside it the jump crosses the finally and
                // reports.
                Some(target) => !chain_contains(c, target, try_node),
                // Unresolved target (`target == null`): the closure exemption
                // decides (the `ancestor == null` branch, :69-77).
                None => !closure_wraps(c, try_idx + 1, node_idx),
            },
            _ => !closure_wraps(c, try_idx + 1, node_idx),
        };
        if report {
            let range = c.utf16_span(node);
            c.report(range, &[kind]);
        }
    }

    /// The resolved jump target's row, `None` for the official
    /// `target == null` face (`_lookupBreakOrContinueTarget`,
    /// `scope_analyzer.dart:1042-1093`).
    fn jump_target(c: &Check<'_>, node: u32, node_idx: usize) -> Option<u32> {
        // The payload carries only the `has_target` bit (a childless
        // `BreakStatementHandle`); the label identifier is the token after the
        // `break`/`continue` keyword, and a synthetic recovery token never
        // names a label.
        if c.payload(node).first().copied().unwrap_or(0) != 0 {
            let label_token = c.head_token(node) + 1;
            if c.token_is_synthetic(label_token) {
                return None;
            }
            let label = c.lexeme(label_token);
            // The label scope chain, innermost first: a labeled statement's
            // labels bind the statement after the labels
            // (`visitLabeledStatement`, scope_analyzer.dart:631-639); a
            // switch statement pre-nests EVERY member's labels over the
            // whole statement (scope_analyzer.dart:846-850), so a forward
            // `continue L` to a later member resolves there — the members
            // walk in reverse source order, a later duplicate shadowing.
            for (idx, &ancestor) in c.path[..node_idx].iter().enumerate().rev() {
                match c.kind(ancestor) {
                    NodeKind::LabeledStatement if declares_label(c, ancestor, label) => {
                        return Some(c.path[idx + 1]);
                    }
                    NodeKind::SwitchStatement => {
                        if let Some(member) = switch_member_with_label(c, ancestor, label) {
                            return Some(member);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        let is_continue = c.kind(node) == NodeKind::Continue;
        for (idx, &ancestor) in c.path[..node_idx].iter().enumerate().rev() {
            match c.kind(ancestor) {
                NodeKind::WhileStatement
                | NodeKind::ForStatement
                | NodeKind::ForIn
                | NodeKind::DoWhileStatement => return Some(c.path[idx]),
                // An untargeted `continue` never binds a switch
                // (scope.dart:336-341).
                NodeKind::SwitchStatement if !is_continue => return Some(c.path[idx]),
                // The context chain resets at every function body (:224) —
                // the jump cannot bind past a closure (`target == null`).
                NodeKind::FunctionExpression
                | NodeKind::NamedFunctionExpression
                | NodeKind::LocalFunctionDeclaration => return None,
                _ => {}
            }
        }
        None
    }

    /// The switch member whose label prefix declares `label`, later members
    /// first (the official scope chain nests member labels in source order,
    /// so the last declared wins); `None` when no member carries it. The
    /// members read over the production tiling — the plain child walk loses
    /// members (the `exhaustive_cases` member walk's documented wrinkle).
    fn switch_member_with_label(c: &Check<'_>, switch_row: u32, label: &str) -> Option<u32> {
        let block = c
            .collect_direct(switch_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)?;
        super::exhaustive_cases::switch_case_rows(c, block)
            .into_iter()
            .rev()
            .find(|&member| declares_label(c, member, label))
    }

    /// Whether `try_row` sits on `target`'s ancestor chain — the official
    /// `target.thisOrAncestorMatching((n) => n == tryStatement)` (the
    /// target itself counts). The walk ascends strictly to the root.
    fn chain_contains(c: &Check<'_>, target: u32, try_row: u32) -> bool {
        let mut current = target;
        loop {
            if current == try_row {
                return true;
            }
            match c.unit.ast.parent(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    /// Whether a labeled statement / switch member declares `label` among
    /// its label-prefix children. The production tiling emits each label as
    /// an `Identifier` (the name) + `Label` (the colon) leaf pair — the
    /// collapse map's `2·label_count` adoption — while hand-built trees
    /// carry a bare `Label` row spanning `name:`; the prefix is the
    /// leading run of both shapes, and the name reads off whichever row
    /// anchors at the identifier token.
    fn declares_label(c: &Check<'_>, node: u32, label: &str) -> bool {
        c.collect_direct(node)
            .into_iter()
            .take_while(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::Label))
            .any(|kid| c.lexeme(c.head_token(kid)) == label)
    }

    /// Whether a closure (function expression / local function) sits on the
    /// path between the finally block and the statement — the official
    /// `FunctionBody inside the finally block` exemption (:69-77); the
    /// function-kind family is the `jump_target` boundary list below.
    fn closure_wraps(c: &Check<'_>, finally_idx: usize, node_idx: usize) -> bool {
        (finally_idx + 1..=node_idx).any(|i| {
            matches!(
                c.kind(c.path[i]),
                NodeKind::FunctionExpression
                    | NodeKind::NamedFunctionExpression
                    | NodeKind::LocalFunctionDeclaration
            )
        })
    }
}

pub(crate) mod no_duplicate_case_values {
    use super::{Check, Group, NodeKind, RuleSpec, utf16_token_span};

    /// Official: `pkg/linter/lib/src/rules/no_duplicate_case_values.dart:37-58`
    /// — a `case` whose constant value equals an earlier `case` of the same
    /// switch reports at the duplicate expression with both sources spelled
    /// into `{0}`/`{1}`. The official visitor processes only legacy members
    /// (`if (member is SwitchCase)`, :41); the `AstBuilder` materializes that
    /// member shape only when `Feature.patterns` is off (`ast_builder.dart:1295`
    /// — patterns shipped in 3.0), so at the default language version every
    /// case is a `SwitchPatternCase` and the lint is silent — the official
    /// test pins exactly that (all default-version duplicates expect no lint;
    /// the reports live in `NoDuplicateCaseValuesTestLanguage219`). The gate
    /// here is the same version face at unit granularity (the members of one
    /// switch are uniformly one shape): the check runs only with `patterns`
    /// off. The official keys the map by the computed constant
    /// (`computeConstantValue`, :44); the face here is an in-rule constant
    /// folder over the syntax substrate — integer/string/bool/null
    /// literals, `+ - * ~/ %` and unary `-`, parentheses (64-bit wrapping
    /// integers, Dart `~/` truncation and `%` divisor-sign semantics),
    /// descending through `ConstantPattern`/`ParenthesizedExpression` and
    /// the `CaseExpression` operand wrapper the fixed parse surface puts
    /// around every legacy case operand (the bridge's parse posture is
    /// patterns-on at every version, so the operand never sits bare under
    /// the `SwitchCase`; the wrapper is found by kind — a compound operand
    /// spills its subexpression rows around it — and the report face is the
    /// operand token range the `exhaustive_cases` member walk's
    /// `head_range` reads, the official `reportAtNode(expression)` anchor).
    /// Expressions outside the folder's grammar evaluate unknown and never
    /// dedupe — the documented under-report family (const identifiers,
    /// doubles, interpolation, adjacent strings, const constructors),
    /// mirroring the official `hasKnownValue` skip (:47-49). Message:
    /// `diagnostic.g.dart` `noDuplicateCaseValues`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_duplicate_case_values",
        group: Group::Core,
        problem: "The value of the case clause ('{0}') is equal to the value of an earlier case clause ('{1}').",
        correction: "Try removing or changing the value.",
        kinds: &[NodeKind::SwitchStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // The official `member is SwitchCase` gate (:41): with `Feature.
        // patterns` on (the 3.0+ default) every member is a pattern case and
        // the visitor finds nothing to process.
        if c.unit.features.patterns {
            return;
        }
        let Some(block) = c
            .collect_direct(c.node())
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
        else {
            return;
        };
        // Collect first: each case re-enters `direct_children` (the scratch
        // RefCell forbids a live outer guard).
        let cases: Vec<u32> = c
            .direct_children(block)
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) == NodeKind::SwitchCase)
            .collect();
        let mut seen: Vec<(ConstValue, core::ops::Range<u32>)> = Vec::new();
        for case in cases {
            // The member's case heads in source order: the real parse merges
            // a `case 1: case 1:` label chain into ONE `SwitchCase` member
            // (payload word 2 counts the heads), each head materializing its
            // `CaseExpression` wrapper row as a direct child — the official
            // shape is one member per head (`member.expression`, singular),
            // so the dedup walk must see every head or a same-member chain
            // never dedupes. The wrapper row is found by kind: a compound
            // operand (`1 + 2`) spills its subexpression rows into the
            // member window around it, so positional extraction lands on a
            // fragment.
            let heads: Vec<u32> = c
                .collect_direct(case)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::CaseExpression)
                .collect();
            for head in heads {
                // The operand's token face (the `head_range` read the member
                // walk shares — the official reports at `expression`). The
                // operand rows tile flat around the childless
                // `ConstantPattern` wrap (`case 1 + 2` keeps its
                // subexpression rows outside it), so the token range — not
                // the row descent — is the fold's substrate.
                let Some(range) = super::exhaustive_cases::head_range(c, head) else {
                    continue;
                };
                // `computeConstantValue` → `hasKnownValue` or skip (:44-49).
                let Some(value) = fold_range(c, range.clone()) else {
                    continue;
                };
                if let Some((_, original)) = seen.iter().find(|(seen, _)| *seen == value) {
                    let dup = token_slice(c, range.clone()).to_owned();
                    let orig = token_slice(c, original.clone()).to_owned();
                    let span = utf16_token_span(c, range.start).start
                        ..utf16_token_span(c, range.end - 1).end;
                    c.report(span, &[&dup, &orig]);
                } else {
                    seen.push((value, range));
                }
            }
        }
    }

    /// The source text of one operand token range — the official
    /// `expression.toString()` face the message spells into `{0}`/`{1}`
    /// (`'1 + 2'`, exactly as written).
    fn token_slice<'a>(c: &'a Check<'_>, range: core::ops::Range<u32>) -> &'a str {
        let Some(first) = c.unit.tokens.get(range.start as usize) else {
            return "";
        };
        let Some(last_index) = range.end.checked_sub(1) else {
            return "";
        };
        let Some(last) = c.unit.tokens.get(last_index as usize) else {
            return "";
        };
        let end = last.byte_offset as usize + c.lexeme(last_index).len();
        &c.unit.source[first.byte_offset as usize..end]
    }

    /// One computed case value — the map key the official builds from
    /// `DartObject`s (:38, :51).
    #[derive(PartialEq)]
    enum ConstValue {
        Null,
        Bool(bool),
        Int(i64),
        Str(Box<str>),
    }

    /// The fold's recursion cap — the parser's expression-depth fuse scale
    /// (`dartq-parser` `MAX_DEPTH = 500`, the official `parser_impl.dart`
    /// :6936 fuse; `pub(crate)` there, so restated). The official
    /// `computeConstantValue` walk (`ConstantVisitor`, analyzer
    /// `evaluation.dart`) carries no depth bound of its own — an operand
    /// nested that deep never reaches it, because the parser fuse recovers
    /// the form first. Same posture here: the fold re-walks the raw token
    /// span, which post-recovery the AST fuse need not bound, so the walk
    /// carries its own fuse. An operand that survived parsing nests strictly
    /// under the parser's cap, so every legit prefix run folds below this;
    /// reaching it means a recovered/pathological span. Past the cap the
    /// value is unknown (the official `hasKnownValue` skip, :47-49) — an
    /// internal bound, not a source error, so no diagnostic.
    const MAX_FOLD_DEPTH: u32 = 500;

    /// The constant fold over one operand's token range: integer, plain
    /// string, bool, and null literals; parentheses; unary `-`; and the
    /// binary `+ - * ~/ %` at Dart precedence (`* ~/ %` binds over
    /// `+ -`), `+` concatenating strings. `None` — outside the grammar,
    /// malformed, or past [`MAX_FOLD_DEPTH`]: the official `hasKnownValue`
    /// skip (:47-49).
    fn fold_range(c: &Check<'_>, range: core::ops::Range<u32>) -> Option<ConstValue> {
        let mut folder = Folder {
            c,
            cursor: range.start,
            end: range.end,
            depth: 0,
        };
        let value = folder.expression(0)?;
        (folder.cursor == folder.end).then_some(value)
    }

    /// One precedence-climbing pass over the operand range (`min_level`:
    /// the lowest binding precedence still folding at this level).
    struct Folder<'a, 'b> {
        c: &'a Check<'b>,
        cursor: u32,
        end: u32,
        /// Recursion depth on the unbounded descent edges (unary prefix
        /// runs, parenthesized nesting) — the binary walk is bounded by the
        /// two-row precedence table and needs no fuse.
        depth: u32,
    }

    impl<'b> Folder<'_, 'b> {
        /// The current lexeme, `None` past the range end.
        fn lexeme(&self) -> Option<&'b str> {
            (self.cursor < self.end).then(|| self.c.lexeme(self.cursor))
        }

        fn expression(&mut self, min_level: u8) -> Option<ConstValue> {
            let mut left = self.unary()?;
            loop {
                let Some(lexeme) = self.lexeme() else {
                    return Some(left);
                };
                let level = match lexeme {
                    "*" | "~/" | "%" => 2u8,
                    "+" | "-" => 1,
                    _ => return Some(left),
                };
                if level < min_level {
                    return Some(left);
                }
                let operator = lexeme;
                self.cursor += 1;
                let right = self.expression(level + 1)?;
                left = fold_binary(operator, left, right)?;
            }
        }

        /// One recursion level on an unbounded descent edge: past
        /// [`MAX_FOLD_DEPTH`] the fold bails. The counter restores on the
        /// success unwind; a `None` discards the whole folder, so no
        /// restore is owed on a bail.
        fn descend(&mut self) -> Option<()> {
            if self.depth >= MAX_FOLD_DEPTH {
                return None;
            }
            self.depth += 1;
            Some(())
        }

        fn unary(&mut self) -> Option<ConstValue> {
            if self.lexeme()? != "-" {
                return self.primary();
            }
            self.cursor += 1;
            self.descend()?;
            let value = match self.unary()? {
                ConstValue::Int(value) => ConstValue::Int(value.wrapping_neg()),
                _ => return None,
            };
            self.depth -= 1;
            Some(value)
        }

        fn primary(&mut self) -> Option<ConstValue> {
            let token = self.cursor;
            let lexeme = self.lexeme()?;
            self.cursor += 1;
            match lexeme {
                "(" => {
                    self.descend()?;
                    let value = self.expression(0)?;
                    (self.lexeme()? == ")").then_some(())?;
                    self.depth -= 1;
                    self.cursor += 1;
                    Some(value)
                }
                "true" => Some(ConstValue::Bool(true)),
                "false" => Some(ConstValue::Bool(false)),
                "null" => Some(ConstValue::Null),
                _ => match self.c.unit.tokens.get(token as usize)?.kind {
                    dartq_scanner::TokenKind::Int => Some(ConstValue::Int(int_literal(lexeme)?)),
                    dartq_scanner::TokenKind::String => string_literal(self.c, token),
                    _ => None,
                },
            }
        }
    }

    /// The folder's binary faces: `+` over integers and string
    /// concatenation, `- * ~/ %` over integers — 64-bit wrapping like the
    /// VM's int, `~/` truncating toward zero, `%` taking the divisor's
    /// sign. Division by zero throws officially, so the value is not a
    /// known constant.
    fn fold_binary(operator: &str, left: ConstValue, right: ConstValue) -> Option<ConstValue> {
        match (operator, left, right) {
            ("+", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_add(b)))
            }
            ("+", ConstValue::Str(a), ConstValue::Str(b)) => {
                Some(ConstValue::Str(format!("{a}{b}").into()))
            }
            ("-", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_sub(b)))
            }
            ("*", ConstValue::Int(a), ConstValue::Int(b)) => {
                Some(ConstValue::Int(a.wrapping_mul(b)))
            }
            ("~/", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
                Some(ConstValue::Int(a.wrapping_div(b)))
            }
            ("%", ConstValue::Int(a), ConstValue::Int(b)) if b != 0 => {
                let mut rem = a.wrapping_rem(b);
                if rem != 0 && (rem < 0) != (b < 0) {
                    rem = rem.wrapping_add(b);
                }
                Some(ConstValue::Int(rem))
            }
            _ => None,
        }
    }

    /// The Dart integer literal grammar the folder reads — decimal and hex
    /// (`0x`), digit separators dropped, wrapping to the VM's 64-bit int.
    fn int_literal(lexeme: &str) -> Option<i64> {
        let text: String = lexeme.chars().filter(|&ch| ch != '_').collect();
        let (digits, radix) = match text.get(..2) {
            Some("0x" | "0X") => (&text[2..], 16),
            _ => (&text[..], 10),
        };
        u64::from_str_radix(digits, radix)
            .ok()
            .map(u64::cast_signed)
    }

    /// The value of one string-literal token: quote forms `'…'` / `"…"` and
    /// the triple variants, `r`-raw bodies verbatim, cooked bodies
    /// escape-decoded. A `$` in a cooked body is interpolation (a
    /// token-layer face) — unknown.
    fn string_literal(c: &Check<'_>, token: u32) -> Option<ConstValue> {
        let text = c.lexeme(token);
        let (raw, text) = match text.strip_prefix('r') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let body = if let Some(rest) = text.strip_prefix("'''") {
            rest.strip_suffix("'''")?
        } else if let Some(rest) = text.strip_prefix("\"\"\"") {
            rest.strip_suffix("\"\"\"")?
        } else {
            let quote = text.chars().next().filter(|&ch| ch == '\'' || ch == '"')?;
            text.strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))?
        };
        if raw {
            return Some(ConstValue::Str(body.into()));
        }
        if body.contains('$') {
            return None;
        }
        let mut value = String::with_capacity(body.len());
        let mut chars = body.chars();
        while let Some(ch) = chars.next() {
            if ch != '\\' {
                value.push(ch);
                continue;
            }
            match chars.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'b' => value.push('\u{0008}'),
                'f' => value.push('\u{000C}'),
                'v' => value.push('\u{000B}'),
                'x' => {
                    let hex: String = chars.by_ref().take(2).collect();
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                'u' => {
                    let mut hex = String::new();
                    let first = chars.next()?;
                    if first == '{' {
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                break;
                            }
                            hex.push(ch);
                        }
                    } else {
                        // The peeked character is the first of four hex
                        // digits.
                        hex.push(first);
                        hex.extend(chars.by_ref().take(3));
                    }
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                escaped @ ('\\' | '\'' | '"') => value.push(escaped),
                _ => return None,
            }
        }
        Some(ConstValue::Str(value.into()))
    }
}

pub(crate) mod use_rethrow_when_possible {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{
        byte_span_of_token, initialized_identifier_name, tail_token, utf16_token_span,
    };

    /// Official: `pkg/linter/lib/src/rules/use_rethrow_when_possible.dart:36-46`
    /// — a `throw` of exactly the caught exception parameter, as a statement,
    /// reports at the throw. The parameter identity
    /// (`element == exceptionParameter`, :39-44) resolves lexically: the
    /// thrown identifier must name the nearest enclosing catch's parameter
    /// and not be shadowed between the throw and that catch clause — a
    /// same-lexeme local variable, pattern variable, loop-variable
    /// declaration, or closure parameter inside the catch body binds the
    /// read instead. The catch clause is the walk's boundary: its parameter
    /// is the candidate itself, never a shadow. The remaining lexical face
    /// (documented): a sibling local-function declaration sharing the name
    /// shadows in the official scope but is not walked here. Message:
    /// `diagnostic.g.dart` `useRethrowWhenPossible`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_rethrow_when_possible",
        group: Group::Recommended,
        problem: "Use 'rethrow' to rethrow a caught exception.",
        correction: "Try replacing the 'throw' with a 'rethrow'.",
        kinds: &[NodeKind::ThrowExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `node.parent is! ExpressionStatement` bails (:37). The production
        // tiling seals the statement as a childless handle right after the
        // throw (the operand rows leak beside it), so the face reads the
        // owner run's next row when the parent itself is not the statement.
        let stmt = match c.ancestor(1) {
            Some(s) if c.kind(s) == NodeKind::ExpressionStatement => s,
            Some(parent) => {
                let kids = c.collect_direct(parent);
                match kids
                    .iter()
                    .position(|&kid| kid == c.node())
                    .and_then(|pos| kids.get(pos + 1).copied())
                {
                    Some(next) if c.kind(next) == NodeKind::ExpressionStatement => next,
                    _ => return,
                }
            }
            None => return,
        };
        // `node.thisOrAncestorOfType<CatchClause>()` (:41).
        let Some(catch_clause) = c
            .path
            .iter()
            .rev()
            .copied()
            .find(|&n| c.kind(n) == NodeKind::CatchClause)
        else {
            return;
        };
        // The exception parameter is the catch's first formal (a stack-trace
        // formal follows it; an `on`-only catch has none and never fires —
        // `exceptionParameter` null, :42-43). Production parses wrap the
        // formals in a `FormalParameters` container (the bridge opens it at
        // the `(`); hand-built trees may nest the parameter directly.
        let formals = c.collect_direct(catch_clause);
        let exception = formals
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .map(|list| c.collect_direct(list))
            .unwrap_or(formals)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameter);
        let Some(exception) = exception else {
            return;
        };
        // The parameter name is payload word 1 (the
        // `FormalParameterEnd` layout).
        let Some(&name) = c.payload(exception).get(1) else {
            return;
        };
        // The thrown expression is exactly the parameter name (upstream
        // `node.expression.canonicalElement == exceptionParameter` — a
        // SimpleIdentifier resolving to the parameter): the statement's
        // token run between the throw keyword and the trailing `;` answers.
        // Production parses leave the throw a childless handle whose
        // operand rows precede it as siblings, so exactly one
        // identifier-shaped token in the run is the simple-identifier face
        // (reserved words cannot name parameters; built-in keywords can).
        let throw_tok = c.head_token(c.node());
        let operand_tok = throw_tok + 1;
        if tail_token(c, stmt) != operand_tok + 1 {
            return;
        }
        let Some(token) = c.unit.tokens.get(operand_tok as usize) else {
            return;
        };
        let thrown = match token.kind {
            dartq_scanner::TokenKind::Identifier => c.lexeme(operand_tok),
            dartq_scanner::TokenKind::Keyword(kw)
                if kw.style() == dartq_scanner::KeywordStyle::BuiltIn =>
            {
                c.lexeme(operand_tok)
            }
            _ => return,
        };
        if thrown == c.lexeme(name) && !shadowed_between(c, operand_tok, catch_clause, thrown) {
            // The official report anchors at the throw expression node —
            // `throw` through the operand (`reportAtNode`, :45).
            let range = utf16_token_span(c, throw_tok).start..utf16_token_span(c, operand_tok).end;
            c.report(range, &[]);
        }
    }

    /// Whether any declaration visible between the thrown operand and the
    /// catch clause binds `name` — the lexical face of the official element
    /// identity: the innermost binding of a read inside the catch body
    /// wins, so a same-name declaration below the catch parameter makes the
    /// thrown identifier a different element.
    fn shadowed_between(c: &Check<'_>, use_tok: u32, catch_clause: u32, name: &str) -> bool {
        let use_start = byte_span_of_token(c, use_tok).start;
        for &ancestor in c.path.iter().rev().skip(1) {
            if ancestor == catch_clause {
                return false;
            }
            match c.kind(ancestor) {
                NodeKind::Block => {
                    if block_declares_before(c, ancestor, use_start, name) {
                        return true;
                    }
                }
                NodeKind::ForIn => {
                    if scope_declares(c, ancestor, name) {
                        return true;
                    }
                }
                NodeKind::ForParts => {
                    // `for (var e = …; …)` declares in the loop parts,
                    // outside the body block.
                    if for_parts_declares(c, ancestor, name) {
                        return true;
                    }
                }
                NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression => {
                    // Parameters scope the whole body regardless of position.
                    if parameter_names(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                NodeKind::LocalFunctionDeclaration => {
                    // The function name binds from its declaration on — it
                    // nests under the `FunctionName` child on the production
                    // tiling — and the parameters scope the whole body.
                    let names_decl =
                        crate::rules::declarations::member_declaration_name(c, ancestor)
                            .is_some_and(|decl| {
                                c.lexeme(c.head_token(decl)) == name
                                    && c.byte_span(decl).start < use_start
                            });
                    if names_decl || parameter_names(c, ancestor).contains(&name) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Local declarations of a block that precede the use: variables
    /// (`VariablesDeclaration`) and pattern bindings. The variable name
    /// rides its [`NodeKind::InitializedIdentifier`] row's span head (the
    /// production row is width-0 at the name — the Identifier row the
    /// parser emits precedes the wrapper, never inside it).
    fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
        for decl in c.collect_direct(block) {
            match c.kind(decl) {
                NodeKind::VariablesDeclaration => {
                    for var in c.collect_direct(decl) {
                        if c.kind(var) != NodeKind::InitializedIdentifier {
                            continue;
                        }
                        if let Some(var_name) = initialized_identifier_name(c, var)
                            && c.lexeme(var_name) == name
                            && byte_span_of_token(c, var_name).start < use_start
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

    /// Any name binding directly inside a for-in header.
    fn scope_declares(c: &Check<'_>, scope: u32, name: &str) -> bool {
        let window = c.children(scope);
        for id in window.start..window.end {
            if c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == name {
                return true;
            }
        }
        false
    }

    /// The loop-variable declarations of a classic for header (the name
    /// read mirrors [`block_declares_before`]'s span-head face).
    fn for_parts_declares(c: &Check<'_>, parts: u32, name: &str) -> bool {
        c.collect_direct(parts).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::VariablesDeclaration
                && c.collect_direct(decl).into_iter().any(|var| {
                    c.kind(var) == NodeKind::InitializedIdentifier
                        && initialized_identifier_name(c, var)
                            .is_some_and(|var_name| c.lexeme(var_name) == name)
                })
        })
    }

    /// The parameter names of a closure or local function (the
    /// `FormalParameters` child's `FormalParameter` name words).
    fn parameter_names<'a>(c: &'a Check<'a>, function: u32) -> Vec<&'a str> {
        let Some(params) = c
            .collect_direct(function)
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
                (name != super::NONE_TOKEN).then(|| c.lexeme(name))
            })
            .collect()
    }
}

pub(crate) mod avoid_returning_null_for_void {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, byte_span_of_token, first_child, last_child};

    const PROBLEM: &str = "Don't return 'null' from a function with a return type of 'void'.";
    const METHOD_PROBLEM: &str = "Don't return 'null' from a method with a return type of 'void'.";
    const CORRECTION: &str = "Try removing the 'null'.";

    /// Official: `pkg/linter/lib/src/rules/avoid_returning_null_for_void.dart:41-81`
    /// — `return null` (statement or arrow body) inside a function or
    /// method whose resolved return type is `void` (sync body, :75) or
    /// `Future<void>` (async body, :77-80) reports at the return statement
    /// or the arrow body. The two official message variants (from-function /
    /// from-method, `diagnostic.g.dart:727/:739`) split on the enclosing
    /// declaration kind.
    ///
    /// The resolved return type reads lexically: a declaration's written
    /// type is its type-family child; an unwritten one never resolves to
    /// `void`/`Future<void>` (an unannotated `f() { return null; }` infers
    /// `Null`, an async one `Future<Null>`). An anonymous
    /// [`NodeKind::FunctionExpression`] resolves through the context type —
    /// the lexical `approximateContextType` faces (extensions.dart:452): a
    /// written-`FunctionType` variable declaration or assignment target
    /// (:494-502), and the enclosing function's written return type for the
    /// arrow/return positions. `isAsync` is the body's `async` keyword
    /// (`FunctionBody.isAsynchronous` — `sync*` carries `sync` and stays on
    /// the sync branch; `async*` is async).
    ///
    /// Faces not ported (documented, the resolution-bound family): a
    /// `typedef`-aliased context or declared type (`typedef V = void
    /// Function(); V f = () { return null; };`), an argument-position
    /// context (`list.map(() { return null; })` under a
    /// `void Function()`-typed parameter), and a non-`Future`/`void`
    /// user-defined same-name type shadowing `dart:async`/`dart:core`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_returning_null_for_void",
        group: Group::Recommended,
        problem: PROBLEM,
        correction: CORRECTION,
        kinds: &[NodeKind::ReturnStatement, NodeKind::ExpressionBody],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.expression case var expression?` (:47): a bare `return;`
        // has no child; a production `ExpressionBody` is a childless
        // handle — its expression is the preceding sibling row.
        let Some(expression) = first_child(c, node).or_else(|| {
            (c.kind(node) == NodeKind::ExpressionBody)
                .then(|| preceding_expression(c, node))
                .flatten()
        }) else {
            return;
        };
        if c.kind(expression) != NodeKind::LiteralNull {
            return;
        }
        // The nearest enclosing function-like declaration decides the face
        // (:55-57 — `thisOrAncestorMatching(FunctionExpression |
        // MethodDeclaration)`).
        for &ancestor in c.path.iter().rev().skip(1) {
            match c.kind(ancestor) {
                NodeKind::FunctionExpression => {
                    report(c, node, ancestor, context_type(c, ancestor), PROBLEM);
                    return;
                }
                NodeKind::Method => {
                    report(c, node, ancestor, written_type(c, ancestor), METHOD_PROBLEM);
                    return;
                }
                NodeKind::TopLevelMethod
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::NamedFunctionExpression => {
                    report(c, node, ancestor, written_type(c, ancestor), PROBLEM);
                    return;
                }
                _ => {}
            }
        }
    }

    /// The expression's outer tail row immediately before a childless
    /// `ExpressionBody` — the production tiling keeps the body's expression
    /// as the owner's last row ahead of the body (the same sibling read
    /// `async_return_with_no_await` runs on this kind).
    fn preceding_expression(c: &Check<'_>, body: u32) -> Option<u32> {
        let parent = c.unit.ast.parent(body)?;
        let kids = c.collect_direct(parent);
        let pos = kids.iter().position(|&kid| kid == body)?;
        let expression = *kids.get(pos.checked_sub(1)?)?;
        (c.unit.ast.token_range(expression).start > c.head_token(body)).then_some(expression)
    }

    /// The official verdict (:75-80) on the resolved `(type row, is_async)`
    /// pair, reporting at `node`. `None` type = a resolution the lexical
    /// face cannot reach — silent.
    fn report(c: &mut Check<'_>, node: u32, function: u32, type_row: Option<u32>, problem: &str) {
        let Some(type_row) = type_row else { return };
        let is_async = body_is_async(c, function);
        let verdict = match c.kind(type_row) {
            // `!isAsync && type is VoidType` (:75).
            NodeKind::VoidType => !is_async,
            // `isAsync && type.isDartAsyncFuture && typeArguments.first is
            // VoidType` (:77-80): the written `Future` head, not nullable
            // (`Future<void>?` is not the `Future` interface type), first
            // argument `void`.
            NodeKind::Type => {
                is_async
                    && c.lexeme(c.head_token(type_row)) == "Future"
                    && c.byte_span(type_row).end
                        == byte_span_of_token(c, c.head_token(type_row)).end
                    && first_type_argument_is_void(c, type_row)
            }
            _ => false,
        };
        if !verdict {
            return;
        }
        // `reportAtNode(node)` (:76/:80): a return statement spans `return
        // null;`; an arrow body spans `async => null;` — the official
        // `ExpressionFunctionBody.beginToken` is the `async` keyword when
        // present (ast.dart ExpressionFunctionBodyImpl.beginToken), the
        // arrow otherwise.
        let range = c.utf16_span(node);
        let range = if c.kind(node) == NodeKind::ExpressionBody {
            let async_kw = c.payload(node).get(2).copied().unwrap_or(NONE_TOKEN);
            if async_kw == NONE_TOKEN {
                range
            } else {
                crate::rules::utf16_token_span(c, async_kw).start..range.end
            }
        } else {
            range
        };
        c.report_texts(range, problem, CORRECTION, &[]);
    }

    /// `body.isAsynchronous` — the owner's body carries the `async` keyword
    /// (`BlockBody` word 0, `ExpressionBody` word 2; a plain statement
    /// `Block` body carries no modifier words). `sync*` spells `sync`, not
    /// `async` — it stays on the sync branch.
    fn body_is_async(c: &Check<'_>, function: u32) -> bool {
        let Some(body) = last_child(c, function) else {
            return false;
        };
        let async_kw = match c.kind(body) {
            NodeKind::ExpressionBody => c.payload(body).get(2).copied(),
            NodeKind::BlockBody => c.payload(body).first().copied(),
            _ => Some(NONE_TOKEN),
        }
        .unwrap_or(NONE_TOKEN);
        async_kw != NONE_TOKEN && c.lexeme(async_kw) == "async"
    }

    /// A declaration's written return type: the type-family direct child
    /// before the parameter list (`NoType` = unwritten — the official
    /// inferred return type never lands on `void`/`Future<void>` here, so
    /// absent answers `None`).
    fn written_type(c: &Check<'_>, function: u32) -> Option<u32> {
        let kids = c.collect_direct(function);
        let params = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .unwrap_or(kids.len());
        kids[..params].iter().copied().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
            )
        })
    }

    /// The resolved return type of an anonymous closure: the context
    /// `FunctionType`'s own return type. `None` when no lexical context
    /// face reaches a written function type (the documented
    /// resolution-bound under-reports).
    fn context_type(c: &Check<'_>, closure: u32) -> Option<u32> {
        // The walk starts above the closure itself (the dispatch node may be
        // the closure's own `ExpressionBody`, one path step below it).
        let start = c
            .path
            .iter()
            .position(|&row| row == closure)
            .unwrap_or(c.path.len());
        for i in (0..start).rev() {
            let ancestor = c.path[i];
            match c.kind(ancestor) {
                // `void Function() f = () { … };` — the written type tiles
                // in the declaring owner's slice before the declaration
                // (the `avoid_types_on_closure_parameters` face).
                NodeKind::VariablesDeclaration => {
                    if i == 0 {
                        return None;
                    }
                    let owner = c.path[i - 1];
                    for kid in c.collect_direct(owner) {
                        if kid == ancestor {
                            return None;
                        }
                        if matches!(
                            c.kind(kid),
                            NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
                        ) {
                            return function_type_return(c, kid);
                        }
                    }
                    return None;
                }
                // `f = () { … };` — the bare-identifier target's written
                // type governs (the `Assignment` payload word 0 is the
                // operator; the target is the identifier token before it).
                NodeKind::Assignment => {
                    let &eq = c.payload(ancestor).first()?;
                    let target_is_identifier = eq.checked_sub(1).is_some_and(|target| {
                        c.unit
                            .tokens
                            .get(target as usize)
                            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Identifier)
                    });
                    if !target_is_identifier {
                        return None;
                    }
                    return match crate::rules::expressions::same_file::binding(
                        c,
                        closure,
                        c.lexeme(eq - 1),
                    ) {
                        Some((_, Some(type_row))) => function_type_return(c, type_row),
                        _ => None,
                    };
                }
                // `=> () { … }` / `return () { … };` — the context is the
                // enclosing function's return type, whatever statement
                // containers sit between (the `approximateContextType`
                // function-body arms); the jump lands on the nearest
                // function-like ancestor.
                NodeKind::ExpressionBody | NodeKind::ReturnStatement => {
                    for &a in c.path[..i].iter().rev() {
                        match c.kind(a) {
                            // A nested closure's context is the enclosing
                            // closure's resolved return type — itself a
                            // context product, so the function-type gate
                            // applies the same way.
                            NodeKind::FunctionExpression => {
                                return context_type(c, a).and_then(|t| function_type_return(c, t));
                            }
                            NodeKind::Method
                            | NodeKind::TopLevelMethod
                            | NodeKind::LocalFunctionDeclaration
                            | NodeKind::NamedFunctionExpression => {
                                return written_type(c, a).and_then(|t| function_type_return(c, t));
                            }
                            _ => {}
                        }
                    }
                    return None;
                }
                // The transparent initializer chain between the closure and
                // the position its context type governs.
                NodeKind::InitializedIdentifier
                | NodeKind::VariableInitializer
                | NodeKind::ParenthesizedExpression
                | NodeKind::ConstExpression
                | NodeKind::ConstLiteral
                | NodeKind::Conditional => {}
                _ => return None,
            }
        }
        None
    }

    /// The return-type child of a written `FunctionType` row (`void` of
    /// `void Function()`); any other written type shape has no function
    /// context return type.
    fn function_type_return(c: &Check<'_>, type_row: u32) -> Option<u32> {
        if c.kind(type_row) != NodeKind::FunctionType {
            return None;
        }
        c.collect_direct(type_row).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
            )
        })
    }

    /// Whether the `TypeArguments` sibling of a `Type` row opens with a
    /// `void` (the `Future<void>` shape): production order puts the
    /// `TypeArguments` before the `Type` row under their shared owner;
    /// hand-built trees may place it after.
    fn first_type_argument_is_void(c: &Check<'_>, type_row: u32) -> bool {
        let Some(owner) = c.unit.ast.parent(type_row) else {
            return false;
        };
        let kids = c.collect_direct(owner);
        let Some(pos) = kids.iter().position(|&kid| kid == type_row) else {
            return false;
        };
        let is_arguments = |row: &u32| c.kind(*row) == NodeKind::TypeArguments;
        let Some(arguments) = kids[..pos]
            .iter()
            .rev()
            .find(|row| is_arguments(row))
            .or_else(|| kids[pos + 1..].iter().find(|row| is_arguments(row)))
        else {
            return false;
        };
        c.collect_direct(*arguments)
            .first()
            .is_some_and(|&arg| c.kind(arg) == NodeKind::VoidType)
    }
}

pub(crate) mod type_literal_in_constant_pattern {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/type_literal_in_constant_pattern.dart:40-63`
    /// — a constant pattern whose expression's static type is the `Type`
    /// type reports at the pattern. The official bails for a
    /// `const`-marked pattern (:45-47) and for a `Type`-typed matched
    /// value (:50-59, the raw-type exemption).
    ///
    /// The type-literal test (`expression.staticType.isDartCoreType`,
    /// :57-63) resolves over the same-file channel: a bare name answers
    /// through the innermost value binding first (a value is never a type
    /// literal — the `const String = 'x'` shadow over-report is dead), a
    /// top-level `const` folds its initializer through the alias channel
    /// (`const t = int; case t:` — a Type-valued constant — reports like
    /// the official static type), a typed binding reports when written
    /// `Type`, and a name with no value binding is a type literal: a
    /// same-file type declaration (`case MyType:` — the user-type
    /// under-report is dead) or an unshadowed built-in type name. A
    /// two-segment name folds a same-file class-static alias (`A.t`);
    /// anything else there (an enum member, a prefixed spelling — the
    /// `Type` row's head is the prefix) stays silent, the prefixed type
    /// literal being the documented under-report. The type-literal test is
    /// the official `isDartCoreType` element read — nullability-blind, so a
    /// `Type`-written binding reports through `Type?` too. The raw-type
    /// exemption reads the matched value through the scrutinee face and —
    /// for a constant nested in an object pattern (`A(field: int)`) — the
    /// field's same-file written type, both the nullability-sensitive
    /// `isSubtypeOf` face (`Type?` exempts nothing). The class-scope
    /// static face (`static const int = 3;` shadowing the spelling inside
    /// its container, the enum's own constants included) reads the
    /// enclosing containers directly; the object-pattern field face reads
    /// instance fields
    /// only (a getter, a prefixed container, a name-less field, and
    /// list/map element positions keep the name reading). Message:
    /// `diagnostic.g.dart` `typeLiteralInConstantPattern`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "type_literal_in_constant_pattern",
        group: Group::Core,
        problem: "Use 'TypeName _' instead of a type literal.",
        correction: "Replace with 'TypeName _'.",
        kinds: &[NodeKind::ConstantPattern],
        check,
    };

    /// The built-in type names a bare constant-pattern identifier can spell
    /// (`dynamic` included; `void` is a keyword and never reaches an
    /// identifier) — the unshadowed fallback the same-file channel cannot
    /// resolve better (dart:core is auto-imported, so an unshadowed
    /// spelling of these IS the library type). The official test corpus
    /// exercises the dart:core names.
    pub(crate) fn is_builtin_type_name(lexeme: &str) -> bool {
        matches!(
            lexeme,
            "int"
                | "double"
                | "num"
                | "bool"
                | "String"
                | "Object"
                | "dynamic"
                | "Never"
                | "Null"
                | "Function"
                | "Iterable"
                | "List"
                | "Map"
                | "Set"
                | "Symbol"
                | "Type"
                | "Future"
                | "Stream"
                | "Enum"
                | "Record"
                | "Uri"
                | "Pattern"
                | "RegExp"
                | "DateTime"
                | "Duration"
        )
    }

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.constKeyword != null` bails (:45-47): a const-marked pattern
        // owns the `const` keyword as its span head.
        if c.lexeme(c.head_token(node)) == "const" {
            return;
        }
        let Some(expression) = super::first_child(c, node) else {
            return;
        };
        // The bare constant parses as a tear-off send group — the pattern
        // adopts the childless `Send` leaf (span = the identifier; the
        // `Identifier` / `NoTypeArguments` leaves precede it as owner
        // siblings), so the constant name reads off the send's span head.
        if !matches!(
            c.kind(expression),
            NodeKind::Identifier | NodeKind::Send | NodeKind::DotAccess
        ) {
            return;
        }
        if !constant_is_type_literal(c, expression) {
            return;
        }
        // OK to use raw types if the matched value is of type `Type`
        // (:50-59) — the scrutinee face or the object-pattern field face.
        if matched_value_is_type(c) || pattern_field_is_type(c) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// The `Type`-matched-value exemption (:50-59): the enclosing match's
    /// scrutinee proves a `Type` matched value. The syntactic face: a
    /// `.runtimeType` tail is always `Type`, and a bare-identifier scrutinee
    /// resolves its written type through the same-file face — the
    /// `Type`-typed parameter / local / field / top-level shapes
    /// (`type_literal_in_constant_pattern_test.dart`
    /// `test_constType_matchType`).
    fn matched_value_is_type(c: &Check<'_>) -> bool {
        for (depth, &ancestor) in c.path.iter().enumerate().rev() {
            // The scrutinee piece run: if-case (the condition container
            // holds the scrutinee itself), a switch statement case, a
            // switch expression case.
            let scrutinee = match c.kind(ancestor) {
                NodeKind::ParenthesizedCondition => {
                    // The condition's pieces leak past the container to the
                    // enclosing statement: the scrutinee is the container's
                    // parent's direct rows before the container row, ending
                    // before the pattern starts. (The path step after the
                    // container is the container's child on the path — the
                    // pattern itself — so the owner sits one step before
                    // it.) The boundary reads the pattern's own head row
                    // (the container's adopted child when present) — the
                    // container's span can anchor wider than the constant.
                    let Some(&outer) = depth.checked_sub(1).and_then(|i| c.path.get(i)) else {
                        return false;
                    };
                    let pattern_window = c.children(c.node());
                    let pattern_start = if pattern_window.start < pattern_window.end {
                        c.byte_span(pattern_window.start).start
                    } else {
                        c.byte_span(c.node()).start
                    };
                    c.collect_direct(outer)
                        .into_iter()
                        .take_while(|&kid| kid != ancestor)
                        .filter(|&kid| c.byte_span(kid).end <= pattern_start)
                        .collect()
                }
                NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                    let boundary = if c.kind(ancestor) == NodeKind::SwitchCase {
                        NodeKind::SwitchBlock
                    } else {
                        NodeKind::SwitchExpressionBlock
                    };
                    let Some(root_index) = depth.checked_sub(2) else {
                        return false;
                    };
                    let root = c.path[root_index];
                    let kids = c.collect_direct(root);
                    let Some(boundary_pos) = kids.iter().position(|&kid| c.kind(kid) == boundary)
                    else {
                        return false;
                    };
                    kids[..boundary_pos].to_vec()
                }
                _ => continue,
            };
            // A `.runtimeType` tail is always `Type`.
            if let Some(&tail) = scrutinee.last()
                && c.kind(tail) == NodeKind::DotAccess
                && c.lexeme(c.head_token(tail)) == "runtimeType"
            {
                return true;
            }
            // A bare-identifier scrutinee resolves through the same-file
            // written-type face — exactly `Type`, not the nullable `Type?`
            // (the official `isSubtypeOf(matchedValueType, typeType)`).
            if let Some(&head) = scrutinee.first()
                && c.kind(head) == NodeKind::Identifier
            {
                let face = crate::rules::written_type_face(c, head, c.lexeme(c.head_token(head)));
                if face.is_some_and(|face| face.name == Some("Type") && !face.nullable) {
                    return true;
                }
            }
            return false;
        }
        false
    }

    /// The static-type question over the expression's name segments (the
    /// module doc carries the decision table).
    fn constant_is_type_literal(c: &Check<'_>, expression: u32) -> bool {
        let range = c.unit.ast.token_range(expression);
        let Some((leading_dot, names)) =
            super::exhaustive_cases::segments(c, super::exhaustive_cases::strip_const(c, range))
        else {
            return false;
        };
        if leading_dot {
            return false;
        }
        match names.as_slice() {
            [name] => bare_name_is_type_literal(c, expression, name),
            [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
                .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0)),
            _ => false,
        }
    }

    /// The bare-name face: the innermost value binding first, then the
    /// type reading.
    fn bare_name_is_type_literal(c: &Check<'_>, use_row: u32, name: &str) -> bool {
        use crate::rules::expressions::same_file::BindingScope;
        match crate::rules::expressions::same_file::binding(c, use_row, name) {
            Some((scope, typed)) => match scope {
                // A `const` binding folds its initializer through the alias
                // channel (a Type-valued constant); any other value ends
                // the reading off the type family.
                BindingScope::TopLevel => super::exhaustive_cases::top_level_const_init(c, name)
                    .map_or(typed_face_is_type(c, typed), |init| {
                        folds_to_type(c, init, &mut Vec::new(), 0)
                    }),
                BindingScope::Local => {
                    crate::rules::expressions::same_file::local_const_initializer(c, use_row, name)
                        .is_some_and(|init| folds_to_type(c, init, &mut Vec::new(), 0))
                }
                BindingScope::Parameter | BindingScope::Field => typed_face_is_type(c, typed),
                // A `static` member read is a value pattern, never a type
                // literal (the `enclosing_static_declares` posture below —
                // the binding walk now reports static fields itself).
                BindingScope::StaticField => false,
            },
            // No same-file value binding: a type literal — a same-file type
            // declaration or an unshadowed built-in type name. A class-scope
            // static getter/method or enum constant of the enclosing
            // container shadows the spelling first (the binding walk sees
            // static fields; the remaining static member kinds stay here).
            None => {
                !enclosing_static_declares(c, name)
                    && (crate::rules::expressions::same_file::declares_type(c, name)
                        || is_builtin_type_name(name))
            }
        }
    }

    /// Whether a class-like container enclosing the use site declares a
    /// STATIC value member spelled `name` — a `static` field, a static
    /// getter/method, or (inside an enum) one of the enum's own constants.
    /// The same-file binding walk reads instance members only, so a user
    /// constant named like a built-in type (`static const int = 3;`) would
    /// otherwise keep the type reading and over-report; the bare name
    /// resolves to that member, which is a value pattern, never a type
    /// literal. The walk stops at the innermost declaring container (an
    /// outer one is shadowed either way).
    fn enclosing_static_declares(c: &Check<'_>, name: &str) -> bool {
        let field_ctx = dartq_parser::IdentifierContext::FieldDeclaration as u32;
        for &ancestor in c.path.iter().rev() {
            match c.kind(ancestor) {
                NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration => {
                    let bodies = core::iter::once(ancestor).chain(
                        c.collect_direct(ancestor)
                            .into_iter()
                            .filter(|&kid| c.kind(kid) == NodeKind::DeclarationBody),
                    );
                    for container in bodies {
                        for member in c.collect_direct(container) {
                            let static_member = dartq_ast::mod_flags(c.unit.ast, member)
                                & crate::rules::expressions::same_file::STATIC_BIT
                                != 0;
                            match c.kind(member) {
                                NodeKind::Fields if static_member => {
                                    if c.collect_direct(member).into_iter().any(|kid| {
                                        c.kind(kid) == NodeKind::Identifier
                                            && c.payload(kid).first() == Some(&field_ctx)
                                            && c.lexeme(c.head_token(kid)) == name
                                    }) {
                                        return true;
                                    }
                                }
                                NodeKind::Method if static_member => {
                                    // The name row's head token — `member_name`
                                    // answers a ROW id, never a token index.
                                    if crate::rules::declarations::member_declaration_name(
                                        c, member,
                                    )
                                    .is_some_and(|row| c.lexeme(c.head_token(row)) == name)
                                    {
                                        return true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                NodeKind::EnumBody => {
                    // The enclosing enum's own constants: plain values in
                    // scope inside the enum's members (a childless handle
                    // anchored at the name — the span head answers).
                    if c.collect_direct(ancestor).into_iter().any(|element| {
                        c.kind(element) == NodeKind::EnumElement
                            && c.lexeme(c.head_token(element)) == name
                    }) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// The object-pattern field face of the matched-value exemption: a
    /// constant pattern nested in an object pattern (`A(field: int)`)
    /// matches the field's own value, whose static type is the field's
    /// written type — a `Type`-written field exempts the nested constant
    /// (`type_literal_in_constant_pattern_test.dart`
    /// `test_constType_matchType_nested`). The bridge leaks the
    /// sub-pattern rows past the field windows (the constant's tree parent
    /// is the statement container; the `PatternField` / `ObjectPattern`
    /// rows are span-only overlays completing right after the leak), so
    /// the wrapping pair resolves by span containment over the container's
    /// arena window — the innermost containing rows. The field reads
    /// through the same-file written-type face of the object pattern's
    /// class; a prefixed spelling (`prefix.A(...)`, payload word 1), a
    /// name-less field (`A(: sub)`, payload word 0 is the colon), and an
    /// unresolvable class keep the name reading.
    fn pattern_field_is_type(c: &Check<'_>) -> bool {
        let node = c.node();
        let span = c.byte_span(node);
        let contains = |row: u32| {
            let outer = c.byte_span(row);
            outer.start <= span.start && span.end <= outer.end
        };
        let Some(container) = c.path.len().checked_sub(2).map(|i| c.path[i]) else {
            return false;
        };
        let window = c.children(container).start..container;
        // Post-order rows: the innermost containing overlay completes
        // first.
        let Some(object) = window
            .clone()
            .filter(|&row| c.kind(row) == NodeKind::ObjectPattern && contains(row))
            .min()
        else {
            return false;
        };
        let Some(field) = (c.children(container).start..object)
            .filter(|&row| c.kind(row) == NodeKind::PatternField && contains(row))
            .min()
        else {
            return false;
        };
        // A prefixed container (`prefix.A(...)`) resolves outside the file.
        let words = c.payload(object);
        let Some(&class_tok) = words.first() else {
            return false;
        };
        if class_tok == NONE_TOKEN || words.get(1).copied() != Some(NONE_TOKEN) {
            return false;
        }
        // The name-less field face carries the colon token, not a name.
        let Some(&name_tok) = c.payload(field).first() else {
            return false;
        };
        if name_tok == NONE_TOKEN || c.lexeme(name_tok) == ":" {
            return false;
        }
        let Some(class_row) = same_file_class_named(c, c.lexeme(class_tok)) else {
            return false;
        };
        instance_field_is_type(c, class_row, c.lexeme(name_tok))
    }

    /// The same-file top-level class declaration named `name` — the local
    /// class-only face of the `exhaustive_cases::class_like_named` walk
    /// (module-private there): the object-pattern head resolves against a
    /// class's instance members.
    fn same_file_class_named(c: &Check<'_>, name: &str) -> Option<u32> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if c.kind(kid) == NodeKind::ClassDeclaration
                    && crate::rules::class_declaration_name(c, kid) == Some(name)
                {
                    return Some(kid);
                }
            }
        }
        None
    }

    /// Whether the instance field `field` of the same-file `class_row` is
    /// written exactly `Type` (the non-static `Fields` walk of
    /// `type_face_of_field`'s index builder, class-row parameterized — the
    /// member's single written type covers all its declarators). Untyped
    /// fields, every other written type, and the nullable `Type?` (not a
    /// subtype of `Type`) keep the name reading; a getter or an
    /// inferred-from-initializer type is invisible (the documented channel
    /// face).
    fn instance_field_is_type(c: &Check<'_>, class_row: u32, field: &str) -> bool {
        let body = c
            .collect_direct(class_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
            .unwrap_or(class_row);
        for member in crate::rules::rows_with_parent(c, body) {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
            {
                continue;
            }
            let written = c
                .collect_direct(member)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type);
            let Some(written) = written else {
                continue;
            };
            let face = crate::rules::type_face(c, written);
            if face.name != Some("Type") || face.nullable {
                continue;
            }
            for decl in c
                .collect_direct(member)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
            {
                for var in c
                    .collect_direct(decl)
                    .into_iter()
                    .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                {
                    if crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|name| c.lexeme(name) == field)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// A typed value binding is a type literal whenever written `Type` —
    /// nullability-blind, the official `isDartCoreType` reading the type's
    /// element (`Type?` still names dart:core `Type`, :792-794) — while
    /// the matched-value exemption is the nullability-sensitive
    /// `isSubtypeOf` face.
    fn typed_face_is_type(c: &Check<'_>, typed: Option<u32>) -> bool {
        typed.is_some_and(|row| {
            c.kind(row) == dartq_ast::NodeKind::Type
                && crate::rules::type_face(c, row).name == Some("Type")
        })
    }

    /// The alias fold over a top-level/class-static const initializer
    /// chain: the chain landing on a type name (a same-file type or an
    /// unshadowed built-in) makes the constant Type-valued. `seen` breaks
    /// alias cycles, `depth` bounds them.
    fn folds_to_type(
        c: &Check<'_>,
        init: core::ops::Range<u32>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> bool {
        if depth > super::exhaustive_cases::MAX_ALIAS_DEPTH || seen.contains(&init.start) {
            return false;
        }
        let init = super::exhaustive_cases::strip_const(c, init);
        let Some((false, names)) = super::exhaustive_cases::segments(c, init.clone()) else {
            return false;
        };
        match names.as_slice() {
            // Within a top-level initializer only top-level names resolve:
            // a const alias recurses, a non-const value ends the chain off
            // the type family.
            [name] => match super::exhaustive_cases::top_level_const_init(c, name) {
                Some(next) => {
                    seen.push(init.start);
                    folds_to_type(c, next, seen, depth + 1)
                }
                None => {
                    crate::rules::expressions::same_file::declares_type(c, name)
                        || is_builtin_type_name(name)
                }
            },
            [.., class, field] => super::exhaustive_cases::class_const_init(c, class, field)
                .is_some_and(|next| folds_to_type(c, next, seen, depth + 1)),
            _ => false,
        }
    }
}

pub(crate) mod exhaustive_cases {
    //! Official: `pkg/linter/lib/src/rules/exhaustive_cases.dart` +
    //! `asEnumLikeType` (`pkg/linter/lib/src/extensions.dart:604-647`) — a
    //! switch over an enum-like type (a concrete, unsubclassed class or an
    //! extension type, with only private generative constructors and at
    //! least two `static const` fields of the type) must cover every
    //! constant value or carry a `default`. Each case head whose constant
    //! resolves to a value removes it (:49-74); a `default` member ends the
    //! whole check silent (:75-77); every remaining value then reports once
    //! — at the statement head through the scrutinee's `)` — spelled with
    //! the group's preferred (first non-deprecated) name (:80-93). Real
    //! enums never enter (the element walk's default arm — the analyzer's
    //! own exhaustiveness errors cover them).
    //!
    //! Mounted face: the element facts ride the same-file declarative
    //! channel — the scrutinee's written type resolves through
    //! [`same_file::written_type`](crate::rules::expressions::same_file::written_type)
    //! (the `type_literal_in_constant_pattern` idiom), the
    //! candidate's constructor/field facts walk the same-file declaration
    //! rows, and constant-value identity interns canonical initializer
    //! texts — `computeConstantValue` same-file edition: two constants
    //! share a value when their initializer chains land on equal terminal
    //! texts, so `static const b = a` aliases group with `a`
    //! (extensions.dart:626-642). A case head covers through the official
    //! variable faces only (`Identifier` / `PropertyAccess` /
    //! `DotShorthandPropertyAccess`, :59-74) — any other constant pattern
    //! covers nothing.
    //!
    //! Known silences (the same-file under-report posture, each the
    //! documented fallback of the channel it rides): a scrutinee whose type
    //! leaves the file (an import, a `var`/for-in binding, a call result —
    //! `written_type`'s contract), a candidate or a subclass declared in
    //! another unit of the library (`asEnumLikeType` sees the whole
    //! defining library, extensions.dart:259-270), and constant values
    //! spelled differently over the same instance (`E._(0x1)` vs `E._(1)`).

    use dartq_ast::{NodeKind, mod_flags};
    use dartq_scanner::{KeywordStyle, TokenKind};

    use super::{Group, NONE_TOKEN, RuleSpec, first_child};
    use crate::engine::Check;
    use crate::rules::{
        SupertypeClause, class_declaration_name, initialized_identifier_name, member_annotations,
        rows_with_parent, supertypes, utf16_token_span, variables_declaration_keyword,
    };

    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "exhaustive_cases",
        group: Group::Recommended,
        problem: "Missing case clauses for some constants in '{0}'.",
        correction: "Try adding case clauses for the missing constants.",
        kinds: &[NodeKind::SwitchStatement],
        check,
    };

    /// The class-family `abstract` modifier bit (bridge `mod_bits`; the
    /// parser lane owns the values).
    const ABSTRACT_BIT: u16 = 1 << 2;

    /// The alias-hop cap: a legitimate constant alias chain is a few names
    /// deep; past this it is pathological and computes to nothing.
    pub(super) const MAX_ALIAS_DEPTH: usize = 8;

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(block_pos) = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::SwitchBlock)
        else {
            return;
        };
        let block = kids[block_pos];
        // The scrutinee's written type → the same-file enum-like candidate
        // (`statement.expression.staticType` → `InterfaceType.element`,
        // :39-46; a type outside the file stays silent — the channel's
        // documented fallback).
        let Some(scrutinee) = scrutinee_identifier(c, &kids[..block_pos]) else {
            return;
        };
        let name = c.lexeme(c.head_token(scrutinee));
        let Some(written) = crate::rules::expressions::same_file::written_type(c, scrutinee, name)
        else {
            return;
        };
        let Some((candidate_row, candidate_kind)) = class_like_named(
            c,
            written,
            &[
                NodeKind::ClassDeclaration,
                NodeKind::ExtensionTypeDeclaration,
            ],
        ) else {
            return;
        };
        let is_extension_type = candidate_kind == NodeKind::ExtensionTypeDeclaration;
        let constructors = constructor_face(c, candidate_row);
        let field_faces = class_const_fields(c, candidate_row);
        let mut values = ConstValues::default();
        let constants = candidate_constants(c, &mut values, &field_faces, written);
        let candidate = EnumLikeCandidate {
            is_extension_type,
            is_abstract: !is_extension_type
                && mod_flags(c.unit.ast, candidate_row) & ABSTRACT_BIT != 0,
            has_subclass_in_defining_unit: subclassed_in_file(c, candidate_row),
            constants: &constants,
            constructors: &constructors,
        };
        let cases = switch_case_rows(c, block);
        let members: Vec<SwitchMember> = cases
            .into_iter()
            .map(|case| member_face(c, &mut values, &field_faces, case))
            .collect();
        // Owned names: the report mutates the check the verdict borrowed.
        let missing: Vec<String> = check_switch(&candidate, &members)
            .into_iter()
            .map(String::from)
            .collect();
        if missing.is_empty() {
            return;
        }
        // The report span: the `switch` head through the scrutinee's `)`
        // (official `reportAtOffset(statement.offset,
        // rightParenthesis.end - statement.offset)`, :81-90).
        let start = c.utf16_span(node).start;
        let Some(rparen) = c
            .head_token(block)
            .checked_sub(1)
            .filter(|&token| token > c.head_token(node))
        else {
            return;
        };
        let range = start..utf16_token_span(c, rparen).end;
        for name in &missing {
            c.report(range.clone(), &[name]);
        }
    }

    /// The block's `SwitchCase` rows, source order. The plain tiling walk
    /// loses members: each case's completion window starts AT the previous
    /// case's arena row (`end_switch_case` snapshots the material start
    /// before the previous case seals), so the reverse tiling jump lands on
    /// that row and the decrement-first walk skips it as window-start
    /// interior — a two-case switch surfaces only the last case. The
    /// jump-landed row is this block's member, not the case's child: the
    /// walk pushes it and continues below it. Shared with the
    /// `control_flow_in_finally` member-label walk.
    pub(super) fn switch_case_rows(c: &Check<'_>, block: u32) -> Vec<u32> {
        let window = c.children(block);
        let mut cases: Vec<u32> = Vec::new();
        let mut i = window.end;
        while i > window.start {
            i -= 1;
            let start = c.children(i).start;
            if start < i && c.kind(start) == NodeKind::SwitchCase {
                cases.push(start);
            }
            if c.kind(i) == NodeKind::SwitchCase {
                cases.push(i);
            }
            i = start;
        }
        // The start-checks push in jump order, not id order.
        cases.sort_unstable();
        cases
    }

    /// The bare-identifier scrutinee row: the leading expression piece
    /// (`matched_value_is_type`'s proven `scrutinee.first()` face,
    /// flow.rs:1011). The bridge leaks the send-group leaves past the
    /// condition wrapper as owner siblings — the `Identifier`, the
    /// `NoTypeArguments` leaf anchored at the follower `)`, and the `Send`
    /// leaf that spans through it — so the piece run says nothing about the
    /// expression shape beyond its head. Only a selector continuation
    /// (a `DotAccess` piece: `a.b`, `e.multi()`) disqualifies the bare
    /// name; any other scrutinee has no same-file written type and stays
    /// silent through `written_type`'s `None`.
    fn scrutinee_identifier(c: &Check<'_>, pieces: &[u32]) -> Option<u32> {
        let &first = pieces.first()?;
        if c.kind(first) != NodeKind::Identifier {
            return None;
        }
        let selector = pieces[1..]
            .iter()
            .any(|&piece| c.kind(piece) == NodeKind::DotAccess);
        (!selector).then_some(first)
    }

    /// The same-file class-like declaration named `name`: `(row, kind)` —
    /// the top-level wrapper walk of `same_file_declaration` under the
    /// caller's kind filter. The candidate face accepts only the
    /// enum-like-eligible kinds (a class or an extension type; a same-file
    /// enum is the analyzer's own exhaustiveness error, a mixin cannot
    /// carry const instances); the superclass chain additionally walks
    /// mixin applications.
    fn class_like_named(c: &Check<'_>, name: &str, kinds: &[NodeKind]) -> Option<(u32, NodeKind)> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if kinds.contains(&c.kind(kid)) && declaration_name(c, kid) == Some(name) {
                    return Some((kid, c.kind(kid)));
                }
            }
        }
        None
    }

    /// The written name of a class-like declaration row: the sibling
    /// identifier for the class family (`class_declaration_name`), the
    /// name-token payload word for an extension type (bridge
    /// `begin_extension_type_declaration`).
    fn declaration_name<'a>(c: &'a Check<'_>, row: u32) -> Option<&'a str> {
        if c.kind(row) == NodeKind::ExtensionTypeDeclaration {
            let name = c.payload(row).get(2).copied()?;
            return (name != NONE_TOKEN).then(|| c.lexeme(name));
        }
        class_declaration_name(c, row)
    }

    /// The `(is_private, is_generative)` constructor face of the candidate:
    /// the declared constructors and factories plus the header primary
    /// constructor (the official `constructors.every((c) => c.isPrivate &&
    /// c.isGenerative)` walk, extensions.dart:620). An empty face is the
    /// element model's synthesized default constructor — public, so the
    /// gate fails on it.
    fn constructor_face(c: &Check<'_>, candidate_row: u32) -> Vec<(bool, bool)> {
        let mut ctors = Vec::new();
        let mut push = |row: u32| match c.kind(row) {
            NodeKind::Constructor => ctors.push((constructor_name_private(c, row), true)),
            // A factory is never generative — the gate fails on it however
            // its name is spelled (:620).
            NodeKind::Factory => ctors.push((false, false)),
            // A primary constructor names through `._` (private) or not at
            // all (the public type-named form).
            NodeKind::PrimaryConstructor => {
                ctors.push((c.payload(row).get(2) == Some(&1), true));
            }
            _ => {}
        };
        // The header face (an extension type's representation declaration
        // hangs off the declaration row) and the declared members.
        for row in rows_with_parent(c, candidate_row) {
            push(row);
        }
        for row in crate::rules::body_members(c, candidate_row) {
            push(row);
        }
        if ctors.is_empty() {
            ctors.push((false, true));
        }
        ctors
    }

    /// Whether the constructor named at the row head is private: the head
    /// is the class-name token, and the element name is that token — bare
    /// for an unnamed constructor, `.`-suffixed for a named one — so the
    /// official name test (`name.isPrivateName`) reduces to the head
    /// lexeme leading with `_`: a library-private class is private in
    /// every spelling (`_Priv(`, `_Priv._(`, `_Priv.named(`), a public
    /// class only through the `._` tail (`E(` and `E.named(` are public).
    fn constructor_name_private(c: &Check<'_>, row: u32) -> bool {
        let head = c.head_token(row);
        if lexeme_at(c, head).starts_with('_') {
            return true;
        }
        lexeme_at(c, head + 1) == "." && lexeme_at(c, head + 2).starts_with('_')
    }

    /// The lexeme of `token`, empty past the table (the tail guard of the
    /// constructor-name walk).
    fn lexeme_at<'a>(c: &'a Check<'_>, token: u32) -> &'a str {
        c.unit
            .tokens
            .get(token as usize)
            .map_or("", |t| &c.unit.source[t.byte_range()])
    }

    /// Whether any same-file class's superclass chain reaches the candidate
    /// — `_hasSubclassInDefiningCompilationUnit` (extensions.dart:259-270)
    /// over the current unit (the official walk sees the whole library;
    /// another unit's subclass stays unseen).
    fn subclassed_in_file(c: &Check<'_>, candidate: u32) -> bool {
        let Some(root) = c.unit.ast.root() else {
            return false;
        };
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if !matches!(
                    c.kind(kid),
                    NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication
                ) {
                    continue;
                }
                if chain_reaches(c, &superclass_faces(c, kid), candidate) {
                    return true;
                }
            }
        }
        false
    }

    /// The superclass-chain faces of a class row: the written `extends`
    /// names (`with` mixins ride on the extends face without joining the
    /// chain — `classType.superclass`, extensions.dart:263), or a mixin
    /// application's `=` name (its superclass face).
    fn superclass_faces<'a>(c: &'a Check<'_>, row: u32) -> Vec<&'a str> {
        match c.kind(row) {
            NodeKind::ClassDeclaration => supertypes(c, row)
                .into_iter()
                .filter(|&(_, clause)| clause == SupertypeClause::Extends)
                .map(|(name, _)| name)
                .collect(),
            NodeKind::NamedMixinApplication => supertypes(c, row)
                .into_iter()
                .next()
                .map(|(name, _)| vec![name])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Whether the written superclass names reach `target` over the
    /// same-file class rows (a cycle-guarded walk; a name resolving
    /// outside the file dead-ends).
    fn chain_reaches(c: &Check<'_>, names: &[&str], target: u32) -> bool {
        let chain_class = |name: &str| {
            class_like_named(
                c,
                name,
                &[NodeKind::ClassDeclaration, NodeKind::NamedMixinApplication],
            )
            .map(|(row, _)| row)
        };
        let mut seen: Vec<u32> = Vec::new();
        let mut queue: Vec<u32> = Vec::new();
        for &name in names {
            if let Some(row) = chain_class(name) {
                if row == target {
                    return true;
                }
                if !seen.contains(&row) {
                    seen.push(row);
                    queue.push(row);
                }
            }
        }
        while let Some(row) = queue.pop() {
            for name in superclass_faces(c, row) {
                if let Some(next) = chain_class(name) {
                    if next == target {
                        return true;
                    }
                    if !seen.contains(&next) {
                        seen.push(next);
                        queue.push(next);
                    }
                }
            }
        }
        false
    }

    /// One `static const` field of a same-file class-like scope, in
    /// declaration order (the official field walk,
    /// extensions.dart:627-642).
    struct ConstFieldFace<'a> {
        name: &'a str,
        /// The initializer's token range; `None` when absent (the
        /// `computeConstantValue() == null` skip).
        init: Option<core::ops::Range<u32>>,
        /// The written type's head name (`E` of `E?`/`E<int>`); empty for a
        /// function/void type (never the candidate type), `None` unwritten.
        written: Option<&'a str>,
        /// `metadata.hasDeprecated` — `@deprecated` / `@Deprecated`.
        deprecated: bool,
    }

    /// The `static const` fields of the class-like `class_row`.
    fn class_const_fields<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<ConstFieldFace<'a>> {
        let body = c
            .collect_direct(class_row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
            .unwrap_or(class_row);
        let mut fields = Vec::new();
        for member in rows_with_parent(c, body) {
            if c.kind(member) != NodeKind::Fields
                || c.payload(member).first()
                    != Some(&(dartq_parser::MemberKind::StaticField as u32))
            {
                continue;
            }
            let deprecated = member_annotations(c, body, member).iter().any(|&meta| {
                c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && matches!(c.lexeme(c.head_token(piece)), "deprecated" | "Deprecated")
                })
            });
            let written = c
                .collect_direct(member)
                .into_iter()
                .find_map(|kid| match c.kind(kid) {
                    NodeKind::Type => Some(c.lexeme(c.head_token(kid))),
                    NodeKind::FunctionType | NodeKind::VoidType => Some(""),
                    _ => None,
                });
            for decl in c
                .collect_direct(member)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
            {
                let Some(kw) = variables_declaration_keyword(c, decl) else {
                    continue;
                };
                if c.lexeme(kw) != "const" {
                    continue;
                }
                for var in c
                    .collect_direct(decl)
                    .into_iter()
                    .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                {
                    let Some(name_row) = initialized_identifier_name(c, var) else {
                        continue;
                    };
                    let init = initializer_range(c, var);
                    fields.push(ConstFieldFace {
                        name: c.lexeme(name_row),
                        init,
                        written,
                        deprecated,
                    });
                }
            }
        }
        fields
    }

    /// The initializer's token range of an `InitializedIdentifier` row —
    /// the tokens after the `=` up to the declarator's end: the first
    /// depth-0 `,` (a sibling declarator's separator) or the terminating
    /// `;`, whichever comes first. The row's own tail can leak past the
    /// declarator (the bridge seals the row at the last completed node,
    /// which a name-shaped initializer — `newFoo`, `E.e` — never
    /// completes, while a creation `E._(1)` does), so a leaked sibling
    /// (`b = E._(2)`) must not join the value text; the `,` cut only
    /// fires outside brackets, depth tracked over `(`, `[`, `{` — inside
    /// them a `,` belongs to the expression (argument lists, list/map/set
    /// literals). `<`/`>` are deliberately not tracked: a depth-0 `,`
    /// inside `<...>` only occurs in a parameterized tear-off or type
    /// literal (`map<int, String>`), whose value can never head the
    /// candidate type, so cutting there is invisible to coverage — while
    /// tracking `<` would swallow the relational shape's real separator
    /// (`a = b < c, d = E._(2)`). A `;` cannot occur inside a constant
    /// initializer (function literals are not const; a string literal's
    /// semicolons ride inside the string token), so it terminates at any
    /// depth.
    pub(crate) fn initializer_range(c: &Check<'_>, var: u32) -> Option<core::ops::Range<u32>> {
        let range = c.unit.ast.token_range(var);
        // The row's own extent can seal before adopted value leaves (the
        // prefix negation's literal completes before the container begins,
        // so `= -1` seals at `-`); the declaring declaration's extent
        // covers the whole declarator list and bounds the scan instead.
        let end_limit = match c.unit.ast.parent(var) {
            Some(parent) if c.kind(parent) == NodeKind::VariablesDeclaration => {
                c.unit.ast.token_range(parent).end
            }
            _ => range.end,
        };
        let mut token = range.start + 1;
        while token < end_limit {
            if c.lexeme(token) == "=" {
                let mut end = token + 1;
                let mut depth = 0usize;
                while end < end_limit {
                    match c.lexeme(end) {
                        "(" | "[" | "{" => depth += 1,
                        ")" | "]" | "}" => depth = depth.saturating_sub(1),
                        ";" => break,
                        "," if depth == 0 => break,
                        _ => {}
                    }
                    end += 1;
                }
                return Some(token + 1..end);
            }
            token += 1;
        }
        None
    }

    /// The candidate's constant fields, valued (extensions.dart:626-642):
    /// each field's value id resolves through the alias channel, and
    /// `of_this_type` reads the written type or — unwritten — the canonical
    /// terminal's head (`field.type != thisType`).
    fn candidate_constants<'a>(
        c: &Check<'a>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'a>],
        candidate_name: &str,
    ) -> Vec<ConstantField<'a>> {
        field_faces
            .iter()
            .map(|face| {
                let value = face
                    .init
                    .clone()
                    .and_then(|init| init_value(c, values, field_faces, init, &mut Vec::new(), 0));
                let of_this_type = match face.written {
                    Some(name) => name == candidate_name,
                    None => value.is_some_and(|id| {
                        values.text(id).split(' ').next() == Some(candidate_name)
                    }),
                };
                ConstantField {
                    name: face.name,
                    is_static_const: true,
                    of_this_type,
                    deprecated: face.deprecated,
                    value,
                }
            })
            .collect()
    }

    /// One switch member's verdict face (the official member walk,
    /// exhaustive_cases.dart:49-77): `default` silence, and coverage
    /// through the head's constant-value identity.
    fn member_face(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        case: u32,
    ) -> SwitchMember {
        let is_default = c.payload(case).first().copied() != Some(NONE_TOKEN);
        let mut covers = None;
        if !is_default {
            // The first case head only — the official reads
            // `member.expression` / the single `guardedPattern` (:51-57).
            let head = c
                .collect_direct(case)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::CaseExpression);
            if let Some(head) = head
                && let Some(range) = head_range(c, head)
                && let Some(face) = head_face(c, range)
            {
                covers = alias_value(c, values, field_faces, face, &mut Vec::new(), 0);
            }
        }
        SwitchMember { is_default, covers }
    }

    /// The head expression's token face from the `CaseExpression` row: the
    /// constant pattern's span, with the bridge's two tiling wrinkles
    /// normalized —
    ///
    /// - a parenthesized head (`case (E.e):`) rides a `ParenthesizedPattern`
    ///   leaf spanning the parens; the inner constant pattern (the window's
    ///   first tile) carries the unparenthesized face (the official
    ///   `pattern.unParenthesized`, :52);
    /// - a direct head's adopted window holds only the trailing operand
    ///   subtree, so a dot-shorthand member name (`case .e:`) leaks as
    ///   preceding `CaseExpression` children (the `Identifier` /
    ///   `DotShorthandHead` leaves — the `type_literal_in_constant_pattern`
    ///   idiom); the range widens over them. Literal/creation heads grow no
    ///   leak, and a `when` guard completes after the pattern, outside the
    ///   widened prefix.
    pub(super) fn head_range(c: &Check<'_>, head: u32) -> Option<core::ops::Range<u32>> {
        let kids = c.collect_direct(head);
        let pos = kids.iter().position(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ConstantPattern | NodeKind::ParenthesizedPattern
            )
        })?;
        let row = kids[pos];
        if c.kind(row) == NodeKind::ParenthesizedPattern {
            let mut inner = first_child(c, row)?;
            while c.kind(inner) == NodeKind::ParenthesizedPattern {
                inner = first_child(c, inner)?;
            }
            return (c.kind(inner) == NodeKind::ConstantPattern)
                .then(|| c.unit.ast.token_range(inner));
        }
        let mut range = c.unit.ast.token_range(row);
        for &kid in &kids[..pos] {
            let kid_range = c.unit.ast.token_range(kid);
            range.start = range.start.min(kid_range.start);
            range.end = range.end.max(kid_range.end);
        }
        Some(range)
    }

    /// The head's alias face — the official variable faces only
    /// (`Identifier` / `PropertyAccess` / `DotShorthandPropertyAccess`,
    /// :59-74): identifier segments joined by `.`, an optional `const`
    /// introducer stripped (`case const E.e:` covers through the same
    /// expression). `None` — list/map/object patterns, literals,
    /// creations — covers nothing.
    fn head_face<'a>(c: &'a Check<'a>, range: core::ops::Range<u32>) -> Option<AliasFace<'a>> {
        let (leading_dot, names) = segments(c, strip_const(c, range))?;
        match names.as_slice() {
            [name] if leading_dot => Some(AliasFace::Shorthand(name)),
            [name] => Some(AliasFace::Bare(name)),
            // A leading-dot chain (`.a.b`) is a selector on a shorthand,
            // not one of the official variable faces.
            [.., class, field] if !leading_dot => Some(AliasFace::Qualified(class, field)),
            _ => None,
        }
    }

    /// The token range past a leading `const` introducer — `const` heads
    /// and initializers carry their constant through the same expression
    /// as the bare spelling.
    pub(super) fn strip_const(
        c: &Check<'_>,
        range: core::ops::Range<u32>,
    ) -> core::ops::Range<u32> {
        if range.start < range.end && c.lexeme(range.start) == "const" {
            return range.start + 1..range.end;
        }
        range
    }

    /// The dot-joined identifier segments of a token range: `(leading_dot,
    /// names)` — `E.e` → `(false, ["E", "e"])`, `prefix.E.e` →
    /// `(false, ["prefix", "E", "e"])`, `.e` → `(true, ["e"])`. `None` for
    /// any other expression (a non-name token inside a segment — the
    /// literal/creation/interpolation faces).
    pub(super) fn segments<'a>(
        c: &'a Check<'_>,
        range: core::ops::Range<u32>,
    ) -> Option<(bool, Vec<&'a str>)> {
        let mut names: Vec<&'a str> = Vec::new();
        let mut shorthand = false;
        let mut current: Option<&'a str> = None;
        for token in range {
            match c.lexeme(token) {
                "." if current.is_none() && names.is_empty() => shorthand = true,
                "." => names.push(current.take()?),
                lex => {
                    if current.is_some() || !is_name_token(c, token) {
                        return None;
                    }
                    current = Some(lex);
                }
            }
        }
        if let Some(last) = current.take() {
            names.push(last);
        }
        (!names.is_empty()).then_some((shorthand, names))
    }

    /// Whether the token lexes as a plain name (an identifier, or a
    /// built-in keyword — a constant named `set` binds;
    /// `initialized_identifier_name`'s tolerance).
    fn is_name_token(c: &Check<'_>, token: u32) -> bool {
        match c.unit.tokens.get(token as usize).map(|t| t.kind) {
            Some(TokenKind::Identifier) => true,
            Some(TokenKind::Keyword(kw)) if kw.style() == KeywordStyle::BuiltIn => true,
            _ => false,
        }
    }

    /// The alias face of a constant — a bare name (`static const b = a`),
    /// a shorthand (`= .e`; never resolvable standalone), or a
    /// `Class.member` face.
    #[derive(Clone, Copy, Debug)]
    enum AliasFace<'a> {
        Bare(&'a str),
        Shorthand(&'a str),
        Qualified(&'a str, &'a str),
    }

    /// The constant value id of one initializer: a dot-joined name segment
    /// aliases into the named constant's value, and any other expression —
    /// the terminal face (`E._(1)`, a literal) — is its own value text (the
    /// `computeConstantValue` identity: equal texts denote the same
    /// instance). A `.e` shorthand has no standalone value. `seen` breaks
    /// alias cycles, `depth` bounds them.
    fn init_value(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        init: core::ops::Range<u32>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> Option<u32> {
        if depth > MAX_ALIAS_DEPTH || seen.contains(&init.start) {
            return None;
        }
        let init = strip_const(c, init);
        match segments(c, init.clone()) {
            None => Some(values.intern(c, init.clone())),
            Some((true, _)) => None,
            Some((false, names)) => {
                seen.push(init.start);
                let target = match names.as_slice() {
                    [name] => field_faces
                        .iter()
                        .find(|field| field.name == *name)
                        .and_then(|field| field.init.clone())
                        .or_else(|| top_level_const_init(c, name)),
                    [.., class, field] => class_const_init(c, class, field),
                    _ => return None,
                }?;
                init_value(c, values, field_faces, target, seen, depth + 1)
            }
        }
    }

    /// The value of a head's alias face: resolve the named constant, then
    /// its initializer's value (unresolvable — nothing covered).
    fn alias_value(
        c: &Check<'_>,
        values: &mut ConstValues,
        field_faces: &[ConstFieldFace<'_>],
        face: AliasFace<'_>,
        seen: &mut Vec<u32>,
        depth: usize,
    ) -> Option<u32> {
        if depth > MAX_ALIAS_DEPTH {
            return None;
        }
        let target = match face {
            // A bare head names the candidate's own constants first (class
            // scope shadows the top level), then the top-level scope.
            AliasFace::Bare(name) => field_faces
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| field.init.clone())
                .or_else(|| top_level_const_init(c, name)),
            // A shorthand head resolves against the scrutinee's type — the
            // candidate's constants.
            AliasFace::Shorthand(name) => field_faces
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| field.init.clone()),
            AliasFace::Qualified(class, field) => class_const_init(c, class, field),
        }?;
        init_value(c, values, field_faces, target, seen, depth + 1)
    }

    /// The initializer of the `static const` field `field` of the same-file
    /// class-like `class` — the `E.e` head face and the `Class.member`
    /// alias target.
    pub(super) fn class_const_init(
        c: &Check<'_>,
        class: &str,
        field: &str,
    ) -> Option<core::ops::Range<u32>> {
        let (row, _) = class_like_named(
            c,
            class,
            &[
                NodeKind::ClassDeclaration,
                NodeKind::ExtensionTypeDeclaration,
            ],
        )?;
        class_const_fields(c, row)
            .into_iter()
            .find(|known| known.name == field)?
            .init
    }

    /// The initializer of the same-file top-level `const` named `name` —
    /// the bare-name alias target outside class scope.
    pub(super) fn top_level_const_init(c: &Check<'_>, name: &str) -> Option<core::ops::Range<u32>> {
        let root = c.unit.ast.root()?;
        for top in c.collect_direct(root) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            for kid in c.collect_direct(top) {
                if c.kind(kid) != NodeKind::TopLevelFields {
                    continue;
                }
                for decl in c
                    .collect_direct(kid)
                    .into_iter()
                    .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
                {
                    let Some(kw) = variables_declaration_keyword(c, decl) else {
                        continue;
                    };
                    if c.lexeme(kw) != "const" {
                        continue;
                    }
                    for var in c
                        .collect_direct(decl)
                        .into_iter()
                        .filter(|&var| c.kind(var) == NodeKind::InitializedIdentifier)
                    {
                        if let Some(name_row) = initialized_identifier_name(c, var)
                            && c.lexeme(name_row) == name
                        {
                            return initializer_range(c, var);
                        }
                    }
                }
            }
        }
        None
    }

    /// The canonical constant-value table: initializer texts interned to
    /// dense ids.
    #[derive(Default)]
    struct ConstValues {
        texts: Vec<String>,
    }

    impl ConstValues {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "interned ids are bounded by one unit's constant count; \
                      u32 is the payload width"
        )]
        fn intern(&mut self, c: &Check<'_>, range: core::ops::Range<u32>) -> u32 {
            let text = token_text(c, range);
            if let Some(id) = self.texts.iter().position(|known| *known == text) {
                return id as u32;
            }
            self.texts.push(text);
            (self.texts.len() - 1) as u32
        }

        fn text(&self, id: u32) -> &str {
            &self.texts[id as usize]
        }
    }

    /// The canonical text of a token range — lexemes joined with a space
    /// (whitespace-free, comment-free value identity).
    fn token_text(c: &Check<'_>, range: core::ops::Range<u32>) -> String {
        let mut text = String::new();
        for token in range {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(c.lexeme(token));
        }
        text
    }

    /// One `static const` field of the candidate type (the official field
    /// walk, extensions.dart:627-642).
    pub struct ConstantField<'a> {
        pub name: &'a str,
        /// `field.isConst && field.isStatic`.
        pub is_static_const: bool,
        /// The field's type is the enclosing type (the
        /// `field.type != thisType` skip).
        pub of_this_type: bool,
        /// `metadata.hasDeprecated` — aliases are reported last.
        pub deprecated: bool,
        /// The computed constant-value identity; aliasing fields
        /// (`static const b = a`) share the id, `None` when the value is
        /// not computable (the `fieldValue == null` skip).
        pub value: Option<u32>,
    }

    /// The candidate type face — `asEnumLikeType`'s inputs
    /// (extensions.dart:604-647).
    pub struct EnumLikeCandidate<'a> {
        /// An extension type (`ExtensionTypeElement`) is always
        /// enum-like-eligible; a class must additionally pass the two
        /// class-only gates below.
        pub is_extension_type: bool,
        /// Class only: `element.isAbstract`.
        pub is_abstract: bool,
        /// Class only: a subclass exists in the defining compilation unit
        /// (`_hasSubclassInDefiningCompilationUnit`).
        pub has_subclass_in_defining_unit: bool,
        /// Every constructor as `(is_private, is_generative)` — all must
        /// hold (:620).
        pub constructors: &'a [(bool, bool)],
        /// The type's fields, declaration order (:627); the enumeration
        /// applies the official `isOriginGetterSetter` skip (getter/setter
        /// pseudo-fields are not fields and never enter the face).
        pub constants: &'a [ConstantField<'a>],
    }

    /// One switch member's face (the official member walk,
    /// exhaustive_cases.dart:49-77).
    pub struct SwitchMember {
        /// `default:` — ends the whole check silent (:75-77).
        pub is_default: bool,
        /// The constant value the head covers: a constant-pattern /
        /// legacy-case head whose expression resolves to one of the type's
        /// constant values (the `Identifier` / `PropertyAccess` /
        /// `DotShorthandPropertyAccess` faces all resolve through
        /// `computeConstantValue`). `None` — other patterns, foreign
        /// constants, unresolved heads — covers nothing.
        pub covers: Option<u32>,
    }

    /// The official verdict: the missing values' preferred names, in field
    /// declaration order — the mounter renders each into the template at
    /// the statement span (statement head through the scrutinee's `)`,
    /// :80-93). Empty when the type is not enum-like or a `default` is
    /// present.
    #[must_use]
    pub fn check_switch<'a>(
        candidate: &'a EnumLikeCandidate<'a>,
        members: &[SwitchMember],
    ) -> Vec<&'a str> {
        if !candidate.is_extension_type
            && (candidate.is_abstract || candidate.has_subclass_in_defining_unit)
        {
            return Vec::new();
        }
        if !candidate
            .constructors
            .iter()
            .all(|&(is_private, is_generative)| is_private && is_generative)
        {
            return Vec::new();
        }
        // The constant groups in declaration order, keyed by value id
        // (extensions.dart:626-642). Constant counts are tiny — linear
        // grouping beats a map.
        let mut groups: Vec<(u32, Vec<&'a ConstantField<'a>>)> = Vec::new();
        let mut value_count = 0;
        for field in candidate.constants {
            if !field.is_static_const || !field.of_this_type {
                continue;
            }
            let Some(value) = field.value else {
                continue;
            };
            value_count += 1;
            match groups.iter_mut().find(|(id, _)| *id == value) {
                Some((_, group)) => group.push(field),
                None => groups.push((value, vec![field])),
            }
        }
        // The two-constant minimum counts fields, not values (:640) — a
        // `static const b = a` pair is enum-like with one group.
        if value_count < 2 {
            return Vec::new();
        }
        let mut covered: Vec<u32> = Vec::new();
        for member in members {
            if member.is_default {
                return Vec::new();
            }
            if let Some(value) = member.covers.filter(|value| !covered.contains(value)) {
                covered.push(value);
            }
        }
        groups
            .iter()
            .filter(|(value, _)| !covered.contains(value))
            .map(|(_, group)| preferred_name(group))
            .collect()
    }

    /// The group's reporting name (:84-88): the first non-deprecated
    /// member, else the first.
    fn preferred_name<'a>(group: &[&'a ConstantField<'a>]) -> &'a str {
        group
            .iter()
            .find(|field| !field.deprecated)
            .unwrap_or(&group[0])
            .name
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const PRIVATE_GENERATIVE: (bool, bool) = (true, true);

        fn candidate<'a>(
            constructors: &'a [(bool, bool)],
            constants: &'a [ConstantField<'a>],
        ) -> EnumLikeCandidate<'a> {
            EnumLikeCandidate {
                is_extension_type: false,
                is_abstract: false,
                has_subclass_in_defining_unit: false,
                constructors,
                constants,
            }
        }

        fn const_field(name: &str, value: Option<u32>) -> ConstantField<'_> {
            ConstantField {
                name,
                is_static_const: true,
                of_this_type: true,
                deprecated: false,
                value,
            }
        }

        fn member(covers: Option<u32>) -> SwitchMember {
            SwitchMember {
                is_default: false,
                covers,
            }
        }

        /// The official `test_class_enumLike_allCases` face — every value
        /// covered, silent.
        #[test]
        fn fully_covered_switch_is_silent() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(Some(1)), member(Some(2))];
            assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
        }

        /// The official `test_class_enumLike_missingCase` — the uncovered
        /// value reports with its name.
        #[test]
        fn missing_value_reports_the_name() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(Some(1))];
            assert_eq!(check_switch(&candidate(&ctors, &fields), &members), ["g"]);
        }

        /// The official `test_class_enumLike_withDefault` — a `default`
        /// silences the whole check even with values missing (:75-77).
        #[test]
        fn default_silences() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [
                member(Some(0)),
                SwitchMember {
                    is_default: true,
                    covers: None,
                },
            ];
            assert!(check_switch(&candidate(&ctors, &fields), &members).is_empty());
        }

        /// A non-constant / foreign / unresolved head covers nothing (the
        /// official `enumConstants.remove` misses) — missing values report
        /// in declaration order.
        #[test]
        fn foreign_head_covers_nothing() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("e", Some(0)),
                const_field("f", Some(1)),
                const_field("g", Some(2)),
            ];
            let members = [member(Some(0)), member(None)];
            assert_eq!(
                check_switch(&candidate(&ctors, &fields), &members),
                ["f", "g"]
            );
        }

        /// The official `test_class_enumLike_deprecatedAliases` — aliases
        /// share a value id; covering either alias covers the value, and a
        /// missing alias group reports the first non-deprecated name.
        #[test]
        fn alias_group_prefers_the_live_name() {
            let ctors = [PRIVATE_GENERATIVE];
            let fields = [
                const_field("newFoo", Some(0)),
                ConstantField {
                    deprecated: true,
                    ..const_field("oldFoo", Some(0))
                },
                const_field("bar", Some(1)),
                const_field("baz", Some(2)),
            ];
            // newFoo + bar + baz covers everything (the deprecated alias
            // never reports).
            let all = [member(Some(0)), member(Some(1)), member(Some(2))];
            assert!(check_switch(&candidate(&ctors, &fields), &all).is_empty());
            // bar + baz leaves the alias group; the live name reports.
            let partial = [member(Some(1)), member(Some(2))];
            assert_eq!(
                check_switch(&candidate(&ctors, &fields), &partial),
                ["newFoo"]
            );
        }

        /// The enum-like gate (extensions.dart:604-647): public or
        /// non-generative constructors, an abstract class, a subclass in
        /// the defining unit, or fewer than two value-carrying
        /// `static const` fields of the type all stay silent.
        #[test]
        fn gate_rejections() {
            let fields = [const_field("e", Some(0)), const_field("f", Some(1))];
            let members = [member(Some(0))];
            // A public constructor.
            let public = [(false, true)];
            assert!(check_switch(&candidate(&public, &fields), &members).is_empty());
            // A non-generative constructor (a factory).
            let factory = [(true, false)];
            assert!(check_switch(&candidate(&factory, &fields), &members).is_empty());
            // Abstract, or subclassed in the defining unit.
            let mut abstract_candidate = candidate(&[PRIVATE_GENERATIVE], &fields);
            abstract_candidate.is_abstract = true;
            assert!(check_switch(&abstract_candidate, &members).is_empty());
            let mut subclassed = candidate(&[PRIVATE_GENERATIVE], &fields);
            subclassed.has_subclass_in_defining_unit = true;
            assert!(check_switch(&subclassed, &members).is_empty());
            // One constant only (the official
            // `test_class_notEnumLike_singleConstant`).
            let single = [const_field("e", Some(0))];
            assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &single), &members).is_empty());
            // Non-const / foreign-typed / non-computable fields do not
            // count toward the minimum.
            let uncounted = [
                ConstantField {
                    is_static_const: false,
                    ..const_field("e", Some(0))
                },
                ConstantField {
                    of_this_type: false,
                    ..const_field("f", Some(1))
                },
                const_field("g", None),
            ];
            let two = [member(Some(0)), member(Some(1))];
            assert!(check_switch(&candidate(&[PRIVATE_GENERATIVE], &uncounted), &two).is_empty());
        }

        /// Extension types skip the class-only gates (the official
        /// `test_extensionType_*` family); duplicate values count fields,
        /// not groups, toward the minimum (:640).
        #[test]
        fn extension_types_and_aliased_pairs() {
            let fields = [const_field("a", Some(0)), const_field("b", Some(0))];
            let mut ext = candidate(&[PRIVATE_GENERATIVE], &fields);
            ext.is_extension_type = true;
            assert_eq!(check_switch(&ext, &[]), ["a"]);
            // As a class, the same shape passes the gates too (two fields).
            assert_eq!(
                check_switch(&candidate(&[PRIVATE_GENERATIVE], &fields), &[]),
                ["a"]
            );
        }

        // —— the mounted face: the official test sources through the
        // production bridge (`Fixture::parse`) ——

        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("exhaustive_cases"))
        }

        /// `test_class_enumLike_allCases` — every value covered, silent.
        #[test]
        fn mounted_all_cases_silent() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void ok(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
      break;
    case E.g:
      break;
  }
}
")
                .is_empty()
            );
        }

        /// `test_class_enumLike_missingCase` — the uncovered value reports
        /// at the statement head through the scrutinee's `)`, with the
        /// official message and correction.
        #[test]
        fn mounted_missing_case_reports() {
            let source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void e(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try adding case clauses for the missing constants.")
            );
            // The span: the `switch` head through the scrutinee's `)`
            // (the sixth close paren — after the four creation spellings
            // and the parameter list).
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert_eq!(
                lints[0].range.end,
                utf16_at(&f, tok(source, &f.tokens, ")", 5)) + 1
            );
        }

        /// `test_class_enumLike_withDefault` — a `default` member silences
        /// the whole check.
        #[test]
        fn mounted_default_silences() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okDefault(E e) {
  switch (e) {
    case E.e:
      break;
    default:
      break;
  }
}
")
                .is_empty()
            );
        }

        /// `test_class_enumLike_parenthesizedCases` — parenthesized case
        /// heads cover (the `unParenthesized` walk).
        #[test]
        fn mounted_parenthesized_cases_cover() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void okParenthesized(E e) {
  switch (e) {
    case (E.e):
      break;
    case ((E.f)):
      break;
    case (E.g):
      break;
  }
}
")
                .is_empty()
            );
        }

        /// `test_class_enumLike_deprecatedAliases` — the deprecated alias
        /// groups with its live twin; exactly one report (`bar`, the group
        /// the second switch leaves open), the other two switches silent.
        #[test]
        fn mounted_deprecated_aliases() {
            let source = r"
class DeprecatedFields {
  final int i;
  const DeprecatedFields._(this.i);

  @deprecated
  static const oldFoo = newFoo;
  static const newFoo = DeprecatedFields._(1);
  static const bar = DeprecatedFields._(2);
  static const baz = DeprecatedFields._(3);
}

void dep(DeprecatedFields e) {
  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.newFoo:
      break;
    case DeprecatedFields.baz:
      break;
  }

  switch (e) {
    case DeprecatedFields.oldFoo:
      break;
    case DeprecatedFields.bar:
      break;
    case DeprecatedFields.baz:
      break;
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'bar'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 1))
            );
        }

        /// `test_class_notEnumLike_publicConstructor` / `_singleConstant` /
        /// `_subclassed` — the enum-like gate rejections, silent.
        #[test]
        fn mounted_not_enum_like_silent() {
            assert!(
                run(r"
class PublicCons {
  const PublicCons();
  static const e = PublicCons();
  static const f = PublicCons();
}

void p(PublicCons e) {
  switch (e) {
    case PublicCons.e:
  }
}
")
                .is_empty()
            );
            assert!(
                run(r"
class TooFew {
  const TooFew._();

  static const e = TooFew._();
}

void t(TooFew e) {
  switch (e) {
    case TooFew.e:
  }
}
")
                .is_empty()
            );
            assert!(
                run(r"
class Subclassed {
  const Subclassed._();

  static const e = Subclassed._();
  static const f = Subclassed._();
  static const g = Subclassed._();
}

class Subclass extends Subclassed {
  Subclass() : super._();
}

void s(Subclassed e) {
  switch (e) {
    case Subclassed.e:
  }
}
")
                .is_empty()
            );
        }

        /// `test_class_enumLike_dotShorthand_missingCase` /
        /// `_withDefault` — the `.e` head face covers and misses like the
        /// spelled form.
        #[test]
        fn mounted_dot_shorthand_heads() {
            let source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    case .f:
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

void fn(E e) {
  switch (e) {
    case .e:
      break;
    default:
      break;
  }
}
")
                .is_empty()
            );
        }

        /// The `test_extensionType_*` family — enum-like extension types
        /// (private representation, two-plus same-typed constants) report
        /// the missing constant; the public-representation and
        /// single-constant shapes stay silent.
        #[test]
        fn mounted_extension_type_family() {
            let source = r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
  static const E c = E._(3);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'c'."
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "switch", 0))
            );
            assert!(
                run(r"
extension type const E(int i) {
  static const E a = E(1);
  static const E b = E(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
")
                .is_empty()
            );
            assert!(
                run(r"
extension type const E._(int i) {
  static const E a = E._(1);
}

void f(E e) {
  switch (e) {}
}
")
                .is_empty()
            );
            assert!(
                run(r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    default:
  }
}
")
                .is_empty()
            );
        }

        /// The shorthand extension-type head (`.a`) — the official
        /// `test_extensionType_enumLike_dotShorthand_missingCase`.
        #[test]
        fn mounted_extension_type_dot_shorthand() {
            let source = r"
extension type const E._(int i) {
  static const E a = E._(1);
  static const E b = E._(2);
}

void f(E e) {
  switch (e) {
    case .a:
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }

        /// `test_enum_missingCase` — a real enum never enters (the
        /// element walk's default arm; the analyzer's own exhaustiveness
        /// error covers it, and that diagnostic is out of this face).
        #[test]
        fn mounted_enum_scrutinee_silent() {
            assert!(
                run(r"
enum Color { red, green }

void f(Color c) {
  switch (c) {
    case Color.red:
  }
}
")
                .is_empty()
            );
        }

        /// The value-based coverage faces: a bare top-level alias head
        /// (`case alias:`) and a cross-class `Class.member` head cover by
        /// value identity — the remaining values report in declaration
        /// order.
        #[test]
        fn mounted_value_based_heads() {
            let alias_source = r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

const alias = E.e;

void fn(E e) {
  switch (e) {
    case alias:
  }
}
";
            let lints = run(alias_source);
            assert_eq!(lints.len(), 2, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'f'."
            );
            assert_eq!(
                lints[1].message,
                "Missing case clauses for some constants in 'g'."
            );
            assert_eq!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
  static const g = E._(3);
}

class Other {
  static const z = E.e;
}

void fn(E e) {
  switch (e) {
    case Other.z:
  }
}
")
                .iter()
                .map(|lint| lint.message.as_str())
                .collect::<Vec<_>>(),
                [
                    "Missing case clauses for some constants in 'f'.",
                    "Missing case clauses for some constants in 'g'.",
                ]
            );
        }

        /// The same-file silences: a non-identifier scrutinee (no written
        /// type on the channel) and an out-of-file candidate (an import
        /// prefix) — the documented under-report family.
        #[test]
        fn mounted_channel_fallbacks_silent() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
}

E make() => E._(1);

void fn() {
  switch (make()) {
    case E.e:
  }
}
")
                .is_empty()
            );
            assert!(
                run(r"
import 'e.dart' as prefixed;

void fn(prefixed.E e) {
  switch (e) {
    case prefixed.E.e:
  }
}
")
                .is_empty()
            );
        }

        /// A creation-only multi-declarator row (`static const a =
        /// E._(1), b = E._(2);`) seals cleanly per declarator, and the
        /// depth-0 `,` cut keeps each value's own tokens: full coverage
        /// is silent, a partial switch reports the uncovered declarator.
        #[test]
        fn mounted_multi_declarator_creations() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
      break;
    case E.b:
      break;
  }
}
")
                .is_empty()
            );
            let source = r"
class E {
  final int i;
  const E._(this.i);

  static const a = E._(1), b = E._(2);
}

void f(E e) {
  switch (e) {
    case E.a:
  }
}
";
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }

        /// The mixed-alias multi-declarator leak pin: a name-shaped
        /// initializer (`newFoo`) never completes a bridge node, so `a`'s
        /// row span can seal past the sibling `,` — without the depth-0
        /// cut the value text interns as `newFoo , b = E._(2)` and `a`'s
        /// alias to `E._(1)` breaks. The official corpus has no
        /// multi-declarator case; the expectation follows
        /// `asEnumLikeType`'s value resolution — `a`'s
        /// `computeConstantValue()` is the `E._(1)` instance, `newFoo`'s
        /// identity, so `case E.newFoo:` covers `a` and the row is
        /// silent, while leaving the shared group open reports it under
        /// its first name.
        #[test]
        fn mounted_multi_declarator_alias_no_false_positive() {
            assert!(
                run(r"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.newFoo:
      break;
    case E.b:
      break;
  }
}
")
                .is_empty()
            );
            let source = r"
class E {
  final int i;
  const E._(this.i);

  static const E a = newFoo, b = E._(2);
  static const newFoo = E._(1);
}

void f(E e) {
  switch (e) {
    case E.b:
  }
}
";
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'a'."
            );
        }

        /// A library-private class's unnamed constructor (`_Priv(`) is
        /// private — the official `isPrivate` is the element name's
        /// leading `_`, which every constructor of the class leads with —
        /// so the enum-like gate holds and the missing case reports; the
        /// public spelling (`Priv(`) stays gate-silent.
        #[test]
        fn mounted_private_class_unnamed_constructor() {
            let source = r"
class _Priv {
  final int i;
  const _Priv(this.i);

  static const a = _Priv(1);
  static const b = _Priv(2);
}

void f(_Priv e) {
  switch (e) {
    case _Priv.a:
  }
}
";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("exhaustive_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Missing case clauses for some constants in 'b'."
            );
        }
    }
}

pub(crate) mod always_put_control_body_on_new_line {
    use super::{Check, Group, NodeKind, RuleSpec, first_child, last_child};

    /// Official: `pkg/linter/lib/src/rules/always_put_control_body_on_new_line.dart:30-78`
    /// — a control structure's body statement on the same line as the
    /// control structure reports at the body's first token. Faces (:38-53):
    /// the `do` body (compared against the `do` keyword's end), the
    /// `for`/`while` bodies and the `if` then-branch (against the closing
    /// `)`), and the `else` branch when it is not another `if` (against the
    /// `else` keyword's end). An empty block body skips (:56).
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "always_put_control_body_on_new_line",
        group: Group::Standalone,
        problem: "Statement should be on a separate line.",
        correction: "Try moving the statement to a new line.",
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
            NodeKind::IfStatement => {
                // The then-branch is the direct child after the condition
                // (the `IfStatement` layout).
                let kids = c.collect_direct(node);
                if let Some(condition) = kids
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
                    && let Some(&then) = kids.get(condition + 1)
                {
                    let control_end = control_end_before(c, then);
                    check_body(c, then, control_end);
                }
                // The else branch (:50-53), skipped for `else if` chains.
                let Some(else_kw) = c
                    .payload(node)
                    .first()
                    .copied()
                    .filter(|&kw| kw != super::NONE_TOKEN)
                else {
                    return;
                };
                if let Some(else_stmt) = last_child(c, node)
                    && c.kind(else_stmt) != NodeKind::IfStatement
                {
                    check_body(c, else_stmt, token_end(c, else_kw));
                }
            }
            NodeKind::WhileStatement | NodeKind::ForStatement | NodeKind::ForIn => {
                if let Some(body) = last_child(c, node) {
                    let control_end = control_end_before(c, body);
                    check_body(c, body, control_end);
                }
            }
            NodeKind::DoWhileStatement => {
                // Compared against the `do` keyword's end (:38-40); the body
                // is the clause's first child.
                if let Some(body) = first_child(c, node) {
                    check_body(c, body, token_end(c, c.head_token(node)));
                }
            }
            _ => {}
        }
    }

    /// The end offset of the token preceding the body — the control
    /// structure's closing `)`.
    fn control_end_before(c: &Check<'_>, body: u32) -> Option<u32> {
        let head = c.head_token(body);
        (head > 0).then(|| token_end(c, head - 1)).flatten()
    }

    fn token_end(c: &Check<'_>, token: u32) -> Option<u32> {
        c.utf16_end_of_token(token)
    }

    /// The official `_checkNodeOnNextLine` (:55-66): an empty block skips,
    /// the compared body offset is the block's first statement's offset,
    /// and the report sits at the body's begin token.
    fn check_body(c: &mut Check<'_>, body: u32, control_end: Option<u32>) {
        let Some(control_end) = control_end else {
            return;
        };
        let first = if c.kind(body) == NodeKind::Block {
            match super::first_child(c, body) {
                Some(first) => first,
                // `node.statements.isEmpty` (:56).
                None => return,
            }
        } else {
            body
        };
        let Some(body_offset) = c.utf16_of_token(c.head_token(first)) else {
            return;
        };
        let same_line = |offset: u32| c.unit.lines.line_col_utf16(c.unit.source, offset).0;
        if same_line(control_end) == same_line(body_offset) {
            c.report(crate::rules::utf16_token_span(c, c.head_token(body)), &[]);
        }
    }
}

pub(crate) mod avoid_catches_without_on_clauses {
    use super::{Check, Group, NodeKind, RuleSpec, last_child};

    /// Official: `pkg/linter/lib/src/rules/avoid_catches_without_on_clauses.dart:30-104`
    /// — a bare `catch` clause (no `on`) reports at the `catch` keyword
    /// unless the caught exception has a *valid use* in the clause body:
    /// a `rethrow` reached before any nested catch clause (:96-101, the
    /// `_canRethrow` fence) or a `throw` expression referencing the caught
    /// exception (:107-117). A clause without an exception parameter or
    /// without a `catch` keyword skips (:69-73, :86-88).
    ///
    /// Approximation (documented): the remaining valid-use faces of the
    /// official `_ValidUseVisitor` (:111-155) key on static types —
    /// `Never`-typed invocations, `Future.error`, `FlutterError.reportError`,
    /// `Completer.completeError` — and stay silent here (under-report). A
    /// throw references the exception when an identifier of its name sits
    /// inside the throw's subtree AND no nearer binder of that name captures
    /// the occurrence (the official `node.element == caughtException`
    /// identity over the same-file scope walk): a closure parameter, a
    /// nested block's local, a nested catch's parameter, a loop variable,
    /// or a case pattern variable of the same lexeme re-binds every lexeme
    /// inside its extent, so those reads are the binder's, never the caught
    /// exception's.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_catches_without_on_clauses",
        group: Group::Standalone,
        problem: "Catch clause should use 'on' to specify the type of exception being caught.",
        correction: "Try adding an 'on' clause before the 'catch'.",
        kinds: &[NodeKind::CatchClause],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `onKeyword != null` (:69-70): the on-form opens at `on`
        // (head-when-present).
        if c.lexeme(c.head_token(node)) == "on" {
            return;
        }
        // `catchKeyword == null` (:86-88).
        let Some(catch_kw) = c
            .payload(node)
            .first()
            .copied()
            .filter(|&kw| kw != super::NONE_TOKEN)
        else {
            return;
        };
        // `exceptionParameter == null` (:72-73): the clause's first
        // `FormalParameter` row (the catch formals ride one
        // `FormalParameters` child, the production probe shape).
        let window = c.children(node);
        let Some(exception) = window
            .clone()
            .find(|&row| c.kind(row) == NodeKind::FormalParameter)
        else {
            return;
        };
        let Some(name) = c
            .payload(exception)
            .get(1)
            .filter(|&&tok| tok != super::NONE_TOKEN)
            .map(|&tok| c.lexeme(tok))
        else {
            return;
        };
        let Some(body) = last_child(c, node) else {
            return;
        };
        if c.kind(body) != NodeKind::Block {
            return;
        }

        // The first nested catch clause row fences the rethrow face
        // (:96-98 `_canRethrow` — false from the first nested `catch` on,
        // never restored).
        let nested_catch = window
            .clone()
            .filter(|&row| row != node && c.kind(row) == NodeKind::CatchClause)
            .min();
        // The bridge flattens a throw's operand rows as siblings completed
        // before the width-0 `throw` row, so a throw references the
        // exception (:107-117) when an identifier of its name sits between
        // the throw's token and the throw row itself — the operand run.
        // Rows after an earlier statement row cannot belong to the throw,
        // which bounds each throw to its own statement.
        let throws: Vec<u32> = window
            .clone()
            .filter(|&row| c.kind(row) == NodeKind::ThrowExpression)
            .collect();
        let mut valid = false;
        for row in window.clone() {
            // The collapse keeps one rethrow kind (the statement form
            // covers `=> rethrow` bodies too).
            if c.kind(row) == NodeKind::RethrowStatement
                && nested_catch.is_none_or(|first| row < first)
            {
                valid = true;
                break;
            }
            if c.kind(row) != NodeKind::Identifier || c.lexeme(c.head_token(row)) != name {
                continue;
            }
            // A nearer binder of the same lexeme re-owns the occurrence (the
            // element identity): the read is not the caught exception.
            if shadowed_by_nearer_binder(c, row, node, name) {
                continue;
            }
            let head = c.head_token(row);
            if throws.iter().any(|&throw| {
                row < throw
                    && head > c.head_token(throw)
                    && window
                        .clone()
                        .filter(|&stmt| stmt < throw && crate::rules::is_statement_row(c, stmt))
                        .all(|stmt| stmt < row)
            }) {
                valid = true;
                break;
            }
        }
        if valid {
            return;
        }
        c.report(crate::rules::utf16_token_span(c, catch_kw), &[]);
    }

    /// Whether a binder of `name` nested between the catch header and the
    /// identifier row `row` captures the occurrence — the same-file stand-in
    /// for the official element identity (`node.element == caughtException`).
    /// The walk descends the subtree extents from the catch clause: the
    /// production tiling keeps the catch body flat (every material row's
    /// parent row is the body block), so `row` lies in `s`'s subtree iff
    /// `children(s).start <= row <= s` — each descending scope that contains
    /// the row contributes its binder set (a block's sibling binding
    /// statements, a nested catch's and a closure's own parameters, a
    /// named/local function's own name and parameters, for-loop heads, and
    /// switch-case pattern variables).
    fn shadowed_by_nearer_binder(c: &Check<'_>, row: u32, catch_clause: u32, name: &str) -> bool {
        let mut child = row;
        let mut scope = catch_clause;
        loop {
            let Some(next) = c
                .collect_direct(scope)
                .into_iter()
                .find(|&kid| kid > row && c.unit.ast.children(kid).start <= row)
            else {
                return false;
            };
            let binds = match c.kind(next) {
                // A function-expression body tiles as `BlockBody`; both
                // block kinds carry sibling statement binders.
                NodeKind::Block | NodeKind::BlockBody => c
                    .collect_direct(next)
                    .into_iter()
                    .any(|stmt| stmt != child && statement_binds(c, stmt, name)),
                NodeKind::CatchClause | NodeKind::FunctionExpression => c
                    .collect_direct(next)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    .is_some_and(|params| parameters_bind(c, params, name)),
                NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                    function_name_binds(c, next, name)
                        || c.collect_direct(next)
                            .into_iter()
                            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                            .is_some_and(|params| parameters_bind(c, params, name))
                }
                NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, next, name),
                NodeKind::ForStatement => c
                    .collect_direct(next)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::ForParts)
                    .is_some_and(|parts| {
                        c.collect_direct(parts)
                            .into_iter()
                            .any(|stmt| statement_binds(c, stmt, name))
                    }),
                NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                    c.collect_direct(next).into_iter().any(|kid| {
                        c.kind(kid) == NodeKind::DeclaredVariablePattern
                            && c.payload(kid).first().is_some_and(|&v| {
                                v != crate::rules::NONE_TOKEN && c.lexeme(v) == name
                            })
                    })
                }
                _ => false,
            };
            if binds {
                return true;
            }
            child = next;
            scope = next;
        }
    }

    /// Whether a statement declares `name`: a variable declaration, a
    /// pattern-variable declaration statement, a local/named function (its
    /// own name binds through its body), or a labeled statement wrapping
    /// one of those.
    fn statement_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
        match c.kind(stmt) {
            NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
                c.kind(decl) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, decl)
                        // The helper answers a TOKEN id — compare its
                        // lexeme directly, never re-head it.
                        .is_some_and(|n| c.lexeme(n) == name)
            }),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                function_name_binds(c, stmt, name)
            }
            NodeKind::PatternVarDeclStatement => c.children(stmt).any(|descendant| {
                c.kind(descendant) == NodeKind::DeclaredVariablePattern
                    && c.payload(descendant)
                        .first()
                        .is_some_and(|&v| v != crate::rules::NONE_TOKEN && c.lexeme(v) == name)
            }),
            NodeKind::LabeledStatement => c
                .collect_direct(stmt)
                .into_iter()
                .any(|kid| c.kind(kid) != NodeKind::Label && statement_binds(c, kid, name)),
            _ => false,
        }
    }

    /// The name a local/named function binds (`FunctionName` child — the
    /// production tiling nests the stamped `Identifier` inside it).
    fn function_name_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
        crate::rules::declarations::member_declaration_name(c, node)
            .is_some_and(|n| c.lexeme(c.head_token(n)) == name)
    }

    /// Whether a formal-parameter list declares `name` (plain and
    /// function-typed formals; the optional named group's formals read
    /// through their wrapper).
    fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
        c.collect_direct(params)
            .into_iter()
            .any(|param| match c.kind(param) {
                NodeKind::FormalParameter => c
                    .payload(param)
                    .get(1)
                    .filter(|&&tok| tok != crate::rules::NONE_TOKEN)
                    .is_some_and(|&tok| c.lexeme(tok) == name),
                NodeKind::FunctionTypedFormalParameter => c
                    .collect_direct(param)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                    .is_some_and(|kid| c.lexeme(c.head_token(kid)) == name),
                NodeKind::OptionalFormalParameters => parameters_bind(c, param, name),
                _ => false,
            })
    }

    /// Whether the for-in loop variable binds `name`: the bare form is an
    /// `Identifier` child completing before the parts' `in` token, the `var
    /// x` form a `DeclaredVariablePattern` child.
    fn for_in_binds(c: &Check<'_>, scope: u32, name: &str) -> bool {
        let kids = c.collect_direct(scope);
        let Some(in_token) = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
            .and_then(|parts| c.payload(parts).get(3).copied())
            .filter(|&token| token != crate::rules::NONE_TOKEN)
        else {
            return false;
        };
        kids.into_iter().any(|kid| match c.kind(kid) {
            NodeKind::DeclaredVariablePattern => c
                .payload(kid)
                .first()
                .is_some_and(|&v| v < in_token && c.lexeme(v) == name),
            // The `var x` form tiles as a `VariablesDeclaration` completing
            // before the `ForInParts` row (its `InitializedIdentifier`
            // carries the loop-variable name on its span head).
            NodeKind::VariablesDeclaration => c.collect_direct(kid).into_iter().any(|var| {
                c.kind(var) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|tok| tok < in_token && c.lexeme(tok) == name)
            }),
            NodeKind::Identifier => {
                c.unit.ast.token_range(kid).end <= in_token && c.lexeme(c.head_token(kid)) == name
            }
            _ => false,
        })
    }
}

pub(crate) mod require_trailing_commas {
    use dartq_parser::AssertKind;

    use super::{Check, Group, NodeKind, RuleSpec, last_child, narrow_u32};

    /// Official: `pkg/linter/lib/src/rules/require_trailing_commas.dart` —
    /// every multi-line argument, parameter, assert, and collection-literal
    /// group ends with a trailing comma. Registration is version-gated: the
    /// rule registers only BELOW the 3.7 tall-style milestone (:52-56,
    /// `languageVersion >= language37` returns before any registration —
    /// from 3.7 `dart format` enforces the commas itself), pinned in
    /// [`crate::registry::VERSION_GATES`].
    ///
    /// Ported faces (:58-121): argument lists, assert statements and
    /// initializers (one [`NodeKind::Assert`] kind here — the upstream
    /// unregistered expression position skips by its kind word), formal
    /// parameter lists (the report pins the optional group's closer,
    /// :100-115; extension-type primary constructors skip, :89-96), and
    /// list/set-or-map literals. `_checkTrailingComma` (:124-146): a
    /// trailing comma after the last element exits, so does a group whose
    /// delimiters share a line; among the exceptions only a MULTILINE last
    /// element qualifies (:137-139) — a block-bodied function literal, a
    /// string literal, an anonymous function call whose argument parens
    /// share a line, or a collection literal. The upstream `isNamed`
    /// formal-parameter gate (:135) is subsumed: no exception class is a
    /// formal parameter, so a named parameter never qualifies either way.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "require_trailing_commas",
        group: Group::Standalone,
        problem: "Missing a required trailing comma.",
        correction: "Try adding a trailing comma.",
        kinds: &[
            NodeKind::Arguments,
            NodeKind::Assert,
            NodeKind::FormalParameters,
            NodeKind::ListLiteral,
            NodeKind::SetOrMapLiteral,
        ],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let span = c.unit.ast.token_range(node);
        match c.kind(node) {
            NodeKind::Arguments => {
                // Named arguments contribute a `name:` handle after their
                // value row; the last argument's value is the last
                // non-handle child. An empty group skips (:61-63).
                let kids = c.collect_direct(node);
                let Some(last) = kids
                    .into_iter()
                    .rev()
                    .find(|&kid| c.kind(kid) != NodeKind::NamedArgument)
                else {
                    return;
                };
                check_group(c, span.start, span.end - 1, last, span.end - 1);
            }
            NodeKind::Assert => {
                // Upstream registers the statement and initializer visitors
                // only (:58-71); the expression position (a recovery shape)
                // is outside the lint. Payload word 0 is the kind, word 1
                // the open paren (the `Assert` payload).
                let kind = c.payload(node).first().copied();
                if kind != Some(AssertKind::Statement as u32)
                    && kind != Some(AssertKind::Initializer as u32)
                {
                    return;
                }
                let Some(last) = last_child(c, node) else {
                    return;
                };
                let open = c
                    .payload(node)
                    .get(1)
                    .copied()
                    .filter(|&t| t != super::NONE_TOKEN)
                    .unwrap_or(span.start);
                check_group(c, open, span.end - 1, last, span.end - 1);
            }
            NodeKind::FormalParameters => {
                // Extension-type primary constructors skip (:89-96).
                if c.ancestor(1)
                    .is_some_and(|a| c.kind(a) == NodeKind::PrimaryConstructor)
                    && c.ancestor(2)
                        .is_some_and(|a| c.kind(a) == NodeKind::ExtensionTypeDeclaration)
                {
                    return;
                }
                let kids = c.collect_direct(node);
                let Some(&last) = kids.last() else {
                    return; // `f()` — the empty list skips (:79).
                };
                if c.kind(last) == NodeKind::OptionalFormalParameters {
                    // The last parameter sits in the optional group; the
                    // report pins the group's own closer (:100-115,
                    // `rightDelimiter ?? rightParenthesis`).
                    let Some(param) = last_child(c, last) else {
                        return; // an empty group is an empty list.
                    };
                    let group_span = c.unit.ast.token_range(last);
                    check_group(c, span.start, span.end - 1, param, group_span.end - 1);
                } else {
                    check_group(c, span.start, span.end - 1, last, span.end - 1);
                }
            }
            NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => {
                let Some(last) = last_child(c, node).or_else(|| {
                    // production: the container seals childless; its
                    // elements leak into the owner run before it — the
                    // last row before the handle is the last element's
                    // tail row.
                    let parent = c.unit.ast.parent(node)?;
                    let kids = c.collect_direct(parent);
                    let pos = kids.iter().position(|&kid| kid == node)?;
                    kids.get(pos.checked_sub(1)?).copied()
                }) else {
                    return;
                };
                check_group(c, span.start, span.end - 1, last, span.end - 1);
            }
            _ => {}
        }
    }

    /// The official `_checkTrailingComma` (:124-146) with the report token
    /// resolved by the face (`errorToken ?? closingToken`).
    fn check_group(c: &mut Check<'_>, open: u32, close: u32, last: u32, error: u32) {
        // A trailing comma already ends the group (:130).
        let next = c.unit.ast.token_range(last).end;
        if c.unit
            .tokens
            .get(next as usize)
            .is_some_and(|_| c.lexeme(next) == ",")
        {
            return;
        }
        // A group whose delimiters share a line needs no comma (:132-136).
        let (Some(open_bytes), Some(close_bytes)) = (token_bytes(c, open), token_bytes(c, close))
        else {
            return;
        };
        if line_of(c, open_bytes.start) == line_of(c, close_bytes.end) {
            return;
        }
        if exception(c, last) {
            return;
        }
        c.report(crate::rules::utf16_token_span(c, error), &[]);
    }

    /// The official `_shouldAllowTrailingCommaException` (:148-171): a
    /// multiline last element that is a block-bodied function literal, a
    /// string literal, an anonymous function call with one-line argument
    /// parens, or a collection literal.
    fn exception(c: &Check<'_>, last: u32) -> bool {
        // The invocation's send leaf tiles only its own tail (`}()`), so
        // its node span lies on the whole-expression multiline face — the
        // arm measures the expression's full extent itself.
        if c.kind(last) == NodeKind::Send {
            return anonymous_call_on_one_line(c, last);
        }
        // A last element that fits on one line never qualifies (:137-139).
        let last_span = c.byte_span(last);
        if line_of(c, narrow_u32(last_span.start)) == line_of(c, narrow_u32(last_span.end)) {
            return false;
        }
        match c.kind(last) {
            // Function literal with a block body (:141-143).
            NodeKind::FunctionExpression => c
                .collect_direct(last)
                .iter()
                .any(|&kid| c.kind(kid) == NodeKind::BlockBody),
            // A (multiline) string literal (:145-147), or a set, map or
            // list literal (:159-160).
            NodeKind::StringLiteral | NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => true,
            _ => false,
        }
    }

    /// The anonymous-function-call exception (:149-157): a send whose
    /// function is the preceding function expression, whose own extent
    /// (function head through call tail) spans lines, and whose argument
    /// parens share a line. The send leaf adopts the function
    /// expression's tail token as its span head (the `}` before the call
    /// parens), so the function pin compares tails, not heads.
    fn anonymous_call_on_one_line(c: &Check<'_>, send: u32) -> bool {
        let Some(parent) = c.unit.ast.parent(send) else {
            return false;
        };
        let kids = c.collect_direct(parent);
        let Some(pos) = kids.iter().position(|&kid| kid == send) else {
            return false;
        };
        // Arena order: the function expression completes, its argument
        // list completes (the bare-identifier send group's type-argument
        // sentinel may ride between them), the send leaf seals last.
        let Some(&args) = pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return false;
        };
        if c.kind(args) != NodeKind::Arguments {
            return false;
        }
        if pos < 2 {
            return false;
        }
        let mut head_pos = pos - 2;
        while matches!(
            c.kind(kids[head_pos]),
            NodeKind::TypeArguments | NodeKind::NoTypeArguments
        ) {
            if head_pos == 0 {
                return false;
            }
            head_pos -= 1;
        }
        let head = kids[head_pos];
        if c.kind(head) != NodeKind::FunctionExpression {
            return false;
        }
        // The send leaf adopts the function expression's tail token as
        // its own span head (the `}` before the call parens) — the head
        // pin is the function's tail, not the send's own `(`.
        if crate::rules::tail_token(c, head) != c.head_token(send) {
            return false;
        }
        // The whole invocation — function head through call tail — must
        // span lines (:137-139), measured over the expression's full
        // extent since the send leaf carries only its own tail.
        let whole = c.byte_span(head).start..c.byte_span(send).end;
        if line_of(c, narrow_u32(whole.start)) == line_of(c, narrow_u32(whole.end)) {
            return false;
        }
        let args_span = c.unit.ast.token_range(args);
        let (Some(first_bytes), Some(last_bytes)) = (
            token_bytes(c, args_span.start),
            token_bytes(c, args_span.end - 1),
        ) else {
            return false;
        };
        line_of(c, first_bytes.start) == line_of(c, last_bytes.end)
    }

    fn token_bytes(c: &Check<'_>, token: u32) -> Option<core::ops::Range<u32>> {
        c.token(token)
            .map(|t| t.byte_offset..t.byte_offset + t.byte_len)
    }

    fn line_of(c: &Check<'_>, byte: u32) -> u32 {
        c.unit.lines.byte_to_line_col(byte).0
    }
}

pub(crate) mod use_test_throws_matchers {
    use super::{Check, Group, NONE_TOKEN, NodeKind, RuleSpec};
    use crate::rules::{is_statement_row, slice};

    /// Official: `pkg/linter/lib/src/rules/use_test_throws_matchers.dart`
    /// — a try statement whose single catch clause is only reached through
    /// a trailing `fail(...)` should use `expect(..., throwsA(...))`
    /// instead (:53-58): exactly one catch clause, a non-empty body, no
    /// `finally`, and the last body statement is an invocation of the
    /// test-framework `fail` (:50-52). Report at the whole statement.
    ///
    /// Approximation (documented): the official resolves the callee to the
    /// top-level `fail` of `package:test_api/src/frontend/expect.dart`
    /// (`isTestInvocation`, :41-49 — `element.library.uri` +
    /// `element.name`). The URI identity face stands in syntactically: the
    /// unit imports a package that exports the test `fail` (`test`,
    /// `test_api`, `flutter_test`), and the callee is a bare unprefixed
    /// `fail` invocation (a member send's trailing access marker fails the
    /// bare-name face, and a `NoArguments` send is a tear-off, not the
    /// call). Over-report: a same-file or shadowing declaration of `fail`
    /// still reports. Under-report: `fail` re-exported through a helper
    /// package with no direct test-framework import stays silent. Message:
    /// `diagnostic.g.dart` `useTestThrowsMatchers`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_test_throws_matchers",
        group: Group::Standalone,
        problem: "Use the 'throwsA' matcher instead of using 'fail' when there is no exception thrown.",
        correction: "Try removing the try-catch and using 'throwsA' to expect an exception.",
        kinds: &[NodeKind::TryStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.catchClauses.length != 1` (:54): the try's direct
        // `CatchClause` children.
        let kids = c.collect_direct(node);
        if kids
            .iter()
            .filter(|&&kid| c.kind(kid) == NodeKind::CatchClause)
            .count()
            != 1
        {
            return;
        }
        // `node.finallyBlock == null` (:56): payload word 0 is the
        // `finally` token (the `TryStatementEnd` payload).
        if c.payload(node).first() != Some(&NONE_TOKEN) {
            return;
        }
        // The body block is the try's first child; `node.body.statements.
        // isEmpty` (:54) and `.last` (:55) read its statement rows — the
        // production tiling lays each statement's expression pieces beside
        // its span-only `ExpressionStatement` leaf, so the trailing send
        // sits immediately before the last statement row.
        let Some(&body) = kids.first() else {
            return;
        };
        if c.kind(body) != NodeKind::Block {
            return;
        }
        let statements = c.collect_direct(body);
        let Some(pos) = statements.iter().rposition(|&row| is_statement_row(c, row)) else {
            return;
        };
        if c.kind(statements[pos]) != NodeKind::ExpressionStatement {
            return;
        }
        if pos == 0 || c.kind(statements[pos - 1]) != NodeKind::Send {
            return;
        }
        // A member send (`x.fail()`) carries a trailing access marker; the
        // official callee is a bare top-level name (:43-46).
        if slice::access_marker(c, &statements, pos - 1, NodeKind::DotAccess).is_some() {
            return;
        }
        let Some((head, arguments)) = slice::send_parts(c, &statements, pos - 1) else {
            return;
        };
        // A `NoArguments` send is a tear-off, not an invocation (:50-52).
        if arguments.is_none() {
            return;
        }
        if c.kind(statements[head]) != NodeKind::Identifier
            || c.lexeme(c.head_token(statements[head])) != "fail"
        {
            return;
        }
        if !imports_test_package(c) {
            return;
        }
        // `rule.reportAtNode(lastBodyStatement)` (:58) — the whole
        // statement through its `;` (the corpus `[!fail('fail');!]`).
        let range = c.utf16_span(statements[pos]);
        c.report(range, &[]);
    }

    /// The URI identity face: the unit imports a package whose public
    /// library exports the test `fail` (`package:test_api` itself, or the
    /// `test` / `flutter_test` facades over it). The written URI's package
    /// segment answers; a relative or non-test import never does.
    fn imports_test_package(c: &Check<'_>) -> bool {
        let Some(&root) = c.path.first() else {
            return false;
        };
        // The import rows nest inside their `TopLevelDeclaration`
        // wrappers — the flat unit window carries them all.
        c.children(root).any(|row| {
            c.kind(row) == NodeKind::Import
                && crate::rules::directives::written_uri(c, row).is_some_and(|(_, uri)| {
                    uri.strip_prefix("package:")
                        .and_then(|rest| rest.split('/').next())
                        .is_some_and(|package| {
                            matches!(package, "test" | "test_api" | "flutter_test")
                        })
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;
    use dartq_language::LanguageVersion;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

    /// A legacy (`Feature.patterns` off, `<=2.19`) fixture — the library
    /// version whose switch members are legacy `SwitchCase` shapes, the only
    /// face `no_duplicate_case_values` processes.
    fn legacy(mut f: Fixture) -> Fixture {
        f.features.patterns = false;
        f
    }

    /// `test/rules/curly_braces_in_flow_control_structures_test.dart`
    /// `test_if_multiline` — a multi-line arm without a block reports.
    #[test]
    fn multiline_if_without_block_reports() {
        let source = "if (b)\n  x;";
        // if0 (1 b2 )3 x4 ;5
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::ExpressionStatement, 4);
                b.leaf_span(NodeKind::Send, 4, 5);
                b.end(NodeKind::ExpressionStatement, 6);
                b.end(NodeKind::IfStatement, 6);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in an if should be enclosed in a block."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// The single-line form is allowed (:62-69).
    #[test]
    fn single_line_if_passes() {
        let source = "if (b) x;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::ExpressionStatement, 4);
                b.leaf_span(NodeKind::Send, 4, 5);
                b.end(NodeKind::ExpressionStatement, 6);
                b.end(NodeKind::IfStatement, 6);
            }),
        );
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    #[test]
    fn block_arm_passes() {
        let source = "if (b) { x; }";
        // if0 (1 b2 )3 {4 x5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.leaf_span(NodeKind::Send, 6, 8);
                b.end(NodeKind::ExpressionStatement, 8);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::IfStatement, 8);
            }),
        );
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    /// With an `else` both arms are checked unconditionally (:71-77).
    #[test]
    fn else_arm_reports_even_on_one_line() {
        let source = "if (b) {} else y;";
        // if0 (1 b2 )3 {4 }5 else6 y7 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, 6);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.leaf_span(NodeKind::Block, 4, 6);
                b.begin(NodeKind::ExpressionStatement, 7);
                b.leaf_span(NodeKind::Send, 7, 8);
                b.end(NodeKind::ExpressionStatement, 9);
                b.end(NodeKind::IfStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
    }

    /// `while` bodies always need a block (`test_while`).
    #[test]
    fn while_body_reports() {
        let source = "while (b) ;";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::WhileStatement, 0);
                identifier(b, 2, 3);
                b.leaf(NodeKind::EmptyStatement, 4);
                b.end(NodeKind::WhileStatement, 5);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a while should be enclosed in a block."
        );
    }

    /// `do` bodies always need a block (`test_do`).
    #[test]
    fn do_body_reports() {
        let source = "do x; while (b);";
        // do0 x1 ;2 while3 (4 b5 )6 ;7
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::DoWhileStatement, 0);
                b.begin(NodeKind::ExpressionStatement, 1);
                b.leaf_span(NodeKind::Send, 1, 2);
                b.end(NodeKind::ExpressionStatement, 3);
                identifier(b, 5, 6);
                b.end(NodeKind::DoWhileStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a do should be enclosed in a block."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 1));
    }

    /// `for` bodies always need a block (`test_for`).
    #[test]
    fn for_body_reports() {
        let source = "for (;b;) ;";
        // for0 (1 ;2 b3 ;4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 1);
                b.patch(1, 2);
                b.patch(2, 4);
                b.patch(3, 0);
                identifier(b, 3, 4);
                b.end(NodeKind::ForParts, 6);
                b.leaf(NodeKind::EmptyStatement, 6);
                b.end(NodeKind::ForStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in a for should be enclosed in a block."
        );
    }

    // —— real-parse arm selection ————————————————————————————————————————
    //
    // Production parses tile the condition into fragment rows ahead of the
    // arms (the head `Identifier`, the `NoTypeArguments` leaf over the
    // follower, the width-0 `NoArguments` anchor, the adopted
    // `ParenthesizedCondition`), which is exactly what fixed child indexes
    // used to report. These pins run through the production bridge.

    /// Block-armed if with an `else` (`tests/corpus/lints/lib/
    /// always_put_control_body_on_new_line.dart:26` shape) — the oracle
    /// emits nothing, and so does the structural arm read.
    #[test]
    fn else_block_arms_stay_silent_on_real_parse() {
        let source = "\
void f(bool c) {
  int x = 0;
  int y = 0;
  if (c) {
    x = 1;
  } else {
    y = 2;
  }
}
";
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    /// `else if` chain with block arms (`tests/corpus/lints/lib/
    /// avoid_double_and_int_checks.dart:10` shape) — the else arm is an
    /// IfStatement and is skipped, the then block passes, and the nested
    /// if's own visit stays silent.
    #[test]
    fn else_if_block_arms_stay_silent_on_real_parse() {
        let source = "\
void f(num p) {
  if (p is double) {
    print(1);
  } else if (p is int) {
    print(2);
  }
}
";
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("curly_braces_in_flow_control_structures"))
                .is_empty()
        );
    }

    /// The `else ;` face (`tests/corpus/lints/lib/flow_family.dart:15`
    /// shape) keeps reporting — at the else statement itself, matching the
    /// oracle anchor (`reportAtNode(elseStatement)`).
    #[test]
    fn else_empty_statement_reports_at_the_semicolon() {
        let source = "\
void f(bool flag) {
  if (flag) {
    print(0);
  } else ;
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in an if should be enclosed in a block."
        );
        let semi = tok(source, &f.tokens, ";", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, semi));
        assert_eq!(lints[0].range.end, lints[0].range.start + 1);
    }

    /// A line-spanning brace-less then arm reports at the arm statement
    /// (`test_if_multiline` through the production bridge — the anchor is
    /// the statement, never a condition fragment).
    #[test]
    fn multiline_braceless_then_reports_at_the_statement() {
        let source = "\
void f(bool b) {
  if (b)
    x;
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Statements in an if should be enclosed in a block."
        );
        let x = tok(source, &f.tokens, "x", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
        assert_eq!(lints[0].range.end, lints[0].range.start + 2);
    }

    // —— control_flow_in_finally ————————————————————————————————————————

    use crate::testsupport::tok as tok_at;

    /// `try {} finally { break; }` — a bare break in a finally reports
    /// (`test/rules/control_flow_in_finally_test.dart` `test_break`).
    #[test]
    fn finally_break_reports() {
        let source = "try {} finally { break; }";
        // try0 {1 }2 finally3 {4 break5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::Break, 5);
                b.patch(0, 0);
                b.end(NodeKind::Break, 7);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::TryStatement, 8);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Use of 'break' in a 'finally' clause.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// A loop inside the finally keeps the break local (:79-91 shape).
    #[test]
    fn finally_break_into_local_loop_passes() {
        let source = "try {} finally { for (;;) { break; } }";
        // try0 {1 }2 finally3 {4 for5 (6 ;7 ;8 )9 {10 break11 ;12 }13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ForStatement, 5);
                b.begin(NodeKind::ForParts, 6);
                b.patch(0, 6);
                b.patch(1, 7);
                b.patch(2, 8);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 10);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::Break, 11);
                b.patch(0, 0);
                b.end(NodeKind::Break, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::ForStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::TryStatement, 15);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// A loop outside the try does not keep the break local — it reports.
    #[test]
    fn finally_break_out_of_enclosing_loop_reports() {
        let source = "for (;;) { try {} finally { break; } }";
        // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 break11 ;12 }13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 2);
                b.patch(1, 2);
                b.patch(2, 3);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::TryStatement, 6);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 7, 9);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::Break, 11);
                b.patch(0, 0);
                b.end(NodeKind::Break, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::TryStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::ForStatement, 15);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
    }

    /// A closure inside the finally keeps the break local (:69-77).
    #[test]
    fn finally_break_inside_closure_passes() {
        let source = "try {} finally { f(() { break; }); }";
        // try0 {1 }2 finally3 {4 f5 (6 (7 )8 {9 break10 ;11 }12 )13 ;14 }15
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ExpressionStatement, 5);
                b.begin(NodeKind::Send, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::Block, 13);
                b.end(NodeKind::FunctionExpression, 13);
                b.end(NodeKind::Arguments, 14);
                b.end(NodeKind::Send, 14);
                b.end(NodeKind::ExpressionStatement, 15);
                b.end(NodeKind::Block, 16);
                b.end(NodeKind::TryStatement, 16);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// `return` in a finally reports (`test_return`).
    #[test]
    fn finally_return_reports() {
        let source = "try {} finally { return; }";
        // try0 {1 }2 finally3 {4 return5 ;6 }7
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.leaf(NodeKind::ReturnStatement, 5);
                b.end(NodeKind::Block, 7);
                b.end(NodeKind::TryStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Use of 'return' in a 'finally' clause.");
    }

    /// A try without a finally never reports.
    #[test]
    fn try_without_finally_passes() {
        let source = "try { break; } catch (e) {}";
        // try0 {1 break2 ;3 }4 catch5 (6 e7 )8 {9 }10
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Block, 1);
                b.begin(NodeKind::Break, 2);
                b.patch(0, 0);
                b.end(NodeKind::Break, 4);
                b.end(NodeKind::Block, 5);
                b.begin(NodeKind::CatchClause, 5);
                b.patch(0, tok_at(source, t, "catch", 0));
                b.patch(1, NONE);
                b.end(NodeKind::CatchClause, 11);
                b.end(NodeKind::TryStatement, 11);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// A labeled loop outside the try: the break's resolved target (the
    /// loop the label binds, `visitLabeledStatement` :631-639) wraps the
    /// try, so the jump crosses the finally — reports.
    #[test]
    fn finally_labeled_break_outside_reports() {
        let source = "loop: for (;;) { try {} finally { break loop; } }";
        // loop0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 break13
        // loop14 ;15 }16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
                b.begin(NodeKind::ForStatement, 2);
                b.begin(NodeKind::ForParts, 3);
                b.patch(0, 3);
                b.patch(1, 4);
                b.patch(2, 5);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::TryStatement, 8);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 9, 11);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 1);
                b.end(NodeKind::Break, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::TryStatement, 17);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::ForStatement, 18);
                b.end(NodeKind::LabeledStatement, 18);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 13));
    }

    /// A label binding a loop inside the finally block itself: the target
    /// sits below the try (its own chain contains it) — no report.
    #[test]
    fn finally_labeled_loop_inside_finally_passes() {
        let source = "try {} finally { inner: for (;;) { break inner; } }";
        // try0 {1 }2 finally3 {4 inner5 :6 for7 (8 ;9 ;10 )11 {12 break13
        // inner14 ;15 }16 }17
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::Label, 5);
                b.end(NodeKind::Label, 7);
                b.begin_adopting(NodeKind::LabeledStatement, 5, 1);
                b.begin(NodeKind::ForStatement, 7);
                b.begin(NodeKind::ForParts, 8);
                b.patch(0, 8);
                b.patch(1, 9);
                b.patch(2, 10);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 12);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 1);
                b.end(NodeKind::Break, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::ForStatement, 17);
                b.end(NodeKind::LabeledStatement, 17);
                b.end(NodeKind::Block, 18);
                b.end(NodeKind::TryStatement, 18);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    /// Nested labels wrapping the try: both labels bind the same loop
    /// through the label scope, which encloses the try — the official
    /// target lookup reports.
    #[test]
    fn finally_nested_labels_wrapping_try_report() {
        let source = "a: b: for (;;) { try {} finally { break a; } }";
        // a0 :1 b2 :3 for4 (5 ;6 ;7 )8 {9 try10 {11 }12 finally13 {14
        // break15 a16 ;17 }18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin(NodeKind::Label, 2);
                b.end(NodeKind::Label, 4);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 2);
                b.begin(NodeKind::ForStatement, 4);
                b.begin(NodeKind::ForParts, 5);
                b.patch(0, 5);
                b.patch(1, 6);
                b.patch(2, 7);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 9);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::TryStatement, 10);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 11, 13);
                b.begin(NodeKind::Block, 14);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 1);
                b.end(NodeKind::Break, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::TryStatement, 19);
                b.end(NodeKind::Block, 20);
                b.end(NodeKind::ForStatement, 20);
                b.end(NodeKind::LabeledStatement, 20);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
    }

    /// The resolution the label walk closes: a loop inside the finally is
    /// the nearest enclosing loop but not the resolved target — `break
    /// outer` binds the outer loop, whose chain lacks the try — reports
    /// (the nearest-loop heuristic kept this silent).
    #[test]
    fn finally_inner_loop_does_not_mask_labeled_target() {
        let source = "outer: for (;;) { try {} finally { for (;;) { break outer; } } }";
        // outer0 :1 for2 (3 ;4 ;5 )6 {7 try8 {9 }10 finally11 {12 for13 (14
        // ;15 ;16 )17 {18 break19 outer20 ;21 }22 }23 }24
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::Label, 0);
                b.end(NodeKind::Label, 2);
                b.begin_adopting(NodeKind::LabeledStatement, 0, 1);
                b.begin(NodeKind::ForStatement, 2);
                b.begin(NodeKind::ForParts, 3);
                b.patch(0, 3);
                b.patch(1, 4);
                b.patch(2, 5);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::TryStatement, 8);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 9, 11);
                b.begin(NodeKind::Block, 12);
                b.begin(NodeKind::ForStatement, 13);
                b.begin(NodeKind::ForParts, 14);
                b.patch(0, 14);
                b.patch(1, 15);
                b.patch(2, 16);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 18);
                b.begin(NodeKind::Block, 18);
                b.begin(NodeKind::Break, 19);
                b.patch(0, 1);
                b.end(NodeKind::Break, 22);
                b.end(NodeKind::Block, 23);
                b.end(NodeKind::ForStatement, 23);
                b.end(NodeKind::Block, 24);
                b.end(NodeKind::TryStatement, 24);
                b.end(NodeKind::Block, 25);
                b.end(NodeKind::ForStatement, 25);
                b.end(NodeKind::LabeledStatement, 25);
            }),
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 19));
    }

    /// A closure resets the unlabeled-break context chain (:224), so a bare
    /// break inside one has no target even with loops outside — the
    /// `FunctionBody` exemption (:69-77) keeps it silent (the nearest-loop
    /// heuristic reported this).
    #[test]
    fn finally_closure_break_with_outer_loop_passes() {
        let source = "for (;;) { try {} finally { f(() { break; }); } }";
        // for0 (1 ;2 ;3 )4 {5 try6 {7 }8 finally9 {10 f11 (12 (13 )14 {15
        // break16 ;17 }18 )19 ;20 }21 }22
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::ForStatement, 0);
                b.begin(NodeKind::ForParts, 1);
                b.patch(0, 1);
                b.patch(1, 2);
                b.patch(2, 3);
                b.patch(3, 0);
                b.end(NodeKind::ForParts, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::TryStatement, 6);
                b.patch(0, tok_at(source, t, "finally", 0));
                b.leaf_span(NodeKind::Block, 7, 9);
                b.begin(NodeKind::Block, 10);
                b.begin(NodeKind::ExpressionStatement, 11);
                b.begin(NodeKind::Send, 11);
                identifier(b, 11, 12);
                b.begin(NodeKind::Arguments, 12);
                b.begin(NodeKind::FunctionExpression, 13);
                b.begin(NodeKind::Block, 15);
                b.begin(NodeKind::Break, 16);
                b.patch(0, 0);
                b.end(NodeKind::Break, 18);
                b.end(NodeKind::Block, 19);
                b.end(NodeKind::FunctionExpression, 19);
                b.end(NodeKind::Arguments, 20);
                b.end(NodeKind::Send, 20);
                b.end(NodeKind::ExpressionStatement, 21);
                b.end(NodeKind::Block, 22);
                b.end(NodeKind::TryStatement, 22);
                b.end(NodeKind::Block, 23);
                b.end(NodeKind::ForStatement, 23);
            }),
        );
        assert!(f.run(&one_rule("control_flow_in_finally")).is_empty());
    }

    // —— no_duplicate_case_values —————————————————————————————————————————

    /// The default-version face (official `test_duplicateIntConstant_ok`/
    /// `test_duplicateStringConstant_ok`/`test_duplicateConstClassValue_ok`/
    /// `test_duplicateEnumValue_ok`): with `Feature.patterns` on (3.0+) every
    /// member is a pattern case and the lint is silent — even on repeated
    /// constant values.
    #[test]
    fn duplicate_case_value_silent_at_default_version() {
        let source = "switch (x) { case 1: break; case 1: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (case_head, int_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                    b.begin(NodeKind::SwitchCase, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1); // one matched expression
                    b.begin(NodeKind::CaseExpression, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.leaf(NodeKind::LiteralInt, int_tok);
                    b.begin_adopting(NodeKind::ConstantPattern, int_tok, 1);
                    b.end(NodeKind::ConstantPattern, int_tok + 1);
                    b.end(NodeKind::CaseExpression, colon + 1);
                    b.begin(NodeKind::Break, int_tok + 2);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, int_tok + 4);
                    b.end(NodeKind::SwitchCase, int_tok + 4);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        );
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// `test/rules/no_duplicate_case_values_test.dart` `test_duplicateCase`
    /// (`NoDuplicateCaseValuesTestLanguage219`) — in a legacy (`patterns`
    /// off) library the duplicate reports with both sources in the message.
    #[test]
    fn duplicate_case_value_reports_legacy() {
        let source = "switch (x) { case 1: break; case 1: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 111 :12 break13 ;14 }15
        let f = legacy(Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (case_head, int_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                    b.begin(NodeKind::SwitchCase, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1); // one matched expression
                    b.begin(NodeKind::CaseExpression, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.leaf(NodeKind::LiteralInt, int_tok);
                    b.begin_adopting(NodeKind::ConstantPattern, int_tok, 1);
                    b.end(NodeKind::ConstantPattern, int_tok + 1);
                    b.end(NodeKind::CaseExpression, colon + 1);
                    b.begin(NodeKind::Break, int_tok + 2);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, int_tok + 4);
                    b.end(NodeKind::SwitchCase, int_tok + 4);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        ));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('1') is equal to the value of an earlier case clause ('1')."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing or changing the value.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 11));
    }

    /// The deepest prefix run the parser fuse lets through (`MAX_DEPTH` − 1
    /// unary operators of AST nesting) still folds and still dedupes — the
    /// fold's cap must never silence a legit span, only the recovered/
    /// pathological runs past it. The minuses are space-separated: an
    /// unbroken `--` run lexes as `MinusMinus` (decrement) tokens, not a
    /// unary chain — `case --1:` is a syntax error upstream, and the fold
    /// already treats that spelling as unknown.
    #[test]
    fn unary_chain_under_cap_still_dedupes() {
        let run = "- ".repeat(499);
        let source = format!("switch (x) {{ case {run}1: break; case {run}1: break; }}");
        // switch0 (1 x2 )3 {4 case5 -(6..504) 1505 :506 break507 ;508
        // case509 -(510..1008) 11009 :1010 break1011 ;1012 }1013
        let f = legacy(Fixture::build(
            &source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (head, one, colon) in [(5u32, 505, 506), (509, 1009, 1010)] {
                    b.begin(NodeKind::SwitchCase, head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1);
                    b.begin(NodeKind::CaseExpression, head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.leaf(NodeKind::LiteralInt, one);
                    b.begin_adopting(NodeKind::ConstantPattern, one - 499, 1);
                    b.end(NodeKind::ConstantPattern, one + 1);
                    b.end(NodeKind::CaseExpression, colon + 1);
                    b.begin(NodeKind::Break, colon + 1);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, colon + 3);
                    b.end(NodeKind::SwitchCase, colon + 3);
                }
                b.end(NodeKind::SwitchBlock, 1014);
                b.end(NodeKind::SwitchStatement, 1014);
            }),
        ));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        let operand = format!("{run}1");
        assert_eq!(
            lints[0].message,
            format!(
                "The value of the case clause ('{operand}') is equal to the value of an earlier case clause ('{operand}')."
            )
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 510));
    }

    /// A pathological prefix run far past the fold's recursion cap: the
    /// walk descends a bounded 501 frames, bails to the unknown-value path,
    /// and the rule stays silent — no stack overflow, no invented
    /// diagnostic (without the cap this span is a ~100k-frame recursion,
    /// past any thread stack). The minuses are space-separated so each is a
    /// real unary `-` token (an unbroken run lexes as `MinusMinus`
    /// decrements, unknown to the fold for the independent syntax-error
    /// reason).
    #[test]
    fn unary_chain_past_depth_cap_stays_silent() {
        let source = format!("switch (x) {{ case {}1: break; }}", "- ".repeat(100_000));
        // switch0 (1 x2 )3 {4 case5 -(6..100005) 1100006 :100007 break100008
        // ;100009 }100010
        let (one, colon) = (100_006u32, 100_007);
        let f = legacy(Fixture::build(
            &source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 5);
                b.patch(0, NONE);
                b.patch(1, colon);
                b.leaf(NodeKind::LiteralInt, one);
                b.begin_adopting(NodeKind::ConstantPattern, 6, 1);
                b.end(NodeKind::ConstantPattern, one + 1);
                b.end(NodeKind::CaseExpression, colon + 1);
                b.begin(NodeKind::Break, colon + 1);
                b.patch(0, 0);
                b.end(NodeKind::Break, colon + 3);
                b.end(NodeKind::SwitchCase, colon + 3);
                b.end(NodeKind::SwitchBlock, one + 5);
                b.end(NodeKind::SwitchStatement, one + 5);
            }),
        ));
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// Distinct values and a `default:` clause (no expressions) pass — the
    /// legacy (`Language219`) face.
    #[test]
    fn distinct_and_default_cases_pass() {
        let source = "switch (x) { case 1: break; case 2: break; default: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 211 :12 break13 ;14
        // default15 :16 break17 ;18 }19
        let f = legacy(Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                // case 1
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.leaf(NodeKind::LiteralInt, 6);
                b.begin_adopting(NodeKind::ConstantPattern, 6, 1);
                b.end(NodeKind::ConstantPattern, 7);
                b.end(NodeKind::CaseExpression, 8);
                b.begin(NodeKind::Break, 8);
                b.patch(0, 0);
                b.end(NodeKind::Break, 10);
                b.end(NodeKind::SwitchCase, 10);
                // case 2
                b.begin(NodeKind::SwitchCase, 10);
                b.patch(0, NONE);
                b.patch(1, 12);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 10);
                b.patch(0, NONE);
                b.patch(1, 12);
                b.leaf(NodeKind::LiteralInt, 11);
                b.begin_adopting(NodeKind::ConstantPattern, 11, 1);
                b.end(NodeKind::ConstantPattern, 12);
                b.end(NodeKind::CaseExpression, 13);
                b.begin(NodeKind::Break, 13);
                b.patch(0, 0);
                b.end(NodeKind::Break, 15);
                b.end(NodeKind::SwitchCase, 15);
                // default (no expressions)
                b.begin(NodeKind::SwitchCase, 15);
                b.patch(0, 15);
                b.patch(1, 16);
                b.patch(2, 0);
                b.begin(NodeKind::Break, 17);
                b.patch(0, 0);
                b.end(NodeKind::Break, 19);
                b.end(NodeKind::SwitchCase, 19);
                b.end(NodeKind::SwitchBlock, 20);
                b.end(NodeKind::SwitchStatement, 20);
            }),
        ));
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// The official keys by computed constant (`computeConstantValue`, :44):
    /// `case 1 + 1:` collides with `case 2:` — the later expression reports
    /// with both sources (legacy face).
    #[test]
    fn computed_duplicate_case_values_report() {
        let source = "switch (x) { case 1 + 1: break; case 2: break; }";
        // switch0 (1 x2 )3 {4 case5 16 +7 18 :9 break10 ;11 case12 213 :14
        // break15 ;16 }17
        let f = legacy(Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.leaf(NodeKind::LiteralInt, 6);
                b.leaf(NodeKind::LiteralInt, 8);
                b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
                b.patch(0, 7);
                b.end(NodeKind::BinaryExpression, 9);
                b.leaf(NodeKind::ConstantPattern, 6);
                b.end(NodeKind::CaseExpression, 10);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::SwitchCase, 12);
                b.begin(NodeKind::SwitchCase, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.leaf(NodeKind::ConstantPattern, 13);
                b.end(NodeKind::CaseExpression, 15);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 0);
                b.end(NodeKind::Break, 17);
                b.end(NodeKind::SwitchCase, 17);
                b.end(NodeKind::SwitchBlock, 18);
                b.end(NodeKind::SwitchStatement, 18);
            }),
        ));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('2') is equal to the value of an earlier case clause ('1 + 1')."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 13));
    }

    /// String concatenation folds: `case 'a' + 'b':` collides with
    /// `case 'ab':` (legacy face).
    #[test]
    fn string_concatenation_duplicate_cases_report() {
        let source = "switch (x) { case 'a' + 'b': break; case 'ab': break; }";
        // switch0 (1 x2 )3 {4 case5 'a'6 +7 'b'8 :9 break10 ;11 case12
        // 'ab'13 :14 break15 ;16 }17
        let f = legacy(Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 5);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.leaf(NodeKind::StringLiteral, 6);
                b.leaf(NodeKind::StringLiteral, 8);
                b.begin_adopting(NodeKind::BinaryExpression, 6, 2);
                b.patch(0, 7);
                b.end(NodeKind::BinaryExpression, 9);
                b.leaf(NodeKind::ConstantPattern, 6);
                b.end(NodeKind::CaseExpression, 10);
                b.begin(NodeKind::Break, 10);
                b.patch(0, 0);
                b.end(NodeKind::Break, 12);
                b.end(NodeKind::SwitchCase, 12);
                b.begin(NodeKind::SwitchCase, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.patch(2, 1);
                b.begin(NodeKind::CaseExpression, 12);
                b.patch(0, NONE);
                b.patch(1, 14);
                b.leaf(NodeKind::ConstantPattern, 13);
                b.end(NodeKind::CaseExpression, 15);
                b.begin(NodeKind::Break, 15);
                b.patch(0, 0);
                b.end(NodeKind::Break, 17);
                b.end(NodeKind::SwitchCase, 17);
                b.end(NodeKind::SwitchBlock, 18);
                b.end(NodeKind::SwitchStatement, 18);
            }),
        ));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1);
        // The official `toString()` arguments carry the source spelling,
        // quotes included (the template's `('…')` wrap doubles them).
        assert_eq!(
            lints[0].message,
            "The value of the case clause (''ab'') is equal to the value of an earlier case clause (''a' + 'b'')."
        );
    }

    /// Expressions outside the folder's grammar evaluate unknown and never
    /// dedupe (the official `hasKnownValue` skip): repeated non-constant
    /// identifiers stay silent (legacy face).
    #[test]
    fn non_constant_case_expressions_pass() {
        let source = "switch (x) { case y: break; case y: break; }";
        // switch0 (1 x2 )3 {4 case5 y6 :7 break8 ;9 case10 y11 :12 break13
        // ;14 }15
        let f = legacy(Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                for (case_head, y_tok, colon) in [(5, 6, 7), (10, 11, 12)] {
                    b.begin(NodeKind::SwitchCase, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    b.patch(2, 1);
                    b.begin(NodeKind::CaseExpression, case_head);
                    b.patch(0, NONE);
                    b.patch(1, colon);
                    identifier(b, y_tok, y_tok + 1);
                    b.begin_adopting(NodeKind::ConstantPattern, y_tok, 1);
                    b.end(NodeKind::ConstantPattern, y_tok + 1);
                    b.end(NodeKind::CaseExpression, colon + 1);
                    b.begin(NodeKind::Break, colon + 1);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, colon + 3);
                    b.end(NodeKind::SwitchCase, colon + 3);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        ));
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
    }

    /// A for-in body is the official `ForStatement.body` face (`for (e in xs)
    /// stmt;`): the non-block body reports "a for" at the statement.
    #[test]
    fn for_in_non_block_body_reports() {
        let source = "void f(List<int> l) {\n  for (final a in l) print(a);\n}";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Statements in a for should be enclosed in a block."
        );
        // ASCII source: the byte offset of the body statement is its utf16
        // column face.
        assert_eq!(
            lints[0].range.start,
            source.find("print(a);").unwrap() as u32
        );
        assert_eq!(
            lints[0].range.end,
            (source.find("print(a);").unwrap() + 9) as u32
        );
    }

    /// With an `else`, each arm reports exactly once — the then arm's
    /// selection reads the window geometrically (the production tiling
    /// spills the else side's inner rows around its leaf, where arena
    /// adjacency lands on the wrong row).
    #[test]
    fn else_arms_report_each_once() {
        let source = "void f(bool c) {\n  if (c) print(1); else print(2);\n}";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        let starts: Vec<u32> = lints.iter().map(|lint| lint.range.start).collect();
        let then_at = source.find("print(1);").unwrap() as u32;
        let else_at = source.find("print(2);").unwrap() as u32;
        assert!(starts.contains(&then_at), "{lints:?}");
        assert!(starts.contains(&else_at), "{lints:?}");
        // Both arms report the full statement leaf (`print(n);`).
        for lint in &lints {
            assert_eq!(lint.range.end - lint.range.start, 9, "{lints:?}");
        }
    }

    /// A `do` body reports at the whole statement (the production tiling's
    /// spilled head fragment — the bare `print` send head — is not the arm).
    #[test]
    fn do_body_reports_at_the_statement() {
        let source = "void f(bool c) {\n  do print(4); while (c);\n}";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("curly_braces_in_flow_control_structures"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Statements in a do should be enclosed in a block."
        );
        assert_eq!(
            lints[0].range.start,
            source.find("print(4);").unwrap() as u32
        );
        assert_eq!(
            lints[0].range.end,
            (source.find("print(4);").unwrap() + 9) as u32
        );
    }

    /// Constant folding across heads: `case 1 + 2:` collides with `case 3:`
    /// — the later reports at `3` with both sources spelled as written (the
    /// real-parse shape, the `lib/invalid_case_patterns.dart` row).
    #[test]
    fn folded_duplicate_case_values_report() {
        let source = "void f(dynamic o) {\n  switch (o) {\n    case 1 + 2:\n      break;\n    case \
                      3:\n      break;\n  }\n}";
        let f = legacy(Fixture::parse(source));
        let lints = f.run(&one_rule("no_duplicate_case_values"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "The value of the case clause ('3') is equal to the value of an earlier case clause \
             ('1 + 2')."
        );
        assert_eq!(lints[0].range.start, source.find("3:").unwrap() as u32);
    }

    // —— use_rethrow_when_possible ————————————————————————————————————————

    /// `test/rules/use_rethrow_when_possible_test.dart` `test_rethrow` — the
    /// caught parameter rethrown as a statement reports at the throw.
    #[test]
    fn rethrow_of_the_exception_reports() {
        let source = "try {} catch (e) { throw e; }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 throw8 e9 ;10 }11
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::ThrowExpression, 8);
                identifier(b, 9, 10);
                b.end(NodeKind::ThrowExpression, 11);
                b.end(NodeKind::ExpressionStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::CatchClause, 12);
                b.end(NodeKind::TryStatement, 12);
            }),
        );
        let lints = f.run(&one_rule("use_rethrow_when_possible"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'rethrow' to rethrow a caught exception."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 8));
    }

    /// Throwing a different name and an `on`-only catch (no exception
    /// parameter) pass.
    #[test]
    fn rethrow_other_name_and_on_form_pass() {
        let other = "try {} catch (e) { throw x; }";
        let f = Fixture::build(
            other,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::ThrowExpression, 8);
                identifier(b, 9, 10);
                b.end(NodeKind::ThrowExpression, 11);
                b.end(NodeKind::ExpressionStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::CatchClause, 12);
                b.end(NodeKind::TryStatement, 12);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());

        // try0 {1 }2 on3 E4 {5 throw6 e7 ;8 }9 — no exception parameter.
        let on_form = "try {} on E { throw e; }";
        let f = Fixture::build(
            on_form,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, NONE);
                b.patch(1, NONE);
                b.leaf_span(NodeKind::Type, 4, 5);
                b.begin(NodeKind::Block, 5);
                b.begin(NodeKind::ExpressionStatement, 6);
                b.begin(NodeKind::ThrowExpression, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::ThrowExpression, 9);
                b.end(NodeKind::ExpressionStatement, 9);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::CatchClause, 10);
                b.end(NodeKind::TryStatement, 10);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    /// A throw inside an argument is not a statement throw (:37).
    #[test]
    fn rethrow_in_argument_position_passes() {
        let source = "try {} catch (e) { g(throw e); }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 g8 (9 throw10 e11 )12 ;13 }14
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ExpressionStatement, 8);
                b.begin(NodeKind::Send, 8);
                identifier(b, 8, 9);
                b.begin(NodeKind::Arguments, 9);
                b.begin(NodeKind::ThrowExpression, 10);
                identifier(b, 11, 12);
                b.end(NodeKind::ThrowExpression, 13);
                b.end(NodeKind::Arguments, 13);
                b.end(NodeKind::Send, 13);
                b.end(NodeKind::ExpressionStatement, 14);
                b.end(NodeKind::Block, 15);
                b.end(NodeKind::CatchClause, 15);
                b.end(NodeKind::TryStatement, 15);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    /// A local shadowing the catch parameter binds the throw — the lexical
    /// face of the official element identity (`e` the local, not the
    /// parameter): no report.
    #[test]
    fn rethrow_of_shadowing_local_passes() {
        let source = "try {} catch (e) { var e = 1; throw e; }";
        // try0 {1 }2 catch3 (4 e5 )6 {7 var8 e9 =10 111 ;12 throw13 e14 ;15 }16
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TryStatement, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Block, 1, 3);
                b.begin(NodeKind::CatchClause, 3);
                b.patch(0, 3);
                b.patch(1, NONE);
                b.begin(NodeKind::FormalParameter, 5);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                b.end(NodeKind::FormalParameter, 6);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::VariablesDeclaration, 8);
                b.patch(0, NONE);
                b.begin(NodeKind::InitializedIdentifier, 9);
                identifier(b, 9, 10);
                b.begin(NodeKind::VariableInitializer, 10);
                b.leaf(NodeKind::LiteralInt, 11);
                b.end(NodeKind::VariableInitializer, 12);
                b.end(NodeKind::InitializedIdentifier, 12);
                b.end(NodeKind::VariablesDeclaration, 13);
                b.begin(NodeKind::ExpressionStatement, 13);
                b.begin(NodeKind::ThrowExpression, 13);
                identifier(b, 14, 15);
                b.end(NodeKind::ThrowExpression, 16);
                b.end(NodeKind::ExpressionStatement, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::CatchClause, 17);
                b.end(NodeKind::TryStatement, 17);
            }),
        );
        assert!(f.run(&one_rule("use_rethrow_when_possible")).is_empty());
    }

    // —— real-parse operand and parameter reads ——————————————————————————
    //
    // Production parses leave the throw a childless handle (the operand
    // rows precede it as siblings) and wrap the catch formals in a
    // `FormalParameters` container; both reads answer on those shapes
    // through the production bridge.

    /// `catch (e) { throw e; }` (`tests/corpus/lints/lib/
    /// avoid_catches_without_on_clauses.dart:47` and
    /// `flow_family.dart:34` shapes) reports at the `throw e` expression.
    #[test]
    fn bare_catch_rethrow_reports_on_real_parse() {
        let source = "\
void f() {
  try {
    print(1);
  } catch (e) {
    throw e;
  }
}
";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("use_rethrow_when_possible"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'rethrow' to rethrow a caught exception."
        );
        let throw_tok = tok(source, &f.tokens, "throw", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, throw_tok));
        // `throw e` — the throw keyword through the operand.
        assert_eq!(lints[0].range.end, lints[0].range.start + 7);
    }

    /// A stack-trace formal does not disturb the exception-parameter read.
    #[test]
    fn rethrow_with_stack_trace_parameter_reports_on_real_parse() {
        let source = "void f() {\n  try {} catch (e, s) {\n    throw e;\n  }\n}\n";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("use_rethrow_when_possible"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'rethrow' to rethrow a caught exception."
        );
    }

    /// Non-simple operands, other names, an `on`-only catch, and a
    /// shadowing local all stay silent on real parses.
    #[test]
    fn rethrow_non_simple_and_shadowed_faces_stay_silent_on_real_parse() {
        let one_rule_set = one_rule("use_rethrow_when_possible");
        // A parenthesized operand is not the parameter reference.
        let f = Fixture::parse("void f() {\n  try {} catch (e) {\n    throw (e);\n  }\n}\n");
        assert!(f.run(&one_rule_set).is_empty());
        // A member invocation is not the parameter reference.
        let f =
            Fixture::parse("void f() {\n  try {} catch (e) {\n    throw e.toString();\n  }\n}\n");
        assert!(f.run(&one_rule_set).is_empty());
        // A different name.
        let f = Fixture::parse("void f() {\n  try {} catch (e) {\n    throw x;\n  }\n}\n");
        assert!(f.run(&one_rule_set).is_empty());
        // An `on`-only catch has no exception parameter (:42-43).
        let f = Fixture::parse("void f() {\n  try {} on E {\n    throw e;\n  }\n}\n");
        assert!(f.run(&one_rule_set).is_empty());
        // A shadowing local binds the throw (`e` the local, not the
        // parameter).
        let f = Fixture::parse(
            "void f() {\n  try {} catch (e) {\n    var e = 1;\n    throw e;\n  }\n}\n",
        );
        assert!(f.run(&one_rule_set).is_empty());
        // A shadowing for-in loop variable binds the throw the same way.
        let f = Fixture::parse(
            "void f() {\n  try {} catch (e) {\n    for (var e in xs) {\n      throw e;\n    }\n  }\n}\n",
        );
        assert!(f.run(&one_rule_set).is_empty());
    }

    // —— avoid_returning_null_for_void ————————————————————————————————————

    /// `test/rules/avoid_returning_null_for_void_test.dart` — a top-level
    /// function carries the function text.
    #[test]
    fn returning_null_from_void_function_reports() {
        let source = "void f() { return null; }";
        // void0 f1 (2 )3 {4 return5 null6 ;7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ReturnStatement, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::ReturnStatement, 8);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::TopLevelMethod, 9);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a function with a return type of 'void'."
        );
        assert_eq!(
            lints[0].correction,
            Some("Try removing the 'null'.".to_owned())
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 5));
    }

    /// Inside a class the method variant reports.
    #[test]
    fn returning_null_from_void_method_reports() {
        let source = "class A { void m() { return null; } }";
        // class0 A1 {2 void3 m4 (5 )6 {7 return8 null9 ;10 }11 }12
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
                b.begin(NodeKind::Method, 3);
                b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::VoidType, 3);
                identifier(b, 4, 5);
                b.begin(NodeKind::FormalParameters, 5);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::Block, 7);
                b.begin(NodeKind::ReturnStatement, 8);
                b.leaf(NodeKind::LiteralNull, 9);
                b.end(NodeKind::ReturnStatement, 11);
                b.end(NodeKind::Block, 12);
                b.end(NodeKind::Method, 12);
                b.end(NodeKind::DeclarationBody, 13);
                b.end(NodeKind::ClassDeclaration, 13);
                b.end(NodeKind::TopLevelDeclaration, 13);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a method with a return type of 'void'."
        );
    }

    /// A non-void declared type, a closure face, and `async void` pass.
    #[test]
    fn returning_null_non_void_and_async_pass() {
        let typed = "int f() { return null; }";
        // int0 f1 (2 )3 {4 return5 null6 ;7 }8
        let f = Fixture::build(
            typed,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::Type, 0, 1);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ReturnStatement, 5);
                b.leaf(NodeKind::LiteralNull, 6);
                b.end(NodeKind::ReturnStatement, 8);
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::TopLevelMethod, 9);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

        // The closure is the nearest function — no written return type.
        let closure = "void f() { g(() { return null; }); }";
        // void0 f1 (2 )3 {4 g5 (6 (7 )8 {9 return10 null11 ;12 }13 )14 ;15 }16
        let f = Fixture::build(
            closure,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::Block, 4);
                b.begin(NodeKind::ExpressionStatement, 5);
                b.begin(NodeKind::Send, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                b.begin(NodeKind::FunctionExpression, 7);
                b.begin(NodeKind::FormalParameters, 7);
                b.end(NodeKind::FormalParameters, 9);
                b.begin(NodeKind::Block, 9);
                b.begin(NodeKind::ReturnStatement, 10);
                b.leaf(NodeKind::LiteralNull, 11);
                b.end(NodeKind::ReturnStatement, 13);
                b.end(NodeKind::Block, 14);
                b.end(NodeKind::FunctionExpression, 14);
                b.end(NodeKind::Arguments, 15);
                b.end(NodeKind::Send, 15);
                b.end(NodeKind::ExpressionStatement, 16);
                b.end(NodeKind::Block, 17);
                b.end(NodeKind::TopLevelMethod, 17);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());

        // `void f() async { return null; }` — the async body skips both
        // official branches (:75-80). The brace body is a `BlockBody`
        // carrying the `async` token in its modifier words.
        let async_fn = "void f() async { return null; }";
        // void0 f1 (2 )3 async4 {5 return6 null7 ;8 }9
        let f = Fixture::build(
            async_fn,
            unit_of(|b, t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::BlockBody, 4);
                b.patch(0, tok_at(async_fn, t, "async", 0));
                b.patch(1, NONE);
                b.begin(NodeKind::ReturnStatement, 6);
                b.leaf(NodeKind::LiteralNull, 7);
                b.end(NodeKind::ReturnStatement, 9);
                b.end(NodeKind::BlockBody, 10);
                b.end(NodeKind::TopLevelMethod, 10);
            }),
        );
        assert!(f.run(&one_rule("avoid_returning_null_for_void")).is_empty());
    }

    /// The arrow-body face: `void f() => null;` reports.
    #[test]
    fn returning_null_from_arrow_body_reports() {
        let source = "void f() => null;";
        // void0 f1 (2 )3 =>4 null5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::TopLevelMethod, 0);
                b.patch(0, NONE);
                b.leaf(NodeKind::VoidType, 0);
                identifier(b, 1, 2);
                b.begin(NodeKind::FormalParameters, 2);
                b.end(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::ExpressionBody, 4);
                b.patch(0, 4);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.leaf(NodeKind::LiteralNull, 5);
                b.end(NodeKind::ExpressionBody, 7);
                b.end(NodeKind::TopLevelMethod, 7);
            }),
        );
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// `test_function_async_returnsFutureVoid_blockBody_returnNull` — the
    /// async `Future<void>` branch (:77-80) reports inside a block body.
    #[test]
    fn returning_null_from_async_future_void_reports() {
        let f = Fixture::parse("Future<void> f() async { return null; }");
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a function with a return type of 'void'."
        );
    }

    /// `test_function_async_returnsFutureVoid_expressionBody_returnNull` —
    /// the async arrow reports over `async => null;` (the official
    /// `ExpressionFunctionBody.beginToken` is the `async` keyword).
    #[test]
    fn returning_null_from_async_arrow_reports() {
        let f = Fixture::parse("Future<void> f() async => null;");
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        // The official body span covers `async => null;` — from the `async`
        // keyword through the `;` (ASCII source: byte == utf16 offset).
        let async_offset = f.source.find("async").expect("async token") as u32;
        assert_eq!(lints[0].range.start, async_offset);
        assert_eq!(lints[0].range.end, f.source.len() as u32);
    }

    /// `test_method_inClass_blockBody_async_returnsFutureVoid_blockBody_returnNull`
    /// — the async method takes the method message variant.
    #[test]
    fn returning_null_from_async_future_void_method_reports() {
        let f = Fixture::parse("class C { Future<void> m() async { return null; } }");
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Don't return 'null' from a method with a return type of 'void'."
        );
    }

    /// `test_localFunction_async_returnsFutureVoid_blockBody_returnNull` —
    /// a local `Future<void> g() async` reports.
    #[test]
    fn returning_null_from_async_local_function_reports() {
        let f = Fixture::parse("void f() { Future<void> g() async { return null; } }");
        let lints = f.run(&one_rule("avoid_returning_null_for_void"));
        assert_eq!(lints.len(), 1);
    }

    /// The inferred-closure face: a `void Function()` context types the
    /// anonymous closure's return as `void` (the resolved
    /// `declaredFragment.element.returnType`), top-level field and local
    /// variable positions alike.
    #[test]
    fn returning_null_from_function_typed_closure_reports() {
        for source in [
            "void Function() f = () { return null; };",
            "void f() { void Function() g = () { return null; }; }",
            "class C { void Function() f = () { return null; }; }",
            "void Function() f = () => null;",
        ] {
            let f = Fixture::parse(source);
            assert_eq!(
                f.run(&one_rule("avoid_returning_null_for_void")).len(),
                1,
                "{source}"
            );
        }
    }

    /// The async closure face: a `Future<void> Function()` context plus an
    /// `async` body reports (:77-80 through the context type).
    #[test]
    fn returning_null_from_async_function_typed_closure_reports() {
        let f = Fixture::parse("Future<void> Function() f = () async { return null; };");
        assert_eq!(f.run(&one_rule("avoid_returning_null_for_void")).len(), 1);
    }

    /// The negative async/context faces: a nullable `Future<void>?`, a
    /// `FutureOr<void>`, a non-void context (`int Function()`), and an
    /// argument-position closure without a resolvable context all stay
    /// silent (`isDartAsyncFuture`/`VoidType` misses, :75-80).
    #[test]
    fn returning_null_async_and_context_negatives_pass() {
        for source in [
            "Future<void>? f() async { return null; }",
            "FutureOr<void> f() async { return null; }",
            "Future<int> f() async { return null; }",
            "int Function() f = () { return null; };",
            "void f() { g(() { return null; }); }",
            "void f() { g(() => null); }",
            "var f = () { return null; };",
        ] {
            let f = Fixture::parse(source);
            assert!(
                f.run(&one_rule("avoid_returning_null_for_void")).is_empty(),
                "{source}"
            );
        }
    }

    // —— type_literal_in_constant_pattern ———————————————————————————————————

    /// `case int:` — a built-in type name in constant-pattern position
    /// reports over the pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
    /// `test_BuiltInTypePattern`).
    #[test]
    fn type_literal_reports_a_builtin_type_pattern() {
        let source = "switch (x) { case int: break; }";
        // switch0 (1 x2 )3 {4 case5 int6 :7 break8 ;9 }10
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ConstantPattern, 7);
                b.end(NodeKind::SwitchCase, 8);
                b.end(NodeKind::SwitchBlock, 10);
                b.end(NodeKind::SwitchStatement, 10);
            }),
        );
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'TypeName _' instead of a type literal."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
        assert_eq!(lints[0].range.end, utf16_at(&f, 7));
    }

    /// A const-marked pattern bails (:45-47), a non-builtin constant
    /// (`case color:`) never matches the name set, and a property access
    /// (`case Foo.bar:`) is not a bare type literal.
    #[test]
    fn type_literal_const_marked_and_non_type_patterns_pass() {
        let const_marked = "switch (x) { case const int: break; }";
        // switch0 (1 x2 )3 {4 case5 const6 int7 :8 break9 ;10 }11
        let f = Fixture::build(
            const_marked,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::ConstantPattern, 8);
                b.end(NodeKind::SwitchCase, 9);
                b.end(NodeKind::SwitchBlock, 11);
                b.end(NodeKind::SwitchStatement, 11);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        let user_constant = "switch (x) { case color: break; }";
        let f = Fixture::build(
            user_constant,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                identifier(b, 6, 7);
                b.end(NodeKind::ConstantPattern, 7);
                b.end(NodeKind::SwitchCase, 8);
                b.end(NodeKind::SwitchBlock, 10);
                b.end(NodeKind::SwitchStatement, 10);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        let property = "switch (x) { case Foo.bar: break; }";
        // switch0 (1 x2 )3 {4 case5 Foo6 .7 bar8 :9 break10 ;11 }12
        let f = Fixture::build(
            property,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.begin(NodeKind::ConstantPattern, 6);
                b.begin(NodeKind::DotAccess, 6);
                b.patch(0, 7);
                b.patch(1, NONE);
                identifier(b, 6, 7);
                identifier(b, 8, 9);
                b.end(NodeKind::DotAccess, 9);
                b.end(NodeKind::ConstantPattern, 9);
                b.end(NodeKind::SwitchCase, 10);
                b.end(NodeKind::SwitchBlock, 12);
                b.end(NodeKind::SwitchStatement, 12);
            }),
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    /// Production-bridge face: the if-case `case int` reports over the
    /// pattern (`test/rules/type_literal_in_constant_pattern_test.dart`
    /// core-type case).
    #[test]
    fn type_literal_if_case_reports_the_pattern() {
        let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'TypeName _' instead of a type literal."
        );
        let int = tok(&f.source, &f.tokens, "int", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, int));
        assert_eq!(lints[0].range.end, utf16_at(&f, int) + 3);
    }

    /// The `Type`-matched-value exemption (:50-59) through the syntactic
    /// face on real parses: a `Type`-typed parameter, a `Type`-typed local,
    /// a `.runtimeType` scrutinee, and a const-marked pattern all stay
    /// silent (`test_constType_matchType`).
    #[test]
    fn type_literal_type_matched_value_passes() {
        let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the matched value is the Type-typed parameter"
        );

        let f = Fixture::parse("void f() { Type t = int; if (t case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the matched value is the Type-typed local"
        );

        let f = Fixture::parse("void f(Object x) { if (x.runtimeType case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "a .runtimeType scrutinee is always Type"
        );

        let f = Fixture::parse("void f(Object x) { if (x case const (int)) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "a const-marked pattern bails (:45-47)"
        );
    }

    // —— the converted audit faces (exemption-reaudit F4/F5): the
    // production-bridge pins for the resolution-backed decisions ——

    /// The forward `continue L` to a LATER switch member resolves through
    /// the member-label scope (scope_analyzer.dart:846-850); the target
    /// member's chain contains the try, so the finally jump stays local
    /// and stays silent (the oracle probe face).
    #[test]
    fn finally_forward_continue_member_label_passes() {
        assert!(
            Fixture::parse(
                r"
void f(bool c) {
  try {
    print(0);
  } finally {
    switch (c) {
      case true:
        continue L;
      L:
      case false:
        print(1);
    }
  }
}
"
            )
            .run(&one_rule("control_flow_in_finally"))
            .is_empty()
        );
    }

    /// The same forward jump with the target member OUTSIDE the try: the
    /// target's chain misses the try and the jump reports (the oracle
    /// probe reports `break L` at the same shape).
    #[test]
    fn finally_forward_continue_outside_try_reports() {
        let f = Fixture::parse(
            r"
void f(bool c) {
  switch (c) {
    case true:
      try {
        print(0);
      } finally {
        continue L;
      }
    L:
    case false:
      print(1);
  }
}
",
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Use of 'continue' in a 'finally' clause.");
    }

    /// A forward `continue L` to a LATER labeled statement (no switch)
    /// stays undefined officially (`LABEL_UNDEFINED` — labeled statements
    /// bind only within their statement) — the unresolved-target face
    /// reports.
    #[test]
    fn finally_forward_continue_to_later_labeled_statement_reports() {
        let f = Fixture::parse(
            r"
void f(bool c) {
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
    L:
    while (c) {
      print(1);
    }
  }
}
",
        );
        let lints = f.run(&one_rule("control_flow_in_finally"));
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// `continue L` onto the try statement itself (`L2: try … finally {
    /// continue L2; }`): the target IS the try — the enabler matches and
    /// the jump stays silent (the oracle probe face).
    #[test]
    fn finally_continue_onto_labeled_try_passes() {
        assert!(
            Fixture::parse(
                r"
void f(bool c) {
  L:
  try {
    print(0);
  } finally {
    if (c) {
      continue L;
    }
  }
}
"
            )
            .run(&one_rule("control_flow_in_finally"))
            .is_empty()
        );
    }

    /// The resolution-backed type-literal test: a same-file user type
    /// reports (the under-report face is dead), a same-file value shadow
    /// of a built-in name stays silent (the over-report face is dead).
    #[test]
    fn type_literal_resolution_backed_faces() {
        // `case MyType` on a same-file class reports.
        let f = Fixture::parse(
            r"
class MyType {}

void f(Object o) {
  if (o case MyType) {
    print(0);
  }
}
",
        );
        let lints = f.run(&one_rule("type_literal_in_constant_pattern"));
        assert_eq!(lints.len(), 1, "{lints:?}");

        // `const String = 'x';` shadows the built-in — the pattern is the
        // String constant, not a type literal.
        let f = Fixture::parse(
            r"
const String = 'x';

void f(Object o) {
  if (o case String) {
    print(0);
  }
}
",
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );

        // A local `var int` shadow — the name resolves to the value.
        let f = Fixture::parse("void f(Object o) { var int = 5; if (o case int) { print(int); } }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    /// The class-scope static face: a `static const` named like a built-in
    /// type is a value in scope inside its container (the binding walk
    /// sees instance members only) — the bare constant resolves to the
    /// member and stays silent; outside the container the spelling is the
    /// type and reports.
    #[test]
    fn type_literal_class_scope_static_shadow_passes() {
        let f = Fixture::parse(
            r"
class C {
  static const int = 3;
  void g(Object o) {
    if (o case int) {
      print(0);
    }
  }
}
",
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the static shadows the spelling inside C"
        );

        // Outside the container the spelling is the type again.
        let f = Fixture::parse(
            r"
class C {
  static const int = 3;
}

void f(Object o) {
  if (o case int) {
    print(0);
  }
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );
    }

    /// The enclosing enum's own constants are values in scope inside the
    /// enum's members — a constant named like a built-in type stays
    /// silent there.
    #[test]
    fn type_literal_enum_constant_shadow_passes() {
        let f = Fixture::parse(
            r"
enum E {
  int;

  void g(Object o) {
    if (o case int) {
      print(0);
    }
  }
}
",
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty(),
            "the enum's own constant is a value inside E"
        );
    }

    /// The const-alias fold: a Type-valued constant reports like the
    /// official static type (`case t` where `t` is `int`) — top-level,
    /// local, and qualified-class spellings; a value alias stays silent.
    #[test]
    fn type_literal_const_alias_faces() {
        // Top-level `const t = int;` reports.
        let f = Fixture::parse(
            r"
const t = int;

void f(Object o) {
  if (o case t) {
    print(0);
  }
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );

        // Local `const t = int;` reports (the oracle probe face).
        let f = Fixture::parse(
            r"
void f(Object o) {
  const t = int;
  if (o case t) {
    print(0);
  }
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );

        // A value alias (`const s = 'x';`) stays silent.
        let f = Fixture::parse(
            r"
const s = 'x';

void f(Object o) {
  if (o case s) {
    print(0);
  }
}
",
        );
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    // —— always_put_control_body_on_new_line ————————————————————————————

    /// `test/rules/always_put_control_body_on_new_line.dart` — a then-body
    /// on the control line reports at the body's first token.
    #[test]
    fn control_body_reports_a_same_line_then_body() {
        let source = "void f(bool c) { if (c) x = 1; }";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Statement should be on a separate line.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try moving the statement to a new line.")
        );
        let x = tok(&f.source, &f.tokens, "x", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// A non-empty block whose first statement shares the control line
    /// reports; the body on its own line passes.
    #[test]
    fn control_body_block_faces() {
        let f = Fixture::parse("void f(bool c) { if (c) { x = 1; } }");
        assert_eq!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .len(),
            1
        );
        let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  }\n}");
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    /// An empty block skips (:56).
    #[test]
    fn control_body_empty_block_passes() {
        let f = Fixture::parse("void f(bool c) { if (c) {} }");
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    /// `while`, `for`, and `for-in` bodies are checked against the
    /// closing `)`.
    #[test]
    fn control_body_loop_faces() {
        let f = Fixture::parse(
            "void f(bool c) { while (c) x = 1; }\n\
             void g() { for (var i = 0; i < 1; i++) x = i; }\n\
             void h() { for (var e in [1]) print(e); }",
        );
        assert_eq!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .len(),
            3
        );
    }

    /// The `do` body is compared against the `do` keyword's end (:38-40).
    #[test]
    fn control_body_do_face() {
        let f = Fixture::parse("void f(bool c) { int x = 0; do x += 1; while (c); }");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        // The reported token is the body's first `x` (the second in the
        // source), not the declaration's.
        let x = tok(&f.source, &f.tokens, "x", 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, x));
    }

    /// The else branch checks against the `else` keyword (:50-53) — the
    /// report sits at the else clause's begin, the block's `{`; an
    /// `else if` chain skips.
    #[test]
    fn control_body_else_faces() {
        let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  } else { y = 2; }\n}");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "{", 2))
        );
        let f = Fixture::parse(
            "void f(bool c, bool d) {\n  if (c) {\n    x = 1;\n  } else if (d) {\n    y = 2;\n  }\n}",
        );
        assert!(
            f.run(&one_rule("always_put_control_body_on_new_line"))
                .is_empty()
        );
    }

    /// Pin: a non-block `else` body on the `else` keyword's line reports
    /// through the `token_end(else_kw)` anchor and the `body_offset` head
    /// lookup over a non-Block `first` — both migrated helpers feed the
    /// same comparison.
    #[test]
    fn control_body_else_single_statement_reports() {
        let f = Fixture::parse("void f(bool c) {\n  if (c) {\n    x = 1;\n  } else y = 2;\n}");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "y", 0))
        );
    }

    /// Pin: a `do` body block on the `do` keyword's line reports through
    /// `token_end(head_token(node))` — the `do`-anchored arm — with
    /// `body_offset` resolved over the block's first statement. The
    /// report anchor is the body's begin token (official
    /// `reportAtToken(node.beginToken)`), i.e. the block's `{`, not the
    /// first statement.
    #[test]
    fn control_body_do_block_body_reports() {
        let f = Fixture::parse("void f(bool c) { int x = 0; do { x += 1; } while (c); }");
        let lints = f.run(&one_rule("always_put_control_body_on_new_line"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "{", 1))
        );
    }

    // —— avoid_catches_without_on_clauses ———————————————————————————————

    /// A bare `catch` clause with no valid use reports at the catch
    /// keyword.
    #[test]
    fn catch_without_on_reports_a_bare_catch() {
        let source = "void f() { try {} catch (e) {} }";
        let f = Fixture::parse(source);
        let lints = f.run(&one_rule("avoid_catches_without_on_clauses"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Catch clause should use 'on' to specify the type of exception being caught."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try adding an 'on' clause before the 'catch'.")
        );
        let kw = tok(&f.source, &f.tokens, "catch", 0);
        assert_eq!(lints[0].range.start, utf16_at(&f, kw));
    }

    /// `on`-clause forms skip — with or without a catch clause.
    #[test]
    fn catch_without_on_on_forms_pass() {
        let f = Fixture::parse(
            "void f() { try {} on int catch (e) {} }\nvoid g() { try {} on int {} }",
        );
        assert!(
            f.run(&one_rule("avoid_catches_without_on_clauses"))
                .is_empty()
        );
    }

    /// `rethrow;` and `throw e;` bodies are valid uses (:96-117).
    #[test]
    fn catch_without_on_valid_uses_pass() {
        let f = Fixture::parse(
            "void f() { try {} catch (e) { rethrow; } }\n\
             void g() { try {} catch (e) { throw e; } }",
        );
        assert!(
            f.run(&one_rule("avoid_catches_without_on_clauses"))
                .is_empty()
        );
    }

    /// Any other body reports; the two-parameter catch form reports too.
    /// (The :96-101 nested-catch rethrow fence is implemented but untested
    /// here: the production parser currently mis-nests a `try` inside a
    /// `catch` body — the builder rejects the row pairing — a parser-side
    /// defect tracked outside this rule.)
    #[test]
    fn catch_without_on_other_bodies_report() {
        let f = Fixture::parse(
            "void f() { try {} catch (e) { print(e); } }\nvoid g() { try {} catch (e, s) {} }",
        );
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            2
        );
    }

    /// A shadowing local inside a closure body re-owns the name: the throw
    /// reads the closure's own local, not the caught exception (the element
    /// identity), so the clause reports.
    #[test]
    fn catch_without_on_shadowed_closure_throw_reports() {
        let f = Fixture::parse("void f() { try {} catch (e) { g(() { var e = 1; throw e; }); } }");
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            1
        );
    }

    /// A closure parameter of the same lexeme captures the same way.
    #[test]
    fn catch_without_on_shadowed_closure_parameter_reports() {
        let f = Fixture::parse("void f() { try {} catch (e) { xs.forEach((e) { throw e; }); } }");
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            1
        );
    }

    /// A nested block's local shadows within its block only; the throw
    /// inside it reads the local and the clause reports.
    #[test]
    fn catch_without_on_shadowed_nested_block_throw_reports() {
        let f = Fixture::parse("void f() { try {} catch (e) { { var e = 1; throw e; } } }");
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            1
        );
    }

    /// A loop variable of the same lexeme shadows across the loop body.
    #[test]
    fn catch_without_on_shadowed_loop_variable_reports() {
        let f = Fixture::parse("void f() { try {} catch (e) { for (var e in xs) { throw e; } } }");
        assert_eq!(
            f.run(&one_rule("avoid_catches_without_on_clauses")).len(),
            1
        );
    }

    /// The capture walk must not over-capture: a throw through an
    /// unshadowing closure is still a valid use (the official's throw face
    /// reads through closures), and a same-block local of a DIFFERENT name
    /// leaves the exception's reads untouched.
    #[test]
    fn catch_without_on_throw_through_closure_still_passes() {
        let f = Fixture::parse(
            "void f() { try {} catch (e) { g(() { throw e; }); } }\n\
             void h() { try {} catch (e) { var t = 1; throw e; } }",
        );
        assert!(
            f.run(&one_rule("avoid_catches_without_on_clauses"))
                .is_empty()
        );
    }

    /// The object-pattern field face of the raw-type exemption — the
    /// official `test_constType_matchType_nested`: a constant nested in an
    /// object pattern against a `Type`-written field stays silent (if-case
    /// and switch-case containers); a non-`Type` field keeps the report.
    #[test]
    fn type_literal_object_pattern_type_field_passes() {
        let type_field = |src: &str| {
            let f = Fixture::parse(src);
            assert!(
                f.run(&one_rule("type_literal_in_constant_pattern"))
                    .is_empty(),
                "the matched value is the Type-typed field: {src}"
            );
        };
        type_field(
            r"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
        );
        type_field(
            r"
class A {
  final Type type;
  A(this.type);
}

void f(A x) {
  switch (x) {
    case A(type: int):
      break;
  }
}
",
        );
        // Nested object patterns: the innermost container decides — B's
        // `Type` field exempts, the outer field's own type is irrelevant.
        type_field(
            r"
class A {
  final B b;
  A(this.b);
}

class B {
  final Type type;
  B(this.type);
}

void f(A x) {
  if (x case A(b: B(type: int))) {}
}
",
        );

        // A non-`Type` field keeps the name reading.
        let f = Fixture::parse(
            r"
class A {
  final int type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "an int-typed field is no exemption"
        );
        // An untyped field keeps the name reading too.
        let f = Fixture::parse(
            r"
class A {
  final type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "an untyped field is no exemption"
        );
        // The nullable `Type?` is not a subtype of `Type` (the official
        // `isSubtypeOf` gate) — the nested constant reports.
        let f = Fixture::parse(
            r"
class A {
  final Type? type;
  A(this.type);
}

void f(A x) {
  if (x case A(type: int)) {}
}
",
        );
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "a Type?-typed field is no exemption"
        );
    }

    /// The matched-value exemption's nullability face on the scrutinee:
    /// `Type x` exempts, `Type? x` reports (the official
    /// `isSubtypeOf(matchedValueType, typeType)` — `Type?` is no subtype
    /// of `Type`; `test_constType_matchObjectNullable` shape).
    #[test]
    fn type_literal_nullable_type_scrutinee_reports() {
        let f = Fixture::parse("void f(Object x) { if (x case int) {} }");
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1
        );
        let f = Fixture::parse("void f(Type? x) { if (x case int) {} }");
        assert_eq!(
            f.run(&one_rule("type_literal_in_constant_pattern")).len(),
            1,
            "a Type? matched value is not of type Type"
        );
        let f = Fixture::parse("void f(Type x) { if (x case int) {} }");
        assert!(
            f.run(&one_rule("type_literal_in_constant_pattern"))
                .is_empty()
        );
    }

    // —— require_trailing_commas —————————————————————————————————————————

    const MULTILINE_ARGS: &str = "void f() {\n  g(\n    1,\n    2\n  );\n}\n";

    /// `test/rules/require_trailing_commas.dart` `FunctionDeclaration`
    /// multiline shape: a multi-line argument list without a trailing
    /// comma reports at the closing paren.
    #[test]
    fn multiline_argument_list_reports() {
        let f = Fixture::parse(MULTILINE_ARGS);
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(lints[0].message, "Missing a required trailing comma.");
        // The `)` closing g's argument list (the `f()` param list is empty
        // and skips).
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, ")", 1))
        );
    }

    #[test]
    fn trailing_comma_argument_list_passes() {
        let f = Fixture::parse("void f() {\n  g(\n    1,\n    2,\n  );\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// A group whose delimiters share a line needs no comma (:132-136).
    #[test]
    fn single_line_argument_list_passes() {
        let f = Fixture::parse("void f() {\n  g(1, 2);\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// The registration gate: the rule registers below the 3.7 tall-style
    /// milestone and stays silent from 3.7 (:52-56); an unset version face
    /// applies no gate (the official null-version posture registers).
    #[test]
    fn version_gate_silences_the_rule_from_3_7() {
        let mut f = Fixture::parse(MULTILINE_ARGS);
        f.version = Some(LanguageVersion::new(3, 6));
        assert_eq!(
            f.run(&one_rule("require_trailing_commas")).len(),
            1,
            "below the milestone the rule registers and reports"
        );
        let mut f = Fixture::parse(MULTILINE_ARGS);
        f.version = Some(LanguageVersion::new(3, 7));
        assert!(
            f.run(&one_rule("require_trailing_commas")).is_empty(),
            "from 3.7 the formatter enforces the commas; the rule is silent"
        );
        let f = Fixture::parse(MULTILINE_ARGS);
        assert_eq!(f.run(&one_rule("require_trailing_commas")).len(), 1);
    }

    #[test]
    fn multiline_formal_parameters_report() {
        let f = Fixture::parse("void f(\n  int a,\n  int b\n) {}\n");
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, ")", 0))
        );
    }

    /// The report pins the optional group's closer (:100-115,
    /// `rightDelimiter ?? rightParenthesis`).
    #[test]
    fn named_group_reports_at_its_delimiter() {
        let f = Fixture::parse("void f(\n  int a, {\n  int b = 1\n}) {}\n");
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "}", 0))
        );
    }

    #[test]
    fn formal_parameters_trailing_comma_pass() {
        let f = Fixture::parse("void f(\n  int a,\n  int b,\n) {}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// A block-bodied function literal as the multiline last argument is
    /// exempt (:141-143).
    #[test]
    fn function_literal_last_argument_passes() {
        let f = Fixture::parse("void f() {\n  g(\n    1,\n    () {\n      h();\n    }\n  );\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// A multiline string literal as the last argument is exempt
    /// (:145-147); a single-line one is not (:137-139).
    #[test]
    fn string_literal_last_argument_exception_follows_multiline() {
        let f = Fixture::parse("void f() {\n  g(\n    1,\n    '''\nhi\n'''\n  );\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
        let f = Fixture::parse("void f() {\n  g(\n    1,\n    'two'\n  );\n}\n");
        assert_eq!(f.run(&one_rule("require_trailing_commas")).len(), 1);
    }

    /// A collection literal as the multiline last argument is exempt
    /// (:159-160).
    #[test]
    fn collection_last_argument_passes() {
        let f = Fixture::parse("void f() {\n  g(\n    1,\n    [\n      2,\n    ]\n  );\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    #[test]
    fn multiline_list_literal_reports() {
        let f = Fixture::parse("void f() {\n  var l = [\n    1,\n    2\n  ];\n}\n");
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, "]", 0))
        );
    }

    #[test]
    fn multiline_assert_reports_and_trailing_comma_passes() {
        let f = Fixture::parse("void f(bool a) {\n  assert(\n    a,\n    'msg'\n  );\n}\n");
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, ")", 1))
        );
        let f = Fixture::parse("void f(bool a) {\n  assert(\n    a,\n    'msg',\n  );\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
        let f = Fixture::parse("void f(bool a) {\n  assert(a, 'msg');\n}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// The extension-type primary constructor's parameter list skips
    /// (:89-96).
    #[test]
    fn extension_type_parameters_skip() {
        let f = Fixture::parse("extension type E(\n  int i\n) {}\n");
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// An anonymous function call as the multiline last argument is exempt
    /// when its own argument parens share a line (:149-157).
    #[test]
    fn anonymous_function_call_last_argument_passes() {
        let f = Fixture::parse(
            "void f() {\n  assert(\n    () {\n      return true;\n    }()\n  );\n}\n",
        );
        assert!(f.run(&one_rule("require_trailing_commas")).is_empty());
    }

    /// Pin: a multiline anonymous call whose own argument parens span
    /// lines fails the `anonymous_call_on_one_line` exemption — the
    /// `token_bytes` lookups over `args_span.start`/`args_span.end - 1`
    /// resolve to different lines, so the enclosing group still reports.
    /// Official visits both argument lists: the inner `}(1, 2)` list also
    /// reports, since its last argument fits on one line and grants no
    /// exception.
    #[test]
    fn anonymous_function_call_multiline_own_args_reports() {
        let f = Fixture::parse(
            "void f() {\n  g(\n    () {\n      h();\n    }(\n      1,\n      2\n    )\n  );\n}\n",
        );
        let lints = f.run(&one_rule("require_trailing_commas"));
        assert_eq!(lints.len(), 2, "{lints:?}");
        // The enclosing `g(...)`'s `)`, then the inner invocation's `)`.
        assert_eq!(
            lints[0].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, ")", 4))
        );
        assert_eq!(
            lints[1].range.start,
            utf16_at(&f, tok(&f.source, &f.tokens, ")", 3))
        );
    }
}

/// The Dart-3.0 migration family: legacy `case` heads whose expression
/// changes meaning (or errors) under patterns.
pub(crate) mod invalid_case_patterns {
    use core::ops::Range;

    use dartq_ast::NodeKind;

    use super::{Check, Group, RuleSpec};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/invalid_case_patterns.dart` — a legacy
    /// `case` head whose expression is not a valid 3.0 constant-pattern
    /// spelling reports at the expression (:50-88): a non-`const`
    /// list/set-or-map literal, a `dart:core` `identical(...)` call, a
    /// unary prefix whose operand is not an integer literal, any binary
    /// or conditional expression, a `dart:core` `length` property access,
    /// an `is` check, or the bare `_` identifier.
    ///
    /// Gate: the official registration bails WITH the feature (:41 — the
    /// lint exists for pre-3.0 libraries only); the same
    /// [`crate::engine::Features::patterns`] bit keys the parser's
    /// case-head branch, so with patterns on the operands are patterns
    /// and the rule is silent.
    ///
    /// Faces: the legacy case operand sits in a [`NodeKind::CaseExpression`]
    /// wrapper whose window's last row is the expression root (the fixed
    /// parse surface wraps every operand; the parenthesized wrappers peel
    /// the same way). The dart:core membership of `identical`/`length` is
    /// name-level (a user `identical`/`length` over-reports). The
    /// implicit-const constructor-call face (`C()` where the constructor
    /// is const, :78-81) needs constructor resolution and under-reports.
    /// Message: `diagnostic.g.dart` `invalidCasePatterns`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "invalid_case_patterns",
        group: Group::Standalone,
        problem: "This expression is not valid in a 'case' clause in Dart 3.0.",
        correction: "Try refactoring the expression to be valid in 3.0.",
        kinds: &[NodeKind::SwitchCase],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // The registration bail (:41).
        if c.unit.features.patterns {
            return;
        }
        // Expressions precede the statements (`SwitchCaseEnd`
        // layout: word 2 counts them; the Label prefix reads off).
        let count = c.payload(c.node()).get(2).copied().unwrap_or(0) as usize;
        if count == 0 {
            return;
        }
        let kids = c.collect_direct(c.node());
        let wrappers: Vec<u32> = kids
            .iter()
            .copied()
            .filter(|&kid| c.kind(kid) != NodeKind::Label)
            .take(count)
            .collect();
        for wrapper in wrappers {
            // The run bound is the case clause's own extent: the adopted
            // window anchors at earlier statements' tail tokens on the
            // recovery shapes, and their rows must not join the head.
            let Some((operand, run)) = operand_root(c, wrapper, c.byte_span(c.node())) else {
                continue;
            };
            if is_invalid(c, &run, operand) {
                // The unary-recovery face reports the whole head run (the
                // `-o` operator is a bare recovery marker, so the operand
                // row carries only the operand); every other face reports
                // the operand row (the official `reportAtNode(expression)`).
                let range = if unary_recovery(c, &run) {
                    let mut span = c.utf16_span(run[0]);
                    for &row in &run[1..] {
                        let other = c.utf16_span(row);
                        span.start = span.start.min(other.start);
                        span.end = span.end.max(other.end);
                    }
                    span
                } else {
                    c.utf16_span(operand)
                };
                c.report(range, &[]);
            }
        }
    }

    /// The operand root and the row run that holds it: the wrapper
    /// window's rows within the wrapper's own extent (earlier statements'
    /// rows leak into the adopted window), peeling the parenthesized
    /// wrappers and the constant-pattern wrap the patterns parse adds
    /// (`unParenthesized`, :51). A wrap whose window is empty seals
    /// childless — its rows precede it in the run — so the peel pops the
    /// wrap row in place.
    fn operand_root(c: &Check<'_>, wrapper: u32, extent: Range<usize>) -> Option<(u32, Vec<u32>)> {
        let mut run: Vec<u32> = c
            .children(wrapper)
            .filter(|&row| {
                let span = c.byte_span(row);
                span.start >= extent.start && span.end <= extent.end
            })
            .collect();
        loop {
            let Some(&last) = run.last() else {
                return None;
            };
            match c.kind(last) {
                NodeKind::ParenthesizedExpression
                | NodeKind::ParenthesizedPattern
                | NodeKind::ConstantPattern => {
                    if c.kind(last) == NodeKind::ConstantPattern
                        && c.payload(last)
                            .first()
                            .is_some_and(|&token| token != super::NONE_TOKEN)
                    {
                        // The explicit `const` operand: the official
                        // const-literal face (:52-59, :78-81 —
                        // `constKeyword != null` skips).
                        return None;
                    }
                    run.pop();
                }
                _ => return Some((last, run)),
            }
        }
    }

    /// Whether the peeled run opens with a unary-prefix recovery (the
    /// `-`/`!` operator tiles as a leading [`NodeKind::UnaryPrefix`]
    /// marker or a bare [`NodeKind::RecoverableError`]).
    fn unary_recovery(c: &Check<'_>, run: &[u32]) -> bool {
        run.len() > 1
            && matches!(
                c.kind(run[0]),
                NodeKind::UnaryPrefix | NodeKind::RecoverableError
            )
    }

    /// The official visitor's expression faces (:52-87). The patterns
    /// parse spells the legacy heads under other names: the collection
    /// literals become list/map patterns, `&&`/`||` become a binary
    /// pattern, an `is` check recovers as an `IsExpression`, the `-`/`!`
    /// prefix recovers into a leading marker, `_` becomes the wildcard
    /// pattern, and `identical(...)` becomes an object pattern whose type
    /// names the callee. A `ListLiteral`/`SetOrMapLiteral` row is the
    /// `const`-expression form (`const [1, 2]`) — the official
    /// const-literal face, never invalid.
    fn is_invalid(c: &Check<'_>, run: &[u32], operand: u32) -> bool {
        // The unary prefix face (:64-67 — `-1` is valid, `-o`/`!b` are
        // not): the recovered operator marker leads the run and only a
        // single integer-literal operand is exempt.
        if unary_recovery(c, run) {
            return !(run.len() == 2 && c.kind(run[1]) == NodeKind::LiteralInt);
        }
        match c.kind(operand) {
            // A non-`const` collection literal (:52-59), any binary or
            // conditional expression, any `is` check (:68-77).
            NodeKind::ListPattern
            | NodeKind::MapPattern
            | NodeKind::BinaryExpression
            | NodeKind::BinaryPattern
            | NodeKind::Conditional
            | NodeKind::IsExpression
            | NodeKind::CastPattern => true,
            // `identical(...)` (:60-63) — the name-level dart:core face
            // over the object-pattern spelling of the call.
            NodeKind::ObjectPattern => c
                .payload(operand)
                .first()
                .is_some_and(|&head| c.lexeme(head) == "identical"),
            // `.length` (:72-75) — the name-level dart:core face.
            NodeKind::DotAccess => c.lexeme(c.head_token(operand)) == "length",
            // The bare `_` identifier (:82-87) — the wildcard spelling
            // under the patterns parse.
            NodeKind::Identifier => c.lexeme(c.head_token(operand)) == "_",
            NodeKind::WildcardPattern => true,
            // The pattern grammar rejects the head's tail outright (the
            // `true ? 1 : 2` ternary recovers into a `?`-marker and error
            // rows) — under the official 2.19 parse the same head is an
            // expression face this rule reports (:50-88).
            NodeKind::RecoverableError => true,
            _ => false,
        }
    }

    #[cfg(test)]
    mod tests {
        use rustc_hash::FxHashSet;

        use crate::engine::{Features, Unit};
        use crate::testsupport::one_rule;

        /// The legacy parse posture: the bridge parses with the patterns
        /// bit off, so the `case` heads are plain expressions (the same
        /// face the official `LanguageVersion219` corpus runs under).
        fn legacy(source: &str) -> Vec<crate::engine::Lint> {
            let features = Features {
                patterns: false,
                ..Features::default()
            };
            let parsed = crate::parse_unit(source, features);
            let unit = Unit {
                source: &parsed.source,
                tokens: &parsed.tokens,
                ast: &parsed.ast,
                lines: &parsed.lines,
                path: Some(std::path::Path::new("lint_fixture.dart")),
                features: parsed.features,
                version: None,
                workspace: None,
                reachability: None,
                async_context_uses: None,
                library_type_uses: None,
            };
            crate::engine::run(
                &unit,
                &one_rule("invalid_case_patterns"),
                &FxHashSet::default(),
            )
        }

        /// `test_binaryExpression_plus` — the message is verbatim.
        #[test]
        fn binary_expression_reports() {
            let lints = legacy("void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n");
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "This expression is not valid in a 'case' clause in Dart 3.0."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try refactoring the expression to be valid in 3.0.")
            );
        }

        /// `test_binaryExpression_logicalAnd` / `test_conditionalExpression`
        /// / `test_isExpression`.
        #[test]
        fn binary_conditional_and_is_faces_report() {
            assert_eq!(
                legacy("f(bool b) {\n  switch (b) {\n    case true && false:\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case true ? 1 : 2:\n  }\n}\n")
                    .len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case 1 is int:\n  }\n}\n").len(),
                1
            );
        }

        /// `test_listLiteral` / `test_mapLiteral` / `test_setLiteral` and
        /// their `const`/type-argument spellings.
        #[test]
        fn collection_literals_report_unless_const() {
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case [1, 2]:\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case const [1, 2]:\n  }\n}\n")
                    .len(),
                0
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case {'k': 'v'}:\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case {1}:\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case <int>[1, 2]:\n  }\n}\n").len(),
                1
            );
        }

        /// `test_mapLiteral_parenthesized` / `..._twice` — the peel walks
        /// every wrapper and the report stays inside the parens.
        #[test]
        fn parenthesized_form_reports_inside_the_parens() {
            let source = "void f(Object o) {\n  switch (o) {\n   case (({'k': 'v'})):\n  }\n}\n";
            let parsed = crate::parse_unit(
                source,
                Features {
                    patterns: false,
                    ..Features::default()
                },
            );
            let unit = Unit {
                source: &parsed.source,
                tokens: &parsed.tokens,
                ast: &parsed.ast,
                lines: &parsed.lines,
                path: Some(std::path::Path::new("lint_fixture.dart")),
                features: parsed.features,
                version: None,
                workspace: None,
                reachability: None,
                async_context_uses: None,
                library_type_uses: None,
            };
            let lints = crate::engine::run(
                &unit,
                &one_rule("invalid_case_patterns"),
                &FxHashSet::default(),
            );
            assert_eq!(lints.len(), 1, "{lints:?}");
            // The third `{` token: the first opens the function body, the
            // second the switch block, the third the case's map literal.
            let brace = parsed
                .tokens
                .as_slice()
                .iter()
                .enumerate()
                .filter(|(_, token)| parsed.source[token.byte_range().clone()] == *"{")
                .nth(2)
                .map(|(index, _)| index)
                .expect("the literal brace");
            assert_eq!(
                lints[0].range.start,
                parsed.tokens.as_slice()[brace].utf16_offset
            );
        }

        /// `test_prefixedExpression_intLiteral_ok` / `test_unaryOperator_minus`
        /// / `test_unaryOperator_not` — only the non-integer operand
        /// reports.
        #[test]
        fn unary_prefix_follows_the_integer_literal_exemption() {
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case -1:\n  }\n}\n").len(),
                0
            );
            assert_eq!(
                legacy("void f() {\n  const o = 1;\n  switch (1) {\n    case -o:\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                legacy("void f() {\n  const b = false;\n  switch (true) {\n    case !b:\n  }\n}\n")
                    .len(),
                1
            );
        }

        /// `test_identicalCall` / `test_lengthCall`.
        #[test]
        fn identical_and_length_faces_report() {
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case identical(1, 2):\n  }\n}\n")
                    .len(),
                1
            );
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case ''.length:\n  }\n}\n").len(),
                1
            );
        }

        /// `test_wildcard` — the bare `_` identifier.
        #[test]
        fn bare_wildcard_reports() {
            assert_eq!(
                legacy("f(int n) {\n  const _ = 3;\n  switch (n) {\n    case _:\n  }\n}\n").len(),
                1
            );
        }

        /// `test_parenthesizedExpression_ok` — `(1)` stays silent.
        #[test]
        fn parenthesized_integer_is_silent() {
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case (1):\n  }\n}\n").len(),
                0
            );
        }

        /// `test_constConstructorCall` — the implicit-const constructor
        /// face is the documented under-report (constructor resolution).
        #[test]
        fn constructor_call_is_the_documented_under_report() {
            assert_eq!(
                legacy("class C {\n  const C();\n}\n\nf(C c) {\n  switch (c) {\n    case C():\n  }\n}\n").len(),
                0
            );
        }

        /// The gate (:41): with the feature on the rule is silent.
        #[test]
        fn patterns_feature_on_is_silent() {
            let f = crate::testsupport::Fixture::parse(
                "void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n",
            );
            assert!(f.run(&one_rule("invalid_case_patterns")).is_empty());
        }

        /// A `const _ = 3;` shadow and plain constants never report.
        #[test]
        fn plain_constant_cases_are_silent() {
            assert_eq!(
                legacy("void f(Object o) {\n  switch (o) {\n    case 1:\n    case 'x':\n    case true:\n  }\n}\n").len(),
                0
            );
        }
    }
}

/// The pattern-shape family: a variable pattern whose explicit field name
/// spells the variable twice.
pub(crate) mod simplify_variable_pattern {
    use dartq_ast::NodeKind;

    use super::{Check, Group, RuleSpec};
    use crate::rules::{MemberQuery, NONE_TOKEN, utf16_token_span};

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/simplify_variable_pattern.dart` — a
    /// pattern field with an explicit name whose variable pattern
    /// declares the same name reports at the field name with the member
    /// kind spelled into the message (`isEven: var isEven` → "The getter
    /// identification 'isEven:' is redundant...", :41-80, :88-118).
    ///
    /// Gate: the official registration bails without the patterns feature
    /// (:32) — the same [`crate::engine::Features::patterns`] bit the
    /// parser keys.
    ///
    /// Faces: the field name is the identifier anchored immediately
    /// before the colon payload token (the named-argument anchor face);
    /// the sub-pattern root peels the parenthesized wrappers (whose
    /// content rows leak into the field's window) and must be a declared
    /// variable pattern (:42-45). The member kind resolves over the
    /// containing pattern: a same-file class chain answers method, getter,
    /// or field (:82-86); a same-file class WITHOUT the member stays
    /// silent (the official `lookUpGetter`/`lookUpMethod` null bail,
    /// :99-104); every unresolvable type (dynamic, typedef, type
    /// parameter, an import) answers getter, matching the official
    /// dynamic/typedef/type-parameter corpus.
    ///
    /// Approximation (documented): the official reports only when the
    /// member exists (the guard is resolution); the name-level face here
    /// reports whenever the names match, so code the analyzer already
    /// rejects (an undefined getter, a missing record field) reports
    /// where the official defers to the compile error — the lexical face
    /// is exact on valid code, where the pattern's member necessarily
    /// exists. Message: `diagnostic.g.dart` `simplifyVariablePattern`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "simplify_variable_pattern",
        group: Group::Standalone,
        problem: "The {1} identification '{0}:' is redundant and can be removed.",
        correction: "Try removing the redundant {1} identification.",
        kinds: &[NodeKind::PatternField],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // The registration bail (:32).
        if !c.unit.features.patterns {
            return;
        }
        let node = c.node();
        // `node.name == null` skips (:44). Payload word 0 carries the
        // field-name token for the named face (the parser hands the
        // `ensureIdentifier` token, patterns.rs object-pattern rest), the
        // colon itself for the name-less `(: sub)` form, and NONE_TOKEN
        // for a positional field — the name reads straight off the word.
        let Some(&name) = c.payload(node).first() else {
            return;
        };
        if name == NONE_TOKEN || c.lexeme(name) == ":" {
            return;
        }
        let lexeme = c.lexeme(name);
        let Some(pattern) = sub_pattern(c, node) else {
            return;
        };
        // The variable name equals the field name (:44-45).
        let Some(&variable) = c.payload(pattern).first() else {
            return;
        };
        if c.lexeme(variable) != lexeme {
            return;
        }
        let Some(container) = pattern_container(c, node) else {
            return;
        };
        let Some(accessor) = accessor(c, container, lexeme) else {
            return;
        };
        c.report(utf16_token_span(c, name), &[lexeme, accessor]);
    }

    /// The sub-pattern root: the rows completing inside the field's own
    /// span right before it (the production tiling seals the field
    /// childless — the name identifier sits ahead of the span, the
    /// sub-pattern pieces tile inside it), peeling the parenthesized
    /// wrappers; `DeclaredVariablePattern` only (:43). A hand-built tree
    /// keeps the rows as direct children instead.
    fn sub_pattern(c: &Check<'_>, node: u32) -> Option<u32> {
        let span = c.byte_span(node);
        let kids = c.collect_direct(node);
        let mut rows: Vec<u32> = if kids.is_empty() {
            (0..node)
                .rev()
                .take_while(|&row| c.byte_span(row).start >= span.start)
                .collect()
        } else {
            kids
        };
        loop {
            let root = *rows.iter().max_by_key(|&&row| {
                let span = c.byte_span(row);
                // The `var`-no-type sentinel spans the variable region
                // identically to its `DeclaredVariablePattern` — a
                // pattern row outranks the marker on an exact tie.
                let pattern = matches!(
                    c.kind(row),
                    NodeKind::DeclaredVariablePattern | NodeKind::ParenthesizedPattern
                );
                (span.end, core::cmp::Reverse(span.start), pattern)
            })?;
            match c.kind(root) {
                NodeKind::ParenthesizedPattern => {
                    let span = c.byte_span(root);
                    rows.retain(|&row| {
                        row != root
                            && c.byte_span(row).start >= span.start
                            && c.byte_span(row).end <= span.end
                    });
                }
                NodeKind::DeclaredVariablePattern => return Some(root),
                _ => return None,
            }
        }
    }

    /// The field's enclosing pattern: a hand-built tree answers the
    /// parent row; the production tiling seals every field before its
    /// container event, so the container is the nearest pattern row after
    /// the field whose span covers it (sibling fields seal in between,
    /// so `node + 1` alone does not suffice).
    fn pattern_container(c: &Check<'_>, node: u32) -> Option<u32> {
        if let Some(parent) = c.ancestor(1)
            && matches!(
                c.kind(parent),
                NodeKind::ObjectPattern | NodeKind::RecordPattern
            )
        {
            return Some(parent);
        }
        let span = c.byte_span(node);
        (node + 1..c.unit.ast.len() as u32).find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::ObjectPattern | NodeKind::RecordPattern
            ) && {
                let outer = c.byte_span(row);
                outer.start <= span.start && outer.end >= span.end
            }
        })
    }

    /// The member-kind word (`_accessor`, :82-86): the containing
    /// `ObjectPattern` resolves the same-file class chain (method/getter/
    /// field, silent without the member); a record pattern or any other
    /// container spells `field`.
    fn accessor(c: &Check<'_>, container: u32, name: &str) -> Option<&'static str> {
        if c.kind(container) == NodeKind::ObjectPattern {
            return object_member_kind(c, container, name);
        }
        // A record pattern (or any other container) spells `field`.
        Some("field")
    }

    /// The member face of the object pattern's type: the same-file class
    /// chain answers method/getter/field and stays silent without the
    /// member (the official lookUp bail, :99-104); an unresolvable type —
    /// dynamic, a typedef, a type parameter, an import — answers getter.
    fn object_member_kind(c: &Check<'_>, object: u32, name: &str) -> Option<&'static str> {
        let words = c.payload(object);
        let &head = words.first()?;
        // A prefixed container (`prefix.A(...)`, payload word 1) resolves
        // outside the file.
        let type_name = if words.get(1).is_some_and(|&dot| dot != NONE_TOKEN) {
            words.get(2).copied().unwrap_or(head)
        } else {
            head
        };
        let Some(class_row) = crate::rules::same_file_declaration(c, c.lexeme(type_name)) else {
            return Some("getter");
        };
        let mut seen = 0usize;
        let mut current = Some(class_row);
        while let Some(row) = current {
            seen += 1;
            if seen > 8 {
                return Some("getter");
            }
            for member in crate::rules::body_members(c, row) {
                if !crate::rules::member_matches(c, member, MemberQuery::Method, name).is_empty() {
                    return Some("method");
                }
                if !crate::rules::member_matches(c, member, MemberQuery::Getter, name).is_empty() {
                    return Some("getter");
                }
                if !crate::rules::member_matches(c, member, MemberQuery::Variable, name).is_empty()
                {
                    return Some("field");
                }
            }
            current = None;
            for (supertype, _) in crate::rules::supertypes(c, row) {
                if let Some(row) = crate::rules::same_file_declaration(c, supertype) {
                    current = Some(row);
                    break;
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("simplify_variable_pattern"))
        }

        /// `test_patternField_explicit` — the getter face, message
        /// verbatim.
        #[test]
        fn named_field_with_same_name_reports() {
            let source = "void f(Object o) {\n  if (o case int(isEven:var isEven)) {}\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("simplify_variable_pattern"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The getter identification 'isEven:' is redundant and can be removed."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try removing the redundant getter identification.")
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "isEven", 0))
            );
        }

        /// `test_noPatternField` / `test_patternField_implicit` /
        /// `test_patternField_otherName` — the name-less, positional, and
        /// mismatched forms stay silent.
        #[test]
        fn other_field_shapes_are_silent() {
            assert!(run("void f(Object o) {\n  if (o case int i) {}\n}\n").is_empty());
            assert!(run("void f(Object o) {\n  if (o case int(:var isEven)) {}\n}\n").is_empty());
            assert!(
                run("void f(Object o) {\n  if (o case int(isEven:var other)) {}\n}\n").is_empty()
            );
        }

        /// `test_patternField_inheritedMethod` — the same-file chain
        /// answers `method`.
        #[test]
        fn inherited_method_answers_the_method_word() {
            let source = "class A {\n  void m() {}\n}\nclass B extends A {}\n\nvoid f(Object o) {\n  if (o case B(m:var m)) {}\n}\n";
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The method identification 'm:' is redundant and can be removed."
            );
        }

        /// `test_patternField_inheritedGetter` — the chain answers
        /// `getter`.
        #[test]
        fn inherited_getter_answers_the_getter_word() {
            let source = "class A {\n  int get x => 0;\n}\nclass B extends A {}\n\nvoid f(Object o) {\n  if (o case B(x:var x)) {}\n}\n";
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The getter identification 'x:' is redundant and can be removed."
            );
        }

        /// `test_patternField_inheritedGetter_mixin` — the `with` chain
        /// resolves like `extends`.
        #[test]
        fn mixin_chain_resolves() {
            let source = "mixin M {\n  int get x => 0;\n}\nclass C with M {}\n\nvoid f(Object o) {\n  if (o case C(x:var x)) {}\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// A same-file class without the member stays silent (the
        /// official lookUp bail, :99-104).
        #[test]
        fn missing_member_is_silent() {
            let source = "class B {\n  int y = 0;\n}\n\nvoid f(Object o) {\n  if (o case B(m:var m)) {}\n}\n";
            assert!(run(source).is_empty());
        }

        /// `test_recordDestructuring` — the record face spells `field`.
        #[test]
        fn record_destructuring_answers_the_field_word() {
            let source =
                "void f((int, {String name}) record) {\n  var (x, name: name) = record;\n}\n";
            let lints = run(source);
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The field identification 'name:' is redundant and can be removed."
            );
        }

        /// `test_patternField_parenthesized` — the peel walks every
        /// wrapper.
        #[test]
        fn parenthesized_pattern_reports() {
            assert_eq!(
                run("void f(Object o) {\n  if (o case int(isEven:((var isEven)))) {}\n}\n").len(),
                1
            );
        }

        /// The gate (:32): with the feature off the rule is silent.
        #[test]
        fn patterns_feature_off_is_silent() {
            let mut f =
                Fixture::parse("void f(Object o) {\n  if (o case int(isEven:var isEven)) {}\n}\n");
            f.features.patterns = false;
            assert!(f.run(&one_rule("simplify_variable_pattern")).is_empty());
        }
    }
}

/// The type-scrutinee family: a switch whose scrutinee is a `Type` value.
pub(crate) mod switch_on_type {
    use dartq_ast::NodeKind;

    use super::{Check, Group, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/switch_on_type.dart` — a switch
    /// statement/expression whose scrutinee's static type is assignable
    /// to `Type` reports once at the scrutinee (:53-60, :71-121): the
    /// recursion faces are the then/else arms of a conditional, both
    /// operands of a `+` (string building), a nested switch expression's
    /// case values, and a string interpolation's segments; the leaf faces
    /// are a `.runtimeType` tail, a bare `Type`-written name, a type
    /// parameter, and a type literal — a `.toString()` call walks its
    /// target (:111-112).
    ///
    /// Gate: the official registration bails without the patterns feature
    /// (:34) — the same [`crate::engine::Features::patterns`] bit.
    ///
    /// Faces over the syntax substrate: the scrutinee is the piece run of
    /// the switch head (the adopted-begin tiling leaks the expression
    /// rows before the condition container; the container's window rows
    /// follow it). The written-type face reads same-file bindings
    /// (`Type t` reports, `String s` and inferred locals stay silent, the
    /// nullability-sensitive `isAssignableTo` posture); the
    /// implicit-`this` `runtimeType` face is name-level (a same-file
    /// override returning a non-`Type` over-reports); the type-literal
    /// face reuses the `type_literal_in_constant_pattern` channel
    /// (same-file type or an unshadowed built-in; a prefixed spelling
    /// under-reports). The string-interpolation faces (a `$t` segment)
    /// are the documented under-report — interpolation segments produce
    /// no AST rows (the token-layer carry). Message:
    /// `diagnostic.g.dart` `switchOnType`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "switch_on_type",
        group: Group::Standalone,
        problem: "Avoid switch statements on a 'Type'.",
        correction: "Try using pattern matching on a variable instead.",
        kinds: &[NodeKind::SwitchStatement, NodeKind::SwitchExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // The registration bail (:34).
        if !c.unit.features.patterns {
            return;
        }
        let Some(rows) = scrutinee_rows(c) else {
            return;
        };
        if type_scrutinee(c, &rows) {
            // One report per switch, at the whole scrutinee run
            // (`reportAtNode(node.expression)`).
            let start = c.utf16_span(rows[0]).start;
            let end = c.utf16_span(rows[rows.len() - 1]).end;
            c.report(start..end, &[]);
        }
    }

    /// The scrutinee piece run: the rows before the condition container
    /// plus the container's window rows (the statement face), or the rows
    /// before the case block (the switch-expression face). A `case`
    /// clause (`switch (x case ...)`) is not the lint's face.
    fn scrutinee_rows(c: &Check<'_>) -> Option<Vec<u32>> {
        let node = c.node();
        let kids = c.collect_direct(node);
        match c.kind(node) {
            NodeKind::SwitchStatement => {
                let position = kids
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)?;
                let container = kids[position];
                if c.payload(container)
                    .first()
                    .is_some_and(|&case| case != NONE_TOKEN)
                {
                    return None;
                }
                let mut rows: Vec<u32> = kids[..position].to_vec();
                rows.extend(c.children(container));
                Some(rows)
            }
            NodeKind::SwitchExpression => {
                let position = kids
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock)?;
                Some(kids[..position].to_vec())
            }
            _ => None,
        }
    }

    /// The official `_processExpression` walk (:71-121): the first
    /// `Type`-valued leaf reports once.
    fn type_scrutinee(c: &Check<'_>, rows: &[u32]) -> bool {
        let Some(position) = rows
            .iter()
            .rposition(|&row| c.kind(row) == NodeKind::SwitchExpression)
        else {
            return walk(c, rows);
        };
        // A nested switch expression: its case values walk (:86-93). The
        // value rows follow the arrow payload token.
        let block = c
            .collect_direct(rows[position])
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::SwitchExpressionBlock);
        let Some(block) = block else {
            return false;
        };
        for case in c.collect_direct(block) {
            if c.kind(case) != NodeKind::SwitchExpressionCase {
                continue;
            }
            let Some(&arrow) = c.payload(case).get(1) else {
                continue;
            };
            if arrow == NONE_TOKEN {
                continue;
            }
            let arrow_byte = token_byte(c, arrow);
            let value_rows: Vec<u32> = c
                .children(case)
                .filter(|&row| c.byte_span(row).start > arrow_byte)
                .collect();
            if type_scrutinee(c, &value_rows) {
                return true;
            }
        }
        false
    }

    fn walk(c: &Check<'_>, rows: &[u32]) -> bool {
        // The conditional face: the arms decide, the condition is ignored
        // (:78-84).
        if let Some(position) = rows
            .iter()
            .rposition(|&row| c.kind(row) == NodeKind::Conditional)
        {
            let arms: Vec<u32> = c.children(rows[position]).collect();
            return walk(c, &arms);
        }
        // The `+` concatenation face: both operand runs walk (:95-104).
        if let Some(position) = rows.iter().rposition(|&row| {
            c.kind(row) == NodeKind::BinaryExpression
                && c.lexeme(c.payload(row).first().copied().unwrap_or(NONE_TOKEN)) == "+"
        }) {
            let operator = c
                .payload(rows[position])
                .first()
                .copied()
                .unwrap_or(NONE_TOKEN);
            let operator_byte = token_byte(c, operator);
            let split = rows.partition_point(|&row| c.byte_span(row).end <= operator_byte);
            // The container row spans the operator, so it falls in neither
            // operand side: the right walk takes only the rows beginning
            // after the operator byte. Each half is therefore strictly
            // smaller than the walked run (the triggering row is in the
            // run but in neither half) — structural progress, no re-walk
            // of the same slice.
            let right: Vec<u32> = rows[split..]
                .iter()
                .copied()
                .filter(|&row| c.byte_span(row).start > operator_byte)
                .collect();
            return walk(c, &rows[..split]) || walk(c, &right);
        }
        // The invocation face: a `.toString()` selector walks its target
        // (:111-112); a bare name is the identifier face itself. A
        // `NoArguments` send is a tear-off, not the official
        // `MethodInvocation` face — the outermost selector's property
        // type decides (the static-type read of :97-107), i.e. the
        // access-chain face below (`n.runtimeType.toString` is silent,
        // `test_runtimeTypeToString_noCall`).
        if let Some(position) = rows.iter().rposition(|&row| c.kind(row) == NodeKind::Send) {
            let Some(mut head) = position.checked_sub(1) else {
                return false;
            };
            match c.kind(rows[head]) {
                NodeKind::Arguments => head -= 1,
                NodeKind::NoArguments => return access_chain_face(c, rows),
                _ => return false,
            }
            loop {
                match c.kind(rows[head]) {
                    NodeKind::TypeArguments | NodeKind::NoTypeArguments => head -= 1,
                    NodeKind::Identifier | NodeKind::DotAccess => break,
                    _ => return false,
                }
            }
            let lexeme = c.lexeme(c.head_token(rows[head]));
            if lexeme == "toString" {
                return head > 0 && walk(c, &rows[..head]);
            }
            // A bare invocation has no real target (:111 — `realTarget?`).
            if c.kind(rows[head]) != NodeKind::Identifier {
                return false;
            }
            return name_is_type(c, rows[head], lexeme);
        }
        // The access-chain face: only the outermost selector names the
        // expression's type (`n.runtimeType` reports,
        // `n.runtimeType.toString` does not).
        access_chain_face(c, rows)
    }

    /// The static-type read's access-chain posture (:97-107): the
    /// outermost selector names the expression's type.
    fn access_chain_face(c: &Check<'_>, rows: &[u32]) -> bool {
        if let Some(&tail) = rows
            .iter()
            .rev()
            .find(|&row| c.kind(*row) == NodeKind::DotAccess)
        {
            return c.lexeme(c.head_token(tail)) == "runtimeType";
        }
        // The bare-name face: the run's root identifier.
        if let Some(&name) = rows
            .iter()
            .rev()
            .find(|&row| c.kind(*row) == NodeKind::Identifier)
        {
            return name_is_type(c, name, c.lexeme(c.head_token(name)));
        }
        false
    }

    /// The `Type`-valued name faces: a written `Type` binding first (the
    /// shadow-correct same-file walk), then the implicit-`this`
    /// `runtimeType` face, a type parameter, and the type-literal
    /// channel.
    fn name_is_type(c: &Check<'_>, use_row: u32, lexeme: &str) -> bool {
        if crate::rules::expressions::same_file::binding(c, use_row, lexeme).is_some() {
            return crate::rules::written_type_face(c, use_row, lexeme)
                .is_some_and(|face| face.name == Some("Type") && !face.nullable);
        }
        // `runtimeType` on implicit `this` (:94-99 — the corpus
        // `runtimeType`/`this.runtimeType` faces); a same-file override
        // returning a non-`Type` over-reports.
        if lexeme == "runtimeType" {
            return true;
        }
        if is_enclosing_type_parameter(c, lexeme) {
            return true;
        }
        crate::rules::expressions::same_file::declares_type(c, lexeme)
            || super::type_literal_in_constant_pattern::is_builtin_type_name(lexeme)
    }

    /// Whether the name is a type parameter of an enclosing declaration
    /// (`switch (T)`, :336-343 — the type-literal face).
    fn is_enclosing_type_parameter(c: &Check<'_>, name: &str) -> bool {
        for &ancestor in c.path.iter().rev().skip(1) {
            let variables = c
                .collect_direct(ancestor)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
            let Some(variables) = variables else {
                continue;
            };
            for variable in c.collect_direct(variables) {
                if c.kind(variable) == NodeKind::TypeVariable
                    && c.lexeme(c.head_token(variable)) == name
                {
                    return true;
                }
            }
        }
        false
    }

    fn token_byte(c: &Check<'_>, token: u32) -> usize {
        c.unit
            .tokens
            .get(token as usize)
            .map_or(usize::MAX, |token| token.byte_offset as usize)
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("switch_on_type"))
        }

        /// `test_variableType` — a `Type`-written parameter reports.
        #[test]
        fn type_written_parameter_reports() {
            let source = "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("switch_on_type"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "Avoid switch statements on a 'Type'.");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try using pattern matching on a variable instead.")
            );
            assert_eq!(
                // The report pins the scrutinee expression
                // (`reportAtNode(node.expression)`) — the `t` inside the
                // switch head, not the parameter spelling.
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "t", 1))
            );
        }

        /// `test_runtimeType` / `test_insideClass_implicitThis` /
        /// `test_insideClass_withThis` — the `.runtimeType` faces.
        #[test]
        fn runtime_type_faces_report() {
            assert_eq!(
                run("void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                run("class A {\n  void m() {\n    switch (runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                run("class A {\n  void m() {\n    switch (this.runtimeType) {\n      case const (A):\n        break;\n      default:\n        break;\n    }\n  }\n}\n").len(),
                1
            );
        }

        /// `test_runtimeTypeToString` / `test_stringAddition` — the
        /// `.toString()` walk.
        #[test]
        fn to_string_calls_walk_their_target() {
            assert_eq!(
                run("void f(num n) {\n  switch (n.runtimeType.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
                1
            );
            assert_eq!(
                run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n").len(),
                1
            );
        }

        /// The `+` concatenation face walks both operand runs and
        /// terminates: the container row spans the operator and belongs to
        /// neither operand side (regression pin — the walked run used to
        /// re-enter its own slice and overflow the stack).
        #[test]
        fn plus_operand_runs_walk_and_terminate() {
            // The left operand is the `Type` leaf.
            assert_eq!(
                run("void f(Type t) {\n  switch (t.toString() + '') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                    .len(),
                1
            );
            // The right operand is the `Type` leaf (the crash shape: the
            // post-operator run held the container row).
            assert_eq!(
                run("void f(Type t) {\n  switch ('type: ' + t.toString()) {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                    .len(),
                1
            );
            // Neither operand is `Type`-valued — silent.
            assert!(
                run("void f(num n) {\n  switch (n.toString() + '') {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                    .is_empty()
            );
        }

        /// `test_runtimeTypeToString_noCall` — the tear-off's outermost
        /// selector is `toString`, not `Type`.
        #[test]
        fn tear_off_access_chain_is_silent() {
            assert!(
                run("void f(num n) {\n  switch (n.runtimeType.toString) {\n    case function:\n      break;\n    default:\n      break;\n  }\n}\n\nvoid function() {}\n")
                    .is_empty()
            );
        }

        /// `test_other` / `test_dynamic` / `test_variable_typeToString` —
        /// non-`Type` scrutinees stay silent.
        #[test]
        fn non_type_scrutinees_are_silent() {
            assert!(
                run("void f(num i) {\n  switch (i) {\n    case int _:\n      break;\n    case double _:\n      break;\n  }\n}\n")
                    .is_empty()
            );
            assert!(run("void foo(dynamic a) {\n  switch (a) {}\n}\n").is_empty());
            assert!(
                run("void f(Object? o) {\n  final type = o.runtimeType.toString();\n  switch (type) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n")
                    .is_empty()
            );
        }

        /// `test_functionToString` / `test_functionToString_prefixed` —
        /// bare and prefixed `toString` invocations have no `Type`
        /// target.
        #[test]
        fn bare_to_string_is_silent() {
            assert!(
                run("void f() {\n  switch (toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                    .is_empty()
            );
            assert!(
                run("import '' as self;\n\nvoid f() {\n  switch (self.toString()) {\n    case 'int':\n      break;\n    default:\n      break;\n  }\n}\n\nString toString() => '';\n")
                    .is_empty()
            );
        }

        /// `test_conditionalBoth` — the arms walk.
        #[test]
        fn conditional_arm_reports() {
            assert_eq!(
                run("void f(Type t) {\n  switch (1 == 1 ? t : 'other') {\n    case 'type: int':\n      break;\n    default:\n      break;\n  }\n}\n")
                    .len(),
                1
            );
        }

        /// `test_nestedSwitch` — the nested switch expression's case
        /// values walk.
        #[test]
        fn nested_switch_expression_reports() {
            assert_eq!(
                run("void f(Type t, Object o) {\n  switch (switch (o) { _ => t }) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                    .len(),
                1
            );
        }

        /// `test_typeParameter` — a bare type parameter is a type
        /// literal.
        #[test]
        fn type_parameter_reports() {
            assert_eq!(
                run("void f<T>() {\n  switch (T) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n")
                    .len(),
                1
            );
        }

        /// The switch-expression face reports on its scrutinee.
        #[test]
        fn switch_expression_scrutinee_reports() {
            assert_eq!(
                run("void f(Type t) {\n  (switch (t) { _ => null });\n}\n").len(),
                1
            );
        }

        /// The gate (:34): with the feature off the rule is silent.
        #[test]
        fn patterns_feature_off_is_silent() {
            let mut f = Fixture::parse(
                "void f(Type t) {\n  switch (t) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n",
            );
            f.features.patterns = false;
            assert!(f.run(&one_rule("switch_on_type")).is_empty());
        }

        /// A `String`-written name shadows the built-in reading
        /// (`switch (int)` with a local `int` binding stays consistent
        /// with the official element read).
        #[test]
        fn value_binding_shadows_the_type_reading() {
            assert!(
                run("void f() {\n  var int = 3;\n  switch (int) {\n    case 3:\n      break;\n    default:\n      break;\n  }\n}\n")
                    .is_empty()
            );
        }

        /// The report pins the scrutinee run (`[!i.runtimeType!]`).
        #[test]
        fn report_pins_the_scrutinee_run() {
            let source = "void f(num i) {\n  switch (i.runtimeType) {\n    case const (int):\n      break;\n    default:\n      break;\n  }\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("switch_on_type"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "i", 1))
            );
        }
    }
}

/// The enum-switch family: a `default` clause on a switch whose scrutinee
/// names an enum type.
pub(crate) mod no_default_cases {
    use dartq_ast::NodeKind;

    use super::{Check, Group, NONE_TOKEN, RuleSpec, last_child};
    use crate::rules::utf16_token_span;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/no_default_cases.dart` — a `default`
    /// member of a switch statement whose scrutinee's static type is an
    /// interface type whose element is an enum (or an enum-like class)
    /// reports at the member (:42-61): the first default only (the walk
    /// returns there), and a switch without a default never reports —
    /// the lint flags the clause's presence, not exhaustive completion.
    /// No feature gate: the registration consults no `isFeatureEnabled`
    /// despite the experimental state.
    ///
    /// Faces over the syntax substrate: the enum verdict keys on the
    /// scrutinee root identifier's same-file value binding (the
    /// shadow-correct innermost walk) whose written type names a
    /// same-file [`NodeKind::EnumDeclaration`] — `void f(E e) { switch
    /// (e) … }` against a same-file `enum E`, the local, parameter, and
    /// implicit-`this` field faces alike. Under-reports (type-system
    /// faces the substrate cannot see): imported enums, inferred
    /// bindings (`var e = E.a`), non-root spellings (`switch (e.value)`,
    /// `switch (E.a)`), and the official enum-like class / extension
    /// type faces (`isEnumLikeType`). A bare enum NAME scrutinee stays
    /// silent here matching the official — a type literal's static type
    /// is `Type`, not the enum. Over-reports: none — a binding's written
    /// type is its static type. Message: `diagnostic.g.dart`
    /// `noDefaultCases`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "no_default_cases",
        group: Group::Standalone,
        problem: "Invalid use of 'default' member in a switch.",
        correction: "Try enumerating all the possible values of the switch expression.",
        kinds: &[NodeKind::SwitchStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The scrutinee run over the statement tiling (the adopted-begin
        // leak rows before the condition container, then the container's
        // window; the `switch_on_type` recipe): the expression root is
        // the run's last row.
        // The scrutinee root over the statement tiling (the adopted-begin
        // leak rows before the condition container, then the container's
        // window; the `switch_on_type` recipe). The bare-identifier
        // scrutinee tiles as the send group `[Identifier, wrappers,
        // NoArguments, Send]` straddling the container, so the root is the
        // run's single non-marker row: any other shape (a member chain, a
        // call) is a non-root spelling and stays silent.
        let kids = c.collect_direct(node);
        let Some(position) = kids
            .iter()
            .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
        else {
            return;
        };
        let container = kids[position];
        // A `case` clause in the switch head is not the lint's face (the
        // container payload word 0 carries the `case` token when present).
        if c.payload(container)
            .first()
            .is_some_and(|&case| case != NONE_TOKEN)
        {
            return;
        }
        let run: Vec<u32> = kids[..position]
            .iter()
            .copied()
            .chain(c.children(container))
            .filter(|&row| !crate::rules::slice::is_send_group_piece(c, row))
            .collect();
        let [root] = run.as_slice() else {
            return;
        };
        if c.kind(*root) != NodeKind::Identifier {
            return;
        }
        let name = c.lexeme(c.head_token(*root));
        // The binding's written type names the scrutinee's static type at
        // the syntax level; untyped (`var`, dynamic) bails.
        let Some(type_name) = crate::rules::expressions::same_file::written_type(c, *root, name)
        else {
            return;
        };
        let Some(declaration) = crate::rules::same_file_declaration(c, type_name) else {
            return;
        };
        if c.kind(declaration) != NodeKind::EnumDeclaration {
            return;
        }
        // The members ride the `SwitchBlock` child; the first `default`
        // member reports (payload word 0 is the `default` keyword, NONE
        // for a `case` — the official `member is SwitchDefault` face).
        let Some(block) =
            last_child(c, node).filter(|&block| c.kind(block) == NodeKind::SwitchBlock)
        else {
            return;
        };
        let Some(default_kw) = c
            .collect_direct(block)
            .into_iter()
            .find(|&member| {
                c.kind(member) == NodeKind::SwitchCase
                    && c.payload(member)
                        .first()
                        .is_some_and(|&kw| kw != NONE_TOKEN)
            })
            .and_then(|member| c.payload(member).first().copied())
        else {
            return;
        };
        c.report(utf16_token_span(c, default_kw), &[]);
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("no_default_cases"))
        }

        /// `test_enum` — the exhaustive-enum shape: a parameter written
        /// with a same-file enum type, message and report verbatim.
        #[test]
        fn enum_parameter_default_reports() {
            let source = "void f(E e) {\n  switch(e) {\n    case E.a :\n      print('a');\n      break;\n    default:\n      print('default');\n  }\n}\n\nenum E {\n  a, b, c;\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("no_default_cases"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Invalid use of 'default' member in a switch."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try enumerating all the possible values of the switch expression.")
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "default", 0))
            );
        }

        /// The local-binding face: a local written with the enum type.
        #[test]
        fn enum_typed_local_reports() {
            let source = "enum E { a, b }\n\nvoid f() {\n  E e = E.a;\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// The implicit-`this` field face: an instance field written with
        /// the enum type, switched bare inside a method.
        #[test]
        fn enum_typed_field_reports() {
            let source = "enum Color { r, g }\n\nclass X {\n  Color c;\n  X(this.c);\n  void m() {\n    switch (c) {\n      default:\n        break;\n    }\n  }\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// `test_class_notEnumLike` — a non-enum scrutinee stays silent.
        #[test]
        fn non_enum_scrutinee_is_silent() {
            let source = "void f(int i) {\n  switch (i) {\n    case 1 :\n      print('1');\n      break;\n    default:\n      print('default');\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// A same-file CLASS-typed scrutinee stays silent (the official
        /// enum element face; the enum-like class is the documented
        /// under-report).
        #[test]
        fn class_typed_scrutinee_is_silent() {
            let source = "class C {\n  const C();\n}\n\nvoid f(C c) {\n  switch (c) {\n    default:\n      break;\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// A switch without a default never reports (the lint flags the
        /// clause's presence).
        #[test]
        fn no_default_clause_is_silent() {
            let source = "enum E { a, b }\n\nvoid f(E e) {\n  switch (e) {\n    case E.a:\n      break;\n    case E.b:\n      break;\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// An untyped binding (dynamic) stays silent.
        #[test]
        fn untyped_binding_is_silent() {
            let source = "enum E { a, b }\n\nvoid f(e) {\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// The unknown-type under-report face: no same-file enum named
        /// `E` — an imported enum's shape — stays silent.
        #[test]
        fn unknown_type_name_is_silent() {
            let source = "void f(E e) {\n  switch (e) {\n    default:\n      break;\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// A bare enum-name scrutinee stays silent — matching the
        /// official, a type literal's static type is `Type`.
        #[test]
        fn bare_enum_name_scrutinee_is_silent() {
            let source = "enum E { a, b }\n\nvoid f() {\n  switch (E) {\n    default:\n      break;\n  }\n}\n";
            assert!(run(source).is_empty());
        }
    }
}

/// The join family: an assignment statement a following `return` makes
/// redundant.
pub(crate) mod join_return_with_assignment {
    use dartq_ast::NodeKind;

    use super::{Check, Group, RuleSpec};
    use core::ops::Range;

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/join_return_with_assignment.dart` — a
    /// block whose last statement is `return e;` and whose second-to-last
    /// statement assigns the same target reports at that statement
    /// (:54-93): the assignment is an expression statement whose
    /// unParenthesized root is an assignment (`=` or compound — the
    /// LHS, :21-23) or a prefix/postfix expression (the operand, :24-28);
    /// the report fires only when the target differs from the
    /// third-to-last statement's target (:84-93 — a same-target
    /// assignment one step further back keeps the assignment, and any
    /// null side of that comparison answers false).
    ///
    /// Faces over the syntax substrate: the official element identity
    /// (`canonicalElementsFromIdentifiersAreEqual`,
    /// `dart_type_utilities.dart:96-136`) is approximated by the written
    /// spelling — both sides must be identifier chains (a simple
    /// identifier or a `.`/`?.` selector chain whose head is neither
    /// `this` nor `super`, the official `PropertyAccess` target walk dying
    /// on every non-chain target) and byte-equal. Whitespace inside a
    /// chain (`a . b` vs `a.b`) under-reports; same-file implicit-`this`
    /// fields join as bare names, matching the official `SimpleIdentifier`
    /// face. Message: `diagnostic.g.dart` `joinReturnWithAssignment`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "join_return_with_assignment",
        group: Group::Standalone,
        problem: "Assignment could be inlined in 'return' statement.",
        correction: "Try inlining the assigned value in the 'return' statement.",
        kinds: &[NodeKind::Block, NodeKind::BlockBody],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // Production bodies tile as `BlockBody` whose direct children
        // interleave each statement's leaked material rows with its
        // trailing handle; the statement sequence is the statement-row
        // filter over the child run.
        let statements: Vec<u32> = c
            .collect_direct(c.node())
            .into_iter()
            .filter(|&row| crate::rules::is_statement_row(c, row))
            .collect();
        let length = statements.len();
        if length < 2 {
            return;
        }
        // The last statement: `return e;` (the expression run is the
        // window the statement adopts; a bare `return;` adopts nothing,
        // :60-66).
        let last = statements[length - 1];
        if c.kind(last) != NodeKind::ReturnStatement {
            return;
        }
        let owner = c.node();
        let Some(last_expression) = expression_span(c, owner, last) else {
            return;
        };
        let second_last = statements[length - 2];
        // The second-to-last statement must assign (:69-75).
        let Some(second_last_expression) = assignment_target(c, owner, second_last) else {
            return;
        };
        // The third-to-last target — `None` for fewer statements or any
        // other statement shape (:77-83).
        let third_last_expression = length
            .checked_sub(3)
            .and_then(|index| assignment_target(c, owner, statements[index]));
        // `!equal(second, third) && equal(last, second)` (:84-93); any
        // null side of the official comparison answers false.
        if !equal(
            c,
            &second_last_expression.span,
            third_last_expression.as_ref().map(|t| &t.span),
        ) && equal(c, &last_expression, Some(&second_last_expression.span))
        {
            // The official node is the assignment statement's expression
            // (its material rows' combined span).
            c.report(utf16_run_span(c, &second_last_expression.rows), &[]);
        }
    }

    /// The statement's material rows: the window the handle adopted, or —
    /// the production tiling — the owner-level leak before the childless
    /// handle (everything after the previous statement row). `owner` is
    /// the dispatched block row.
    fn material(c: &Check<'_>, owner: u32, statement: u32) -> Option<Vec<u32>> {
        let window = c.collect_direct(statement);
        if !window.is_empty() {
            return Some(window);
        }
        let kids = c.collect_direct(owner);
        let position = kids.iter().position(|&kid| kid == statement)?;
        Some(crate::rules::slice::statement_run(c, &kids[..position]))
    }

    /// The statement's expression byte span: the material rows' combined
    /// extent, with matching outer parens peeled (`unParenthesized`). The
    /// send group's marker rows anchor at the statement's trailing tokens
    /// (`;`) and are excluded — only the content rows carry the
    /// expression text.
    fn expression_span(c: &Check<'_>, owner: u32, statement: u32) -> Option<Range<usize>> {
        let rows = material(c, owner, statement)?;
        expression_rows_span(c, &rows)
    }

    fn expression_rows_span(c: &Check<'_>, rows: &[u32]) -> Option<Range<usize>> {
        let content: Vec<u32> = rows
            .iter()
            .copied()
            .filter(|&row| {
                !crate::rules::slice::is_send_group_piece(c, row)
                    // The childless access markers adopt the statement's
                    // trailing tokens (`a.f;` seals its `DotAccess` over
                    // `f;`) — they carry no expression text of their own.
                    && !matches!(
                        c.kind(row),
                        NodeKind::DotAccess | NodeKind::Qualified
                    )
            })
            .collect();
        let start = content.iter().map(|&row| c.byte_span(row).start).min()?;
        let end = content.iter().map(|&row| c.byte_span(row).end).max()?;
        Some(peel_parens(c, start..end))
    }

    fn peel_parens(c: &Check<'_>, mut span: Range<usize>) -> Range<usize> {
        let source = c.unit.source.as_bytes();
        while source.get(span.start) == Some(&b'(')
            && source.get(span.end.saturating_sub(1)) == Some(&b')')
        {
            // Trim only a pair wrapping the whole span (a `()`-free
            // interior would leave the depth negative mid-span).
            let mut depth = 0i32;
            let mut matched = true;
            for &byte in &source[span.start..span.end] {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth < 0 {
                            matched = false;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if !matched || depth != 0 {
                break;
            }
            span.start += 1;
            span.end -= 1;
        }
        span
    }

    /// `_getExpressionFromAssignmentStatement` (:18-30): the statement
    /// must be an expression statement whose unParenthesized root is an
    /// assignment (the LHS, :21-23) or a prefix/postfix expression (the
    /// operand, :24-28). The target is the operand run's byte span over
    /// its rows: the assignment splits at the operator token, the
    /// prefix/postfix marker takes the run before it.
    fn assignment_target(c: &Check<'_>, owner: u32, statement: u32) -> Option<Target> {
        if c.kind(statement) != NodeKind::ExpressionStatement {
            return None;
        }
        let rows = material(c, owner, statement)?;
        let root = *rows.last()?;
        let position = rows.iter().position(|&row| row == root)?;
        let operand_rows: Vec<u32> = match c.kind(root) {
            NodeKind::Assignment => {
                // The LHS completes before the RHS (operands×2 contract);
                // the operator byte splits the run.
                let operator = c.payload(root).first().copied()?;
                let operator_byte = c
                    .unit
                    .tokens
                    .get(operator as usize)
                    .map_or(usize::MAX, |token| token.byte_offset as usize);
                let split = rows.partition_point(|&row| c.byte_span(row).end <= operator_byte);
                rows[..split].to_vec()
            }
            NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => rows[..position].to_vec(),
            _ => return None,
        };
        if operand_rows.is_empty() {
            return None;
        }
        let span = expression_rows_span(c, &operand_rows)?;
        Some(Target {
            span,
            rows: operand_rows,
        })
    }

    struct Target {
        /// The target's byte span over the source (the equality face).
        span: Range<usize>,
        /// The target's rows (the report span).
        rows: Vec<u32>,
    }

    /// The written-spelling face of the official element identity: both
    /// sides must be identifier chains and byte-equal.
    fn equal(c: &Check<'_>, left: &Range<usize>, right: Option<&Range<usize>>) -> bool {
        let Some(right) = right else {
            return false;
        };
        let is_chain = |span: &Range<usize>| {
            c.unit
                .source
                .get(span.clone())
                .is_some_and(|spelling| is_identifier_chain(spelling))
        };
        is_chain(left)
            && is_chain(right)
            && c.unit.source[left.clone()] == c.unit.source[right.clone()]
    }

    /// UTF-16 min/max over a row run — the engine's external coordinates.
    fn utf16_run_span(c: &Check<'_>, rows: &[u32]) -> Range<u32> {
        let mut span = c.utf16_span(rows[0]);
        for &row in &rows[1..] {
            let other = c.utf16_span(row);
            span.start = span.start.min(other.start);
            span.end = span.end.max(other.end);
        }
        span
    }

    /// `ident('.' ident)*` with optional `?.` separators and a
    /// non-`this`/`super` head — the `SimpleIdentifier` /
    /// `PrefixedIdentifier` / `PropertyAccess` faces.
    fn is_identifier_chain(spelling: &str) -> bool {
        let mut segments = spelling.split('.');
        // `split` always yields at least one (possibly empty) segment.
        let head = segments.next().unwrap_or("");
        if head == "this" || head == "super" || !is_identifier(head) {
            return false;
        }
        segments.all(|segment| is_identifier(segment.strip_suffix('?').unwrap_or(segment)))
    }

    fn is_identifier(spelling: &str) -> bool {
        let mut characters = spelling.chars();
        matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$')
            && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_' || rest == '$')
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("join_return_with_assignment"))
        }

        /// `test_localVariable_assignment` — message, correction, and the
        /// report pinning the assignment statement.
        #[test]
        fn local_variable_assignment_reports() {
            let source = "int f(int a) {\n  a = 0;\n  return a;\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("join_return_with_assignment"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "Assignment could be inlined in 'return' statement."
            );
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try inlining the assigned value in the 'return' statement.")
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "a", 1))
            );
        }

        /// `test_localVariable_assignment_compound` — `+=` joins.
        #[test]
        fn compound_assignment_reports() {
            assert_eq!(run("int f(int a) {\n  a += 0;\n  return a;\n}\n").len(), 1);
        }

        /// `test_class_field_withoutPrefix` — `??=` joins; the
        /// implicit-`this` field reads as the bare name.
        #[test]
        fn null_aware_assignment_reports() {
            let source =
                "class A {\n  int? _a;\n  int? foo() {\n    _a ??= 0;\n    return _a;\n  }\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// `test_localVariable_postfix` / `test_localVariable_prefix`.
        #[test]
        fn prefix_and_postfix_increment_report() {
            assert_eq!(run("int f(int a) {\n  a++;\n  return a;\n}\n").len(), 1);
            assert_eq!(run("int f(int a) {\n  ++a;\n  return a;\n}\n").len(), 1);
        }

        /// `test_class_field_propertyAccess` / `..._nested` — chains join.
        #[test]
        fn property_access_chains_report() {
            let source =
                "class A {\n  int f = 0;\n}\n\nint f(A a) {\n  a.f = 0;\n  return a.f;\n}\n";
            assert_eq!(run(source).len(), 1);
            let source = "class A {\n  int f = 0;\n}\n\nclass B {\n  A a = A();\n}\n\nint f(B b) {\n  b.a.f = 0;\n  return b.a.f;\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// `test_localVariable_hasPreviousAssignment` — a same-target
        /// assignment one step further back keeps the assignment.
        #[test]
        fn three_statement_same_target_is_silent() {
            assert!(run("int f(int a) {\n  a += 1;\n  a += 2;\n  return a;\n}\n").is_empty());
        }

        /// `test_class_field_propertyAccess_notSame` /
        /// `..._nested_notSame` — different chains stay silent.
        #[test]
        fn different_targets_are_silent() {
            let source = "class A {\n  int f = 0;\n}\n\nint f(A a1, A a2) {\n  a1.f = 0;\n  return a2.f;\n}\n";
            assert!(run(source).is_empty());
            let source = "class A {\n  int f = 0;\n}\n\nclass B {\n  A a = A();\n}\n\nint f(B b1, B b2) {\n  b1.a.f = 0;\n  return b2.a.f;\n}\n";
            assert!(run(source).is_empty());
        }

        /// `test_localVariable_declaration` — a declaration is not an
        /// assignment.
        #[test]
        fn declaration_is_silent() {
            assert!(run("int f(int a) {\n  var a = 0;\n  return a;\n}\n").is_empty());
        }

        /// `test_patternAssignment_multiple` /
        /// `test_patternVariableDeclaration_multiple` — pattern shapes
        /// are not the assignment face.
        #[test]
        fn pattern_shapes_are_silent() {
            assert!(run("int f(int a, int b) {\n  (a, b) = (0, 1);\n  return a;\n}\n").is_empty());
            assert!(
                run("int f(int a, int b) {\n  var (a, b) = (0, 1);\n  return a;\n}\n").is_empty()
            );
        }

        /// A bare `return;` has no expression (:60-66); a non-return last
        /// statement bails (:64).
        #[test]
        fn bare_return_and_wrong_order_are_silent() {
            assert!(run("int f(int a) {\n  a = 0;\n  return;\n}\n").is_empty());
            assert!(run("int f(int a) {\n  return a;\n  a = 0;\n}\n").is_empty());
        }

        /// The explicit `this` selector never joins (the official
        /// `PropertyAccess` target walk dies on `this`).
        #[test]
        fn explicit_this_chain_is_silent() {
            let source = "class A {\n  int? _a;\n  int? foo() {\n    this._a = 0;\n    return this._a;\n  }\n}\n";
            assert!(run(source).is_empty());
        }

        /// Non-chain shapes (`a[0]`) stay silent — the official
        /// comparison answers false off the identifier-chain faces.
        #[test]
        fn indexed_target_is_silent() {
            assert!(run("int f(List<int> a) {\n  a[0] = 1;\n  return a[0];\n}\n").is_empty());
        }

        /// `test_class_field_withoutPrefix_ifThenElse` — each block's
        /// tail is judged independently.
        #[test]
        fn nested_blocks_report_independently() {
            let source = "class A {\n  int? _a;\n  int? foo(bool b) {\n    if (b) {\n      _a = 0;\n      return _a;\n    } else {\n      _a = 1;\n      return _a;\n    }\n  }\n}\n";
            assert_eq!(run(source).len(), 2);
        }
    }
}

/// The catch family: an `on` clause catching the dart:core `Error`
/// family.
pub(crate) mod avoid_catching_errors {
    use dartq_ast::NodeKind;

    use super::{Check, Group, RuleSpec};

    /// The dart:core `Error` family by name (sdk/lib/core/errors.dart):
    /// `Error` itself plus its public subtypes. The official asks the
    /// type system (`exceptionType.implementsInterface('Error',
    /// 'dart.core')`, :41).
    const ERROR_FAMILY: [&str; 13] = [
        "Error",
        "ArgumentError",
        "AssertionError",
        "ConcurrentModificationError",
        "IndexError",
        "NoSuchMethodError",
        "OutOfMemoryError",
        "RangeError",
        "StackOverflowError",
        "StateError",
        "TypeError",
        "UnimplementedError",
        "UnsupportedError",
    ];

    const CLASS_PROBLEM: &str = "The type 'Error' should not be caught.";
    const SUBCLASS_PROBLEM: &str =
        "The type '{0}' should not be caught because it is a subclass of 'Error'.";
    const CORRECTION: &str = "Try removing the catch or catching an 'Exception' instead.";

    /// Official: `upstream Dart SDK source,
    /// pkg/linter/lib/src/rules/avoid_catching_errors.dart` — a catch
    /// clause whose `on` type implements dart:core `Error` reports at
    /// the clause (:39-52): the exact `Error` type with the class code,
    /// any subtype with the subclass code (the written type spelled into
    /// `{0}`); a clause without an `on` type never enters.
    ///
    /// Approximation (documented, class R2): the subtype verdict is a
    /// spelling allowlist — the dart:core `Error` family by name.
    /// Under-reports: user `extends Error` hierarchies and prefixed or
    /// aliased spellings (the type-system faces the allowlist cannot
    /// see). Over-reports: user classes named like a family member
    /// without implementing `Error` (pinned by test). Message:
    /// `diagnostic.g.dart` `avoidCatchingErrorsClass` /
    /// `avoidCatchingErrorsSubclass`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_catching_errors",
        group: Group::Standalone,
        problem: CLASS_PROBLEM,
        correction: CORRECTION,
        kinds: &[NodeKind::CatchClause],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // `node.exceptionType` (:40) — the `on` form only; the head token
        // is the `on` keyword when present (head-when-present).
        if c.lexeme(c.head_token(node)) != "on" {
            return;
        }
        // The written type: the `Type` child after the `on` keyword
        // (`compute_type`, parser try-statement lane).
        let Some(type_row) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)
        else {
            return;
        };
        let name = c.lexeme(c.head_token(type_row));
        if !ERROR_FAMILY.contains(&name) {
            return;
        }
        // The whole clause is the reported node (:43, :45-50); the two
        // official codes split on the exact-`Error` face, the written
        // name filling the subclass `{0}`.
        let range = c.utf16_span(node);
        if name == "Error" {
            c.report_texts(range, CLASS_PROBLEM, CORRECTION, &[]);
        } else {
            c.report_texts(range, SUBCLASS_PROBLEM, CORRECTION, &[name]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

        fn run(source: &str) -> Vec<crate::engine::Lint> {
            Fixture::parse(source).run(&one_rule("avoid_catching_errors"))
        }

        /// `test_exactlyError` — the class code, message verbatim, the
        /// report pinning the whole clause.
        #[test]
        fn on_error_reports_the_class_message() {
            let source = "void f() {\n  try {} on Error catch (_) {}\n}\n";
            let f = Fixture::parse(source);
            let lints = f.run(&one_rule("avoid_catching_errors"));
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(lints[0].message, "The type 'Error' should not be caught.");
            assert_eq!(
                lints[0].correction.as_deref(),
                Some("Try removing the catch or catching an 'Exception' instead.")
            );
            assert_eq!(
                lints[0].range.start,
                utf16_at(&f, tok(source, &f.tokens, "on", 0))
            );
        }

        /// The subclass code with the written type spelled into `{0}`.
        #[test]
        fn on_error_subtype_reports_the_subclass_message() {
            let lints = run("void f() {\n  try {} on StateError catch (_) {}\n}\n");
            assert_eq!(lints.len(), 1, "{lints:?}");
            assert_eq!(
                lints[0].message,
                "The type 'StateError' should not be caught because it is a subclass of 'Error'."
            );
        }

        /// Family members beyond the `Error` head all report.
        #[test]
        fn family_members_report() {
            for name in [
                "ArgumentError",
                "AssertionError",
                "ConcurrentModificationError",
                "IndexError",
                "NoSuchMethodError",
                "OutOfMemoryError",
                "RangeError",
                "StackOverflowError",
                "StateError",
                "TypeError",
                "UnimplementedError",
                "UnsupportedError",
            ] {
                let source = format!("void f() {{\n  try {{}} on {name} catch (_) {{}}\n}}\n");
                assert_eq!(run(&source).len(), 1, "{name}");
            }
        }

        /// A bare `catch` has no exception type (:40) — silent.
        #[test]
        fn bare_catch_is_silent() {
            assert!(run("void f() {\n  try {} catch (e) {}\n}\n").is_empty());
        }

        /// `test_exactlyException` — `Exception` is not the `Error`
        /// family.
        #[test]
        fn on_exception_is_silent() {
            assert!(run("void f() {\n  try {} on Exception catch (_) {}\n}\n").is_empty());
        }

        /// Any non-family type stays silent.
        #[test]
        fn on_other_type_is_silent() {
            assert!(run("void f() {\n  try {} on String catch (_) {}\n}\n").is_empty());
        }

        /// The pinned over-report face: a user class named like a family
        /// member without implementing `Error` still reports (the
        /// allowlist is name-level).
        #[test]
        fn user_same_name_still_reports() {
            let source =
                "class StateError {}\n\nvoid f() {\n  try {} on StateError catch (_) {}\n}\n";
            assert_eq!(run(source).len(), 1);
        }

        /// The pinned under-report face: a user `extends Error` hierarchy
        /// outside the allowlist stays silent.
        #[test]
        fn unknown_error_subtype_under_reports() {
            let source = "class MyError extends Error {}\n\nvoid f() {\n  try {} on MyError catch (_) {}\n}\n";
            assert!(run(source).is_empty());
        }

        /// A prefixed spelling's head is the prefix — under-report.
        #[test]
        fn prefixed_spelling_is_silent() {
            assert!(
                run("import 'dart:io' as io;\n\nvoid f() {\n  try {} on io.SocketException catch (_) {}\n}\n")
                    .is_empty()
            );
        }
    }
}

/// The W5 (#1124) approximation pins over real parser trees (the rule
/// dispatches [`NodeKind::TryStatement`] — production parses throughout).
#[cfg(test)]
mod w5_approx_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    /// The corpus `test_failInTry`: a trailing `fail(...)` under a single
    /// catch clause reports at the whole statement through its `;`.
    #[test]
    fn fail_in_try_reports() {
        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {}\n}\n",
        );
        let lints = f.run(&one_rule("use_test_throws_matchers"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Use the 'throwsA' matcher instead of using 'fail' when there is no \
             exception thrown."
        );
        let start = utf16_at(&f, tok(&f.source, &f.tokens, "fail", 0));
        assert_eq!(lints[0].range, start..start + 13);
    }

    /// The corpus `test_failWithExpectInCatch` and
    /// `test_failWithExpectInOnCatch`: a `finally` block and a second catch
    /// clause each silence the face.
    #[test]
    fn finally_and_second_catch_stay_silent() {
        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {\n    expect(e, null);\n  } finally {\n    print('hello');\n  }\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n    fail('fail');\n  } on Exception catch (e) {\n    expect(e, null);\n  } catch (e) {\n    expect(e, null);\n  }\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
    }

    /// The corpus `test_noFail` and the empty-body gate: a try whose body
    /// never calls `fail` — or has no statements at all — stays silent.
    #[test]
    fn no_fail_stays_silent() {
        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    f();\n  } catch (e) {}\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {}\n  catch (e) {\n    fail('fail');\n  }\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
    }

    /// The URI identity stand-in: without an import of a package that
    /// exports the test `fail`, the bare name never reports.
    #[test]
    fn missing_test_import_stays_silent() {
        let f = Fixture::parse(
            "void f() {\n  try {\n    f();\n    fail('fail');\n  } catch (e) {}\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
    }

    /// `fail` is not the last body statement, or the send is a member call:
    /// neither reports (the `x.fail` send carries the trailing access
    /// marker the bare top-level name face excludes).
    #[test]
    fn member_send_and_non_last_statement_stay_silent() {
        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nvoid f() {\n  try {\n    fail('fail');\n    f();\n  } catch (e) {}\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());

        let f = Fixture::parse(
            "import 'package:test/test.dart';\n\nclass C {\n  void fail(String m) {}\n}\n\nvoid f(C c) {\n  try {\n    c.fail('fail');\n  } catch (e) {}\n}\n",
        );
        assert!(f.run(&one_rule("use_test_throws_matchers")).is_empty());
    }
}

pub(crate) mod use_build_context_synchronously {
    use std::path::Component;

    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: upstream Dart SDK source,
    /// `pkg/linter/lib/src/rules/use_build_context_synchronously.dart` —
    /// a `BuildContext` used across an async gap reports at the use
    /// (`Don't use 'BuildContext's across async gaps.`), upgraded to the
    /// unrelated-guard variant (`... across async gaps, guarded by an
    /// unrelated 'mounted' check.`) when a `mounted` check stood in guard
    /// position but resolved to the wrong association. Messages:
    /// `diagnostic.g.dart` `useBuildContextSynchronouslyAsyncUse` /
    /// `useBuildContextSynchronouslyWrongMounted` — one rule name, two
    /// upstream codes sharing the `name:` field, so the machine output and
    /// the freeze universe carry one row.
    ///
    /// The shell is the thin consumer of the #1198 async-gap face: use
    /// identification (BuildContext-typed values, the `State.context`
    /// getter shape, the nullable exemption), await-gap segmentation, and
    /// mounted-guard recognition are the face's per-use verdict rows,
    /// mounted through [`crate::engine::Unit::async_context_uses`]; each
    /// row is reported once, at its own span. The shell selects the
    /// message variant and applies the two registration gates: the
    /// test-directory bail (the official registration runs only outside a
    /// test directory) and the same-file `BuildContext` shadow gate (the
    /// face resolves `BuildContext` by name, so a same-file declaration of
    /// the type breaks the element identity — the unit stays silent rather
    /// than mis-judge).
    ///
    /// The face does not model the protected-callback closure class
    /// (upstream's `Future`/`Stream`/`StreamController` argument table):
    /// closure-internal uses emit no rows and so never report here. The
    /// closure face is the substrate's to add — the shell never re-walks
    /// flow or guesses verdicts.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_build_context_synchronously",
        group: Group::Standalone,
        problem: ASYNC_USE_PROBLEM,
        correction: ASYNC_USE_CORRECTION,
        kinds: &[NodeKind::CompilationUnit],
        check,
    };

    const ASYNC_USE_PROBLEM: &str = "Don't use 'BuildContext's across async gaps.";
    const ASYNC_USE_CORRECTION: &str = "Try rewriting the code to not use the 'BuildContext', or guard the use with a 'mounted' check.";
    const WRONG_MOUNTED_PROBLEM: &str =
        "Don't use 'BuildContext's across async gaps, guarded by an unrelated 'mounted' check.";
    const WRONG_MOUNTED_CORRECTION: &str = "Guard a 'State.context' use with a 'mounted' check on the State, and other BuildContext use with a 'mounted' check on the BuildContext.";

    fn check(c: &mut Check<'_>) {
        let Some(rows) = c.unit.async_context_uses else {
            return;
        };
        // The registration bail: the official rule registers only outside
        // a test directory (`context.isInTestDirectory`).
        if in_test_directory(c) {
            return;
        }
        // The element-identity gate: a same-file `BuildContext` type
        // declaration breaks the face's name-based resolution.
        if declares_build_context_type(c) {
            return;
        }
        for row in rows {
            if row.guarded {
                continue;
            }
            let (problem, correction) = if row.unrelated_mounted {
                (WRONG_MOUNTED_PROBLEM, WRONG_MOUNTED_CORRECTION)
            } else {
                (ASYNC_USE_PROBLEM, ASYNC_USE_CORRECTION)
            };
            c.report_texts(row.range.clone(), problem, correction, &[]);
        }
    }

    /// The official test-directory face (`workspace.dart`
    /// `_testDirectoryNames`): the package-relative path's first component
    /// is one of the four test directory names. Without a path or package
    /// facts the gate stays out (the official no-package posture).
    fn in_test_directory(c: &Check<'_>) -> bool {
        let (Some(path), Some(workspace)) = (c.unit.path, c.unit.workspace) else {
            return false;
        };
        path.strip_prefix(&workspace.root)
            .ok()
            .and_then(|relative| relative.components().next())
            .is_some_and(|component| {
                matches!(
                    component,
                    Component::Normal(name)
                        if name == "test"
                            || name == "integration_test"
                            || name == "test_driver"
                            || name == "testing"
                )
            })
    }

    /// Whether the unit declares a type named `BuildContext` (the
    /// element-identity gate): a type-kind row whose wrapper-level name
    /// identifier carries the spelling.
    fn declares_build_context_type(c: &Check<'_>) -> bool {
        const TYPE_KINDS: [NodeKind; 6] = [
            NodeKind::ClassDeclaration,
            NodeKind::MixinDeclaration,
            NodeKind::EnumDeclaration,
            NodeKind::ExtensionDeclaration,
            NodeKind::ExtensionTypeDeclaration,
            NodeKind::Typedef,
        ];
        for top in c.collect_direct(c.node()) {
            if c.kind(top) != NodeKind::TopLevelDeclaration {
                continue;
            }
            let Some(&decl) = c
                .collect_direct(top)
                .iter()
                .find(|row| TYPE_KINDS.contains(&c.kind(**row)))
            else {
                continue;
            };
            // The declaration's own name Identifier carries a
            // declaration-name context; it tiles inside the window for a
            // typedef, or as a leading sibling for the class-likes.
            let is_name = |kid: u32| {
                c.kind(kid) == NodeKind::Identifier
                    && c.lexeme(c.head_token(kid)) == "BuildContext"
                    && matches!(
                        c.payload(kid).first().copied(),
                        Some(ctx)
                            if ctx
                                == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                                    as u32
                                || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
                                || ctx == dartq_parser::IdentifierContext::TypedefDeclaration as u32
                    )
            };
            if c.children(decl).any(&is_name) {
                return true;
            }
            let siblings = c.collect_direct(top);
            if let Some(pos) = siblings.iter().position(|&kid| kid == decl)
                && siblings[..pos].iter().copied().any(is_name)
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod use_build_context_synchronously_tests {
    use std::path::{Path, PathBuf};

    use crate::engine::{AsyncContextUse, Lint};
    use crate::testsupport::{Fixture, one_rule};

    /// One unguarded row anchored at `offset` (width 3, the shape of a
    /// `context` use).
    fn unguarded(offset: u32) -> AsyncContextUse {
        AsyncContextUse {
            range: offset..offset + 3,
            guarded: false,
            unrelated_mounted: false,
        }
    }

    /// Runs the rule with `rows` mounted (no workspace facts — the
    /// test-directory gate stays out).
    fn run(f: &Fixture, rows: &[AsyncContextUse]) -> Vec<Lint> {
        f.run_with(
            &one_rule("use_build_context_synchronously"),
            None,
            None,
            Some(rows),
        )
    }

    /// The upstream async-use message pair, verbatim from
    /// `diagnostic.g.dart`.
    #[test]
    fn unguarded_row_reports_async_use() {
        let f = Fixture::parse("void f() {}\n");
        let lints = run(&f, &[unguarded(8)]);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Don't use 'BuildContext's across async gaps."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some(
                "Try rewriting the code to not use the 'BuildContext', or guard the use with a 'mounted' check."
            )
        );
        assert_eq!(lints[0].range, 8..11);
    }

    /// A guarded verdict never reports; the unrelated-guarded verdict
    /// upgrades to the `wrong_mounted` message pair.
    #[test]
    fn guarded_stays_silent_unrelated_guard_upgrades() {
        let f = Fixture::parse("void f() {}\n");
        let guarded = AsyncContextUse {
            range: 8..11,
            guarded: true,
            unrelated_mounted: false,
        };
        assert!(run(&f, &[guarded]).is_empty());
        let unrelated = AsyncContextUse {
            range: 8..11,
            guarded: false,
            unrelated_mounted: true,
        };
        let lints = run(&f, &[unrelated]);
        assert_eq!(lints.len(), 1, "{lints:?}");
        assert_eq!(
            lints[0].message,
            "Don't use 'BuildContext's across async gaps, guarded by an unrelated 'mounted' check."
        );
        assert_eq!(
            lints[0].correction.as_deref(),
            Some(
                "Guard a 'State.context' use with a 'mounted' check on the State, and other BuildContext use with a 'mounted' check on the BuildContext."
            )
        );
    }

    /// Rows report in face order at their own spans.
    #[test]
    fn rows_report_in_order_at_their_spans() {
        let f = Fixture::parse(
            "void f() {}\n\n// The row anchors below must sit inside the source.\n\n",
        );
        let lints = run(&f, &[unguarded(8), unguarded(40), unguarded(64)]);
        assert_eq!(lints.len(), 3, "{lints:?}");
        assert_eq!(lints[0].range.start, 8);
        assert_eq!(lints[1].range.start, 40);
        assert_eq!(lints[2].range.start, 64);
    }

    /// The registration bail: files under the package's test directories
    /// (`workspace.dart` `_testDirectoryNames`) emit nothing; the same
    /// rows outside them report.
    #[test]
    fn test_directories_bail() {
        let root = Path::new("/pkg");
        let facts = crate::WorkspaceFacts {
            own_package: "t".to_owned(),
            dependencies: ["t".to_owned()].into_iter().collect(),
            dev_dependencies: rustc_hash::FxHashSet::default(),
            root: root.to_owned(),
            lib_dir: root.join("lib"),
        };
        for directory in ["test", "integration_test", "test_driver", "testing"] {
            let mut f = Fixture::parse("void f() {}\n");
            f.path = PathBuf::from(format!("/pkg/{directory}/a.dart"));
            let lints = f.run_with(
                &one_rule("use_build_context_synchronously"),
                Some(&facts),
                None,
                Some(&[unguarded(8)]),
            );
            assert!(lints.is_empty(), "{directory}: {lints:?}");
        }
        let mut f = Fixture::parse("void f() {}\n");
        f.path = PathBuf::from("/pkg/lib/a.dart");
        let lints = f.run_with(
            &one_rule("use_build_context_synchronously"),
            Some(&facts),
            None,
            Some(&[unguarded(8)]),
        );
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// The element-identity gate: a same-file type declaration named
    /// `BuildContext` silences the whole unit.
    #[test]
    fn build_context_shadow_silences_the_unit() {
        let f = Fixture::parse("class BuildContext {}\n\nvoid f() {}\n");
        assert!(run(&f, &[unguarded(28)]).is_empty());
        // A use of the NAME in a non-declaration position is not a
        // shadow: the unit still reports.
        let f = Fixture::parse("void f(BuildContext? context) {}\n");
        let lints = run(&f, &[unguarded(8)]);
        assert_eq!(lints.len(), 1, "{lints:?}");
    }

    /// A typedef carries the type-name spelling the same as a class.
    #[test]
    fn build_context_typedef_shadow_silences_the_unit() {
        let f = Fixture::parse("typedef BuildContext = Object;\n\nvoid f() {}\n");
        assert!(run(&f, &[unguarded(38)]).is_empty());
    }
}
