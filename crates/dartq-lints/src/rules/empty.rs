//! The emptiness family: `empty_statements`, `empty_constructor_bodies`,
//! `empty_catches`, `avoid_empty_else`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::RuleSpec;
use crate::rules::{is_just_underscores, last_child};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`).
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

/// The last token index of `node`'s span.
fn last_token_of(c: &Check<'_>, node: u32) -> u32 {
    c.unit.ast.token_range(node).end - 1
}

pub(crate) mod empty_statements {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_statements.dart:33-53` —
    /// report every `EmptyStatement` except one that "defines semantics":
    /// the last statement of a switch pattern case whose statements are all
    /// empty (the `case _: ;` idiom, `empty_statements.dart:34-46`).
    /// Message: `diagnostic.g.dart` `emptyStatements`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "empty_statements",
    &[NodeKind::EmptyStatement],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if defines_semantics(c, node) {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// `definesSemantics` (empty_statements.dart:34-46): parent is a switch
    /// case, the node is its last statement, and every statement in the case
    /// is empty. Statements are the direct children after the matched
    /// expressions ([`NodeKind::SwitchCase`] payload word 2 counts them)
    /// and the optional `when` clause.
    fn defines_semantics(c: &Check<'_>, node: u32) -> bool {
        let Some(parent) = c.ancestor(1) else {
            return false;
        };
        if c.kind(parent) != NodeKind::SwitchCase {
            return false;
        }
        let expressions = c.payload(parent).get(2).copied().unwrap_or(0) as usize;
        let kids = c.direct_children(parent);
        if expressions >= kids.len() {
            return false;
        }
        let mut statements = expressions;
        if c.kind(kids[statements]) == NodeKind::SwitchCaseWhenClause {
            statements += 1;
        }
        kids.len() > statements
            && kids[kids.len() - 1] == node
            && kids[statements..]
                .iter()
                .all(|&s| c.kind(s) == NodeKind::EmptyStatement)
    }
}

pub(crate) mod empty_constructor_bodies {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_constructor_bodies.dart:34-61`
    /// — a constructor (or primary constructor body) whose block body is
    /// empty AND whose `}` carries no preceding comments reports at the
    /// block. The official factory exclusion is structural here: factory
    /// methods land as [`NodeKind::Factory`] nodes, not
    /// [`NodeKind::Constructor`] (the `FactoryEnd` verdict), so no
    /// `factoryKeyword` test is needed. Message: `diagnostic.g.dart`
    /// `emptyConstructorBodies`.
    ///
    /// dartq face: the production bridge emits NO `Block` child inside a
    /// function body — `parseFunctionBody` fires `begin_block_function_body`/
    /// `end_block_function_body` (never `begin_block`/`end_block`), so
    /// [`NodeKind::BlockBody`] IS the block face: its span covers the braces
    /// and its completion window holds the statement rows. Emptiness is the
    /// empty window; the `}` is the body's last token.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "empty_constructor_bodies",
    &[NodeKind::Constructor, NodeKind::PrimaryConstructorBody],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The body: a block body rides the `BlockBody` wrapper (bridge
        // `begin_block_function_body`), statements tiling inside it.
        let Some(body) = super::last_child(c, node) else {
            return;
        };
        if c.kind(body) != NodeKind::BlockBody {
            return;
        }
        let span = c.children(body);
        if span.start < span.end {
            return; // statements present
        }
        // `block.endToken.precedingComments == null`
        // (empty_constructor_bodies.dart:42).
        if c.comments
            .has_attached(super::last_token_of(c, body) as usize)
        {
            return;
        }
        let range = c.utf16_span(body);
        c.report(range, &[]);
    }
}

pub(crate) mod empty_catches {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_catches.dart:31-45` — an
    /// empty catch block with no comments before `}` reports at the block,
    /// unless the exception parameter is named with bare underscores
    /// (`catch (_)` is a deliberate discard, empty_catches.dart:33-38).
    /// Message: `diagnostic.g.dart` `emptyCatches`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "empty_catches",
    &[NodeKind::CatchClause],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The exception formal: production wraps the catch parameters in
        // a `FormalParameters` row (bridge `end_formal_parameters`), so
        // the formal is the first `FormalParameter` inside that wrapper;
        // hand-built tables carry it as a direct child. The name is
        // payload word 1 (the `FormalParameterEnd` layout).
        let exception = c.collect_direct(node).into_iter().find_map(|kid| {
            match c.kind(kid) {
                NodeKind::FormalParameter => Some(kid),
                NodeKind::FormalParameters => c
                    .collect_direct(kid)
                    .into_iter()
                    .find(|&formal| c.kind(formal) == NodeKind::FormalParameter),
                _ => None,
            }
        });
        if let Some(exception) = exception
            && let Some(&name) = c.payload(exception).get(1)
            && super::is_just_underscores(c.lexeme(name))
        {
            return;
        }
        let Some(body) = super::last_child(c, node) else {
            return;
        };
        if c.kind(body) != NodeKind::Block {
            return;
        }
        let span = c.children(body);
        if span.start < span.end {
            return;
        }
        if c.comments
            .has_attached(super::last_token_of(c, body) as usize)
        {
            return;
        }
        let range = c.utf16_span(body);
        c.report(range, &[]);
    }
}

pub(crate) mod avoid_empty_else {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_empty_else.dart:32-41` — an
    /// `else` branch that is a non-synthetic empty statement reports at the
    /// empty statement. Message: `diagnostic.g.dart` `avoidEmptyElse`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_empty_else",
    &[NodeKind::IfStatement],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Else present iff the `else` payload token is set;
        // the else statement is the last child.
        if c.payload(node).first().copied() == Some(crate::rules::NONE_TOKEN) {
            return;
        }
        let Some(else_stmt) = super::last_child(c, node) else {
            return;
        };
        if c.kind(else_stmt) != NodeKind::EmptyStatement {
            return;
        }
        if c.token_is_synthetic(c.head_token(else_stmt)) {
            return;
        }
        let range = c.utf16_span(else_stmt);
        c.report(range, &[]);
    }
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};
    use dartq_ast::NodeKind;

    /// Official cases: `test/rules/avoid_empty_else_test.dart`
    /// (`test_emptyIf_followedByStatement`, `test_blockBodies`).
    #[test]
    fn avoid_empty_else_reports_the_else_semicolon() {
        let source = "if (b) ; else ;";
        // if0 (1 b2 )3 ;4 else5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, tok(source, t, "else", 0));
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.leaf(NodeKind::EmptyStatement, 4);
                b.leaf(NodeKind::EmptyStatement, tok(source, t, ";", 1));
                b.end(NodeKind::IfStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("avoid_empty_else"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Empty statements are not allowed in an 'else' clause."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    }

    #[test]
    fn avoid_empty_else_ignores_block_else() {
        let source = "if (b) ; else {}";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::IfStatement, 0);
                b.patch(0, 5);
                b.begin(NodeKind::ParenthesizedCondition, 1);
                b.patch(0, 1);
                b.patch(1, 3);
                b.end(NodeKind::ParenthesizedCondition, 4);
                b.leaf(NodeKind::EmptyStatement, 4);
                b.leaf_span(NodeKind::Block, 6, 8);
                b.end(NodeKind::IfStatement, 8);
            }),
        );
        assert!(f.run(&one_rule("avoid_empty_else")).is_empty());
    }

    /// `test/rules/empty_statements_test.dart` `test_emptyWhile`.
    #[test]
    fn empty_statements_reports_a_bare_semicolon() {
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
        let lints = f.run(&one_rule("empty_statements"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary empty statement.");
        assert_eq!(
            lints[0].correction.as_deref(),
            Some("Try removing the empty statement or restructuring the code.")
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// The `case _: ;` idiom defines semantics
    /// (empty_statements.dart:34-46) — the only empty statement allowed.
    #[test]
    fn empty_statements_case_last_semicolon_defines_semantics() {
        let source = "switch (x) { case 1: ; }";
        // switch0 (1 x2 )3 {4 case5 16 :7 ;8 }9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1); // one matched expression
                b.leaf(NodeKind::LiteralInt, 6);
                b.leaf(NodeKind::EmptyStatement, 8);
                b.end(NodeKind::SwitchCase, 9);
                b.end(NodeKind::SwitchBlock, 9);
                b.end(NodeKind::SwitchStatement, 10);
            }),
        );
        assert!(f.run(&one_rule("empty_statements")).is_empty());
    }

    /// Only the case's LAST empty statement defines semantics.
    #[test]
    fn empty_statements_case_non_last_semicolon_reports() {
        let source = "switch (x) { case 1: ; ; }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::SwitchStatement, 0);
                b.begin(NodeKind::SwitchBlock, 4);
                b.begin(NodeKind::SwitchCase, 5);
                b.patch(0, NONE);
                b.patch(1, 7);
                b.patch(2, 1);
                b.leaf(NodeKind::LiteralInt, 6);
                b.leaf(NodeKind::EmptyStatement, 8);
                b.leaf(NodeKind::EmptyStatement, 9);
                b.end(NodeKind::SwitchCase, 10);
                b.end(NodeKind::SwitchBlock, 10);
                b.end(NodeKind::SwitchStatement, 11);
            }),
        );
        let lints = f.run(&one_rule("empty_statements"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].range.start, utf16_at(&f, 8));
    }

    /// A real-bridge pin (the hand-built trees above bypass the block-head
    /// snapshot arithmetic): the default member's `;` is the case's only
    /// statement, so `definesSemantics` keeps the rule silent.
    #[test]
    fn empty_statements_default_case_semicolon_is_silent_through_the_bridge() {
        let f = Fixture::parse("void f(int x) { switch (x) { default: ; } }");
        assert!(f.run(&one_rule("empty_statements")).is_empty());
        // The face `defines_semantics` reads: the `;`'s parent is the case,
        // not the block.
        let node = (0..crate::engine::narrow_u32(f.ast.len()))
            .find(|&row| f.ast.kind(row) == NodeKind::EmptyStatement)
            .expect("the fixture has an empty statement");
        assert_eq!(
            f.ast.parent(node).map(|parent| f.ast.kind(parent)),
            Some(NodeKind::SwitchCase)
        );
    }

    /// `test/rules/empty_constructor_bodies_test.dart` `test_emptyBlock`,
    /// on the production bridge shape (the body rides the `BlockBody`
    /// wrapper). Oracle row: at the `{}`, len 2.
    #[test]
    fn empty_constructor_bodies_reports_an_empty_block() {
        let f = Fixture::parse("class BareStart {\n  BareStart() {}\n}\n");
        let lints = f.run(&one_rule("empty_constructor_bodies"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Empty constructor bodies should be written using a ';' rather than '{}'."
        );
        let open = tok(&f.source, &f.tokens, "{", 1); // class `{`, then the body's
        let start = utf16_at(&f, open);
        assert_eq!(lints[0].range, start..start + 2);
    }

    /// `test/rules/empty_constructor_bodies_test.dart` `test_comment` — a
    /// comment before `}` keeps the block.
    #[test]
    fn empty_constructor_bodies_comment_keeps_the_block() {
        let f = Fixture::parse("class A {\n  A() {\n    /* c */\n  }\n}\n");
        assert!(f.run(&one_rule("empty_constructor_bodies")).is_empty());
    }

    #[test]
    fn empty_constructor_bodies_non_empty_block_passes() {
        let f = Fixture::parse("class A {\n  A() { x; }\n}\n");
        assert!(f.run(&one_rule("empty_constructor_bodies")).is_empty());
    }

    /// A `;` body is the desired form — never reports.
    #[test]
    fn empty_constructor_bodies_semicolon_body_passes() {
        let f = Fixture::parse("class A {\n  A();\n}\n");
        assert!(f.run(&one_rule("empty_constructor_bodies")).is_empty());
    }

    /// `test/rules/empty_catches_test.dart` `test_emptyCatchBlock`.
    #[test]
    fn empty_catches_reports_an_empty_catch_block() {
        let source = "try {} catch (e) {}";
        // try0 {1 }2 catch3 (4 e5 )6 {7 }8
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
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::CatchClause, 9);
                b.end(NodeKind::TryStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("empty_catches"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Empty catch block.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 7));
    }

    /// `catch (_)` is a deliberate discard (empty_catches.dart:33-38).
    #[test]
    fn empty_catches_underscore_exception_passes() {
        let source = "try {} catch (_) {}";
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
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::CatchClause, 9);
                b.end(NodeKind::TryStatement, 9);
            }),
        );
        assert!(f.run(&one_rule("empty_catches")).is_empty());
    }

    /// Production-bridge pins: the catch parameters wrap in a
    /// `FormalParameters` row, so the wildcard exemption must read the
    /// formal through the wrapper — `catch (_)` passes (`test_underscore`),
    /// `catch (e)` still reports.
    #[test]
    fn empty_catches_production_wildcard_passes() {
        assert!(Fixture::parse("void f() { try {} catch (_) {} }")
            .run(&one_rule("empty_catches"))
            .is_empty());
        assert!(Fixture::parse("void f() { try {} catch (___) {} }")
            .run(&one_rule("empty_catches"))
            .is_empty());
        assert_eq!(
            Fixture::parse("void f() { try {} catch (e) {} }")
                .run(&one_rule("empty_catches"))
                .len(),
            1
        );
        // An `on T catch (_)` clause reads the formal past the type.
        assert!(Fixture::parse("void f() { try {} on Exception catch (_) {} }")
            .run(&one_rule("empty_catches"))
            .is_empty());
    }

    /// A comment before `}` keeps the block (empty_catches.dart:39-44).
    #[test]
    fn empty_catches_comment_keeps_the_block() {
        let source = "try {} catch (e) { /* c */ }";
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
                b.end(NodeKind::Block, 9);
                b.end(NodeKind::CatchClause, 9);
                b.end(NodeKind::TryStatement, 9);
            }),
        );
        assert!(f.run(&one_rule("empty_catches")).is_empty());
    }
}

pub(crate) mod prefer_asserts_with_message {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_asserts_with_message.dart`
    /// — an assert statement or constructor assert initializer without a
    /// message reports at the assert (`_Visitor`: `node.message == null`).
    /// Message: `diagnostic.g.dart` `preferAssertsWithMessage`.
    ///
    /// dartq face: the [`NodeKind::Assert`] payload word 2 carries the
    /// separating comma, `TOKEN_NONE` exactly when no message follows;
    /// payload word 0 discriminates the official's two registered faces
    /// (initializer / statement) from the expression-position assert the
    /// official never visits.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_asserts_with_message",
    &[NodeKind::Assert],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let payload = c.payload(node);
        // Registered faces only: initializer (0) and statement (1).
        if payload.first().copied().unwrap_or(NONE_TOKEN) > 1 {
            return;
        }
        if payload.get(2).copied() == Some(NONE_TOKEN) {
            let range = c.utf16_span(node);
            c.report(range, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/prefer_asserts_with_message_test.dart`
        /// `test_messagelessAsserts`.
        #[test]
        fn reports_a_messageless_assert() {
            let f = Fixture::parse("void f(bool b) { assert(b); }");
            let lints = f.run(&one_rule("prefer_asserts_with_message"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Missing a message in an assert.");
            assert_eq!(lints[0].range.start, 17);
        }

        #[test]
        fn ignores_an_assert_with_message() {
            let f = Fixture::parse("void f(bool b) { assert(b, 'must hold'); }");
            assert!(f.run(&one_rule("prefer_asserts_with_message")).is_empty());
        }
    }
}

pub(crate) mod throw_in_finally {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/throw_in_finally.dart`
    /// with the shared `ControlFlowInFinallyBlockReporter`
    /// (control_flow_in_finally.dart:42-90) — a `throw` whose nearest
    /// enclosing [`NodeKind::TryStatement`] has a `finally` block and whose
    /// chain to it stays inside that block reports, unless the throw sits
    /// inside a nested function body within the block (the enabler face:
    /// the throw is then that closure's own control flow).
    /// Message: `diagnostic.g.dart` `throwInFinally`.
    ///
    /// dartq face: the [`NodeKind::TryStatement`] finally keyword is
    /// payload word 0 (`TOKEN_NONE` when absent) and the finally block
    /// tiles as the node's last child, so the chain test is the
    /// try-statement child on the path being that last child.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "throw_in_finally",
    &[NodeKind::ThrowExpression],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let path = c.path;
        let Some(ts_pos) = path
            .iter()
            .rposition(|&n| c.kind(n) == NodeKind::TryStatement)
        else {
            return;
        };
        let try_statement = path[ts_pos];
        if c.payload(try_statement).first().copied() == Some(NONE_TOKEN) {
            return;
        }
        // The node's chain must pass through the finally block (the last
        // child of the try statement).
        let Some(&branch) = path.get(ts_pos + 1) else {
            return;
        };
        if c.collect_direct(try_statement).last() != Some(&branch) {
            return;
        }
        // Enabler: a nested function body between the try statement and the
        // throw keeps the throw inside that closure's own control flow.
        let enabler = path[ts_pos + 1..path.len() - 1].iter().any(|&n| {
            matches!(
                c.kind(n),
                NodeKind::FunctionExpression
                    | NodeKind::NamedFunctionExpression
                    | NodeKind::LocalFunctionDeclaration
            )
        });
        if !enabler {
            let range = c.utf16_span(node);
            c.report(range, &["throw"]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/throw_in_finally_test.dart` `test_throwInFinally`.
        /// The upstream `reportAtNode` lands at the `ThrowExpression` head —
        /// the `throw` keyword (offset 28 in this fixture).
        #[test]
        fn reports_a_throw_in_finally() {
            let f = Fixture::parse("void f() { try {} finally { throw 1; } }");
            let lints = f.run(&one_rule("throw_in_finally"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Use of 'throw' in a 'finally' block.");
            assert_eq!(lints[0].range.start, 28);
        }

        #[test]
        fn ignores_a_throw_in_try_block() {
            let f = Fixture::parse("void f() { try { throw 1; } finally {} }");
            assert!(f.run(&one_rule("throw_in_finally")).is_empty());
        }

        /// The closure enabler: a throw inside a nested function body of the
        /// finally block is that closure's own control flow.
        #[test]
        fn ignores_a_throw_inside_a_closure_of_finally() {
            let f = Fixture::parse("void f() { try {} finally { g(() { throw 1; }); } }");
            assert!(f.run(&one_rule("throw_in_finally")).is_empty());
        }
    }
}

pub(crate) mod unnecessary_breaks {
    use super::{Check, NodeKind, RuleSpec};

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_breaks.dart`
    /// — a labeled-`break`-free `break` that is the last of more than one
    /// statement in a switch member reports (`_Visitor.visitBreakStatement`:
    /// a sole statement keeps the break, the implicit fall-through of the
    /// pattern cases covers the rest). The official registers only with the
    /// `patterns` feature on. Message: `diagnostic.g.dart`
    /// `unnecessaryBreaks`.
    ///
    /// dartq face: the [`NodeKind::SwitchCase`] statements tile after the
    /// matched expressions (payload word 2 counts them) and the optional
    /// `when` clause, so the last-statement test is a direct-child walk. A
    /// labeled break carries its label identifier as a child and is
    /// exempted by the label scan.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_breaks",
    &[NodeKind::Break],
    check,
);

    fn check(c: &mut Check<'_>) {
        if !c.unit.features.patterns {
            return;
        }
        let node = c.node();
        // Labeled breaks transfer control and never report.
        if c.collect_direct(node)
            .iter()
            .any(|&n| c.kind(n) == NodeKind::Identifier)
        {
            return;
        }
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if c.kind(parent) != NodeKind::SwitchCase {
            return;
        }
        let expressions = c.payload(parent).get(2).copied().unwrap_or(0) as usize;
        let is_last = {
            let kids = c.direct_children(parent);
            if expressions >= kids.len() {
                false
            } else {
                let mut statements = expressions;
                if c.kind(kids[statements]) == NodeKind::SwitchCaseWhenClause {
                    statements += 1;
                }
                // A sole statement keeps its break.
                kids.len() - statements > 1 && kids[kids.len() - 1] == node
            }
        };
        if is_last {
            let range = c.utf16_span(node);
            c.report(range, &[]);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/unnecessary_breaks_test.dart` `test_breakOverReported`
        /// (legacy case face): the trailing break of a multi-statement case.
        #[test]
        fn reports_a_trailing_break() {
            let f = Fixture::parse("void f(int x) { switch (x) { case 1: g(); break; } }");
            let lints = f.run(&one_rule("unnecessary_breaks"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Unnecessary 'break' statement.");
        }

        #[test]
        fn ignores_a_sole_break() {
            let f = Fixture::parse("void f(int x) { switch (x) { case 1: break; } }");
            assert!(f.run(&one_rule("unnecessary_breaks")).is_empty());
        }

        #[test]
        fn ignores_a_break_outside_a_switch_case() {
            let f = Fixture::parse("void f() { while (true) { break; } }");
            assert!(f.run(&one_rule("unnecessary_breaks")).is_empty());
        }
    }
}

pub(crate) mod unnecessary_final {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, utf16_token_span};

    /// The two official message variants under one name
    /// (`diagnostic.g.dart` `unnecessaryFinalWithoutType` /
    /// `unnecessaryFinalWithType`): same `problemMessage`, the correction
    /// keys on the written type's presence.
    const PROBLEM: &str = "Local variables should not be marked as 'final'.";
    const WITHOUT_TYPE: &str = "Replace 'final' with 'var'.";
    const WITH_TYPE: &str = "Remove the 'final'.";

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_final.dart`
    /// — a `final` on a local variable declaration, a formal parameter, a
    /// for-each loop variable, or a declared variable pattern reports at
    /// the keyword (`MultiAnalysisRule`: four registered processors). The
    /// official's `visitFormalParameterList` skips every parameter when
    /// `Feature.primary_constructors` is enabled
    /// (unnecessary_final.dart:64-68 — shipped `enabledIn: 3.13.0`), the
    /// settled 3.13+ posture where the parser already rejects
    /// `final`/`var` on ordinary formals (`extraneous_modifier`); the
    /// parameter faces re-arm under a pre-3.13 language version. Faces:
    ///
    /// - local declaration ([`NodeKind::VariablesDeclaration`] in statement
    ///   position — fields and for-parts rows are the official's unvisited
    ///   faces and are excluded by parent kind);
    /// - formal parameters (both kinds; the keyword sits in the bounded
    ///   modifier run after metadata, the written type is the token gap
    ///   between the keyword and the name, with `this`/`super` receivers
    ///   not counting as type);
    /// - the for-each loop variable ([`NodeKind::ForInParts`] keyword word;
    ///   the pattern form takes the explicit without-type variant);
    /// - declared variable patterns (own keyword, else the nearest
    ///   [`NodeKind::PatternVarDeclStatement`] keyword — the official
    ///   `thisOrAncestorOfType` walk).
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_final",
    &[
            NodeKind::VariablesDeclaration,
            NodeKind::FormalParameter,
            NodeKind::FunctionTypedFormalParameter,
            NodeKind::ForInParts,
            NodeKind::DeclaredVariablePattern,
        ],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        match c.kind(node) {
            NodeKind::VariablesDeclaration => local_declaration(c, node),
            NodeKind::FormalParameter => parameter(c, node),
            NodeKind::FunctionTypedFormalParameter => function_typed_parameter(c, node),
            NodeKind::ForInParts => for_in_parts(c, node),
            NodeKind::DeclaredVariablePattern => declared_pattern(c, node),
            _ => {}
        }
    }

    /// Reports at `keyword` with the variant keyed on the written type.
    fn report_at(c: &mut Check<'_>, keyword: u32, with_type: bool) {
        c.report_texts(
            utf16_token_span(c, keyword),
            PROBLEM,
            if with_type { WITH_TYPE } else { WITHOUT_TYPE },
            &[],
        );
    }

    /// Face 1: the local declaration row. Field rows (`Fields` /
    /// `TopLevelFields` parents) and for-parts rows (`ForIn` /
    /// `ForStatement` parents) are the official's unvisited faces.
    fn local_declaration(c: &mut Check<'_>, node: u32) {
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        if matches!(
            c.kind(parent),
            NodeKind::Fields | NodeKind::TopLevelFields | NodeKind::ForIn | NodeKind::ForStatement
        ) {
            return;
        }
        let Some(keyword) = crate::rules::variables_declaration_keyword(c, node) else {
            return;
        };
        if c.lexeme(keyword) != "final" {
            return;
        }
        // The written type fills the token gap between the keyword and the
        // first declarator name (`final int x` vs `final x`).
        let Some(name) = c
            .collect_direct(node)
            .first()
            .copied()
            .map(|first| c.head_token(first))
        else {
            return;
        };
        report_at(c, keyword, name > keyword + 1);
    }

    /// The `final` keyword inside the bounded modifier run that opens a
    /// parameter (the official begin token is the first token after
    /// metadata, so the run starts at the span head).
    fn modifier_run_final(c: &Check<'_>, node: u32) -> Option<u32> {
        let mut token = c.head_token(node);
        for _ in 0..4 {
            match c.lexeme(token) {
                "final" => return Some(token),
                "required" | "covariant" | "var" | "const" => token += 1,
                _ => return None,
            }
        }
        None
    }

    /// Face 2: the simple formal parameter (covers `super` formals; a
    /// receiver `this`/`super` in the gap is not a type). The official
    /// visitor's parameter loop skips every parameter while
    /// `primary_constructors` is enabled (unnecessary_final.dart:66-68).
    fn parameter(c: &mut Check<'_>, node: u32) {
        if c.unit.features.primary_constructors {
            return;
        }
        let Some(keyword) = modifier_run_final(c, node) else {
            return;
        };
        let name = c.payload(node).get(1).copied().unwrap_or(NONE_TOKEN);
        if name == NONE_TOKEN {
            return;
        }
        report_at(c, keyword, written_type_in_gap(c, keyword, name));
    }

    /// Whether any non-receiver token sits in `keyword + 1 .. name`.
    fn written_type_in_gap(c: &Check<'_>, keyword: u32, name: u32) -> bool {
        (keyword + 1..name).any(|t| !matches!(c.lexeme(t), "this" | "super" | "."))
    }

    /// Face 2': the function-typed parameter — the written return type is
    /// the gap between the keyword and the `FunctionName` head. Same
    /// parameter-loop gate as face 2 (the official visitor handles both
    /// kinds in one `visitFormalParameterList`).
    fn function_typed_parameter(c: &mut Check<'_>, node: u32) {
        if c.unit.features.primary_constructors {
            return;
        }
        let Some(keyword) = modifier_run_final(c, node) else {
            return;
        };
        let Some(name) = c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FunctionName)
        else {
            return;
        };
        let name_head = c.head_token(name);
        report_at(c, keyword, name_head > keyword + 1);
    }

    /// Face 3: the for-each loop variable — two official shapes:
    ///
    /// - `ForEachPartsWithDeclaration` (`for (final int x in xs)`): the loop
    ///   variable tiles as a [`NodeKind::VariablesDeclaration`] sibling of
    ///   the parts row inside `ForIn` (the parts row seals AFTER the
    ///   iterable, so adjacency is not the read — the declaration is the
    ///   parts row's only `VariablesDeclaration` sibling); the keyword and
    ///   the written-type gap are read off it like the local-declaration
    ///   face. The parts' own payload word 2 is absent here — it is the
    ///   pattern form's word only.
    /// - `ForEachPartsWithPattern` (`for (final (a, b) in xs)`): payload word
    ///   2 carries `pattern_keyword`; the official passes the without-type
    ///   variant unconditionally.
    fn for_in_parts(c: &mut Check<'_>, node: u32) {
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let kids = c.collect_direct(parent);
        if let Some(&decl) = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
        {
            // The declaration form: `loopVariable.keyword` /
            // `loopVariable.isFinal` / `loopVariable.type`
            // (unnecessary_final.dart:88-93).
            let Some(keyword) = crate::rules::variables_declaration_keyword(c, decl) else {
                return;
            };
            if c.lexeme(keyword) != "final" {
                return;
            }
            let Some(name) = c
                .collect_direct(decl)
                .first()
                .copied()
                .map(|first| c.head_token(first))
            else {
                return;
            };
            report_at(c, keyword, name > keyword + 1);
            return;
        }
        let keyword = c.payload(node).get(2).copied().unwrap_or(NONE_TOKEN);
        if keyword == NONE_TOKEN || c.lexeme(keyword) != "final" {
            return;
        }
        report_at(c, keyword, false);
    }

    /// Face 4: the declared variable pattern — own keyword (the span head
    /// when `final`), else the enclosing pattern-declaration statement, the
    /// official `thisOrAncestorOfType<PatternVariableDeclaration>` walk.
    ///
    /// dartq face: [`NodeKind::PatternVarDeclStatement`] is a CHILDLESS
    /// sibling tile, never an ancestor — the pattern rows (and this
    /// variable) sit beside it in the enclosing statement container's
    /// window, while the marker's own span covers `final <pattern> =
    /// <expr>;` whole. The walk becomes a sibling scan: the marker whose
    /// token extent covers this variable's head owns its keyword (the
    /// marker's span head). The code is always the with-type variant: the
    /// official keys it on `matchedValueType`, non-null on every resolved
    /// run.
    fn declared_pattern(c: &mut Check<'_>, node: u32) {
        let head = c.head_token(node);
        if c.lexeme(head) == "final" {
            report_at(c, head, true);
            return;
        }
        let Some(parent) = c.ancestor(1) else {
            return;
        };
        let keyword = c.children(parent).find(|&row| {
            c.kind(row) == NodeKind::PatternVarDeclStatement
                && c.unit.ast.token_range(row).contains(&head)
                && c.lexeme(c.head_token(row)) == "final"
        });
        if let Some(statement) = keyword {
            report_at(c, c.head_token(statement), true);
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/unnecessary_final_test.dart` `test_withType`.
        #[test]
        fn reports_a_typed_local() {
            let f = Fixture::parse("void f() { final int x = 1; print(x); }");
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
            assert_eq!(
                lints[0].message,
                "Local variables should not be marked as 'final'."
            );
        }

        #[test]
        fn reports_an_untyped_local() {
            let f = Fixture::parse("void f() { final x = 1; print(x); }");
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
        }

        /// Fields are the official's unvisited face.
        #[test]
        fn ignores_a_field() {
            let f = Fixture::parse("class A { final int x = 0; }");
            assert!(f.run(&one_rule("unnecessary_final")).is_empty());
        }

        /// `test_parameter` face — the official visitor's parameter loop is
        /// gated on `primary_constructors` (unnecessary_final.dart:66-68):
        /// the shipped feature (the default) skips parameters — the parser
        /// already rejects `final`/`var` on ordinary formals there — and a
        /// pre-3.13 feature set re-arms the row (the upstream tests pin the
        /// lint face under `// @dart=3.12` only).
        #[test]
        fn parameter_face_is_primary_constructors_gated() {
            let mut f = Fixture::parse("void f(final int x) {}");
            assert!(f.run(&one_rule("unnecessary_final")).is_empty());
            f.features.primary_constructors = false;
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
        }

        #[test]
        fn ignores_a_var_parameter() {
            let f = Fixture::parse("void f(var x) {}");
            assert!(f.run(&one_rule("unnecessary_final")).is_empty());
        }

        /// `test_forLoop` face: the for-each loop variable.
        #[test]
        fn reports_a_for_each_loop_variable() {
            let f = Fixture::parse("void f() { for (final x in [1]) print(x); }");
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
        }

        /// The C-style for initializer row is the official's unvisited
        /// face (`ForPartsWithDeclarations`).
        #[test]
        fn ignores_a_c_style_for_initializer() {
            let f = Fixture::parse("void f() { for (final int i = 0; i < 1; i++) {} }");
            assert!(f.run(&one_rule("unnecessary_final")).is_empty());
        }

        /// `test_patternVariableDeclaration` face: the declaration keyword
        /// reports once per declared pattern variable — but both variables
        /// of `final (a, b) = …` anchor at the one statement keyword, and
        /// the oracle's DiagnosticSet dedup (code, offset, length, message)
        /// collapses the identical rows: the oracle-of-record emits ONE
        /// row (probed 2026-10-18 on 3.13.1), and so does dartq.
        #[test]
        fn reports_pattern_declaration_variables() {
            let f = Fixture::parse("void f() { final (a, b) = (1, 2); print(a + b); }");
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
        }

        /// A case-pattern variable reports at its own keyword.
        #[test]
        fn reports_a_case_pattern_variable() {
            let f = Fixture::parse("void f(int x) { switch (x) { case final y: print(y); } }");
            let lints = f.run(&one_rule("unnecessary_final"));
            assert_eq!(lints.len(), 1);
        }
    }
}

pub(crate) mod one_member_abstracts {
    use super::{Check, NodeKind, RuleSpec};
    use crate::rules::NONE_TOKEN;

    /// `NodeHeader.flags` modifier-mask bit for the class-family `abstract`
    /// modifier (the member family shares positions; bridge `mod_bits`).
    const ABSTRACT_BIT: u16 = 1 << 2;

    /// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/one_member_abstracts.dart`
    /// — an abstract class with no `extends`/`with`/`implements` clauses,
    /// no fields, and exactly one abstract *method* reports at the class
    /// name with the method name as the argument. The member tally is
    /// version-bound: the oracle-of-record (Dart 3.13.x) counts accessor
    /// members toward it — a getter beside the lone method lifts the
    /// count to two and the class stays silent — while the 3.15-main
    /// element model's `element.methods` excludes accessors. The tally
    /// follows the 3.13.x oracle; revisit on oracle upgrade. Every
    /// queried fact is syntactic here: the clause words are the
    /// declaration payload, the fields are the [`NodeKind::Fields`]
    /// members, a member is a [`NodeKind::Method`] row (the get/set word
    /// marks accessors), and an abstract method's body is an
    /// [`NodeKind::EmptyBody`]. The official `isAugmentation`/null-element
    /// gates are wave-inert: `augment` is gated off at the scanner. The
    /// rule is `deprecated` upstream since 3.13 but stays registered and
    /// fires. Message: `diagnostic.g.dart` `oneMemberAbstracts`.
    pub(crate) static SPEC: RuleSpec = crate::spec!(
    "one_member_abstracts",
    &[NodeKind::ClassDeclaration],
    check,
);

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        if dartq_ast::mod_flags(c.unit.ast, node) & ABSTRACT_BIT == 0 {
            return;
        }
        // Payload words 1..3 are the `extends`/`with`/`implements` clause
        // keywords: any present clause keeps the class.
        let payload = c.payload(node);
        if payload.len() < 4 || payload[1..4].iter().any(|&word| word != NONE_TOKEN) {
            return;
        }
        let members = crate::rules::names::class_like_members(c, node);
        if members
            .iter()
            .any(|&member| c.kind(member) == NodeKind::Fields)
        {
            return;
        }
        // Version-bound tally: the oracle-of-record (Dart 3.13.x, the
        // pre-element-model visitor) counts accessor members in the member
        // tally — a getter beside the lone abstract method lifts the count
        // to two and the class stays silent. The 3.15-main element model
        // reads `element.methods` (accessors excluded); revisit this count
        // when the oracle-of-record upgrades. The reported member must
        // still be a plain abstract method: an accessor-only class holds
        // one member yet has no method to promote.
        let tally: Vec<u32> = members
            .into_iter()
            .filter(|&member| c.kind(member) == NodeKind::Method)
            .collect();
        let [method] = tally[..] else {
            return;
        };
        if c.payload(method).get(1).copied() != Some(NONE_TOKEN) {
            return;
        }
        // `method.isAbstract`: the body is the empty `;` form.
        let Some(body) = super::last_child(c, method) else {
            return;
        };
        if c.kind(body) != NodeKind::EmptyBody {
            return;
        }
        // The class name is the enclosing `TopLevelDeclaration` wrapper's
        // first `Identifier` child; the method name is
        // the method row's span head (`begin_method` anchors at the name).
        let name = c.ancestor(1).and_then(|wrapper| {
            c.collect_direct(wrapper)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        });
        let Some(name) = name else {
            return;
        };
        let range = c.utf16_span(name);
        let arg = c.lexeme(c.head_token(method));
        c.report(range, &[arg]);
    }

    #[cfg(test)]
    mod tests {
        use crate::testsupport::{Fixture, one_rule};

        /// `test/rules/one_member_abstracts_test.dart` `test_isAbstract`.
        #[test]
        fn reports_a_one_member_abstract_class() {
            let f = Fixture::parse("abstract class A { void m(); }");
            let lints = f.run(&one_rule("one_member_abstracts"));
            assert_eq!(lints.len(), 1);
            assert_eq!(lints[0].message, "Unnecessary use of an abstract class.");
        }

        #[test]
        fn ignores_a_concrete_class() {
            let f = Fixture::parse("class A { void m() {} }");
            assert!(f.run(&one_rule("one_member_abstracts")).is_empty());
        }

        #[test]
        fn ignores_an_abstract_class_with_a_field() {
            let f = Fixture::parse("abstract class A { int x = 0; void m(); }");
            assert!(f.run(&one_member_abstracts_set()).is_empty());
        }

        #[test]
        fn ignores_an_abstract_class_with_extends() {
            let f = Fixture::parse("class B {} abstract class A extends B { void m(); }");
            assert!(f.run(&one_rule("one_member_abstracts")).is_empty());
        }

        /// Accessors count toward the member tally but are not the
        /// reported method: an accessor-only class has one member yet no
        /// plain method, so it stays silent.
        #[test]
        fn ignores_an_accessor_only_abstract_class() {
            let f = Fixture::parse("abstract class A { int get x; }");
            assert!(f.run(&one_member_abstracts_set()).is_empty());
        }

        /// Version-bound face: the oracle-of-record (Dart 3.13.x) counts
        /// the getter in the member tally — a getter beside the lone
        /// abstract method lifts the count to two and the class stays
        /// silent. (3.15-main's `element.methods` excludes accessors and
        /// would report; revisit on oracle upgrade.)
        #[test]
        fn accessor_counts_in_the_member_tally() {
            let f = Fixture::parse("abstract class A { int get x; void m(); }");
            assert!(f.run(&one_member_abstracts_set()).is_empty());
        }

        fn one_member_abstracts_set() -> crate::registry::EnableSet {
            one_rule("one_member_abstracts")
        }
    }
}
