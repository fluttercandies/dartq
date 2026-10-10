//! The flow-control family: `curly_braces_in_flow_control_structures`,
//! `control_flow_in_finally`, `no_duplicate_case_values`,
//! `use_rethrow_when_possible`, `avoid_returning_null_for_void`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child, last_child};

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
        NodeKind::DoWhileStatement,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::WhileStatement => {
            // Body is the last child (the condition is the first,
            // collapse map §3.4 WhileStatement row).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a while");
            }
        }
        NodeKind::ForStatement => {
            // Body is the last child after the for-parts (:82-84).
            if let Some(body) = last_child(c, node) {
                check_body(c, body, "a for");
            }
        }
        NodeKind::DoWhileStatement => {
            // Body is the first child, before the `while (...)` condition
            // (collapse map §3.4 DoWhileStatement row: body = the children
            // segment before the While token).
            if let Some(body) = first_child(c, node) {
                check_body(c, body, "a do");
            }
        }
        NodeKind::IfStatement => check_if(c, node),
        _ => {}
    }
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
    // Direct children: condition, then-arm, [else-arm]. The scratch guard is
    // scoped so the reporting calls below can take `&mut Check`.
    let arms = {
        let kids = c.direct_children(node);
        (
            kids.get(1).copied(),
            kids.get(2).copied(),
            last_child(c, node),
        )
    };
    let Some(then) = arms.0 else {
        return;
    };
    if else_present {
        // With an `else`, both arms are checked; an else-if chain arm is
        // skipped (`elseStatement is! IfStatement`, :71-77).
        check_body(c, then, "an if");
        if let Some(else_stmt) = arms.1 {
            if c.kind(else_stmt) != NodeKind::IfStatement {
                check_body(c, else_stmt, "an if");
            }
        }
        return;
    }
    // An `if` without `else` that IS the else branch of an outer if: the
    // outer chain already requires blocks, so the then-arm is checked
    // unconditionally (:55-61).
    if let Some(parent) = c.ancestor(1) {
        if c.kind(parent) == NodeKind::IfStatement
            && c.payload(parent).first().copied() != Some(NONE_TOKEN)
            && arms.2 == Some(node)
        {
            check_body(c, then, "an if");
            return;
        }
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
    /// locally), or — for targeted `break`/`continue` — the target is inside
    /// the try statement (the jump stays local to it). Message:
    /// `diagnostic.g.dart` `controlFlowInFinally`.
    ///
    /// Approximation (documented): the official resolves the break/continue
    /// target through the label scope; here an untargeted statement uses its
    /// nearest enclosing loop/switch on the path and a targeted one its
    /// nearest enclosing `LabeledStatement`/loop — for valid code the
    /// enclosing-label chain is the target chain, so only nested labels
    /// wrapping the try can diverge.
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
        // `finally` token (collapse map §3.4 TryStatementEnd).
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
        let report = match c.kind(node) {
            NodeKind::Break | NodeKind::Continue => {
                let has_target = c.payload(node).first().copied().unwrap_or(0) != 0;
                let target = nearest(c, node_idx, |kind| match kind {
                    NodeKind::WhileStatement
                    | NodeKind::ForStatement
                    | NodeKind::ForIn
                    | NodeKind::DoWhileStatement
                    | NodeKind::SwitchStatement => true,
                    NodeKind::LabeledStatement => has_target,
                    _ => false,
                });
                match target {
                    // `ancestor.thisOrAncestorMatching(n == tryStatement)`
                    // (:79-91): the enabler exists when the jump target's own
                    // ancestor chain contains the try statement — the target
                    // sits below the try on the path (idx > try_idx) and the
                    // jump stays local to it.
                    Some(idx) => idx <= try_idx,
                    // Unresolved target: the closure exemption decides
                    // (`ancestor == null` branch, :69-77).
                    None => !closure_wraps(c, try_idx + 1, node_idx),
                }
            }
            _ => !closure_wraps(c, try_idx + 1, node_idx),
        };
        if report {
            let range = c.utf16_span(node);
            c.report(range, &[kind]);
        }
    }

    /// The nearest path index below `from` whose node kind satisfies `pred`.
    fn nearest(c: &Check<'_>, from: usize, pred: impl Fn(NodeKind) -> bool) -> Option<usize> {
        (0..from).rev().find(|&i| pred(c.kind(c.path[i])))
    }

    /// Whether a closure (function expression / local function) sits on the
    /// path between the finally block and the statement — the official
    /// `FunctionBody inside the finally block` exemption (:69-77).
    fn closure_wraps(c: &Check<'_>, finally_idx: usize, node_idx: usize) -> bool {
        (finally_idx + 1..=node_idx).any(|i| {
            matches!(
                c.kind(c.path[i]),
                NodeKind::FunctionExpression | NodeKind::LocalFunctionDeclaration
            )
        })
    }
}

pub(crate) mod no_duplicate_case_values {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::token_seq_equal;

    /// Official: `pkg/linter/lib/src/rules/no_duplicate_case_values.dart:37-58`
    /// — a `case` whose constant value equals an earlier `case` of the same
    /// switch reports at the duplicate expression with both sources spelled
    /// into `{0}`/`{1}`.
    ///
    /// Approximation (documented): the official keys the map by the
    /// computed constant (`computeConstantValue`, :44); duplicate
    /// detection here compares token-lexeme sequences, so only
    /// syntactically identical expressions collide — `case 1 + 1:` vs
    /// `case 2:` diverges (under-reports), and an identifier repeated
    /// across two cases reports even when non-constant (the official skips
    /// unknown values — over-reports on erroneous code only). Message:
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
        let mut seen: Vec<u32> = Vec::new();
        for case in cases {
            // Expressions follow the label prefix (`labelCount` pruned to
            // the Label children, collapse map §3.4 SwitchCaseEnd); word 2
            // counts them.
            let count = c.payload(case).get(2).copied().unwrap_or(0) as usize;
            if count == 0 {
                continue;
            }
            let kids = c.collect_direct(case);
            let expressions: Vec<u32> = kids
                .into_iter()
                .filter(|&kid| c.kind(kid) != NodeKind::Label)
                .take(count)
                .collect();
            for expression in expressions {
                if let Some(original) = seen
                    .iter()
                    .copied()
                    .find(|&earlier| token_seq_equal(c, earlier, expression))
                {
                    let dup = source_slice(c, expression).to_owned();
                    let orig = source_slice(c, original).to_owned();
                    let range = c.utf16_span(expression);
                    c.report(range, &[&dup, &orig]);
                } else {
                    seen.push(expression);
                }
            }
        }
    }

    /// The source text of one expression (the official `toString()`
    /// arguments).
    fn source_slice<'a>(c: &'a Check<'_>, node: u32) -> &'a str {
        let bytes = c.byte_span(node);
        &c.unit.source[bytes]
    }
}

pub(crate) mod use_rethrow_when_possible {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::first_child;

    /// Official: `pkg/linter/lib/src/rules/use_rethrow_when_possible.dart:36-46`
    /// — a `throw` of exactly the caught exception parameter, as a statement,
    /// reports at the throw. The parameter identity
    /// (`element == exceptionParameter`, :39-44) is
    /// approximated lexically: the thrown identifier must name the nearest
    /// enclosing catch's parameter (same lexeme). The documented divergence
    /// is shadowing — a local named like the parameter inside the catch body
    /// over-reports. Message: `diagnostic.g.dart` `useRethrowWhenPossible`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "use_rethrow_when_possible",
        group: Group::Recommended,
        problem: "Use 'rethrow' to rethrow a caught exception.",
        correction: "Try replacing the 'throw' with a 'rethrow'.",
        kinds: &[NodeKind::ThrowExpression],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `node.parent is! ExpressionStatement` bails (:37).
        if c.ancestor(1)
            .is_none_or(|parent| c.kind(parent) != NodeKind::ExpressionStatement)
        {
            return;
        }
        // The thrown expression is the throw's operand child (collapse map
        // §3.5 ThrowExpressionHandle).
        let Some(operand) = first_child(c, c.node()) else {
            return;
        };
        if c.kind(operand) != NodeKind::Identifier {
            return;
        }
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
        // `exceptionParameter` null, :42-43).
        let Some(exception) = c
            .collect_direct(catch_clause)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameter)
        else {
            return;
        };
        // The parameter name is payload word 1 (collapse map §3.3
        // FormalParameterEnd).
        let Some(&name) = c.payload(exception).get(1) else {
            return;
        };
        if c.lexeme(c.head_token(operand)) == c.lexeme(name) {
            let range = c.utf16_span(c.node());
            c.report(range, &[]);
        }
    }
}

pub(crate) mod avoid_returning_null_for_void {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, first_child, last_child};

    const PROBLEM: &str = "Don't return 'null' from a function with a return type of 'void'.";
    const METHOD_PROBLEM: &str = "Don't return 'null' from a method with a return type of 'void'.";
    const CORRECTION: &str = "Try removing the 'null'.";

    /// Official: `pkg/linter/lib/src/rules/avoid_returning_null_for_void.dart:41-81`
    /// — `return null` (statement or arrow body) inside a sync function or
    /// method whose declared return type is `void` reports at the return.
    /// The two official message variants (from-function / from-method,
    /// `diagnostic.g.dart:727/:739`) split on the enclosing declaration kind.
    ///
    /// Faces not ported (documented): the async `Future<void>` branch
    /// (:77-80) needs the declared type resolved (`type.isDartAsyncFuture`),
    /// and the closure face under-reports when the return type is inferred
    /// from context (`void Function() f = () { return null; };`) — a
    /// function expression has no written return type here. `async void`
    /// functions never report (neither official branch takes them), so the
    /// enclosing body's `async` token is read (collapse map §3.9:
    /// `ExpressionBody` word 2, `BlockBody` word 0; a plain statement `Block`
    /// body carries no async words).
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
        // has no child.
        let Some(expression) = first_child(c, node) else {
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
                // A closure has no written return type here — out of face.
                NodeKind::FunctionExpression => return,
                NodeKind::Method => {
                    report_void(c, node, ancestor, METHOD_PROBLEM);
                    return;
                }
                NodeKind::TopLevelMethod
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::NamedFunctionExpression => {
                    report_void(c, node, ancestor, PROBLEM);
                    return;
                }
                _ => {}
            }
        }
    }

    /// `isAsync`/`type is VoidType` (:75), then the report at `node`.
    fn report_void(c: &mut Check<'_>, node: u32, function: u32, problem: &str) {
        let Some(body) = last_child(c, function) else {
            return;
        };
        let async_kw = match c.kind(body) {
            NodeKind::ExpressionBody => c.payload(body).get(2).copied(),
            NodeKind::BlockBody => c.payload(body).first().copied(),
            _ => Some(NONE_TOKEN),
        };
        if async_kw != Some(NONE_TOKEN) {
            return;
        }
        // The written return type precedes the name; `void` is a
        // [`NodeKind::VoidType`] child (collapse map §3.3/§3.8). An inferred
        // or non-void written type never takes the official void branch.
        let declares_void = c
            .collect_direct(function)
            .into_iter()
            .any(|kid| c.kind(kid) == NodeKind::VoidType);
        if !declares_void {
            return;
        }
        let range = c.utf16_span(node);
        c.report_texts(range, problem, CORRECTION, &[]);
    }
}

pub(crate) mod type_literal_in_constant_pattern {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/type_literal_in_constant_pattern.dart:40-63`
    /// — a constant pattern whose expression is a type literal reports at
    /// the pattern. The official bails for a `const`-marked pattern (:45-47)
    /// and for a `Type`-typed matched value (:50-59, the raw-type
    /// exemption); `case int:`-style built-in names report.
    ///
    /// Approximation (documented): the type-literal test
    /// (`expression.staticType.isDartCoreType`, :61-63) is a resolution
    /// question. The lexical face reports a bare identifier whose lexeme is
    /// a built-in Dart type name — a set that can never be a type literal
    /// miss (`int`, `String`, ...), while the over-report faces stay inside
    /// the user-shadowed family: a user constant named like a built-in type
    /// (`const String = 'x'; case String:`) and the `Type`-typed matched
    /// value exemption (`Type t = ...; switch (t) { case int: }`) both
    /// over-report. User type literals (`case MyType:` — indistinguishable
    /// from a capitalized constant without resolution) under-report. Message:
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
    /// identifier). The official test corpus exercises the dart:core names.
    fn is_builtin_type_name(lexeme: &str) -> bool {
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
        // owns the `const` keyword as its span head (collapse map §3.7).
        if c.lexeme(c.head_token(node)) == "const" {
            return;
        }
        let Some(expression) = super::first_child(c, node) else {
            return;
        };
        if c.kind(expression) != NodeKind::Identifier {
            return;
        }
        if !is_builtin_type_name(c.lexeme(c.head_token(expression))) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, unit_of, utf16_at};

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

    // —— no_duplicate_case_values —————————————————————————————————————————

    /// `test/rules/no_duplicate_case_values_test.dart` `test_duplicateCase`
    /// — the duplicate reports with both sources in the message.
    #[test]
    fn duplicate_case_value_reports() {
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
                    b.leaf(NodeKind::LiteralInt, int_tok);
                    b.begin(NodeKind::Break, int_tok + 2);
                    b.patch(0, 0);
                    b.end(NodeKind::Break, int_tok + 4);
                    b.end(NodeKind::SwitchCase, int_tok + 4);
                }
                b.end(NodeKind::SwitchBlock, 16);
                b.end(NodeKind::SwitchStatement, 16);
            }),
        );
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

    /// Distinct values and a `default:` clause (no expressions) pass.
    #[test]
    fn distinct_and_default_cases_pass() {
        let source = "switch (x) { case 1: break; case 2: break; default: break; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 break8 ;9 case10 211 :12 break13 ;14
        // default15 :16 break17 ;18 }19
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                // case 1
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 6);
                b.begin(NodeKind::Break, 8);
                b.patch(0, 0);
                b.end(NodeKind::Break, 10);
                b.end(NodeKind::SwitchCase, 10);
                // case 2
                b.begin(NodeKind::SwitchCase, 10);
                b.patch(0, NONE);
                b.patch(1, 12);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 11);
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
        );
        assert!(f.run(&one_rule("no_duplicate_case_values")).is_empty());
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
        // carrying the `async` token (collapse map §3.9).
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
}
