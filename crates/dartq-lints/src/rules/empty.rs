//! The emptiness family: `empty_statements`, `empty_constructor_bodies`,
//! `empty_catches`, `avoid_empty_else`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};
use crate::rules::{first_child, is_just_underscores, last_child};

/// The last token index of `node`'s span.
fn last_token_of(c: &Check<'_>, node: u32) -> u32 {
    c.unit.ast.token_range(node).end - 1
}

pub(crate) mod empty_statements {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_statements.dart:33-53` —
    /// report every `EmptyStatement` except one that "defines semantics":
    /// the last statement of a switch pattern case whose statements are all
    /// empty (the `case _: ;` idiom, `empty_statements.dart:34-46`).
    /// Message: `diagnostic.g.dart` `emptyStatements`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "empty_statements",
        group: Group::Recommended,
        problem: "Unnecessary empty statement.",
        correction: "Try removing the empty statement.",
        kinds: &[NodeKind::EmptyStatement],
        check,
    };

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
    /// expressions ([`NodeKind::SwitchCase`] payload word 2 counts them,
    /// collapse map §3.4) and the optional `when` clause.
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_constructor_bodies.dart:34-61`
    /// — a constructor (or primary constructor body) whose block body is
    /// empty AND whose `}` carries no preceding comments reports at the
    /// block. The official factory exclusion is structural here: factory
    /// methods land as [`NodeKind::Factory`] nodes, not
    /// [`NodeKind::Constructor`] (collapse map §3.3 FactoryEnd row), so no
    /// `factoryKeyword` test is needed. Message: `diagnostic.g.dart`
    /// `emptyConstructorBodies`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "empty_constructor_bodies",
        group: Group::Recommended,
        problem: "Empty constructor bodies should be written using a ';' rather than '{}'.",
        correction: "Try replacing the constructor body with ';'.",
        kinds: &[NodeKind::Constructor, NodeKind::PrimaryConstructorBody],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some(body) = super::last_child(c, node) else {
            return;
        };
        if c.kind(body) != NodeKind::Block {
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/empty_catches.dart:31-45` — an
    /// empty catch block with no comments before `}` reports at the block,
    /// unless the exception parameter is named with bare underscores
    /// (`catch (_)` is a deliberate discard, empty_catches.dart:33-38).
    /// Message: `diagnostic.g.dart` `emptyCatches`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "empty_catches",
        group: Group::Core,
        problem: "Empty catch block.",
        correction: "Try adding statements to the block, adding a comment to the block, or removing the 'catch' clause.",
        kinds: &[NodeKind::CatchClause],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // The exception formal is the first child; its name token is payload
        // word 1 (collapse map §3.3 FormalParameter row).
        if let Some(exception) = super::first_child(c, node) {
            if c.kind(exception) == NodeKind::FormalParameter {
                if let Some(&name) = c.payload(exception).get(1) {
                    if super::is_just_underscores(c.lexeme(name)) {
                        return;
                    }
                }
            }
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
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/avoid_empty_else.dart:32-41` — an
    /// `else` branch that is a non-synthetic empty statement reports at the
    /// empty statement. Message: `diagnostic.g.dart` `avoidEmptyElse`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "avoid_empty_else",
        group: Group::Core,
        problem: "Empty statements are not allowed in an 'else' clause.",
        correction: "Try removing the empty statement or removing the else clause.",
        kinds: &[NodeKind::IfStatement],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        // Else present iff the `else` payload token is set (collapse map
        // §3.4 IfStatement row); the else statement is the last child.
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

    /// `test/rules/empty_constructor_bodies_test.dart` `test_emptyBlock`.
    #[test]
    fn empty_constructor_bodies_reports_an_empty_block() {
        let source = "class A { A() {} }";
        // class0 A1 {2 A3 (4 )5 {6 }7 }8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Constructor, 3);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, 4);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::Constructor, 8);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 9);
            }),
        );
        let lints = f.run(&one_rule("empty_constructor_bodies"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Empty constructor bodies should be written using a ';' rather than '{}'."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 6));
    }

    /// `test/rules/empty_constructor_bodies_test.dart` `test_comment` — a
    /// comment before `}` keeps the block.
    #[test]
    fn empty_constructor_bodies_comment_keeps_the_block() {
        let source = "class A { A() { /* c */ } }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Constructor, 3);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, 4);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.end(NodeKind::Block, 8);
                b.end(NodeKind::Constructor, 8);
                b.end(NodeKind::DeclarationBody, 9);
                b.end(NodeKind::ClassDeclaration, 9);
            }),
        );
        assert!(f.run(&one_rule("empty_constructor_bodies")).is_empty());
    }

    #[test]
    fn empty_constructor_bodies_non_empty_block_passes() {
        let source = "class A { A() { x; } }";
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ClassDeclaration, 0);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 1, 2);
                b.begin(NodeKind::DeclarationBody, 2);
                b.begin(NodeKind::Constructor, 3);
                b.patch(0, 0);
                b.patch(1, NONE);
                b.patch(2, 4);
                b.patch(3, NONE);
                identifier(b, 3, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.end(NodeKind::FormalParameters, 6);
                b.begin(NodeKind::Block, 6);
                b.begin(NodeKind::ExpressionStatement, 7);
                b.leaf_span(NodeKind::Send, 7, 9);
                b.end(NodeKind::ExpressionStatement, 9);
                b.end(NodeKind::Block, 10);
                b.end(NodeKind::Constructor, 10);
                b.end(NodeKind::DeclarationBody, 11);
                b.end(NodeKind::ClassDeclaration, 11);
            }),
        );
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
