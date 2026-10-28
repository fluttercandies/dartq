use super::{Check, NodeKind, RuleSpec, async_words, member_declaration_name, override_annotation};
use crate::rules::{NONE_TOKEN, is_statement_row, last_child, type_before, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/unnecessary_async.dart:100-203`
/// — a function, closure or method declared `async` (never a
/// generator, :139-141) whose body uses no `await` (:148-153, any
/// `await` expression or `await for`; nested function bodies are
/// pruned, :74-76) reports at the `async` keyword when the return
/// type admits removal (:156-188): absent (the dynamic face),
/// `void`, `FutureOr<…>`, `dynamic`, or `Object` always report; a
/// `Future<…>` return reports only when the body cannot complete
/// normally (:193-195) and every `return` (and the `=>` expression)
/// carries a non-nullable `Future` value (:79-97/:198-201).
///
/// Approximation (documented): closures are out of the lexical face —
/// the `imposedType` context (:120) is unresolvable, and reporting on
/// it would fabricate on `Future<T>`-imposed parameters. The
/// `Future`-expression face reads the head spelling (`Future.value(…)`
/// et al.); a future-typed variable or call under-reports. A block
/// body "cannot complete normally" when its last statement is a value
/// `return` or a `throw` (the flow-analysis face narrows to that
/// shape); other bodies under-report. A method with no written return
/// type answers the dynamic face — unless it carries `@override` or a
/// same-file ancestor declares the name (the resolved overridden
/// return type may be `Future`). Message: `diagnostic.g.dart`
/// `unnecessaryAsync`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_async",
    &[
        NodeKind::TopLevelMethod,
        NodeKind::Method,
        NodeKind::LocalFunctionDeclaration,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(body) = last_child(c, node) else {
        return;
    };
    // `body.keyword == null || != Keyword.ASYNC` / `body.star != null`
    // (:134-141) — the body's modifier words (see `async_words`).
    let (async_kw, star) = async_words(c, body);
    if async_kw == NONE_TOKEN || c.lexeme(async_kw) != "async" || star != NONE_TOKEN {
        return;
    }
    // `_HasAwaitVisitor` (:44-98) over the owner's window (an
    // `ExpressionBody` is a childless handle — its expression rows are
    // owner-window siblings): any `await` silences (:151-153).
    let scan = scan_body(c, node);
    if scan.has_await {
        return;
    }
    match return_type_face(c, node) {
        Face::Report => {}
        Face::Future => {
            // `mayCompleteNormally` (:193-195) plus every value a
            // `Future` (:198-201).
            if !scan.returns_only_future
                || !scan.every_return_has_value
                || !cannot_complete_normally(c, body)
            {
                return;
            }
        }
        Face::Skip => return,
    }
    c.report(utf16_token_span(c, async_kw), &[]);
}

/// The written return type's verdict (:156-188).
fn return_type_face(c: &Check<'_>, node: u32) -> Face {
    let Some(name) = member_declaration_name(c, node) else {
        return Face::Skip;
    };
    match type_before(c, node, name) {
        // The written type tiles as the row's left sibling subtree;
        // `void` spells its own `VoidType` kind (not `Type`), and a
        // `NoType` row means unwritten.
        Some((t, NodeKind::Type | NodeKind::VoidType)) => match c.lexeme(c.head_token(t)) {
            "void" | "FutureOr" | "dynamic" | "Object" => Face::Report,
            "Future" => Face::Future,
            _ => Face::Skip,
        },
        Some((_, NodeKind::NoType)) => absent_face(c, node),
        _ => Face::Skip, // function/record types
    }
}

/// The absent-written-type face: dynamic for top-level and local
/// functions; a method may resolve an overridden return type — the
/// override signals bail.
fn absent_face(c: &Check<'_>, node: u32) -> Face {
    if c.kind(node) != NodeKind::Method {
        return Face::Report;
    }
    if override_annotation(c, node) {
        return Face::Skip;
    }
    let Some(name) = member_declaration_name(c, node) else {
        return Face::Skip;
    };
    let name = c.lexeme(c.head_token(name));
    let overridden = c
        .path
        .iter()
        .rev()
        .find(|&&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        })
        .is_some_and(|&container| {
            crate::rules::same_file_ancestors(c, container, false)
                .iter()
                .any(|&a| {
                    super::container_members(c, a).1.iter().any(|&m| {
                        !crate::rules::member_matches(
                            c,
                            m,
                            crate::rules::MemberQuery::Method,
                            name,
                        )
                        .is_empty()
                    })
                })
        });
    if overridden { Face::Skip } else { Face::Report }
}

/// `_HasAwaitVisitor`'s verdict over the owner row's subtree.
struct BodyScan {
    has_await: bool,
    every_return_has_value: bool,
    returns_only_future: bool,
}

fn scan_body(c: &Check<'_>, owner: u32) -> BodyScan {
    let mut scan = BodyScan {
        has_await: false,
        every_return_has_value: true,
        returns_only_future: true,
    };
    let window = c.children(owner);
    // Nested function subtrees prune (`visitFunctionExpression` stops
    // the recursion, :74-76); their row ranges are contiguous.
    let mut pruned: Vec<(u32, u32)> = Vec::new();
    for row in window.start..=owner {
        if pruned.iter().any(|&(s, e)| row >= s && row <= e) {
            continue;
        }
        match c.kind(row) {
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration => {
                let subtree = c.children(row);
                pruned.push((subtree.start.min(row), row));
            }
            NodeKind::AwaitExpression => scan.has_await = true,
            NodeKind::ForIn => {
                // `visitForStatement`/`visitForElement` await keyword
                // (:62-71): `await for` — the parts' head is the
                // `await` token (payload word 0 present).
                if c.collect_direct(row)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
                    .is_some_and(|parts| {
                        c.payload(parts).first().copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN
                    })
                {
                    scan.has_await = true;
                }
            }
            NodeKind::ReturnStatement => match c.collect_direct(row).into_iter().next() {
                // `_updateWithExpression` (:79-97): the value must be a
                // non-nullable `Future` — the head-spelling face reads
                // the expression's leading token (`Future.value(1)`
                // heads at `Future`, the receiver, not the selector).
                Some(value) => {
                    if !future_headed(c, value) {
                        scan.returns_only_future = false;
                    }
                }
                None => scan.every_return_has_value = false,
            },
            NodeKind::ExpressionBody => {
                // `visitExpressionFunctionBody` (:56-59): the `=>`
                // value face reads the token interior.
                let arrow = c.payload(row).first().copied().unwrap_or(NONE_TOKEN);
                let semi = c.payload(row).get(1).copied().unwrap_or(NONE_TOKEN);
                if semi == NONE_TOKEN || !future_headed(c, arrow + 1) {
                    scan.returns_only_future = false;
                }
            }
            _ => {}
        }
    }
    scan
}

/// The expression row's value starts a `Future` spelling
/// (`Future.value(…)` / `Future<int>.delayed(…)`); `FutureOr` is not a
/// `Future` subtype and other heads under-report.
fn future_headed(c: &Check<'_>, expression: u32) -> bool {
    c.lexeme(c.head_token(expression)) == "Future"
}

/// `bodyContext.mayCompleteNormally` (:193-195) — the narrow lexical
/// face: the block's last statement is a value `return` or a `throw`
/// statement, so control cannot fall off the end; an `=>` body always
/// returns.
fn cannot_complete_normally(c: &Check<'_>, body: u32) -> bool {
    match c.kind(body) {
        NodeKind::ExpressionBody => true,
        NodeKind::BlockBody => {
            let statements: Vec<u32> = c
                .collect_direct(body)
                .into_iter()
                .filter(|&row| is_statement_row(c, row))
                .collect();
            match statements.last() {
                Some(&last) => match c.kind(last) {
                    NodeKind::ReturnStatement => last_child(c, last).is_some(),
                    NodeKind::ExpressionStatement => c.lexeme(c.head_token(last)) == "throw",
                    _ => false,
                },
                None => false,
            }
        }
        _ => false,
    }
}

enum Face {
    Report,
    Future,
    Skip,
}
