//! The collection-literal family: `prefer_inlined_adds`,
//! `prefer_spread_collections`, `prefer_collection_literals`,
//! `unnecessary_to_list_in_spreads`.
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

pub(crate) mod prefer_collection_literals {
    use super::{Check, Group, NodeKind, RuleSpec};

    const PROBLEM: &str = "Unnecessary constructor invocation.";
    const CORRECTION: &str = "Try using a collection literal.";

    /// Official: `pkg/linter/lib/src/rules/prefer_collection_literals.dart:41-95`
    /// — no-arg `Map()`/`Set()` constructor invocations (the core types and
    /// their `Linked*`/`Hash*` forms) report; `Set.from([…])`/`Set.of([…])`
    /// with a single list literal reports; `listLiteral.toSet()` reports.
    ///
    /// Approximations (documented): the official pins the types through the
    /// element model (`staticType.isDartCoreMap` etc., :99-105) and exempts
    /// typedef constructors and matching context types (:44-46/:50-53); the
    /// lexical face matches the constructor name, so a user-defined `Map`/`Set`
    /// family class over-reports and the typedef/context exemptions are
    /// dropped. Prefixed callees (`core.Map()`) and `new`/`const` wrappers
    /// report at the inner invocation rather than the whole creation.
    /// Message: `diagnostic.g.dart` `preferCollectionLiterals`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_collection_literals",
        group: Group::Recommended,
        problem: PROBLEM,
        correction: CORRECTION,
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
        let args = c.collect_direct(kids[args_pos]);
        let Some(&callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        match c.kind(callee) {
            // `Map()` / `Set()` — a plain type name with no arguments
            // (:55-59/:68-74).
            NodeKind::Identifier => {
                let name = c.lexeme(c.head_token(callee));
                if !is_collection_type(name) || !args.is_empty() {
                    return;
                }
            }
            // `Set.from([…])` / `Set.of([…])` — one list-literal argument
            // (:75-81).
            NodeKind::Qualified => {
                let qualified = c.collect_direct(callee);
                let (Some(type_name), Some(ctor)) =
                    (qualified.first().copied(), qualified.last().copied())
                else {
                    return;
                };
                if c.kind(type_name) != NodeKind::Identifier
                    || c.kind(ctor) != NodeKind::Identifier
                    || !is_collection_type(c.lexeme(c.head_token(type_name)))
                    || !matches!(c.lexeme(c.head_token(ctor)), "from" | "of")
                {
                    return;
                }
                if args.len() != 1 || c.kind(args[0]) != NodeKind::ListLiteral {
                    return;
                }
            }
            // `['a'].toSet()` (:87-95).
            NodeKind::DotAccess => {
                let access = c.collect_direct(callee);
                let Some(name) = access.last().copied() else {
                    return;
                };
                if c.kind(name) != NodeKind::Identifier || c.lexeme(c.head_token(name)) != "toSet" {
                    return;
                }
                let Some(target) = access.first().copied() else {
                    return;
                };
                if c.kind(target) != NodeKind::ListLiteral {
                    return;
                }
            }
            _ => return,
        }
        let range = c.utf16_span(node);
        c.report_texts(range, PROBLEM, CORRECTION, &[]);
    }

    /// The `Map`/`Set` family names the official type checks accept
    /// (`isDartCoreMap`/`isDartCoreSet` plus the `Linked*`/`Hash*` forms).
    fn is_collection_type(name: &str) -> bool {
        matches!(
            name,
            "Map" | "HashMap" | "LinkedHashMap" | "Set" | "HashSet" | "LinkedHashSet"
        )
    }
}

pub(crate) mod unnecessary_to_list_in_spreads {
    use super::{Check, Group, NodeKind, RuleSpec};

    /// Official: `pkg/linter/lib/src/rules/unnecessary_to_list_in_spreads.dart:37-47`
    /// — a spread of `x.toList()` reports at the method name.
    ///
    /// Approximation (documented): the official requires the receiver to
    /// implement `Iterable` (`implementsInterface('Iterable', 'dart.core')`,
    /// :45) — a resolution fact; a spread expression's type is checked by
    /// the compiler, so the lexical name check matches on all valid code and
    /// a user `toList` on a non-Iterable (already a compile error in a
    /// spread) over-reports. Message: `diagnostic.g.dart`
    /// `unnecessaryToListInSpreads`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "unnecessary_to_list_in_spreads",
        group: Group::Recommended,
        problem: "Unnecessary use of 'toList' in a spread.",
        correction: "Try removing the invocation of 'toList'.",
        kinds: &[NodeKind::Spread],
        check,
    };

    fn check(c: &mut Check<'_>) {
        // `node.expression` is the spread's operand child (collapse map
        // §3.5 SpreadExpressionHandle).
        let Some(expression) = c
            .collect_direct(c.node())
            .into_iter()
            .next()
            .filter(|&kid| c.kind(kid) == NodeKind::Send)
        else {
            return;
        };
        let kids = c.collect_direct(expression);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments))
        else {
            return;
        };
        let Some(&callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        // `expression.methodName.name == 'toList'` with a target
        // (:43-45): the callee is `x.toList` — a `DotAccess` whose last
        // child names the method.
        if c.kind(callee) != NodeKind::DotAccess {
            return;
        }
        let access = c.collect_direct(callee);
        let Some(name) = access.last().copied() else {
            return;
        };
        if c.kind(name) != NodeKind::Identifier
            || c.lexeme(c.head_token(name)) != "toList"
            || access.len() < 2
        {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }
}

pub(crate) mod prefer_iterable_where_type {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::{NONE_TOKEN, first_child};

    /// Official: `pkg/linter/lib/src/rules/prefer_iterable_whereType.dart:31-83`
    /// — `x.where((e) => e is T)` (or the single-return block form) reports
    /// at the method name.
    ///
    /// Approximation (documented): the receiver type check
    /// (`implementsInterface('Iterable')`, :48-49) is dropped — the
    /// `prefer_contains` precedent — so a user class with a `where` method
    /// over-reports. Message: `diagnostic.g.dart` `preferIterableWheretype`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_iterable_whereType",
        group: Group::Core,
        problem: "Use 'whereType' to filter elements by type.",
        correction: "Try using 'whereType' rather than 'where'.",
        kinds: &[NodeKind::Send],
        check,
    };

    fn check(c: &mut Check<'_>) {
        let node = c.node();
        let kids = c.collect_direct(node);
        let Some(args_pos) = kids
            .iter()
            .position(|&kid| matches!(c.kind(kid), NodeKind::Arguments | NodeKind::NoArguments))
        else {
            return;
        };
        // `node.methodName.name != 'where'` with a target bails (:47).
        let Some(callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        let name = match c.kind(*callee) {
            NodeKind::DotAccess | NodeKind::Qualified => {
                let access = c.collect_direct(*callee);
                match access.last().copied() {
                    Some(name)
                        if c.kind(name) == NodeKind::Identifier
                            && c.lexeme(c.head_token(name)) == "where" =>
                    {
                        name
                    }
                    _ => return,
                }
            }
            _ => return,
        };
        // `args.length != 1` bails (:50-51).
        let args = c.collect_direct(kids[args_pos]);
        if args.len() != 1 {
            return;
        }
        // `arg is FunctionExpression` (:52).
        if c.kind(args[0]) != NodeKind::FunctionExpression {
            return;
        }
        if !is_type_test_closure(c, args[0]) {
            return;
        }
        let range = c.utf16_span(name);
        c.report(range, &[]);
    }

    /// The closure shape (:53-81): one parameter, an expression body (or a
    /// single-return block body) whose unparenthesized expression is `e is T`
    /// (no `is!`) with the parameter as the tested expression.
    fn is_type_test_closure(c: &Check<'_>, closure: u32) -> bool {
        let kids = c.collect_direct(closure);
        let params = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::FormalParameters);
        let Some(params) = params else {
            return false;
        };
        if c.collect_direct(*params).len() != 1 {
            return false;
        }
        let param_name = parameter_name(c, c.collect_direct(*params)[0]);
        let Some(param_name) = param_name else {
            #[cfg(test)]
            eprintln!(
                "DBG param_name=None params_kids={:?}",
                c.collect_direct(
                    c.collect_direct(closure)
                        .iter()
                        .copied()
                        .find(|&k| c.kind(k) == NodeKind::FormalParameters)
                        .unwrap()
                )
            );
            return false;
        };
        let body = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::ExpressionBody)
            .copied();
        let expression = if let Some(body) = body {
            first_child(c, body)
        } else {
            // `hasSingleExpressionBody`: one `return` statement.
            let body = kids
                .iter()
                .find(|&&kid| c.kind(kid) == NodeKind::BlockBody)
                .copied();
            let Some(body) = body else {
                return false;
            };
            // The statements live under the body's `Block` child.
            let Some(block) = c
                .collect_direct(body)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Block)
            else {
                return false;
            };
            let statements = c.collect_direct(block);
            if statements.len() != 1 || c.kind(statements[0]) != NodeKind::ReturnStatement {
                return false;
            }
            first_child(c, statements[0])
        };
        let Some(expression) = expression else {
            return false;
        };
        let mut expression = expression;
        while c.kind(expression) == NodeKind::ParenthesizedExpression {
            let Some(inner) = first_child(c, expression) else {
                return false;
            };
            expression = inner;
        }
        if c.kind(expression) != NodeKind::IsExpression {
            return false;
        }
        // `expression.notOperator == null` (:71).
        if c.payload(expression).first().copied().unwrap_or(0) != 0 {
            return false;
        }
        let Some(tested) = first_child(c, expression) else {
            return false;
        };
        c.kind(tested) == NodeKind::Identifier && c.lexeme(c.head_token(tested)) == param_name
    }

    /// A simple formal parameter's name (`FormalParameter` payload word 1,
    /// collapse map §3.3).
    fn parameter_name<'a>(c: &'a Check<'a>, parameter: u32) -> Option<&'a str> {
        if c.kind(parameter) != NodeKind::FormalParameter {
            return None;
        }
        let name = c.payload(parameter).get(1).copied()?;
        (name != NONE_TOKEN).then(|| c.lexeme(name))
    }
}

pub(crate) mod prefer_for_elements_to_map_from_iterable {
    use super::{Check, Group, NodeKind, RuleSpec};
    use crate::rules::first_child;

    /// Official: `pkg/linter/lib/src/rules/prefer_for_elements_to_map_fromIterable.dart:31-90`
    /// — a `Map.fromIterable(a, key: …, value: …)` creation whose key and
    /// value arguments are single-parameter single-expression closures
    /// reports at the creation.
    ///
    /// Approximation (documented): the constructor element pin
    /// (`element.enclosingElement != typeProvider.mapElement`, :46-49) is
    /// taken from the `Map.fromIterable` name, so a user-defined `Map` class
    /// over-reports (the `prefer_collection_literals` precedent). Message:
    /// `diagnostic.g.dart` `preferForElementsToMapFromiterable`.
    pub(crate) static SPEC: RuleSpec = RuleSpec {
        name: "prefer_for_elements_to_map_fromIterable",
        group: Group::Recommended,
        problem: "Use 'for' elements when building maps from iterables.",
        correction: "Try using a collection literal with a 'for' element.",
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
        // `element.name != 'fromIterable'` through the name face: a
        // `Map.fromIterable` callee is a `Qualified` type/constructor pair.
        let Some(callee) = args_pos.checked_sub(1).and_then(|i| kids.get(i)) else {
            return;
        };
        if c.kind(*callee) != NodeKind::Qualified {
            return;
        }
        let qualified = c.collect_direct(*callee);
        let (Some(type_name), Some(ctor)) = (qualified.first().copied(), qualified.last().copied())
        else {
            return;
        };
        if c.kind(type_name) != NodeKind::Identifier
            || c.kind(ctor) != NodeKind::Identifier
            || c.lexeme(c.head_token(type_name)) != "Map"
            || c.lexeme(c.head_token(ctor)) != "fromIterable"
        {
            return;
        }
        // `arguments.length != 3` bails (:57-59).
        let args = c.collect_direct(kids[args_pos]);
        if args.len() != 3 {
            return;
        }
        // The key/value closure extraction (:61-67, the TODO's named-args-
        // anywhere face kept positional).
        let second = args[1];
        let third = args[2];
        let key = extract_closure(c, "key", second).or_else(|| extract_closure(c, "key", third));
        let value =
            extract_closure(c, "value", third).or_else(|| extract_closure(c, "value", second));
        if key.is_none() || value.is_none() {
            return;
        }
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }

    /// `_extractClosure` (:71-88): a named argument whose expression (no
    /// parens) is a one-required-parameter function expression with a single
    /// expression body.
    fn extract_closure(c: &Check<'_>, name: &str, argument: u32) -> Option<()> {
        if c.kind(argument) != NodeKind::NamedArgument {
            return None;
        }
        let kids = c.collect_direct(argument);
        let label = kids.first().copied()?;
        if c.kind(label) != NodeKind::Identifier || c.lexeme(c.head_token(label)) != name {
            return None;
        }
        let mut expression = *kids.get(1)?;
        while c.kind(expression) == NodeKind::ParenthesizedExpression {
            expression = first_child(c, expression)?;
        }
        if c.kind(expression) != NodeKind::FunctionExpression {
            return None;
        }
        let kids = c.collect_direct(expression);
        // One parameter, required (not inside the optional wrapper — the
        // official `parameters.first.isRequired`; a required-named closure
        // parameter under-reports through this face).
        let params = kids
            .iter()
            .find(|&&kid| c.kind(kid) == NodeKind::FormalParameters)
            .copied()?;
        if c.collect_direct(params).len() != 1 {
            return None;
        }
        // `hasSingleExpressionBody`: an expression body, or one return.
        let has_expression_body = kids
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::ExpressionBody)
            || kids.iter().any(|&kid| c.kind(kid) == NodeKind::BlockBody) && {
                let body = kids
                    .iter()
                    .find(|&&kid| c.kind(kid) == NodeKind::BlockBody)
                    .copied()?;
                // The statements live under the body's `Block` child.
                let block = c
                    .collect_direct(body)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Block)?;
                let statements = c.collect_direct(block);
                statements.len() == 1 && c.kind(statements[0]) == NodeKind::ReturnStatement
            };
        has_expression_body.then_some(())
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

    // —— prefer_collection_literals ———————————————————————————————————————

    /// `test/rules/prefer_collection_literals_test.dart` `test_map` — the
    /// no-arg `Map()` invocation reports.
    #[test]
    fn collection_literals_reports_map_and_set() {
        let map = "Map();";
        // Map0 (1 )2 ;3
        let f = Fixture::build(
            map,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Arguments, 1);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::Send, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        let lints = f.run(&one_rule("prefer_collection_literals"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary constructor invocation.");
        assert_eq!(
            lints[0].correction,
            Some("Try using a collection literal.".to_owned())
        );

        let set = "Set();";
        // Set0 (1 )2 ;3
        let f = Fixture::build(
            set,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                identifier(b, 0, 1);
                b.begin(NodeKind::Arguments, 1);
                b.end(NodeKind::Arguments, 4);
                b.end(NodeKind::Send, 4);
                b.end(NodeKind::ExpressionStatement, 4);
            }),
        );
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);
    }

    /// `Set.from([1])` reports; a named constructor on the map family, a
    /// non-list argument, and arguments on `Map()` pass.
    #[test]
    fn collection_literals_from_of_and_negatives() {
        let from_list = "Set.from([1]);";
        // Set0 .1 from2 (3 [4 15 ]6 )7 ;8
        let f = Fixture::build(
            from_list,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 4);
                b.begin(NodeKind::Arguments, 3);
                b.begin(NodeKind::ListLiteral, 4);
                b.patch(0, NONE);
                b.leaf(NodeKind::LiteralInt, 5);
                b.end(NodeKind::ListLiteral, 7);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::ExpressionStatement, 9);
            }),
        );
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        // `HashMap.identity()` — a named constructor on the map family
        // (`constructorName != null`, :56).
        let named_map = "HashMap.identity();";
        // HashMap0 .1 identity2 (3 )4 ;5
        let f = Fixture::build(
            named_map,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 4);
                b.begin(NodeKind::Arguments, 4);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());

        // `Set.from(1)` — the argument is not a list literal (:76-78).
        let from_int = "Set.from(1);";
        // Set0 .1 from2 (3 14 )5 ;6
        let f = Fixture::build(
            from_int,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::Qualified, 0);
                b.patch(0, 1);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Qualified, 4);
                b.begin(NodeKind::Arguments, 3);
                b.leaf(NodeKind::LiteralInt, 4);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());
    }

    /// `['a'].toSet()` reports; `x.toSet()` on a non-literal target passes
    /// (:87-95).
    #[test]
    fn collection_literals_to_set_on_list_literal() {
        let to_set = "['a'].toSet();";
        // [0 'a'1 ]2 .3 toSet4 (5 )6 ;7
        let f = Fixture::build(
            to_set,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 3);
                b.patch(1, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.leaf_span(NodeKind::StringLiteral, 1, 2);
                b.end(NodeKind::ListLiteral, 4);
                identifier(b, 4, 5);
                b.end(NodeKind::DotAccess, 7);
                b.begin(NodeKind::Arguments, 5);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::ExpressionStatement, 8);
            }),
        );
        assert_eq!(f.run(&one_rule("prefer_collection_literals")).len(), 1);

        let non_literal = "x.toSet();";
        // x0 .1 toSet2 (3 )4 ;5
        let f = Fixture::build(
            non_literal,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::Send, 0);
                b.begin(NodeKind::DotAccess, 0);
                b.patch(0, 1);
                b.patch(1, 0);
                identifier(b, 0, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::DotAccess, 5);
                b.begin(NodeKind::Arguments, 3);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::ExpressionStatement, 6);
            }),
        );
        assert!(f.run(&one_rule("prefer_collection_literals")).is_empty());
    }

    // —— unnecessary_to_list_in_spreads ————————————————————————————————————

    /// `test/rules/unnecessary_to_list_in_spreads_test.dart` — `...x.toList()`
    /// reports at the method name.
    #[test]
    fn to_list_spread_reports() {
        let source = "[...list.toList()];";
        // [0 ...1 list2 .3 toList4 (5 )6 ]7 ;8
        let f = Fixture::build(
            source,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                b.begin(NodeKind::Send, 2);
                b.begin(NodeKind::DotAccess, 2);
                b.patch(0, 3);
                b.patch(1, 0);
                identifier(b, 2, 3);
                identifier(b, 4, 5);
                b.end(NodeKind::DotAccess, 7);
                b.begin(NodeKind::Arguments, 5);
                b.end(NodeKind::Arguments, 8);
                b.end(NodeKind::Send, 8);
                b.end(NodeKind::Spread, 8);
                b.end(NodeKind::ListLiteral, 9);
                b.end(NodeKind::ExpressionStatement, 9);
            }),
        );
        let lints = f.run(&one_rule("unnecessary_to_list_in_spreads"));
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].message, "Unnecessary use of 'toList' in a spread.");
        assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    }

    /// A plain spread element and a bare `toList()` call pass.
    #[test]
    fn plain_spread_and_bare_to_list_pass() {
        let plain = "[...list];";
        // [0 ...1 list2 ]3 ;4
        let f = Fixture::build(
            plain,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                identifier(b, 2, 3);
                b.end(NodeKind::Spread, 4);
                b.end(NodeKind::ListLiteral, 5);
                b.end(NodeKind::ExpressionStatement, 5);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );

        let bare = "[...toList()];";
        // [0 ...1 toList2 (3 )4 ]5 ;6
        let f = Fixture::build(
            bare,
            unit_of(|b, _t| {
                b.begin(NodeKind::ExpressionStatement, 0);
                b.begin(NodeKind::ListLiteral, 0);
                b.patch(0, NONE);
                b.begin(NodeKind::Spread, 1);
                b.begin(NodeKind::Send, 2);
                identifier(b, 2, 3);
                b.begin(NodeKind::Arguments, 3);
                b.end(NodeKind::Arguments, 6);
                b.end(NodeKind::Send, 6);
                b.end(NodeKind::Spread, 6);
                b.end(NodeKind::ListLiteral, 7);
                b.end(NodeKind::ExpressionStatement, 7);
            }),
        );
        assert!(
            f.run(&one_rule("unnecessary_to_list_in_spreads"))
                .is_empty()
        );
    }

    // —— prefer_iterable_whereType —————————————————————————————————————————

    /// `test/rules/prefer_iterable_whereType_test.dart` `test_whereType` —
    /// `x.where((e) => e is int)` reports at the method name.
    #[test]
    fn where_type_reports_a_type_test_closure() {
        let source = "x.where((e) => e is int);";
        // x0 .1 where2 (3 (4 e5 )6 =>7 e8 is9 int10 )11 ;12
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
                b.begin(NodeKind::FunctionExpression, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::FormalParameter, 4);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 5, 6);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::ExpressionBody, 7);
                b.patch(0, 7);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::IsExpression, 8);
                b.patch(0, 0);
                identifier(b, 8, 9);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::IsExpression, 11);
                b.end(NodeKind::ExpressionBody, 11);
                b.end(NodeKind::FunctionExpression, 12);
                b.end(NodeKind::Arguments, 12);
                b.end(NodeKind::Send, 12);
                b.end(NodeKind::ExpressionStatement, 13);
            }),
        );
        let lints = f.run(&one_rule("prefer_iterable_whereType"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'whereType' to filter elements by type."
        );
        assert_eq!(lints[0].range.start, utf16_at(&f, 2));
    }

    /// An `is!` closure and a foreign-parameter test pass (:71-79).
    #[test]
    fn where_type_not_is_and_foreign_param_pass() {
        let source = "x.where((e) => e is! int);";
        // x0 .1 where2 (3 (4 e5 )6 =>7 e8 is!9 int10 )11 ;12
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
                b.begin(NodeKind::FunctionExpression, 4);
                b.begin(NodeKind::FormalParameters, 4);
                b.begin(NodeKind::FormalParameter, 4);
                b.patch(0, NONE);
                b.patch(1, 5);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 5, 6);
                b.end(NodeKind::FormalParameter, 7);
                b.end(NodeKind::FormalParameters, 7);
                b.begin(NodeKind::ExpressionBody, 7);
                b.patch(0, 7);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.begin(NodeKind::IsExpression, 8);
                b.patch(0, 1);
                identifier(b, 8, 9);
                b.leaf_span(NodeKind::Type, 10, 11);
                b.end(NodeKind::IsExpression, 11);
                b.end(NodeKind::ExpressionBody, 11);
                b.end(NodeKind::FunctionExpression, 12);
                b.end(NodeKind::Arguments, 12);
                b.end(NodeKind::Send, 12);
                b.end(NodeKind::ExpressionStatement, 13);
            }),
        );
        assert!(f.run(&one_rule("prefer_iterable_whereType")).is_empty());
    }

    // —— prefer_for_elements_to_map_fromIterable ———————————————————————————

    /// `Map.fromIterable(x, key: …, value: …)` with two single-expression
    /// closures reports at the creation.
    #[test]
    fn map_from_iterable_reports_with_both_closures() {
        let source = "Map.fromIterable(x, key: (e) => e, value: (e) => e);";
        // Map0 .1 fromIterable2 (3 x4 ,5 key6 :7 (8 e9 )10 =>11 e12 ,13 value14 :15 (16 e17 )18 =>19 e20 )21 ;22
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
                b.begin(NodeKind::NamedArgument, 5);
                b.patch(0, 7);
                identifier(b, 6, 7);
                b.begin(NodeKind::FunctionExpression, 8);
                b.begin(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::FormalParameter, 8);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 9, 10);
                b.end(NodeKind::FormalParameter, 11);
                b.end(NodeKind::FormalParameters, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 13);
                b.end(NodeKind::FunctionExpression, 13);
                b.end(NodeKind::NamedArgument, 13);
                b.begin(NodeKind::NamedArgument, 13);
                b.patch(0, 15);
                identifier(b, 14, 15);
                b.begin(NodeKind::FunctionExpression, 16);
                b.begin(NodeKind::FormalParameters, 16);
                b.begin(NodeKind::FormalParameter, 16);
                b.patch(0, NONE);
                b.patch(1, 17);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 17, 18);
                b.end(NodeKind::FormalParameter, 19);
                b.end(NodeKind::FormalParameters, 19);
                b.begin(NodeKind::ExpressionBody, 19);
                b.patch(0, 19);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 20, 21);
                b.end(NodeKind::ExpressionBody, 22);
                b.end(NodeKind::FunctionExpression, 22);
                b.end(NodeKind::NamedArgument, 22);
                b.end(NodeKind::Arguments, 22);
                b.end(NodeKind::Send, 23);
                b.end(NodeKind::ExpressionStatement, 23);
            }),
        );
        let lints = f.run(&one_rule("prefer_for_elements_to_map_fromIterable"));
        assert_eq!(lints.len(), 1);
        assert_eq!(
            lints[0].message,
            "Use 'for' elements when building maps from iterables."
        );
    }

    /// A missing value closure (two arguments) passes (:57-69).
    #[test]
    fn map_from_iterable_without_value_closure_passes() {
        let source = "Map.fromIterable(x, key: (e) => e);";
        // Map0 .1 fromIterable2 (3 x4 ,5 key6 :7 (8 e9 )10 =>11 e12 )13 ;14
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
                b.begin(NodeKind::NamedArgument, 5);
                b.patch(0, 7);
                identifier(b, 6, 7);
                b.begin(NodeKind::FunctionExpression, 8);
                b.begin(NodeKind::FormalParameters, 8);
                b.begin(NodeKind::FormalParameter, 8);
                b.patch(0, NONE);
                b.patch(1, 9);
                b.patch(2, NONE);
                b.patch(3, NONE);
                b.patch(4, 0);
                identifier(b, 9, 10);
                b.end(NodeKind::FormalParameter, 11);
                b.end(NodeKind::FormalParameters, 11);
                b.begin(NodeKind::ExpressionBody, 11);
                b.patch(0, 11);
                b.patch(1, NONE);
                b.patch(2, NONE);
                b.patch(3, NONE);
                identifier(b, 12, 13);
                b.end(NodeKind::ExpressionBody, 14);
                b.end(NodeKind::FunctionExpression, 14);
                b.end(NodeKind::NamedArgument, 14);
                b.end(NodeKind::Arguments, 14);
                b.end(NodeKind::Send, 15);
                b.end(NodeKind::ExpressionStatement, 15);
            }),
        );
        assert!(
            f.run(&one_rule("prefer_for_elements_to_map_fromIterable"))
                .is_empty()
        );
    }
}
