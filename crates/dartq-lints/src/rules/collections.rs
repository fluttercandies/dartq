//! The collection-literal family: `prefer_inlined_adds`,
//! `prefer_spread_collections`.
//!
//! Cascade shape (collapse map §3.5 CascadeEnd): a [`NodeKind::Cascade`]
//! node's direct children are its sections; the target expression is the
//! sibling preceding the Cascade among the parent's children (the cascade
//! operator is the token right after the target's last token). A cascaded
//! invocation is therefore `Send` → `CascadeAccess` → `Cascade`.

use dartq_ast::NodeKind;

use crate::engine::Check;
use crate::registry::{Group, RuleSpec};

/// The direct sibling immediately before `id` (the cascade target slot).
fn sibling_before(c: &Check<'_>, id: u32) -> Option<u32> {
    // `id` is always on the current ancestor path (a cascade above the
    // dispatch node); its parent is the path entry right above it.
    let pos = c.path.iter().position(|&n| n == id)?;
    let parent = *c.path.get(pos.checked_sub(1)?)?;
    let kids = c.collect_direct(parent);
    let kid_pos = kids.iter().position(|&kid| kid == id)?;
    kid_pos.checked_sub(1).map(|prev| kids[prev])
}

/// The `(method name node, arguments node)` of a `Send` invocation: the
/// callee expression nodes precede the argument list (collapse map §3.5
/// `PositionalArgumentHandle` D1).
fn invocation_parts(c: &Check<'_>, send: u32) -> Option<(u32, u32)> {
    let kids = c.collect_direct(send);
    let args_pos = kids
        .iter()
        .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments | NodeKind::NoArguments))?;
    let args = kids[args_pos];
    let name = *kids.get(args_pos.checked_sub(1)?)?;
    Some((name, args))
}

/// Whether the cascade section `access` opens the cascade
/// (`sections.first == invocation`, prefer_inlined_adds.dart:47-52).
fn is_first_section(c: &Check<'_>, cascade: u32, access: u32) -> bool {
    c.collect_direct(cascade).first() == Some(&access)
}

/// The syntactic const-context approximation (the official
/// `target.inConstantContext` is constant-evaluated): an explicit `const`
/// wrapper or a `const` variable declaration on the ancestor path.
fn in_const_context(c: &Check<'_>, above: usize) -> bool {
    (0..above).any(|depth| match c.ancestor(depth) {
        Some(a) => match c.kind(a) {
            NodeKind::ConstExpression | NodeKind::ConstLiteral => true,
            NodeKind::VariablesDeclaration => c
                .payload(a)
                .first()
                .copied()
                .is_some_and(|kw| kw != crate::rules::NONE_TOKEN && c.lexeme(kw) == "const"),
            _ => false,
        },
        None => false,
    })
}

/// The shared gate: a first cascaded `add`/`addAll` section with one
/// argument on a list-literal target. Returns the method-name node and the
/// argument list node.
fn cascaded_add(c: &Check<'_>, send: u32, name: &str) -> Option<(u32, u32)> {
    let (method, args) = invocation_parts(c, send)?;
    if c.kind(method) != NodeKind::Identifier || c.lexeme(c.head_token(method)) != name {
        return None;
    }
    // `invocation.isCascaded` (:27-29).
    let access = c.ancestor(1)?;
    if c.kind(access) != NodeKind::CascadeAccess {
        return None;
    }
    let cascade = c.ancestor(2)?;
    if c.kind(cascade) != NodeKind::Cascade || !is_first_section(c, cascade, access) {
        return None;
    }
    // The cascade target must be a list literal (:42-52 / :44-50).
    if !matches!(sibling_before(c, cascade), Some(target) if c.kind(target) == NodeKind::ListLiteral)
    {
        return None;
    }
    // Exactly one argument (:28-29).
    (c.collect_direct(args).len() == 1).then_some((method, args))
}

pub(crate) mod prefer_inlined_adds {
    use super::{Check, Group, NodeKind, RuleSpec, cascaded_add};

    /// Official: `pkg/linter/lib/src/rules/prefer_inlined_adds.dart:22-58` —
    /// the first cascaded `add`/`addAll` on a list literal with one argument
    /// reports at the method name; `addAll` carries the multiple-elements
    /// template (only when its argument is a list literal — a spread argument
    /// belongs to `prefer_spread_collections`, :53-55). Message templates:
    /// `diagnostic.g.dart` `preferInlinedAddsMultiple`/`preferInlinedAddsSingle`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_inlined_adds",
        group: Group::Recommended,
        problem: "The addition of multiple list items could be inlined.",
        correction: "Try adding the items to the list literal directly.",
        kinds: &[NodeKind::Send],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some((method, args)) =
            cascaded_add(c, node, "add").or_else(|| cascaded_add(c, node, "addAll"))
        else {
            return;
        };
        let add_all = c.lexeme(c.head_token(method)) == "addAll";
        if add_all {
            // A list-literal argument is inlined here; anything else is the
            // spread rule's finding (:53-55).
            if c.kind(c.collect_direct(args)[0]) != NodeKind::ListLiteral {
                return;
            }
        }
        let range = c.utf16_span(method);
        if add_all {
            c.report_texts(
                range,
                "The addition of multiple list items could be inlined.",
                "Try adding the items to the list literal directly.",
                &[],
            );
        } else {
            c.report_texts(
                range,
                "The addition of a list item could be inlined.",
                "Try adding the item to the list literal directly.",
                &[],
            );
        }
    }
}

pub(crate) mod prefer_spread_collections {
    use super::{Check, Group, NodeKind, RuleSpec, cascaded_add, in_const_context};

    /// Official: `pkg/linter/lib/src/rules/prefer_spread_collections.dart:22-55`
    /// — the first cascaded `addAll` on a non-constant list literal whose
    /// single argument is NOT a list literal (that form is
    /// `prefer_inlined_adds`'s) reports at the method name. Message:
    /// `diagnostic.g.dart` `preferSpreadCollections`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_spread_collections",
        group: Group::Recommended,
        problem: "The addition of multiple elements could be inlined.",
        correction: "Try using the spread operator ('...') to inline the addition.",
        kinds: &[NodeKind::Send],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let Some((method, args)) = cascaded_add(c, node, "addAll") else {
            return;
        };
        // `target.inConstantContext` (:46-49) — syntactic approximation on
        // the ancestor path.
        if in_const_context(c, c.path.len()) {
            return;
        }
        if c.kind(c.collect_direct(args)[0]) == NodeKind::ListLiteral {
            // Handled by prefer_inlined_adds (:50-53).
            return;
        }
        let range = c.utf16_span(method);
        c.report(range, &[]);
    }
}

#[cfg(test)]
mod tests {
    use dartq_ast::NodeKind;

    use crate::testsupport::{Fixture, NONE, identifier, one_rule, unit_of, utf16_at};

    /// `[]..addAll([1, 2]);` — a cascaded list-literal addAll reports with
    /// the multiple template (`test/rules/prefer_inlined_adds_test.dart`).
    #[test]
    fn inlined_adds_reports_addall_of_list_literal() {
        let source = "[]..addAll([1, 2]);";
        // []0 ..1 addAll2 (3 [4 15 ,6 27 ]8 )9 ;10
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.end(NodeKind::ListLiteral, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 1);
                b.patch(0, 1);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.begin(NodeKind::ListLiteral, 4);
                b.patch(0, NONE);
                b.leaf(NodeKind::LiteralInt, 5);
                b.leaf(NodeKind::LiteralInt, 7);
                b.end(NodeKind::ListLiteral, 9);
                b.end(NodeKind::Arguments, 10);
                b.end(NodeKind::Send, 10);
                b.end(NodeKind::CascadeAccess, 10);
                b.end(NodeKind::Cascade, 10);
                b.end(NodeKind::ExpressionStatement, 11);
            }),
        );
        let lints = f.run(&one_rule("prefer_inlined_adds"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The addition of multiple list items could be inlined."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
        assert!(f.run(&one_rule("prefer_spread_collections")).is_empty());
    }

    /// `[]..add(1);` — the single template.
    #[test]
    fn inlined_adds_reports_add() {
        let source = "[]..add(1);";
        // []0 ..1 add2 (3 14 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.end(NodeKind::ListLiteral, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 1);
                b.patch(0, 1);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::CascadeAccess, 6);
                b.end(NodeKind::Cascade, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        let lints = f.run(&one_rule("prefer_inlined_adds"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "The addition of a list item could be inlined."
        );
    }

    /// A non-first section is skipped (`sections.first == invocation`).
    #[test]
    fn inlined_adds_second_section_passes() {
        let source = "[]..addAll(1, 2)..add(3);";
        // []0 ..1 addAll2 (3 14 ,5 26 )7 ..8 add9 (10 311 )12 ;13
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.end(NodeKind::ListLiteral, 1);
                b.begin(NodeKind::Cascade, 1);
                // Section 1: two arguments — passes the count gate.
                b.begin(NodeKind::CascadeAccess, 1);
                b.patch(0, 1);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.leaf(NodeKind::LiteralInt, 6);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::CascadeAccess, 8);
                // Section 2: one argument but not the first section.
                b.begin(NodeKind::CascadeAccess, 8);
                b.patch(0, 8);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 9);
                identifier(b, 9, 10);
                b.begin(NodeKind::Arguments, 10);
                b.leaf(NodeKind::LiteralInt, 11);
                b.end(NodeKind::Arguments, 13);
                b.end(NodeKind::Send, 13);
                b.end(NodeKind::CascadeAccess, 13);
                b.end(NodeKind::Cascade, 13);
                b.end(NodeKind::ExpressionStatement, 14);
            }),
        );
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
    }

    /// A non-list target passes.
    #[test]
    fn inlined_adds_non_list_target_passes() {
        let source = "ts..add(1);";
        // ts0 ..1 add2 (3 14 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.leaf_span(NodeKind::Send, 0, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 1);
                b.patch(0, 1);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::CascadeAccess, 6);
                b.end(NodeKind::Cascade, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
    }

    /// `[]..addAll(b);` — spread argument: the spread rule reports, the
    /// inlined rule stays silent (list-literal gate, :53-55).
    #[test]
    fn spread_reports_addall_of_expression() {
        let source = "[]..addAll(b);";
        // []0 ..1 addAll2 (3 b4 )5 ;6
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.end(NodeKind::ListLiteral, 1);
                b.begin(NodeKind::Cascade, 1);
                b.begin(NodeKind::CascadeAccess, 1);
                b.patch(0, 1);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                identifier(b, 4, 5);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::CascadeAccess, 6);
                b.end(NodeKind::Cascade, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        let spread = f.run(&one_rule("prefer_spread_collections"));
        assert_eq!(spread.len(), 1);
        assert_eq!(
            spread[0].message,
            "The addition of multiple elements could be inlined."
        );
        assert!(f.run(&one_rule("prefer_inlined_adds")).is_empty());
    }

    /// A `const` declaration ancestor passes the spread rule (the syntactic
    /// `inConstantContext` approximation).
    #[test]
    fn spread_const_target_passes() {
        let source = "const a = []..addAll(b);";
        // const0 a1 =2 []3 ..4 addAll5 (6 b7 )8 ;9
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::VariablesDeclaration, 0);
                b.patch(0, 0); // the `const` keyword
                b.begin(NodeKind::InitializedIdentifier, 1);
                b.begin(NodeKind::VariableInitializer, 2);
                b.begin(NodeKind::ListLiteral, 3);
                b.patch(0, NONE);
                b.end(NodeKind::ListLiteral, 4);
                b.begin(NodeKind::Cascade, 4);
                b.begin(NodeKind::CascadeAccess, 4);
                b.patch(0, 4);
                b.patch(1, 0);
                b.begin(NodeKind::Send, 5);
                identifier(b, 5, 6);
                b.begin(NodeKind::Arguments, 6);
                identifier(b, 7, 8);
                b.end(NodeKind::Arguments, 9);
                b.end(NodeKind::Send, 9);
                b.end(NodeKind::CascadeAccess, 9);
                b.end(NodeKind::Cascade, 9);
                b.end(NodeKind::VariableInitializer, 9);
                b.end(NodeKind::InitializedIdentifier, 9);
                b.end(NodeKind::VariablesDeclaration, 10);
            }),
        );
        assert!(f.run(&one_rule("prefer_spread_collections")).is_empty());
    }
}
