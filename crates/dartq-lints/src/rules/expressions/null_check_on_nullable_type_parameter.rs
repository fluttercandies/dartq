use dartq_semantic::model::Ty;

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, switch_scrutinee_read,
    written_type_sibling,
};
use crate::rules::utf16_token_span;

/// Official: `pkg/linter/lib/src/rules/null_check_on_nullable_type_parameter.dart`
/// — a `!` whose operand's static type is a nullable type parameter
/// `T?`, in a context whose expected type is that same `T` after
/// promotion (`promoteToNonNull`) and potentially nullable (:62-68: a
/// written `T?` always; a bare `T` whose bound does not pin it
/// non-nullable — no bound, `dynamic`, `Object?` — while `T extends
/// Object` stays silent), reports at the operator.
///
/// The context face (`getExpectedType`, `unnecessary_null_checks.dart`)
/// ports for seven syntax/resolution-knowable sites: a `return` or
/// arrow body whose nearest function ancestor is a function shape —
/// the official lookup is `thisOrAncestorOfType<FunctionExpression>`
/// (:36-41), which class methods never satisfy (their returns carry no
/// expected type, so `T? m() { return f!; }` stays silent exactly like
/// the oracle) — a variable initializer with the declaration's written
/// type, an assignment whose bare/`this.` target carries the written
/// type (the official `writeType` face; a member-chain target
/// under-reports), an argument whose callee's bridge signature
/// formal declares it (positional by slot, named by label), a
/// collection member whose literal sits under the return, a `yield`
/// through the generator's first type argument, and the `await`
/// wrapper (`withAwait` — one `Future`/`FutureOr` flatten at the outer
/// face). The collection/yield faces are the `unnecessary_null_checks`
/// helpers verbatim, so the two rules cannot drift apart. The
/// pattern face (`visitNullAssertPattern`, :52-56 — `matchedValueType`)
/// rides [`Analysis::matched_value_type`] keyed at the switch scrutinee
/// read's own span: the matched type must be the nullable type
/// parameter itself. The operand face reads the written type: a bare
/// name through the shadow-correct same-file binding walk (a local, a
/// parameter, a top-level), a `this.`-prefixed operand the same-class
/// field face, and the head must be an in-scope type parameter
/// (class-like and method-like ancestors). Message: `diagnostic.g.dart`
/// `nullCheckOnNullableTypeParameter`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "null_check_on_nullable_type_parameter",
    // The `!` tiles as the `NonNullAssert` token leaf (bridge
    // `handle_non_null_assert_expression`); `UnaryPostfix` is `++`/`--`.
    // The pattern `!` is the `NullAssertPattern` token leaf (bridge
    // `handle_null_assert_pattern`).
    &[NodeKind::NonNullAssert, NodeKind::NullAssertPattern],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if c.kind(node) == NodeKind::NullAssertPattern {
        check_pattern(c);
        return;
    }
    // `node.operator.type != BANG` bails (:56).
    if c.lexeme(c.head_token(node)) != "!" {
        return;
    }
    // The operand rows precede the leaf (the owner-slice idiom). An
    // assignment's window spans the left target too — the right run
    // splits at the `=` ([`assignment_operand_rows`], the
    // `void_checks` idiom keyed at the container row) minus the
    // dispatch leaf.
    let parent = c.ancestor(1);
    let operand_rows: Vec<u32> = match parent {
        Some(parent) if c.kind(parent) == NodeKind::Assignment => {
            let Some((_, right)) = assignment_operands(c, parent) else {
                return;
            };
            let Some(bang) = c
                .unit
                .tokens
                .get(c.head_token(node) as usize)
                .map(|t| t.byte_offset as usize)
            else {
                return;
            };
            right
                .into_iter()
                .filter(|&row| row != node && c.byte_span(row).end <= bang)
                .collect()
        }
        Some(parent) => {
            let kids = c.collect_direct(parent);
            let Some(pos) = kids.iter().position(|&kid| kid == c.node()) else {
                return;
            };
            // The arrow posture roots the body rows flat in the
            // declaration's own window (the childless `ExpressionBody`
            // handle trails them): the operand read tiles as its own
            // bare-send group ending just before the leaf, and the
            // name row is the group's head (`send_parts`) — the whole
            // window would drown the read in declaration-header rows.
            if kids.len() >= 2 && c.kind(kids[kids.len() - 1]) == NodeKind::ExpressionBody {
                // The arrow posture roots the body rows flat in the
                // declaration's own window (the childless
                // `ExpressionBody` handle trails them): the operand read
                // tiles as its own bare-send group ending just before
                // the leaf, and the name row is the group's head
                // (`send_parts`) — the whole window would drown the read
                // in declaration-header rows.
                match pos
                    .checked_sub(1)
                    .filter(|&send| c.kind(kids[send]) == NodeKind::Send)
                    .and_then(|send| slice::send_parts(c, &kids, send))
                {
                    Some((head, _)) => kids[head..head + 1].to_vec(),
                    None => Vec::new(),
                }
            } else {
                kids[..pos].to_vec()
            }
        }
        None => return,
    };
    let Some((name, this_prefixed)) = operand_field_name(c, &operand_rows) else {
        return;
    };
    // The operand's written type (`operand.staticType` is a nullable
    // type parameter, :66): a bare name resolves shadow-correct through
    // the same-file binding walk (a local, a parameter, a top-level);
    // a `this.`-prefixed operand is the field itself; an unbound bare
    // name falls back to the same-class field face. The head must be an
    // in-scope type parameter (`isNullableTypeParameterType`).
    let operand = if this_prefixed {
        crate::rules::type_face_of_field(c, name).and_then(|face| Some((face.name?, face.nullable)))
    } else {
        written_target_type(c, node, name)
    };
    let Some((type_parameter, nullable)) = operand else {
        return;
    };
    let parameters = in_scope_type_parameters(c);
    if !nullable || !parameters.iter().any(|(n, _)| *n == type_parameter) {
        return;
    }
    if !context_expects_same_nullable(c, &parameters, type_parameter, (name, this_prefixed)) {
        return;
    }
    let range = utf16_token_span(c, c.head_token(node));
    c.report(range, &[]);
}

/// `slice::binary_operands` keyed at an explicit operator container
/// row (the postfix `!` dispatch node is not the container, so the
/// generic helper — which reads the dispatch node's own payload —
/// cannot be used): the adopted window plus owner-sibling pieces,
/// split at the `=` token, with the lazy `Send` + `DotAccess`
/// completion pair pulled back to the left run. Returns `(left,
/// right)`.
fn assignment_operands(c: &Check<'_>, assignment: u32) -> Option<(Vec<u32>, Vec<u32>)> {
    let op_token = c.field(assignment, PayloadField::Operator)?;
    let op_start = c.unit.tokens.get(op_token as usize)?.byte_offset as usize;
    let window = c.children(assignment);
    let parent = c.unit.ast.parent(assignment)?;
    let kids = c.collect_direct(parent);
    let below: Vec<u32> = match kids.iter().position(|&kid| kid == assignment) {
        Some(pos) if pos > 0 => slice::statement_run(c, &kids[..pos]),
        _ => {
            let grand = c.unit.ast.parent(parent)?;
            let owner_kids = c.collect_direct(grand);
            let container_pos = owner_kids.iter().position(|&kid| kid == parent)?;
            slice::statement_run(c, &owner_kids[..container_pos])
        }
    };
    let mut rows: Vec<u32> = below
        .into_iter()
        .filter(|&kid| kid < window.start)
        .collect();
    rows.extend(window.start..window.end);
    let split = rows.partition_point(|&piece| c.byte_span(piece).end <= op_start);
    if split == 0 || split == rows.len() {
        return None;
    }
    let mut left = rows[..split].to_vec();
    let mut right = rows[split..].to_vec();
    if let Some(marker) = right.iter().position(|&piece| {
        c.kind(piece) == NodeKind::DotAccess
            && c.field(piece, PayloadField::Operator).is_some_and(|dot| {
                c.unit
                    .tokens
                    .get(dot as usize)
                    .is_some_and(|t| (t.byte_offset as usize) < op_start)
            })
    }) && marker >= 1
        && c.kind(right[marker - 1]) == NodeKind::Send
    {
        left.extend(right.drain(marker - 1..=marker));
    }
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

/// `visitNullAssertPattern` (:60-64): the `!` pattern whose matched
/// value type is a nullable type-parameter type reports at the
/// operator token — the pattern leaf's own head
/// (`reportAtToken(node.operator)`). The matched type rides
/// [`Analysis::matched_value_type`], keyed at the scrutinee read's own
/// span (not the pattern's): the enclosing switch's scrutinee must be
/// a lone identifier read — the face's read-row shape; other
/// scrutinee shapes and untyped reads under-report — and the matched
/// name must be the enclosing class's type parameter (the
/// element-level `TypeParameterType` cut at the name level). An
/// if-case pattern carries no switch ancestor here and stays silent.
fn check_pattern(c: &mut Check<'_>) {
    let Some(scrutinee) = switch_scrutinee_read(c) else {
        return;
    };
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some(Ty::Named {
        name,
        nullable: true,
        ..
    }) = analysis.matched_value_type(query_site(c, scrutinee))
    else {
        return;
    };
    let matched = analysis.strings().resolve(name);
    // The element-level `TypeParameterType` cut at the name level: any
    // in-scope type parameter (class-like and method-like ancestors)
    // — a same-named class is not a type parameter and stays silent.
    if !in_scope_type_parameters(c).iter().any(|(n, _)| *n == matched) {
        return;
    }
    let range = utf16_token_span(c, c.head_token(c.node()));
    c.report(range, &[]);
}

/// The operand's field name and shape: `(name, this-prefixed)` —
/// `this.<field>` or bare `<field>`, read through the access-run form
/// (the bare-name send group tiles beside the chain rows inside the
/// statement container).
fn operand_field_name<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<(&'a str, bool)> {
    match crate::rules::slice::access_run(c, rows).as_slice() {
        [single] if c.kind(*single) == NodeKind::Identifier => {
            Some((c.lexeme(c.head_token(*single)), false))
        }
        [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
            Some((c.lexeme(c.head_token(*name)), true))
        }
        _ => None,
    }
}

/// The type parameters in scope at the dispatch node — `(name, its
/// TypeVariable row)` pairs (the row carries the bound): every
/// class-like ancestor's type variables plus every method-like
/// ancestor's — a method sees both its own and the class's (`T?` in a
/// method body may name either; the same name answers the same verdict
/// at the use). A method's `TypeVariables` tiles as the member row's
/// immediate left sibling in the enclosing window (the production
/// parser opens the member at the name, so `<T>` completes just before
/// it), with hand-built trees nesting it inside the row.
fn in_scope_type_parameters<'a>(c: &'a Check<'_>) -> Vec<(&'a str, Option<u32>)> {
    let mut parameters = Vec::new();
    for &ancestor in c.path.iter() {
        match c.kind(ancestor) {
            NodeKind::ClassDeclaration | NodeKind::MixinDeclaration | NodeKind::EnumDeclaration => {
                let owner = c.unit.ast.parent(ancestor).unwrap_or(ancestor);
                let variables = c
                    .collect_direct(owner)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
                    .or_else(|| {
                        c.collect_direct(ancestor)
                            .into_iter()
                            .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
                    });
                if let Some(variables) = variables {
                    parameters.extend(type_variable_pairs(c, variables));
                }
            }
            NodeKind::TopLevelMethod | NodeKind::Method => {
                let nested = c
                    .collect_direct(ancestor)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables);
                if let Some(variables) = nested {
                    parameters.extend(type_variable_pairs(c, variables));
                    continue;
                }
                let Some(owner) = c.unit.ast.parent(ancestor) else {
                    continue;
                };
                let members = c.collect_direct(owner);
                let Some(index) = members.iter().position(|&kid| kid == ancestor) else {
                    continue;
                };
                // The immediate left sibling: the member's own
                // `<T>` completes directly before its row.
                if index > 0 && c.kind(members[index - 1]) == NodeKind::TypeVariables {
                    parameters.extend(type_variable_pairs(c, members[index - 1]));
                }
            }
            _ => {}
        }
    }
    parameters
}

/// The `(name, TypeVariable row)` pairs of one `TypeVariables` run: the
/// production tiling spells each name as a context-marked `Identifier`
/// directly before its span-only `TypeVariable` row (the row holds only
/// the bound, not the name); hand-built trees nest the name identifier
/// inside the row. A name with no row at all keeps `None` (an unbounded
/// parameter — the implicit `Object?` default).
fn type_variable_pairs<'a>(c: &'a Check<'_>, variables: u32) -> Vec<(&'a str, Option<u32>)> {
    let ctx = dartq_parser::IdentifierContext::TypeVariableDeclaration as u32;
    let kids = c.collect_direct(variables);
    let mut pairs = Vec::new();
    for (index, &kid) in kids.iter().enumerate() {
        match c.kind(kid) {
            NodeKind::Identifier if c.field(kid, PayloadField::Context) == Some(ctx) => {
                let variable = kids
                    .get(index + 1)
                    .copied()
                    .filter(|&row| c.kind(row) == NodeKind::TypeVariable);
                pairs.push((c.lexeme(c.head_token(kid)), variable));
            }
            NodeKind::TypeVariable => {
                if let Some(name) = c
                    .collect_direct(kid)
                    .into_iter()
                    .find(|&inner| c.kind(inner) == NodeKind::Identifier)
                {
                    pairs.push((c.lexeme(c.head_token(name)), Some(kid)));
                }
            }
            _ => {}
        }
    }
    pairs
}

/// Whether the context at the site expects the same type parameter
/// `expected` under the official predicate (:62-68): the expected type
/// is potentially nullable (`isPotentiallyNullable` — a written `T?`
/// always; a bare `T` when its bound does not pin it non-nullable) and
/// `promoteToNonNull(type) == promoteToNonNull(expectedType)` — a
/// written `T?` and the bare `T` both promote to `T`. `operand` is the
/// `!`'s operand `(name, this-prefixed)` — the official assignment
/// guard (`x = x!` promotes, so no expected type) needs it.
fn context_expects_same_nullable(
    c: &Check<'_>,
    parameters: &[(&str, Option<u32>)],
    expected: &str,
    operand: (&str, bool),
) -> bool {
    let Some((name, written_nullable)) = expected_type_type_parameter(c, operand) else {
        return false;
    };
    name == expected && (written_nullable || parameter_potentially_nullable(c, parameters, name))
}

/// `isPotentiallyNullable` at the name level (:66): a type parameter
/// with no bound row, no bound (the implicit `Object?` default), or a
/// `dynamic`/nullable-written bound is potentially nullable; a
/// non-null `Object`/named bound (`T extends Object`) is not.
fn parameter_potentially_nullable(
    c: &Check<'_>,
    parameters: &[(&str, Option<u32>)],
    name: &str,
) -> bool {
    parameters
        .iter()
        .find(|(parameter, _)| *parameter == name)
        .and_then(|&(_, variable)| variable)
        .and_then(|variable| bound_face(c, variable))
        .is_none_or(|(bound, bound_nullable)| bound == "dynamic" || bound_nullable)
}

/// The written `(name, nullable)` of a `TypeVariable` row's bound — the
/// `Type` row under it (`walk_type_variables`' read); `None` when the
/// parameter is unbounded.
fn bound_face<'a>(c: &'a Check<'_>, variable: u32) -> Option<(&'a str, bool)> {
    let bound = c
        .children(variable)
        .filter(|&row| {
            c.kind(row) == NodeKind::Type && c.unit.ast.parent(row) == Some(variable)
        })
        .next()?;
    let token = c.head_token(bound);
    let nullable = c.byte_span(bound).end > crate::rules::byte_span_of_token(c, token).end;
    Some((c.lexeme(token), nullable))
}

/// The context's expected type face `(name, written-nullable)`: the
/// nearest meaningful ancestor decides — a `return` or arrow body with
/// the enclosing function's written return type, a variable initializer
/// with the declaration's written type, an assignment whose target's
/// written type is the face (the official `writeType`), an argument
/// whose callee's formal table (the bridge signature face) declares
/// it, a collection member through the literal's element type, or a
/// `yield` through the generator's first type argument. The
/// potentially-nullable question answers at the caller. A block body
/// boundary first bails.
fn expected_type_type_parameter<'a>(
    c: &'a Check<'_>,
    operand: (&str, bool),
) -> Option<(&'a str, bool)> {
    // `getExpectedType`'s `withAwait` (:30-37): an `await` on the way
    // flattens one `Future`/`FutureOr` at the outer face.
    let mut awaited = false;
    for pair in c.path.windows(2).rev() {
        let ancestor = pair[0];
        let child = pair[1];
        match c.kind(ancestor) {
            NodeKind::AwaitExpression => {
                // The awaited expression's own window holds the literal
                // rows beside the `!` (a literal row completes after its
                // elements), so the member face answers here (:46-53).
                awaited = true;
                if let Some(face) = collection_face(c, ancestor, c.node(), awaited) {
                    return Some(face);
                }
            }
            // The map entry's element nesting is transparent
            // (`:129-137`): the for/if wrappers climb to their literal.
            NodeKind::ForControlFlow
            | NodeKind::IfControlFlow
            | NodeKind::ThenControlFlow
            | NodeKind::ElseControlFlow => {}
            NodeKind::VariableInitializer => {
                // `T? x = <e>!;` — the declaration's written type.
                let decl = c.unit.ast.parent(ancestor)?;
                return written_declaration_type(c, decl);
            }
            NodeKind::ReturnStatement | NodeKind::ExpressionBody => {
                // `return <e>!;` / `=> <e>!` — the enclosing function's
                // written return type (the official return and
                // `ExpressionFunctionBody` faces share the lookup).
                let function = enclosing_function(c, ancestor)?;
                return return_expected(c, function, awaited);
            }
            // `Iterable<T> f<T>(T? p) sync* { yield p!; }` (:59-72).
            NodeKind::YieldStatement => {
                return super::unnecessary_null_checks::yield_type(c);
            }
            // `List<T> f<T>(T? p) => [p!]` (:107-114) — the literal's
            // element type, read as the written return's head argument.
            NodeKind::ListLiteral => {
                return super::unnecessary_null_checks::member_type(c, 0, awaited);
            }
            NodeKind::SetOrMapLiteral => {
                let index = super::unnecessary_null_checks::map_index(c, ancestor, c.node());
                return super::unnecessary_null_checks::member_type(c, index, awaited);
            }
            NodeKind::MapEntry => {
                let index = super::unnecessary_null_checks::entry_side(c, ancestor, c.node());
                return super::unnecessary_null_checks::member_type(c, index, awaited);
            }
            NodeKind::TopLevelMethod
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression => {
                // The arrow posture: the expression rows tile as the
                // declaration's own tail children before the childless
                // `ExpressionBody` handle, so `=> <e>!` roots at the
                // function row itself (the `unnecessary_null_checks`
                // arrow face). Block bodies fall through — their
                // returns were caught at the `ReturnStatement` arm.
                let kids = c.collect_direct(ancestor);
                if kids.len() < 2 || c.kind(kids[kids.len() - 1]) != NodeKind::ExpressionBody {
                    continue;
                }
                let body = &kids[..kids.len() - 1];
                let awaited = awaited
                    || body
                        .iter()
                        .any(|&row| c.kind(row) == NodeKind::AwaitExpression);
                if let Some(face) = collection_face(c, ancestor, c.node(), awaited) {
                    return Some(face);
                }
                if body.last() == Some(&child) {
                    return return_expected(c, ancestor, awaited);
                }
            }
            NodeKind::Assignment => {
                // `x = <e>!;` — the target's written type (the
                // official writeType; a bare or `this.` target). The
                // official guard first: `x = x!` — a bare identifier
                // target with a bare same-named operand — promotes and
                // carries no expected type (:64-68).
                let Some((target, bare)) = assignment_target_name(c, ancestor) else {
                    return None;
                };
                if bare && !operand.1 && target == operand.0 {
                    return None;
                }
                return written_target_type(c, ancestor, target);
            }
            NodeKind::Arguments => {
                // `f(<e>!);` — the callee's formal table declares the
                // expected type (the official `getExpectedType`).
                return argument_context_type_parameter(c, ancestor);
            }
            NodeKind::Block | NodeKind::BlockBody => {
                return None; // a body boundary — no written context face
            }
            _ => {}
        }
    }
    None
}

/// The collection-member expected type when a literal row sits in
/// `owner`'s window with `node` inside it (:107-114 lists/sets,
/// :115-129 map entries; the element nesting is transparent). A literal
/// row completes AFTER its element rows, so it is located by the `!`'s
/// own position rather than by row order.
fn collection_face<'a>(
    c: &'a Check<'_>,
    owner: u32,
    node: u32,
    awaited: bool,
) -> Option<(&'a str, bool)> {
    let kids = c.collect_direct(owner);
    if let Some(literal) = kids.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
        ) && super::unnecessary_null_checks::belongs_to(c, row, node)
    }) {
        let index = if c.kind(literal) == NodeKind::SetOrMapLiteral {
            super::unnecessary_null_checks::map_index(c, literal, node)
        } else {
            0
        };
        return super::unnecessary_null_checks::member_type(c, index, awaited);
    }
    let entry = kids.iter().rev().copied().find(|&row| {
        c.kind(row) == NodeKind::MapEntry
            && super::unnecessary_null_checks::belongs_to(c, row, node)
    })?;
    let index = super::unnecessary_null_checks::entry_side(c, entry, node);
    super::unnecessary_null_checks::member_type(c, index, awaited)
}

/// The expected type at a whole-body or `return` site: the written
/// return type, flattened one level when the site awaits or the body is
/// `async` (:50-53 — only the `Future`/`FutureOr` wrapper decides).
fn return_expected<'a>(
    c: &'a Check<'_>,
    function: u32,
    awaited: bool,
) -> Option<(&'a str, bool)> {
    let face = written_return_face(c, function)?;
    if awaited
        || c.unit
            .ast
            .token_range(function)
            .any(|token| c.lexeme(token) == "async")
    {
        if !matches!(face.name, Some("Future" | "FutureOr")) {
            return None;
        }
        return face.type_arguments.first().copied();
    }
    Some((face.name?, face.nullable))
}

/// The bare or `this.`-qualified target name of an assignment whose
/// right side carries the dispatch node: the adopted window's left
/// run ([`assignment_operands`]; the window spans the left target, so
/// the owner siblings hold nothing of it). Answers `(name, bare)`.
fn assignment_target_name<'a>(c: &'a Check<'_>, assignment: u32) -> Option<(&'a str, bool)> {
    let (left, _) = assignment_operands(c, assignment)?;
    match slice::access_run(c, &left).as_slice() {
        [single] if c.kind(*single) == NodeKind::Identifier => {
            Some((c.lexeme(c.head_token(*single)), true))
        }
        [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
            Some((c.lexeme(c.head_token(*name)), false))
        }
        _ => None,
    }
}

/// The written `(name, nullable)` of the assignment target: the
/// shadow-correct same-file binding first, then the same-class field
/// face.
fn written_target_type<'a>(c: &'a Check<'_>, use_node: u32, name: &str) -> Option<(&'a str, bool)> {
    if let Some((_, Some(row))) = super::same_file::binding(c, use_node, name)
        && c.kind(row) == NodeKind::Type
    {
        let head = c.head_token(row);
        let nullable = c.byte_span(row).end > crate::rules::byte_span_of_token(c, head).end;
        return Some((c.lexeme(head), nullable));
    }
    let face = crate::rules::type_face_of_field(c, name)?;
    Some((face.name?, face.nullable))
}

/// The argument context: the callee's bridge signature formal type at
/// the dispatch node's own argument slot — a positional slot by index,
/// a named slot by label; the formal's face answers directly.
fn argument_context_type_parameter<'a>(
    c: &'a Check<'_>,
    arguments: u32,
) -> Option<(&'a str, bool)> {
    let analysis = c.semantics()?;
    let parent = c.unit.ast.parent(arguments)?;
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == arguments) else {
        return None;
    };
    // The invocation send is the sibling right after the argument list
    // (the production tiling).
    if c.kind(*kids.get(pos.checked_add(1)?)?) != NodeKind::Send {
        return None;
    }
    let (head, _) = crate::rules::slice::send_parts(c, &kids, pos + 1)?;
    let sig = analysis.function_signature_of(query_site(c, kids[head]))?;
    // The dispatch node's argument slot: the operand rows end at the
    // piece before the leaf (a bare-read operand tiles as a send group
    // and strands the `!` leaf outside its argument range, so the slot
    // range is the one covering `node_pos - 1`). A named argument
    // pairs `name … NamedArgument` immediately before the value range —
    // the formal reads by label there; otherwise positionally by the
    // count of preceding positional ranges.
    let raw = c.collect_direct(arguments);
    let ranges = crate::rules::slice::arguments(c, &raw);
    let node_pos = raw.iter().position(|&kid| kid == c.node())?;
    let index = ranges
        .iter()
        .position(|range| range.contains(&(node_pos - 1)))?;
    let named_pair = index
        .checked_sub(1)
        .and_then(|prev| ranges.get(prev))
        .filter(|prev| c.kind(raw[prev.end - 1]) == NodeKind::NamedArgument)
        .map(|prev| prev.end);
    let ty = if let Some(end) = named_pair {
        let name_idx = crate::rules::slice::named_arg_name(c, &raw, end - 1)?;
        let label = c.lexeme(c.head_token(raw[name_idx]));
        sig.formals
            .named_formal(analysis.strings().intern(label))?
            .ty
    } else {
        let slot = ranges[..index]
            .iter()
            .filter(|r| c.kind(raw[r.end - 1]) != NodeKind::NamedArgument)
            .count();
        sig.formals.positional.get(slot)?.ty
    };
    match ty {
        dartq_semantic::model::Ty::Named { name, nullable, .. } => {
            Some((analysis.strings().resolve(name), nullable))
        }
        _ => None,
    }
}

/// The enclosing function-shaped row of a statement: the official
/// return-face lookup is `thisOrAncestorOfType<FunctionExpression>`
/// (`unnecessary_null_checks.dart:36-41`) — a class method never
/// qualifies (the analyzer nests a `FunctionExpression` only under
/// function declarations and literals), so its returns carry no
/// expected type and stay silent.
fn enclosing_function(c: &Check<'_>, statement: u32) -> Option<u32> {
    let pos = c.path.iter().position(|&a| a == statement)?;
    c.path[..pos].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::TopLevelMethod | NodeKind::FunctionExpression
        )
    })
}

/// The written type sibling of a declaration row: `(name, nullable)`.
type WrittenType<'a> = (&'a str, bool);
fn written_declaration_type<'a>(c: &'a Check<'_>, decl: u32) -> Option<WrittenType<'a>> {
    let row = written_type_sibling(c, decl, &[NodeKind::Type, NodeKind::VoidType])?;
    let name_token = c.head_token(row);
    let nullable = c.byte_span(row).end > crate::rules::byte_span_of_token(c, name_token).end;
    Some((c.lexeme(name_token), nullable))
}

/// The written return type face of a method-like row.
fn written_return_face<'a>(c: &'a Check<'_>, function: u32) -> Option<crate::rules::TypeFace<'a>> {
    // The member-name identifier — a written return type's own name
    // tiles before it in the completion order, so the first
    // `Identifier` is the type, not the member.
    let kids = c.collect_direct(function);
    let is_declaration_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.field(kid, PayloadField::Context),
                Some(ctx)
                    if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                        || ctx
                            == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                as u32
            )
    };
    let name = kids
        .iter()
        .copied()
        .find(|&kid| is_declaration_name(kid))
        .or_else(|| {
            kids.iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        })?;
    let (type_row, kind) = crate::rules::type_before(c, function, name)?;
    if kind != NodeKind::Type {
        return None;
    }
    Some(crate::rules::type_face(c, type_row))
}
