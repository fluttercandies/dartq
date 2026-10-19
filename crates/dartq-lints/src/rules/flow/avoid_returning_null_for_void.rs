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
