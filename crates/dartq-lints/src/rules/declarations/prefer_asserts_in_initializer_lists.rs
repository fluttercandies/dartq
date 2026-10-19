use super::{Check, Group, NodeKind, RuleSpec, formal_parameter_rows, modifier_keyword};
use crate::rules::last_child;

/// Official:
/// `pkg/linter/lib/src/rules/prefer_asserts_in_initializer_lists.dart:148-165`
/// — a non-factory constructor's (or primary constructor body's)
/// leading `assert` statements report at the `assert` keyword unless
/// they reference instance state (:151-163: the loop breaks at the
/// first non-assert statement; the `_AssertVisitor` walk
/// :41-100 marks a `this`, an instance method of the class/super
/// chain, or an instance accessor not covered by a field formal
/// parameter). Message: `diagnostic.g.dart`
/// `preferAssertsInInitializerLists`.
///
/// Approximations: the element walk folds to a token scan — a `this`
/// (or `super`) keyword or an identifier token that is not a
/// constructor parameter marks the assert instance-dependent.
/// Under-reports where the official element check would answer
/// foreign: property accesses on parameters (`a.b`), static members,
/// and top-level references all silence the assert here; the
/// class/super-chain membership face (:72-80/:103-125) folds into the
/// same direction.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_asserts_in_initializer_lists",
    group: Group::Standalone,
    problem: "Assert should be in the initializer list.",
    correction: "Try moving the assert to the initializer list.",
    kinds: &[NodeKind::Constructor, NodeKind::PrimaryConstructorBody],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::Constructor => {
            // `declaredElement.isFactory` (:149) — the row anchors at
            // the declared name, so the head modifiers sit before it.
            if modifier_keyword(c, node, "factory").is_some() {
                return;
            }
            let names = parameter_names(c, formal_parameter_rows(c, node));
            check_leading_asserts(c, node, &names);
        }
        NodeKind::PrimaryConstructorBody => {
            // `node.declaration` (:145) — the paired primary
            // constructor carries the parameters.
            let Some(class_row) = container_row(c) else {
                return;
            };
            let Some(primary) = c
                .collect_direct(class_row)
                .into_iter()
                .find(|&member| c.kind(member) == NodeKind::PrimaryConstructor)
            else {
                return;
            };
            let names = parameter_names(c, formal_parameter_rows(c, primary));
            check_leading_asserts(c, node, &names);
        }
        _ => {}
    }
}

/// The leading-assert loop (:151-163): every `Assert` statement before
/// the first other statement reports unless the instance fold fires.
/// The production `BlockBody` parents its statement rows directly —
/// there is no intermediate `Block` row.
fn check_leading_asserts(c: &mut Check<'_>, owner: u32, params: &[&str]) {
    let Some(body) = last_child(c, owner) else {
        return;
    };
    if c.kind(body) != NodeKind::BlockBody {
        return;
    }
    for statement in c.collect_direct(body) {
        if c.kind(statement) != NodeKind::Assert {
            break;
        }
        if need_instance(c, statement, params) {
            continue;
        }
        c.report(
            crate::rules::utf16_token_span(c, c.head_token(statement)),
            &[],
        );
    }
}

/// The `_AssertVisitor` fold (:45-70): a `this`/`super` keyword or an
/// identifier outside the parameter names marks instance access.
fn need_instance(c: &Check<'_>, assert_row: u32, params: &[&str]) -> bool {
    c.unit.ast.token_range(assert_row).any(|token| {
        let Some(t) = c.unit.tokens.get(token as usize) else {
            return false;
        };
        match t.kind {
            dartq_scanner::TokenKind::Identifier => !params.contains(&c.lexeme(token)),
            dartq_scanner::TokenKind::Keyword(_) => {
                matches!(c.lexeme(token), "this" | "super")
            }
            _ => false,
        }
    })
}

/// The written parameter names of the formal rows.
fn parameter_names<'a>(c: &Check<'a>, rows: Vec<u32>) -> Vec<&'a str> {
    rows.into_iter()
        .filter_map(|row| c.payload(row).get(1).copied())
        .filter(|&token| token != crate::rules::NONE_TOKEN)
        .map(|token| c.lexeme(token))
        .collect()
}

/// The class-like container row of the primary body (the paired
/// primary constructor hangs beside it).
fn container_row(c: &Check<'_>) -> Option<u32> {
    c.path[..c.path.len() - 1].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
        )
    })
}
