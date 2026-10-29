use dartq_semantic::{BindingKind, model::Ty};

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, static_type_faces as faces,
};
use crate::rules::{body_members, type_face};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_int_literals.dart`
/// — a double literal whose value is an exact integer reports when the
/// context demands a double (:36-58 `visitDoubleLiteral` +
/// `hasTypeDouble`): there the `int` rewrite keeps the static type. The
/// official context faces port over the production tiling:
/// - the parameter faces (:40-43, :50-54 `correspondingParameter`) —
///   the argument's formal type, resolved for a member invocation
///   through the semantic member walk (cross-file classes included)
///   and for a bare callee through the same-file declarations gated on
///   the callee's binding; a named argument reads its formal by name;
/// - the variable-declaration face (:69-75) — the written type is the
///   nearest preceding `Type` sibling of the declaration row;
/// - the `return` face (:52-56) — the `ReturnStatement` container owns
///   the literal, and the innermost enclosing declaration with a written
///   return type answers (an anonymous function boundary has none and
///   stays silent, the `hasReturnTypeDouble` `FunctionExpression` bail);
/// - the arrow-body face (:60-63) — the childless `ExpressionBody`
///   handle leaves the expression pieces as the declaration row's own
///   children, so the parent row with an arrow body answers;
/// - the typed-list face (:44-49) — the `TypeArguments` sibling precedes
///   the [`NodeKind::ListLiteral`] row in the owner slice.
///
/// The `-` prefix face (:18-23) needs no separate read: the negated
/// literal tiles in the same owner as the `UnaryPrefix` piece, so the
/// owner-context faces answer the prefix's context identically.
/// Approximations (documented, under-report): a constructor callee
/// (`C(1.0)`), an imported bare function, a function-typed variable's
/// invocation, and a generic callee's unbound parameter type
/// (`void g<T>(T x)` — the written `T` is not `double`) resolve to
/// nothing and stay silent. Message: `diagnostic.g.dart`
/// `preferIntLiterals`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("prefer_int_literals", &[NodeKind::LiteralDouble], check,);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    // `value != value.truncate()` bails (:47-52, the UnsupportedError
    // arm included): the lexeme must fold to a finite double equal to
    // its own truncation.
    let digits: String = c
        .lexeme(c.head_token(node))
        .chars()
        .filter(|&ch| ch != '_')
        .collect();
    let Ok(value) = digits.parse::<f64>() else {
        return;
    };
    #[expect(
        clippy::float_cmp,
        reason = "integrality test on an exactly-parsed f64; both sides                       are exact binary values, no rounding involved"
    )]
    if !value.is_finite() || value != value.trunc() {
        return;
    }
    if context_demands_double(c, analysis) {
        let range = c.utf16_span(node);
        c.report(range, &[]);
    }
}

/// The parent dispatch of `hasTypeDouble` (:39-76) over the production
/// tiling; `false` keeps the literal silent.
fn context_demands_double(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>) -> bool {
    let Some(parent) = c.ancestor(1) else {
        return false;
    };
    match c.kind(parent) {
        // `return e;` (:52-56).
        NodeKind::ReturnStatement => innermost_return_type_is_double(c),
        // The arrow-body declarations (:60-63): `Method`/`TopLevelMethod`
        // rows own their arrow expression pieces; the arrow-body child
        // tells the position apart from every other member-level piece.
        NodeKind::Method | NodeKind::TopLevelMethod => {
            has_arrow_body(c, parent) && return_type_is_double(c, parent)
        }
        // `double x = e;` (:69-75) — the initializer containers. The
        // same owner also tiles `<double>[e]` (:44-49): the literal
        // completes before the `ListLiteral` row and sits as its sibling
        // inside the initializer, so the typed-list face reads the owner
        // too and wins on a preceding `TypeArguments`+`ListLiteral` run.
        NodeKind::VariableInitializer | NodeKind::InitializedIdentifier => {
            list_context_is_double(c, c.node(), parent) || variable_type_is_double(c)
        }
        // `<double>[e]` (:44-49) — the nested-literal arm (the literal
        // inside an already-tiled `ListLiteral` owner).
        NodeKind::ListLiteral => list_element_type_is_double(c, parent),
        // `g(1.0)` / `g(x: 1.0)` (:40-43, :50-54).
        NodeKind::Arguments => argument_formal_is_double(c, analysis),
        _ => false,
    }
}

/// The argument's formal is `double` (:40-43, :50-54
/// `correspondingParameter?.type`): the literal sits in one argument
/// group of the enclosing `Arguments` row; the callee's formal table
/// answers positional-by-index or named-by-name. `double?` counts (the
/// upstream `isDartCoreDouble` reads through nullability).
fn argument_formal_is_double(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>) -> bool {
    let Some(args) = c.ancestor(1) else {
        return false;
    };
    let pieces = c.collect_direct(args);
    let ranges = slice::arguments(c, &pieces);
    let Some(pos) = pieces.iter().position(|&kid| kid == c.node()) else {
        return false;
    };
    let Some(index) = ranges.iter().position(|range| range.contains(&pos)) else {
        return false;
    };
    let named = c.kind(pieces[ranges[index].end - 1]) == NodeKind::NamedArgument;
    let query = if named {
        FormalQuery::Named(c.lexeme(c.head_token(pieces[ranges[index].start])))
    } else {
        // This group's 0-based index among the positional groups.
        FormalQuery::Positional(
            ranges[..index]
                .iter()
                .filter(|range| c.kind(pieces[range.end - 1]) != NodeKind::NamedArgument)
                .count(),
        )
    };
    formal_is_double(c, analysis, query).unwrap_or(false)
}

/// The argument-position query against a callee's formal table.
enum FormalQuery<'a> {
    Positional(usize),
    Named(&'a str),
}

/// Whether the queried formal of the invocation at the enclosing
/// `Arguments` row is `double`. A member invocation resolves through
/// the semantic member walk (cross-file classes included); a bare
/// callee resolves over the same-file declarations, gated on the
/// binding face (`Decl` — a local, type-parameter, prefix, or member
/// binding of the name is a different callable). `None` = the callee
/// carries no formal face.
fn formal_is_double(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    query: FormalQuery<'_>,
) -> Option<bool> {
    let args = c.ancestor(1)?;
    let parent = c.unit.ast.parent(args)?;
    // The invocation face: a `Send` sibling right after the argument
    // list (the send row completes after it).
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == args)?;
    let &send = siblings.get(pos + 1)?;
    if c.kind(send) != NodeKind::Send {
        return None;
    }
    let (head, arguments) = slice::send_parts(c, &siblings, pos + 1)?;
    // A tear-off send carries no invocation.
    arguments?;
    let selector_row = siblings[head];
    let name = c.lexeme(c.head_token(selector_row));
    if slice::access_marker(c, &siblings, pos + 1, NodeKind::DotAccess).is_some() {
        let selector = c.head_token(selector_row);
        let receiver = faces::single_receiver(c, &siblings[..head], selector)?;
        let member =
            analysis.member_of(query_site(c, receiver), analysis.strings().intern(name))?;
        let formals = member.entry.formals.as_deref()?;
        let ty = match query {
            FormalQuery::Positional(index) => formals.positional.get(index)?.ty,
            FormalQuery::Named(arg) => formals.named_formal(analysis.strings().intern(arg))?.ty,
        };
        return Some(is_double_ty(analysis, ty));
    }
    // The bare face: the binding gate, then the same-file declarations.
    if !matches!(
        analysis
            .binding_of(query_site(c, selector_row))
            .map(|b| b.kind),
        Some(BindingKind::Decl)
    ) {
        return None;
    }
    same_file_formal_is_double(c, name, query)
}

/// `isDartCoreDouble` over the semantic type (nullability reads
/// through).
fn is_double_ty(analysis: &dartq_semantic::Analysis<'_>, ty: Ty) -> bool {
    let Ty::Named { name, .. } = ty else {
        return false;
    };
    analysis.strings().resolve(name) == "double"
}

/// The same-file callable `name` — a top-level function, a local
/// function on the path, or the implicit-`this` method of the enclosing
/// class-like — resolved to its declaration row.
fn same_file_callable(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.unit.ast.root()?;
    let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for decl in c.collect_direct(top) {
            if c.kind(decl) != NodeKind::TopLevelMethod {
                continue;
            }
            let named = c.collect_direct(decl).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(kid)) == name
            });
            if named {
                return Some(decl);
            }
        }
    }
    let local = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression
        ) && crate::rules::declarations::member_declaration_name(c, row)
            .is_some_and(|n| c.lexeme(c.head_token(n)) == name)
    });
    if let Some(row) = local {
        return Some(row);
    }
    let class_row = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })?;
    body_members(c, class_row).into_iter().find(|&member| {
        c.kind(member) == NodeKind::Method
            && !crate::rules::member_matches(c, member, crate::rules::MemberQuery::Method, name)
                .is_empty()
    })
}

/// Whether the queried formal of the same-file callable is written
/// `double` (the row-walk face behind the bare-callee argument query).
fn same_file_formal_is_double(c: &Check<'_>, name: &str, query: FormalQuery<'_>) -> Option<bool> {
    let decl = same_file_callable(c, name)?;
    let params = c
        .collect_direct(decl)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
    formal_row_is_double(c, params, query)
}

/// The queried formal's written type over a parameter-list row is
/// `double` (`isDartCoreDoubleTypeAnnotation` :77-79 — a written name;
/// nullability does not change the core-type verdict).
fn formal_row_is_double(c: &Check<'_>, params: u32, query: FormalQuery<'_>) -> Option<bool> {
    let mut positional: Vec<u32> = Vec::new();
    let mut named: Vec<(&str, u32)> = Vec::new();
    for kid in c.collect_direct(params) {
        match c.kind(kid) {
            NodeKind::FormalParameter => positional.push(kid),
            NodeKind::OptionalFormalParameters => {
                for inner in c.collect_direct(kid) {
                    if c.kind(inner) != NodeKind::FormalParameter {
                        continue;
                    }
                    let named_kind = c.field(inner, PayloadField::FormalParameterKind)
                        == Some(dartq_parser::FormalParameterKind::OptionalNamed as u32);
                    if named_kind {
                        let formal = c.field(inner, PayloadField::Name)?;
                        named.push((c.lexeme(formal), inner));
                    } else {
                        positional.push(inner);
                    }
                }
            }
            _ => {}
        }
    }
    let param = match query {
        FormalQuery::Positional(index) => *positional.get(index)?,
        FormalQuery::Named(arg) => named.into_iter().find(|(formal, _)| *formal == arg)?.1,
    };
    let type_row = c
        .collect_direct(param)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)?;
    Some(type_face(c, type_row).name == Some("double"))
}

/// The written return type of the innermost enclosing declaration with
/// one (:52-56): the first function-like row outside the return, so an
/// anonymous function boundary (no written return type) stays silent.
fn innermost_return_type_is_double(c: &Check<'_>) -> bool {
    let mut seen_return = false;
    for &ancestor in c.path.iter().rev().skip(1) {
        match c.kind(ancestor) {
            NodeKind::ReturnStatement => seen_return = true,
            NodeKind::Method | NodeKind::TopLevelMethod if seen_return => {
                return return_type_is_double(c, ancestor);
            }
            NodeKind::FunctionExpression
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::NamedFunctionExpression
            | NodeKind::Constructor
            | NodeKind::Factory
                if seen_return =>
            {
                return false;
            }
            _ => {}
        }
    }
    false
}

/// Whether the declaration row carries an arrow body (the `ExpressionBody`
/// child of the childless-handle shape, bridge
/// `handleExpressionFunctionBody`).
fn has_arrow_body(c: &Check<'_>, declaration: u32) -> bool {
    c.collect_direct(declaration)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::ExpressionBody)
}

/// The written return type of a `Method`/`TopLevelMethod` row is `double`
/// (the `isDartCoreDoubleTypeAnnotation` face :77-79 — a written name;
/// nullability does not change the core-type verdict). The return type
/// tiles in the declaration's leading children (the `Type`/`VoidType`
/// row before the name and parameter list); a `Type` child answers.
fn return_type_is_double(c: &Check<'_>, declaration: u32) -> bool {
    c.collect_direct(declaration)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
        .is_some_and(|head| type_face(c, head).name == Some("double"))
}

/// The written type of the enclosing declaration row is `double`: the
/// nearest preceding `Type` sibling in the owner slice (the
/// `block_binding` production shape — locals keep the type as a block
/// piece; fields and top-level variables as a member piece).
fn variable_type_is_double(c: &Check<'_>) -> bool {
    let Some(&vars) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::VariablesDeclaration)
    else {
        return false;
    };
    let Some(type_row) = preceding_type_row(c, vars) else {
        return false;
    };
    type_face(c, type_row).name == Some("double")
}

/// The nearest preceding type-row sibling of `row` in its owner's child
/// list.
fn preceding_type_row(c: &Check<'_>, row: u32) -> Option<u32> {
    let owner = c.unit.ast.parent(row)?;
    let kids = c.collect_direct(owner);
    let pos = kids.iter().position(|&kid| kid == row)?;
    kids[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
}

/// The typed-list face over an owner slice (`<double>[e]` :44-49): the
/// literal completes before the `ListLiteral` row completes, so it tiles
/// as a sibling of the `TypeArguments` + `ListLiteral` pair in the
/// owner (`VariableInitializer`) — the literal's own list marker is the
/// `ListLiteral` row that follows it and is preceded by a `TypeArguments`.
fn list_context_is_double(c: &Check<'_>, node: u32, owner: u32) -> bool {
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return false;
    };
    // The typed-list run is `TypeArguments` … literal … `ListLiteral`:
    // the element literal tiles between the argument row and the
    // completing `ListLiteral` marker in the owner slice.
    let args_ok = pos
        .checked_sub(1)
        .and_then(|i| kids.get(i))
        .is_some_and(|&args| {
            c.kind(args) == NodeKind::TypeArguments && type_args_is_double(c, args)
        });
    args_ok
        && kids
            .get(pos + 1)
            .is_some_and(|&kid| c.kind(kid) == NodeKind::ListLiteral)
}

/// The `TypeArguments` row's single type is `double` (:45-48).
fn type_args_is_double(c: &Check<'_>, args: u32) -> bool {
    let types: Vec<u32> = c
        .collect_direct(args)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::Type)
        .collect();
    types.len() == 1 && type_face(c, types[0]).name == Some("double")
}

/// The single type argument of the typed list literal is `double`
/// (:44-49): the `TypeArguments` sibling precedes the literal row in the
/// owner slice (the `type_arguments_sibling` production shape).
fn list_element_type_is_double(c: &Check<'_>, list: u32) -> bool {
    let Some(owner) = c.unit.ast.parent(list) else {
        return false;
    };
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == list) else {
        return false;
    };
    let Some(&args) = pos
        .checked_sub(1)
        .and_then(|i| kids.get(i))
        .filter(|&&args| c.kind(args) == NodeKind::TypeArguments)
    else {
        return false;
    };
    type_args_is_double(c, args)
}
