use super::{Check, NodeKind, RuleSpec, written_type_sibling};
use crate::rules::{TypeFace, type_face, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_null_checks.dart` — a `!`
/// whose context expects a nullable type reports at the operator
/// (:209-216: the expected type decides — removing the check cannot
/// change the program when the context accepts `null`). The expected
/// type faces (:26-161) cover returns, arrow bodies, variable
/// declarations, assignments, operator right sides, collection
/// members, and arguments.
///
/// dartq faces over the production tiling: the decidable context slice
/// ports — the variable/field-declaration initializer (the written
/// type row tiles as the declaration's left sibling,
/// [`written_type_sibling`]), the return statement, and the arrow
/// body, each through the nearest enclosing declaration's written
/// return type with the async flatten (:50-57 — a `Future`/
/// `FutureOr` wrapper decides through its first type argument). The
/// enclosing-declaration lookup keeps the official
/// `thisOrAncestorOfType<FunctionExpression>` shape (:41): class
/// methods, constructors and factories never satisfy it, so their
/// return and arrow faces stay silent exactly like the oracle. The
/// pattern arm (:183-205) belongs to the pattern family and stays
/// unported; assignments, operator right sides, collection members,
/// await wrappers, and arguments under-report (the promotable-LHS
/// guard and the formal-table availability make them undecidable at
/// M1). Message: `diagnostic.g.dart` `unnecessaryNullChecks`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("unnecessary_null_checks", &[NodeKind::NonNullAssert], check,);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    if c.semantics().is_none() {
        return;
    }
    let node = c.node();
    // `getExpectedType`'s realNode (:30-33): the `!`'s nearest
    // ancestor whose own parent is not a transparent `(...)` wrapper.
    let mut real = node;
    while let Some(parent) = c.unit.ast.parent(real)
        && c.kind(parent) == NodeKind::ParenthesizedExpression
    {
        real = parent;
    }
    let Some(context) = c.unit.ast.parent(real) else {
        return;
    };
    let admits = match c.kind(context) {
        // `int? y = x!;` — the direct-initializer face (:95-98): the
        // written type row decides. The production tiling parks the
        // expression pieces flat in the initializer window, so the
        // `!` is the whole initializer exactly when it is the
        // window's tail row (`foo(x!)`'s pieces fill the argument
        // container and trail the `!`; a nested `x!.foo()` completes
        // later pieces — both carry no expected type, :132-159).
        NodeKind::VariableInitializer => {
            c.children(context).end - 1 == real
                && c.path
                    .iter()
                    .rev()
                    .copied()
                    .find(|&row| c.kind(row) == NodeKind::VariablesDeclaration)
                    .is_some_and(|decl| written_nullable(c, declaration_written_type(c, decl)))
        }
        // `int? f() { return x!; }` — the return's own expression
        // window roots the `!` and the enclosing declaration's
        // written return type decides (:40-57).
        NodeKind::ReturnStatement => {
            c.collect_direct(context).last() == Some(&real) && enclosing_return_admits(c)
        }
        // `int? f() => x!;` — the childless `ExpressionBody` handle
        // leaves the arrow expression as the declaration's own tail
        // rows, so the arrow posture reads at the declaration row
        // (:40-57's ExpressionFunctionBody arm): the `!` is the tail
        // row before the body handle.
        NodeKind::TopLevelMethod
        | NodeKind::LocalFunctionDeclaration
        | NodeKind::FunctionExpression
        | NodeKind::NamedFunctionExpression => {
            let kids = c.collect_direct(context);
            kids.len() >= 2
                && c.kind(kids[kids.len() - 1]) == NodeKind::ExpressionBody
                && kids[kids.len() - 2] == real
                && return_admits_at(c, context)
        }
        _ => false,
    };
    if admits {
        let range = utf16_token_span(c, c.head_token(node));
        c.report(range, &[]);
    }
}

/// The written type row of the declaration an initializer belongs to:
/// the local shape tiles the type as the declaration's left sibling
/// ([`written_type_sibling`]); the field shape nests the declaration
/// in a `Fields` row whose own type child heads the member group.
fn declaration_written_type(c: &Check<'_>, decl: u32) -> Option<u32> {
    written_type_sibling(c, decl, &[NodeKind::Type]).or_else(|| {
        let fields = c.unit.ast.parent(decl)?;
        let type_row = c
            .collect_direct(fields)
            .into_iter()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::FunctionType));
        (c.kind(fields) == NodeKind::Fields).then_some(type_row)?
    })
}

/// The written type row's nullable verdict (the declaration-context
/// face — an `async` local variable is not a thing).
fn written_nullable(c: &Check<'_>, type_row: Option<u32>) -> bool {
    let Some(type_row) = type_row else {
        return false;
    };
    let face = type_face(c, type_row);
    face.nullable || face.name == Some("dynamic")
}

/// The nearest enclosing function-like declaration's written return
/// type with the async flatten (:349-380). The official lookup is
/// `thisOrAncestorOfType<FunctionExpression>` (:41): class methods,
/// constructors and factories never satisfy it, so their return faces
/// stay silent exactly like the oracle; inferred lambdas carry no
/// written face.
fn enclosing_return_admits(c: &Check<'_>) -> bool {
    for &row in c.path.iter().rev().skip(1) {
        match c.kind(row) {
            NodeKind::TopLevelMethod
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression => return return_admits_at(c, row),
            NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::CompilationUnit => return false,
            _ => {}
        }
    }
    false
}

/// The declaration's own written return type, read at the row.
fn return_admits_at(c: &Check<'_>, declaration: u32) -> bool {
    let written = super::void_checks::written_return_face(c, declaration);
    admits_return(c, declaration, written)
}

/// The `isNullable` read on the flattened expected type (:50-57).
fn admits_return(c: &Check<'_>, declaration: u32, written: Option<TypeFace<'_>>) -> bool {
    let Some(face) = written else {
        return false;
    };
    let Some(name) = face.name else {
        return false;
    };
    let async_marked = c
        .unit
        .ast
        .token_range(declaration)
        .any(|token| c.lexeme(token) == "async");
    if async_marked && matches!(name, "Future" | "FutureOr") {
        return match face.type_arguments.first() {
            Some((arg, arg_nullable)) => *arg_nullable || *arg == "dynamic",
            None => false,
        };
    }
    face.nullable || name == "dynamic"
}
