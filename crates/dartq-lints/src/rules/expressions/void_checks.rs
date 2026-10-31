use dartq_semantic::model::{ArgListId, Ty};

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, same_file, slice, static_type_faces,
    value_run_end,
};
use crate::rules::{TypeFace, type_face, type_face_of_field};
/// Official: `pkg/linter/lib/src/rules/void_checks.dart` — a value
/// written where `void` (or `FutureOr<void>`) is expected reports
/// (:141-173, the `_check` core). The ported faces:
///
/// - the assignment branch (:76-84): assigning to a `void`- or
///   `FutureOr<void>`-typed target, reported over the assignment
///   (:156-158);
/// - the return branch (:106-133): a `FutureOr<void>` function's
///   `return e;` whose value is not acceptable reports at the
///   statement (:150-153). A plain `void` expectation never reports
///   (:142-146: every non-dynamic value early-silents, and `dynamic`
///   is void-acceptable), so only the `FutureOr<void>` face is live;
///   a function literal's return needs the inferred function type and
///   stays silent;
/// - the argument branches (:87-103): a `void` formal — a method or
///   function invocation through the bridge signature, an instance
///   creation through the resolved constructor formals — with a
///   decidable non-acceptable value reports at the argument
///   expression (:175-187).
///
/// The acceptability cut is `isTypeAcceptableWhenExpectingVoid` /
/// `...FutureOrVoid` (:40-65): dynamic, `null`, `void`, `Never` and
/// the `Future`/`FutureOr` family pass — a `Future<T>` needs its type
/// argument, unreadable through the bridge, so the family
/// conservatively passes (the under-report direction); `Unknown`
/// (inferred) values pass. Assignment targets resolve
/// shadow-correct: the same-file binding walk decides a bare name (a
/// local of the same name shadows the field), a `this.` target is
/// the field itself. Value types read off the literal faces and the
/// bridge expression face; undecidable values stay silent. The
/// pattern branch (:68-74, `matchedValueType`) needs the pattern's
/// matched value type and stays unported. Message: `diagnostic.g.dart`
/// `voidChecks`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "void_checks",
    &[
        NodeKind::Assignment,
        NodeKind::PatternAssignment,
        NodeKind::ReturnStatement,
        NodeKind::Send,
        NodeKind::NewExpression,
        NodeKind::ImplicitCreation,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Assignment => assignment_face(c),
        NodeKind::PatternAssignment => pattern_assignment_face(c),
        NodeKind::ReturnStatement => return_face(c),
        NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation => argument_face(c),
        _ => {}
    }
}

/// The assignment face (:76-84): the target's written type decides —
/// `void` reports a decidable non-acceptable value, `FutureOr<void>`
/// runs the wider acceptability set (:40-53).
fn assignment_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    if c.lexeme(op) != "=" {
        return;
    }
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    // The pattern-assignment face (`visitAssignedVariablePattern`,
    // :68-74): a `[p] = …` / `(p,) = …` left side reports each
    // void-typed bound variable at the variable itself, when the
    // matched value is not void-acceptable. The pattern pieces tile
    // flat inside the left run — the `AssignedVariablePattern` rows are
    // siblings of the pattern root, not its children.
    if left
        .iter()
        .any(|&row| matches!(c.kind(row), NodeKind::ListPattern | NodeKind::RecordPattern))
    {
        pattern_face(c, c.semantics(), &left, &right);
        return;
    }
    // The target: bare `<field>` or `this.<field>` or `<recv>.<field>`.
    let run = slice::access_run(c, &left);
    let (name, this_prefixed, receiver) = match run.as_slice() {
        [single] if c.kind(*single) == NodeKind::Identifier => {
            (c.lexeme(c.head_token(*single)), false, None)
        }
        [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
            (c.lexeme(c.head_token(*name)), true, None)
        }
        // The member-write target `recv.<field>` (`a.x = 1`): the
        // field's written type decides through the receiver's declared
        // type (`visitAssignmentExpression`'s `writeType`).
        [receiver, name] if c.kind(*receiver) == NodeKind::Identifier => {
            (c.lexeme(c.head_token(*name)), false, Some(*receiver))
        }
        _ => return,
    };
    // Shadow-correct target type: a bare name goes through the
    // same-file binding walk (a local of the same name shadows the
    // field); a `this.` target is the field itself; a member write
    // reads the field off the receiver's declared type.
    let face = if let Some(receiver) = receiver {
        member_write_face(c, node, receiver, name)
    } else if this_prefixed {
        type_face_of_field(c, name)
    } else {
        match same_file::binding(c, node, name) {
            Some((_, Some(row))) => Some(type_face(c, row)),
            // An untyped binding (`var x`) carries no verdict.
            Some((_, None)) => None,
            None => type_face_of_field(c, name),
        }
    };
    let Some(face) = face else {
        return;
    };
    if !face.is_void && !face.is_future_or_void {
        return;
    }
    // The value's static type: the literal faces, the bridge expression
    // face (a bare read, a callee return, a single-name member access),
    // then the written-scalar face for an unmounted bridge. Undecidable
    // values stay silent (`rightHandSide.staticType` null bails).
    let Some(analysis) = c.semantics() else {
        // The unmounted run keeps the written-scalar face: `null` is
        // acceptable when expecting void (:57), any scalar is not.
        if right_scalar_face(c, &right).is_some_and(|face| face != "Null") {
            c.report(assignment_report_range(c, node, &left, &right), &[]);
        }
        return;
    };
    let right_type = literal_ty(c, analysis, &right)
        .or_else(|| static_type_faces::expr_type(c, analysis, &right))
        .or_else(|| {
            right_scalar_face(c, &right).map(|name| Ty::Named {
                name: analysis.strings().intern(name),
                nullable: false,
                args: ArgListId::EMPTY,
            })
        });
    let Some(right_type) = right_type else {
        return;
    };
    // The acceptability cut splits on the target (`_check` :141-173):
    // a `void` target runs `isTypeAcceptableWhenExpectingVoid` (:55-65),
    // a `FutureOr<void>` target the wider set (:30-43).
    let acceptable = if face.is_void {
        acceptable_when_void_expected(analysis, right_type)
    } else {
        acceptable_when_future_or_void_expected(analysis, right_type)
    };
    if acceptable {
        return;
    }
    // The official reports at the assignment expression; the
    // production row opens at the right side's head, so the left
    // run's head bounds the start, and the row's span swallows the
    // `;`, so the right run's last value row bounds the end.
    let range = assignment_report_range(c, node, &left, &right);
    c.report(range, &[]);
}

/// The reported span of an assignment expression: from the left run's
/// head to the right run's last value row (the container row's own
/// span swallows the statement's `;`, and the statement-level
/// completion wrappers — the send group's `Send` and its argument/
/// type-argument markers — finalize at that `;`).
fn assignment_report_range(
    c: &Check<'_>,
    node: u32,
    left: &[u32],
    right: &[u32],
) -> core::ops::Range<u32> {
    c.utf16_span(left[0]).start..value_run_end(c, right, node)
}

/// The return face (:106-133): only a `FutureOr<void>` written return
/// type ever reports (:142-146 silents every plain-`void` return).
/// The official reports at the return statement (:150-153).
fn return_face(c: &mut Check<'_>) {
    let node = c.node();
    // The enclosing function shape (a method, a top-level or local
    // function declaration); a function literal's return needs the
    // inferred function type and stays silent (:109-121).
    let Some(function) = c.path.iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::TopLevelMethod | NodeKind::Method | NodeKind::LocalFunctionDeclaration
        )
    }) else {
        return;
    };
    let Some(face) = written_return_face(c, function) else {
        return;
    };
    if !face.is_future_or_void && !face.is_void {
        return;
    }
    let Some(analysis) = c.semantics() else {
        return;
    };
    // The value's static type; `return;` carries none and stays
    // silent (`node.expression?.staticType` is null). The value rows
    // can tile as the statement's siblings under the block (the
    // send-group adoption), so the run reads the owner's window inside
    // the statement's extent.
    let extent = c.byte_span(node);
    let mut rows = c.collect_direct(node);
    if let Some(parent) = c.unit.ast.parent(node) {
        for kid in c.collect_direct(parent) {
            if kid == node || rows.contains(&kid) {
                continue;
            }
            let span = c.byte_span(kid);
            if span.start >= extent.start && span.end <= extent.end {
                rows.push(kid);
            }
        }
    }
    rows.sort_by_key(|&row| c.byte_span(row).start);
    let Some(ty) =
        literal_ty(c, analysis, &rows).or_else(|| static_type_faces::expr_type(c, analysis, &rows))
    else {
        return;
    };
    // The return-statement cut (`_check` :141-173): a `void` expectation
    // reports only the `dynamic` value — the
    // `expectedType is VoidType && type is! DynamicType && ReturnStatement`
    // bail silents every decidable non-dynamic value upstream; a
    // `FutureOr<void>` expectation runs the wider acceptability set.
    let acceptable = if face.is_void {
        ty != Ty::Dynamic
    } else {
        acceptable_when_future_or_void_expected(analysis, ty)
    };
    if acceptable {
        return;
    }
    c.report(c.utf16_span(node), &[]);
}

/// The argument faces (:87-103): a `void` formal — a method or
/// function invocation through the bridge signature, an instance
/// creation through the resolved constructor formals — with a
/// decidable non-acceptable value reports at the argument expression
/// (:175-187).
fn argument_face(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    let (formals, arguments, type_vars, subst) = match c.kind(node) {
        NodeKind::Send => {
            let Some((kids, pos)) = slice::owner_slice(c) else {
                return;
            };
            let Some((head, Some(arguments))) = slice::send_parts(c, &kids, pos) else {
                return;
            };
            // A constructor invocation tiles as a plain send (`K(1)`)
            // — when no function signature answers, the resolved
            // constructor's formals do (the official
            // InstanceCreationExpression branch). The container's type
            // variables ride along so a bare `T` formal substitutes
            // through the invocation's own `TypeArguments`
            // (`A<void>.c(p)`).
            let (formals, type_vars) = match analysis
                .function_signature_of(query_site(c, kids[head]))
            {
                Some(sig) => (sig.formals, Vec::new()),
                None => {
                    let Some(binding) =
                        analysis.resolve_constructor(query_site(c, kids[head]), None)
                    else {
                        return;
                    };
                    let Some(formals) = binding.formals else {
                        return;
                    };
                    let vars = class_type_vars(c, analysis, binding.container);
                    (formals, vars)
                }
            };
            let subst = invocation_type_args(c, &kids[..pos]);
            (formals, arguments, type_vars, subst)
        }
        NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            let kids = c.collect_direct(node);
            let Some(args_pos) = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::Arguments)
            else {
                return;
            };
            let Some(callee) = kids[..args_pos].iter().rev().find_map(|&kid| {
                match c.kind(kid) {
                    NodeKind::Identifier => Some(kid),
                    // The explicit-new shape: the marker owns the
                    // callee name.
                    NodeKind::ConstructorReference => c
                        .collect_direct(kid)
                        .into_iter()
                        .find(|&k| c.kind(k) == NodeKind::Identifier),
                    _ => None,
                }
            }) else {
                return;
            };
            let Some(binding) = analysis.resolve_constructor(query_site(c, callee), None) else {
                return;
            };
            let Some(formals) = binding.formals else {
                return;
            };
            let type_vars = class_type_vars(c, analysis, binding.container);
            let subst = invocation_type_args(c, &kids[..args_pos]);
            (formals, kids[args_pos], type_vars, subst)
        }
        _ => return,
    };
    let pieces = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &pieces);
    let mut positional = 0usize;
    for range in &ranges {
        // The formal at the dispatch slot: positional by order, named
        // by label. A named argument flattens as `name value…
        // NamedArgument` (the marker anchors at the value head).
        let (formal_ty, value) = if c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument {
            let Some(name_idx) = slice::named_arg_name(c, &pieces, range.end - 1) else {
                continue;
            };
            let label = analysis
                .strings()
                .intern(c.lexeme(c.head_token(pieces[name_idx])));
            let Some(formal) = formals.named_formal(label) else {
                continue;
            };
            (formal.ty, &pieces[name_idx + 1..range.end - 1])
        } else {
            let Some(formal) = formals.positional.get(positional) else {
                break;
            };
            positional += 1;
            (formal.ty, &pieces[range.start..range.end])
        };
        // A bare type-variable formal (`A<void>.c(p)` — `T value`)
        // decides through the invocation's own type arguments: the
        // class's parameter index names the substitution slot
        // (:87-103 — `parameterElement.type` reads the instantiated
        // signature).
        let formal_is_void = formal_ty == Ty::Void
            || match formal_ty {
                Ty::Named { name, .. } => {
                    let var = analysis.strings().resolve(name);
                    type_vars
                        .iter()
                        .position(|v| v == var)
                        .and_then(|index| subst.get(index))
                        .is_some_and(|arg| arg == "void")
                }
                _ => false,
            };
        if !formal_is_void {
            continue;
        }
        // The value's static type; undecidable values stay silent
        // (`expression.staticType` null bails).
        let Some(ty) = literal_ty(c, analysis, value)
            .or_else(|| static_type_faces::expr_type(c, analysis, value))
        else {
            continue;
        };
        if acceptable_when_void_expected(analysis, ty) {
            continue;
        }
        let start = c.utf16_span(value[0]).start;
        let end = value_run_end(c, value, *value.last().unwrap_or(&c.node()));
        c.report(start..end, &[]);
    }
}

/// The `PatternAssignment` row face (`addAssignedVariablePattern` +
/// `addAssignmentExpression`, :30-36): the childless row anchors over the
/// `=` (`Equals` word); the pattern pieces tile as the parent's left
/// siblings, the value pieces to its right — the same peel
/// `parameter_assignments`'s pattern arm walks.
fn pattern_assignment_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some(equals) = c.field(node, PayloadField::Equals) else {
        return;
    };
    let eq_start = c
        .unit
        .tokens
        .get(equals as usize)
        .map(|t| t.byte_offset as usize);
    let Some(eq_start) = eq_start else {
        return;
    };
    let eq_end = c
        .unit
        .tokens
        .get(equals as usize)
        .map(|t| t.byte_offset as usize + t.byte_len as usize);
    let (Some(parent), Some(eq_end)) = (c.unit.ast.parent(node), eq_end) else {
        return;
    };
    let siblings = c.collect_direct(parent);
    let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
        return;
    };
    // The pattern and the value pieces tile together BEFORE the marker
    // row under the statement — the left run ends at the `=`, the right
    // run opens after it (the same byte peel as the `Operator` face).
    let run = slice::statement_run(c, &siblings[..pos]);
    let left: Vec<u32> = run
        .iter()
        .copied()
        .filter(|&row| c.byte_span(row).end <= eq_start)
        .collect();
    let right: Vec<u32> = run
        .iter()
        .copied()
        .filter(|&row| c.byte_span(row).start >= eq_end)
        .collect();
    pattern_face(c, c.semantics(), &left, &right);
}

/// The pattern-assignment face (`visitAssignedVariablePattern`, :75-81):
/// each `AssignedVariablePattern` inside the left pattern whose bound
/// variable is `void`- or `FutureOr<void>`-typed reports at the variable
/// when the matched value type is not acceptable — the matched type reads
/// the right literal's element face (a `<int>[…]`'s `int`, a `(7,)`'s
/// first field). The bound variables tile as siblings of the pattern
/// root (the `AssignedVariablePattern` row's head IS the variable name —
/// the production tiling keeps it flat under the statement, not nested
/// under the pattern row). Undecidable matched types stay silent.
fn pattern_face(
    c: &mut Check<'_>,
    analysis: Option<&dartq_semantic::Analysis<'_>>,
    left: &[u32],
    right: &[u32],
) {
    let Some(analysis) = analysis else {
        return;
    };
    let is_list = left
        .iter()
        .any(|&row| c.kind(row) == NodeKind::ListPattern);
    // The matched value type: a list pattern reads the right literal's
    // element type (`<int>[7]` → `int`); a record pattern the first
    // positional field's literal type. Anything undecidable bails.
    let matched = if is_list {
        right_literal_element_ty(c, analysis, right)
    } else {
        right_record_field_ty(c, analysis, right)
    };
    let Some(matched) = matched else {
        return;
    };
    for &name_row in left {
        if c.kind(name_row) != NodeKind::AssignedVariablePattern {
            continue;
        }
        let name = c.lexeme(c.head_token(name_row));
        let face = match same_file::binding(c, name_row, name) {
            Some((_, Some(row))) => Some(type_face(c, row)),
            _ => type_face_of_field(c, name),
        };
        let Some(face) = face else { continue };
        if !face.is_void && !face.is_future_or_void {
            continue;
        }
        let acceptable = if face.is_void {
            acceptable_when_void_expected(analysis, matched)
        } else {
            acceptable_when_future_or_void_expected(analysis, matched)
        };
        if !acceptable {
            c.report(c.utf16_span(name_row), &[]);
        }
    }
}

/// The right literal's element type for a `[p]` match (`<int>[7]` →
/// `int`, `<int>{…}` likewise; an untyped literal stays undecidable).
fn right_literal_element_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    right: &[u32],
) -> Option<Ty> {
    let lit = right.iter().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
        )
    })?;
    let args = c
        .collect_direct(lit)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeArguments)?;
    let arg = c
        .collect_direct(args)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)?;
    let face = type_face(c, arg);
    face.name.map(|name| Ty::Named {
        name: analysis.strings().intern(name),
        nullable: face.nullable,
        args: ArgListId::EMPTY,
    })
}

/// The first positional field's type of a `(7,)` record literal (the
/// `(p,)` match reads the positional slot).
fn right_record_field_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    right: &[u32],
) -> Option<Ty> {
    let lit = right
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::RecordLiteral)?;
    let kids = c.collect_direct(lit);
    literal_ty(c, analysis, &kids)
}

/// The declared type-parameter names of the same-file class `container`
/// (`class A<T>` → `["T"]`), in declaration order — the substitution
/// slots a bare type-variable formal reads.
fn class_type_vars(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    container: dartq_source::StringId,
) -> Vec<String> {
    let name = analysis.strings().resolve(container);
    let Some(class_row) = crate::rules::same_file_declaration(c, name) else {
        return Vec::new();
    };
    let Some(vars) = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
    else {
        return Vec::new();
    };
    c.collect_direct(vars)
        .into_iter()
        .flat_map(|tv| c.collect_direct(tv))
        .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.lexeme(c.head_token(kid)).to_owned())
        .collect()
}

/// The invocation site's explicit type-argument names (`A<void>.c(p)` →
/// `["void"]`), in order — the substitution values a generic
/// container's bare `T` formal reads.
fn invocation_type_args(c: &Check<'_>, pieces: &[u32]) -> Vec<String> {
    let Some(ta) = pieces
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::TypeArguments)
    else {
        return Vec::new();
    };
    c.collect_direct(ta)
        .into_iter()
        .filter(|&arg| c.kind(arg) == NodeKind::Type)
        .map(|arg| c.lexeme(c.head_token(arg)).to_owned())
        .collect()
}

/// The member-write target's written face (`a.x = v`): the receiver's
/// same-file declared type names the class; the field's written type is
/// the face. A generic setter's type-variable parameter substitutes
/// through the receiver's written type arguments (`A<void> a; a.f = v`
/// — `set f(T value)` decides through `void`).
fn member_write_face<'a>(
    c: &'a Check<'_>,
    node: u32,
    receiver: u32,
    name: &str,
) -> Option<TypeFace<'a>> {
    let recv_name = c.lexeme(c.head_token(receiver));
    let Some((_, Some(type_row))) = same_file::binding(c, node, recv_name) else {
        return None;
    };
    let recv_face = type_face(c, type_row);
    let class_name = recv_face.name?;
    let class_row = crate::rules::same_file_declaration(c, class_name)?;
    // A plain field's written face: the `Fields` member whose
    // declaration names `name` — the type row precedes the variable row
    // inside the `Fields` window.
    let members = crate::rules::body_members(c, class_row);
    let field_face = members.iter().copied().find_map(|member| {
        if !matches!(
            c.kind(member),
            NodeKind::Fields | NodeKind::VariablesDeclaration | NodeKind::TopLevelFields
        ) {
            return None;
        }
        let name_row = c.collect_direct(member).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
        })?;
        crate::rules::type_before(c, member, name_row)
            .map(|(t, _)| type_face(c, t))
    });
    if field_face.is_some() {
        return field_face;
    }
    // The generic-setter face: `set f(T value)` — the parameter's bare
    // type variable substitutes through the receiver's written type
    // arguments at the class's parameter index.
    let setter = members.iter().copied().find(|&member| {
        c.kind(member) == NodeKind::Method
            && crate::rules::get_set_token(c, member) == Some("set")
            && c.collect_direct(member).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
            })
    })?;
    let params = c
        .collect_direct(setter)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
    let param_type = c
        .collect_direct(params)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)?;
    let var_name = c.lexeme(c.head_token(param_type));
    // The class's type-parameter list (`class A<T>`) — the index of the
    // bare `T` names the substitution slot.
    let type_params = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)?;
    let param_names: Vec<&str> = c
        .collect_direct(type_params)
        .into_iter()
        .flat_map(|tv| c.collect_direct(tv))
        .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.lexeme(c.head_token(kid)))
        .collect();
    let index = param_names.iter().position(|&p| p == var_name)?;
    let &(subst_name, subst_nullable) = recv_face.type_arguments.get(index)?;
    Some(TypeFace {
        name: Some(subst_name),
        nullable: subst_nullable,
        is_void: subst_name == "void",
        is_future_or_void: false,
        is_function_type: false,
        type_arguments: Vec::new(),
    })
}

/// The written return type of a method-like row, as its full
/// [`TypeFace`] (the type row preceding the declaration name). Shared
/// with the `implicit_call_tearoffs` return face.
pub(crate) fn written_return_face<'a>(c: &'a Check<'_>, function: u32) -> Option<TypeFace<'a>> {
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
    // A function-typed return (`void Function() f()`) tiles as the
    // `FunctionType` row; a bare `void` return tiles as `VoidType` (the
    // `void f(dynamic p)` return face reports a `dynamic` value —
    // `type_face` on `VoidType` answers `is_void`). The caller's face
    // gate decides whether the face answers (the implicit_call_tearoffs
    // return face rejects non-function types; the void_checks cut reads
    // `is_void`/`is_future_or_void`).
    (matches!(
        kind,
        NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
    ))
    .then(|| type_face(c, type_row))
}

/// The value type the literal faces answer: `null` and the scalar
/// literals (the `right_scalar_face` shape, as a [`Ty`]).
fn literal_ty(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, rows: &[u32]) -> Option<Ty> {
    let run = slice::access_run(c, rows);
    let [root] = run.as_slice() else {
        return None;
    };
    let named = |name: &str| Ty::Named {
        name: analysis.strings().intern(name),
        nullable: false,
        args: ArgListId::EMPTY,
    };
    match c.kind(*root) {
        NodeKind::LiteralNull => Some(Ty::Null),
        NodeKind::LiteralInt => Some(named("int")),
        NodeKind::LiteralDouble => Some(named("double")),
        NodeKind::StringLiteral => Some(named("String")),
        NodeKind::LiteralBool => Some(named("bool")),
        _ => None,
    }
}

/// The acceptability cut the `void` expectation shares (:55-65):
/// dynamic, `null`, `void`, `Never` and the `Future` family pass. A
/// `Future<T>` needs its type argument — unreadable through the
/// bridge — so the family conservatively passes; `FutureOr` is **not**
/// void-acceptable upstream (:60-63 — only a `Future` head recurses)
/// and reports; `Unknown` (inferred) values pass.
fn acceptable_when_void_expected(analysis: &dartq_semantic::Analysis<'_>, ty: Ty) -> bool {
    match ty {
        // `isTypeAcceptableWhenExpectingVoid` (:55-65) has no `Dynamic`
        // arm — only the `FutureOr` variant passes `dynamic` (:45), so a
        // `void` formal rejects a `dynamic` argument upstream.
        Ty::Null | Ty::Void | Ty::Never | Ty::Unknown => true,
        Ty::Named { name, .. } => analysis.strings().resolve(name) == "Future",
        // A function-typed value is no `Future` head — it reports.
        Ty::Dynamic | Ty::Record { .. } | Ty::Function { .. } => false,
    }
}

/// The wider `FutureOr<void>` cut (:30-43): the void set plus
/// `FutureOr` itself (`type.isDartAsyncFutureOr`); a `FutureOr<T>`
/// needs its argument — unreadable — so the head conservatively passes.
fn acceptable_when_future_or_void_expected(
    analysis: &dartq_semantic::Analysis<'_>,
    ty: Ty,
) -> bool {
    match ty {
        Ty::Named { name, .. } => {
            matches!(analysis.strings().resolve(name), "Future" | "FutureOr")
        }
        _ => true,
    }
}

/// The right side's face: `Null` for a null literal, the scalar type
/// for scalar literals and for bare names whose same-file written
/// type is one of the scalars; `None` (no verdict) otherwise.
fn right_scalar_face<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
    let run = slice::access_run(c, rows);
    let [root] = run.as_slice() else {
        return None;
    };
    match c.kind(*root) {
        NodeKind::LiteralNull => Some("Null"),
        NodeKind::LiteralInt => Some("int"),
        NodeKind::LiteralDouble => Some("double"),
        NodeKind::StringLiteral => Some("String"),
        NodeKind::LiteralBool => Some("bool"),
        NodeKind::Identifier => {
            let name = c.lexeme(c.head_token(*root));
            match crate::rules::written_type_face(c, c.node(), name)?.name {
                Some(scalar @ ("int" | "double" | "String" | "bool")) => Some(scalar),
                _ => None,
            }
        }
        _ => None,
    }
}
