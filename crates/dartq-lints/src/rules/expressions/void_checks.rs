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
///   statement (:150-153). A plain `void` expectation bails on every
///   non-`dynamic` value (:142-146) and then rejects `dynamic`
///   (`isTypeAcceptableWhenExpectingVoid` has no `DynamicType` arm), so a
///   `void` function's `return <dynamic>;` reports too; a function
///   literal's return needs the inferred function type and stays silent;
/// - the argument branches (:87-103): a `void` formal — a method or
///   function invocation through the bridge signature or the receiver's
///   resolved member, an instance creation through the resolved
///   constructor formals — with a decidable non-acceptable value reports
///   at the argument expression (:175-187);
/// - the pattern branch (:68-74, `matchedValueType`): an assigned
///   variable pattern whose bound variable is `void`- or
///   `FutureOr<void>`-typed reports at the variable when the matched
///   value type is not acceptable.
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
/// expected types are the *instantiated* ones the official reads
/// (`writeType`, `staticInvokeType`, the instantiated constructor's
/// `formalParameters`): a bare `T` in a generic container substitutes
/// through the receiver's written type arguments (`A<void> a` — `T
/// value`, `void m(T p)`, `set f(T value)`) or the invocation's own
/// (`A<void>.c(p)`). Message: `diagnostic.g.dart` `voidChecks`.
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
    let (is_void, is_future_or_void) = if let Some(receiver) = receiver {
        match member_write_expectation(c, node, receiver, name) {
            Some(flags) => flags,
            None => return,
        }
    } else {
        let face = if this_prefixed {
            type_face_of_field(c, name)
        } else {
            match binding_type_row(c, node, name) {
                Some(Some(row)) => Some(type_face(c, row)),
                // An untyped binding (`var x`) carries no verdict.
                Some(None) => return,
                None => type_face_of_field(c, name),
            }
        };
        let Some(face) = face else {
            return;
        };
        (face.is_void, face.is_future_or_void)
    };
    if !is_void && !is_future_or_void {
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
    let acceptable = if is_void {
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

/// The return face (:106-133): a `FutureOr<void>` written return type
/// reports every non-acceptable value; a plain `void` one reports a
/// `dynamic` value only (:142-146 bails on the non-`dynamic` values,
/// and the void cut then rejects `dynamic`). The official reports at the
/// return statement (:150-153).
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
    let (formals, arguments, container, type_vars, subst) = match c.kind(node) {
        NodeKind::Send => {
            let Some((kids, pos)) = slice::owner_slice(c) else {
                return;
            };
            let Some((head, Some(arguments))) = slice::send_parts(c, &kids, pos) else {
                return;
            };
            let selector = c.lexeme(c.head_token(kids[head]));
            // A constructor invocation tiles as a plain send (`K(1)`) —
            // when no function signature answers, the resolved
            // constructor's formals do (the official
            // InstanceCreationExpression branch). An explicit receiver
            // (`a.m(1)`) resolves through the receiver's type — the
            // official `staticInvokeType`. Either way the container's type
            // variables ride along so a bare `T` formal substitutes through
            // the invocation's own `TypeArguments` (`A<void>.c(p)`) or the
            // receiver's written type arguments (`A<void> a; a.m(1)`).
            let resolved = if let Some(found) =
                member_invocation(c, analysis, &kids[..head], selector)
            {
                found
            } else if let Some(sig) = analysis.function_signature_of(query_site(c, kids[head])) {
                (sig.formals, None, Vec::new(), Vec::new())
            } else {
                let Some(found) = constructor_invocation(c, analysis, &kids, head, selector) else {
                    return;
                };
                found
            };
            let (formals, container, type_vars, subst) = resolved;
            (formals, arguments, container, type_vars, subst)
        }
        NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            let kids = c.collect_direct(node);
            let Some(args_pos) = kids
                .iter()
                .position(|&kid| c.kind(kid) == NodeKind::Arguments)
            else {
                return;
            };
            // The callee's names ride inside the `ConstructorReference` row
            // (`A<void>.c` → `[A, c]`): the first identifier is the type
            // reference the constructor resolves at, a second one names the
            // constructor (the official `constructorName.element`). The
            // argument rows tile flat beside it, so a bare identifier scan
            // would read the argument instead.
            let ctor_ref = kids
                .iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference);
            let names: Vec<u32> = ctor_ref.map_or_else(Vec::new, |ctor_ref| {
                c.collect_direct(ctor_ref)
                    .into_iter()
                    .filter(|&kid| c.kind(kid) == NodeKind::Identifier)
                    .collect()
            });
            let Some(&type_ref) = names.first() else {
                return;
            };
            let selector = names
                .last()
                .filter(|_| names.len() > 1)
                .map(|&name| analysis.strings().intern(c.lexeme(c.head_token(name))));
            let Some(binding) = analysis.resolve_constructor(query_site(c, type_ref), selector)
            else {
                return;
            };
            let Some(formals) = binding.formals else {
                return;
            };
            let type_vars = class_type_vars(c, analysis, binding.container);
            // The creation's `<…>` may tile inside the `ConstructorReference`
            // row rather than at the owner level — read both windows.
            let mut scope = kids[..args_pos].to_vec();
            if let Some(ctor_ref) = ctor_ref {
                scope.extend(c.collect_direct(ctor_ref));
            }
            let subst = send_type_args(c, &scope);
            (
                formals,
                kids[args_pos],
                Some(binding.container),
                type_vars,
                subst,
            )
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
        let (formal, value) = if c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument {
            let Some(name_idx) = slice::named_arg_name(c, &pieces, range.end - 1) else {
                continue;
            };
            let label = analysis
                .strings()
                .intern(c.lexeme(c.head_token(pieces[name_idx])));
            let Some(formal) = formals.named_formal(label) else {
                continue;
            };
            (formal, &pieces[name_idx + 1..range.end - 1])
        } else {
            let Some(formal) = formals.positional.get(positional) else {
                break;
            };
            positional += 1;
            (formal, &pieces[range.start..range.end])
        };
        let formal_ty = formal.ty;
        // A bare type-variable formal (`A<void>.c(p)` — `T value`, or
        // `A<void> a; a.m(1)` — `void m(T p)`) decides through the
        // substituted type argument: the class's parameter index names the
        // substitution slot (:87-103 — `parameterElement.type` and
        // `staticInvokeType` read the instantiated signature). An
        // initializing formal (`this.value`) carries no captured type, so
        // its field's written type answers instead.
        let substituted = match formal_ty {
            Ty::Named { name, .. } => {
                let var = analysis.strings().resolve(name);
                type_vars
                    .iter()
                    .position(|v| v == var)
                    .and_then(|index| subst.get(index))
                    .and_then(Option::as_deref)
                    .is_some_and(|arg| arg == "void")
            }
            Ty::Unknown => container.is_some_and(|container| {
                container_field_expectation(c, container, analysis, formal.name, &subst)
                    .is_some_and(|(is_void, _)| is_void)
            }),
            _ => false,
        };
        let formal_is_void = formal_ty == Ty::Void || substituted;
        if !formal_is_void {
            continue;
        }
        // The value's static type; undecidable values stay silent
        // (`expression.staticType` null bails).
        let ty = literal_ty(c, analysis, value)
            .or_else(|| static_type_faces::expr_type(c, analysis, value));
        let Some(ty) = ty else {
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
    let is_list = left.iter().any(|&row| c.kind(row) == NodeKind::ListPattern);
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
        let face = match binding_type_row(c, name_row, name) {
            Some(Some(row)) => Some(type_face(c, row)),
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
    // The literal's `<…>` seals as a sibling at its `[` (the literal row
    // covers `[…]` only), so the element type reads off the run's
    // `TypeArguments` row rather than the literal's own children.
    let args = right
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == NodeKind::TypeArguments)
        .find(|&row| c.byte_span(row).end <= c.byte_span(lit).start)?;
    let arg = c
        .collect_direct(args)
        .into_iter()
        .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::VoidType))?;
    let face = type_face(c, arg);
    face.name.map(|name| Ty::Named {
        name: analysis.strings().intern(name),
        nullable: face.nullable,
        args: ArgListId::EMPTY,
    })
}

/// The first positional field's type of a `(7,)` record literal (the
/// `(p,)` match reads the positional slot): the lexically first scalar
/// literal inside the literal's own subtree.
fn right_record_field_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    right: &[u32],
) -> Option<Ty> {
    let lit = right
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::RecordLiteral)?;
    let first = first_scalar_within(c, lit)?;
    literal_ty(c, analysis, &[first])
}

/// The lexically first scalar-literal row inside `root`'s subtree.
fn first_scalar_within(c: &Check<'_>, root: u32) -> Option<u32> {
    let mut best: Option<u32> = None;
    let mut stack = vec![root];
    while let Some(row) = stack.pop() {
        for kid in c.collect_direct(row) {
            if is_scalar(c, kid) {
                let earlier = match best {
                    None => true,
                    Some(kept) => c.byte_span(kid).start < c.byte_span(kept).start,
                };
                if earlier {
                    best = Some(kid);
                }
            }
            stack.push(kid);
        }
    }
    best
}

/// Whether the row is a scalar literal the value faces name.
fn is_scalar(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::LiteralInt
            | NodeKind::LiteralDouble
            | NodeKind::LiteralBool
            | NodeKind::LiteralNull
            | NodeKind::StringLiteral
    )
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
    crate::rules::same_file_declaration(c, name).map_or_else(Vec::new, |class_row| {
        class_type_params(c, class_row)
            .into_iter()
            .map(str::to_owned)
            .collect()
    })
}

/// The declared type-parameter names of `class_row`, in declaration order.
/// The header's `TypeVariables` tiles beside the `ClassDeclaration` row
/// under the top-level wrapper, so the wrapper window is the fallback.
fn class_type_params<'a>(c: &'a Check<'_>, class_row: u32) -> Vec<&'a str> {
    let vars = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
        .or_else(|| {
            c.unit.ast.parent(class_row).and_then(|wrapper| {
                c.collect_direct(wrapper)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::TypeVariables)
            })
        });
    let Some(vars) = vars else {
        return Vec::new();
    };
    c.collect_direct(vars)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::TypeVariable)
        .map(|var| c.lexeme(c.head_token(var)))
        .collect()
}

/// The written type row the bare name `name` binds to at `use_node` —
/// `None` when no same-file binding answers, `Some(None)` for a binding
/// with no written type. A `void p` parameter tiles as a `VoidType` row
/// that [`same_file::binding`]'s type filter drops, so the enclosing
/// formals are read directly for the untyped answer.
fn binding_type_row(c: &Check<'_>, use_node: u32, name: &str) -> Option<Option<u32>> {
    let (_, row) = same_file::binding(c, use_node, name)?;
    if row.is_some() {
        return Some(row);
    }
    Some(enclosing_parameter_type_row(c, use_node, name))
}

/// The type row of the enclosing function's formal `name` (`void p` →
/// the `VoidType` row).
fn enclosing_parameter_type_row(c: &Check<'_>, use_node: u32, name: &str) -> Option<u32> {
    let mut current = c.unit.ast.parent(use_node);
    while let Some(row) = current {
        if matches!(
            c.kind(row),
            NodeKind::TopLevelMethod
                | NodeKind::Method
                | NodeKind::Constructor
                | NodeKind::Factory
                | NodeKind::LocalFunctionDeclaration
                | NodeKind::FunctionExpression
                | NodeKind::NamedFunctionExpression
        ) && let Some(params) = c
            .collect_direct(row)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            && let Some(type_row) = formal_type_row(c, params, name)
        {
            return Some(type_row);
        }
        current = c.unit.ast.parent(row);
    }
    None
}

/// The written type row of the formal named `name` inside `params`.
fn formal_type_row(c: &Check<'_>, params: u32, name: &str) -> Option<u32> {
    for kid in c.collect_direct(params) {
        if c.kind(kid) != NodeKind::FormalParameter {
            continue;
        }
        let named = c
            .field(kid, PayloadField::Name)
            .is_some_and(|token| c.lexeme(token) == name);
        if !named {
            continue;
        }
        return c.collect_direct(kid).into_iter().find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::Type | NodeKind::VoidType | NodeKind::FunctionType | NodeKind::RecordType
            )
        });
    }
    None
}

/// The written type-argument names of the `TypeArguments` row `args`, in
/// order: `void` keeps its slot (`A<void>` — the argument tiles as a
/// `VoidType`, not a `Type`), and a shape the face cannot name keeps an
/// empty slot so the indices stay aligned with the class's parameters.
fn type_arg_names(c: &Check<'_>, args: u32) -> Vec<Option<String>> {
    c.collect_direct(args)
        .into_iter()
        .map(|arg| match c.kind(arg) {
            NodeKind::Type | NodeKind::VoidType => Some(c.lexeme(c.head_token(arg)).to_owned()),
            _ => None,
        })
        .collect()
}

/// The `TypeArguments` sibling of the type row `type_row` — production
/// completion puts it before the row (the `Type` span-only seal), hand-built
/// trees after — read as its argument names.
fn type_row_args(c: &Check<'_>, type_row: u32) -> Vec<Option<String>> {
    let Some(owner) = c.unit.ast.parent(type_row) else {
        return Vec::new();
    };
    let members = c.collect_direct(owner);
    let Some(index) = members.iter().position(|&row| row == type_row) else {
        return Vec::new();
    };
    let is_args = |row: &u32| c.kind(*row) == NodeKind::TypeArguments;
    let Some(args) = members[..index]
        .iter()
        .rev()
        .find(|row| is_args(row))
        .or_else(|| members[index + 1..].iter().find(|row| is_args(row)))
        .copied()
    else {
        return Vec::new();
    };
    type_arg_names(c, args)
}

/// The explicit type-argument names a send's pieces carry (`A<void>.c(p)`,
/// `foo<int>(x)`), in order — the substitution values a generic
/// container's bare `T` formal reads.
fn send_type_args(c: &Check<'_>, pieces: &[u32]) -> Vec<Option<String>> {
    pieces
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::TypeArguments)
        .map_or_else(Vec::new, |ta| type_arg_names(c, ta))
}

/// The written type arguments of the binding `name` at `node`
/// (`A<void> a` → `["void"]`) — the receiver-side substitution values.
fn receiver_type_args(c: &Check<'_>, node: u32, name: &str) -> Vec<Option<String>> {
    match same_file::binding(c, node, name) {
        Some((_, Some(type_row))) => type_row_args(c, type_row),
        _ => Vec::new(),
    }
}

/// The expectation flags of a written face with the class's type
/// parameters substituted by the receiver's written type arguments —
/// upstream reads the *instantiated* types (`writeType`), so on `A<void> a`
/// the field `T value` and the setter `set f(T value)` both decide through
/// `void`. A face whose head is no type parameter of the class answers its
/// own flags.
fn substituted_expectation(
    face: &TypeFace<'_>,
    type_params: &[&str],
    args: &[Option<String>],
) -> (bool, bool) {
    let substituted = face
        .name
        .and_then(|name| type_params.iter().position(|&param| param == name))
        .and_then(|index| args.get(index))
        .and_then(Option::as_deref);
    match substituted {
        Some(arg) => (arg == "void", false),
        None => (face.is_void, face.is_future_or_void),
    }
}

/// The explicit-receiver invocation's formals (`a.m(1)`): the member
/// resolves through the receiver's declared type (the official
/// `staticInvokeType`), and a bare `T` formal substitutes through the
/// receiver's written type arguments (`A<void> a; a.m(1)` — `void m(T p)`).
fn member_invocation<'a>(
    c: &Check<'_>,
    analysis: &'a dartq_semantic::Analysis<'_>,
    before_selector: &[u32],
    selector: &str,
) -> Option<(
    &'a dartq_semantic::model::Formals,
    Option<dartq_source::StringId>,
    Vec<String>,
    Vec<Option<String>>,
)> {
    let receiver = before_selector
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::Identifier)?;
    let binding =
        analysis.member_of(query_site(c, receiver), analysis.strings().intern(selector))?;
    let sig = dartq_semantic::FnSig::of_member(binding.entry)?;
    Some((
        sig.formals,
        Some(binding.container),
        class_type_vars(c, analysis, binding.container),
        receiver_type_args(c, c.node(), c.lexeme(c.head_token(receiver))),
    ))
}

/// The resolved constructor's formals for a creation that tiles as a send
/// (`K(1)`, `A<void>.c(p)`): the type reference is the selector's own name
/// unless the selector is qualified by one (`A<void>.c`), and the
/// container's type parameters substitute through the send's own
/// `TypeArguments` (the official `formalParameters` of the instantiated
/// element).
fn constructor_invocation<'a>(
    c: &Check<'_>,
    analysis: &'a dartq_semantic::Analysis<'_>,
    kids: &[u32],
    head: usize,
    selector: &str,
) -> Option<(
    &'a dartq_semantic::model::Formals,
    Option<dartq_source::StringId>,
    Vec<String>,
    Vec<Option<String>>,
)> {
    let named = kids[..head]
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::Identifier)
        .and_then(|type_ref| {
            analysis.resolve_constructor(
                query_site(c, type_ref),
                Some(analysis.strings().intern(selector)),
            )
        });
    let binding =
        named.or_else(|| analysis.resolve_constructor(query_site(c, kids[head]), None))?;
    let formals = binding.formals?;
    Some((
        formals,
        Some(binding.container),
        class_type_vars(c, analysis, binding.container),
        send_type_args(c, kids),
    ))
}

/// The declared field's expectation flags on the resolved `container` —
/// the type an initializing formal (`this.value`) stands for upstream,
/// with the container's type parameters substituted by the call's own
/// type arguments.
fn container_field_expectation(
    c: &Check<'_>,
    container: dartq_source::StringId,
    analysis: &dartq_semantic::Analysis<'_>,
    name: dartq_source::StringId,
    args: &[Option<String>],
) -> Option<(bool, bool)> {
    let class_row = crate::rules::same_file_declaration(c, analysis.strings().resolve(container))?;
    field_expectation(c, class_row, analysis.strings().resolve(name), args)
}

/// The field `name`'s written expectation flags inside `class_row`, with
/// the class's type parameters substituted by `args`.
fn field_expectation(
    c: &Check<'_>,
    class_row: u32,
    name: &str,
    args: &[Option<String>],
) -> Option<(bool, bool)> {
    let members = crate::rules::body_members(c, class_row);
    let face = members.iter().copied().find_map(|member| {
        if !matches!(
            c.kind(member),
            NodeKind::Fields | NodeKind::VariablesDeclaration | NodeKind::TopLevelFields
        ) {
            return None;
        }
        let name_row = c.collect_direct(member).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
        })?;
        crate::rules::type_before(c, member, name_row).map(|(t, _)| type_face(c, t))
    })?;
    let type_params = class_type_params(c, class_row);
    Some(substituted_expectation(&face, &type_params, args))
}

/// A `kind` child of `owner`, falling back to the owner's parent window
/// for the header rows that tile beside the member row (`FormalParameters`
/// of a member declaration).
fn child_within(c: &Check<'_>, owner: u32, kind: NodeKind) -> Option<u32> {
    let direct = c.collect_direct(owner);
    if let Some(&row) = direct.iter().find(|&&row| c.kind(row) == kind) {
        return Some(row);
    }
    let span = c.byte_span(owner);
    c.unit.ast.parent(owner).and_then(|parent| {
        c.collect_direct(parent).into_iter().find(|&row| {
            c.kind(row) == kind && {
                let child = c.byte_span(row);
                child.start >= span.start && child.end <= span.end
            }
        })
    })
}

/// The member-write target's expectation flags (`a.x = v`): the receiver's
/// same-file declared type names the class, and the field's (or the setter
/// parameter's) written type substitutes through the receiver's written type
/// arguments (`A<void> a` — `T value` and `set f(T value)` both decide
/// through `void`). `None` when the receiver's type is undecidable.
fn member_write_expectation(
    c: &Check<'_>,
    node: u32,
    receiver: u32,
    name: &str,
) -> Option<(bool, bool)> {
    let recv_name = c.lexeme(c.head_token(receiver));
    let Some((_, Some(type_row))) = same_file::binding(c, node, recv_name) else {
        return None;
    };
    let recv_face = type_face(c, type_row);
    let class_name = recv_face.name?;
    let class_row = crate::rules::same_file_declaration(c, class_name)?;
    let args = type_row_args(c, type_row);
    if let Some(flags) = field_expectation(c, class_row, name, &args) {
        return Some(flags);
    }
    // The generic-setter face: `set f(T value)` — the parameter's written
    // type substitutes through the receiver's written type arguments.
    let type_params = class_type_params(c, class_row);
    let members = crate::rules::body_members(c, class_row);
    let setter = members.iter().copied().find(|&member| {
        c.kind(member) == NodeKind::Method
            && crate::rules::get_set_token(c, member) == Some("set")
            && c.collect_direct(member).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
            })
    })?;
    let params = child_within(c, setter, NodeKind::FormalParameters)?;
    // The parameter's written type is a child of the `FormalParameter`
    // row, not of the list.
    let param = child_within(c, params, NodeKind::FormalParameter)?;
    let param_type = child_within(c, param, NodeKind::Type)
        .or_else(|| child_within(c, param, NodeKind::VoidType))?;
    Some(substituted_expectation(
        &type_face(c, param_type),
        &type_params,
        &args,
    ))
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

#[cfg(test)]
mod tests {
    use crate::engine::Lint;
    use crate::testsupport::{Fixture, one_rule};

    /// The `(line, column, text)` rows a run reported, 1-based, over the
    /// ASCII sources the corpus pins carry (UTF-16 offsets equal byte
    /// offsets there).
    fn reported(source: &str, lints: &[Lint]) -> Vec<(u32, u32, String)> {
        lints
            .iter()
            .map(|lint| {
                let start = lint.range.start as usize;
                let end = lint.range.end as usize;
                let before = &source[..start];
                let line = before.matches('\n').count() as u32 + 1;
                let col = before.rsplit('\n').next().map_or(0, str::len) as u32 + 1;
                (line, col, source[start..end].to_owned())
            })
            .collect()
    }

    /// The whole 41-case `void_checks` oracle corpus, pinned row for row
    /// against the pinned oracle's `expected.tsv`: every case's materialized
    /// source (built by the corpus extractor from
    /// `pkg/linter/test/rules/void_checks_test.dart` and
    /// `messages.yaml#voidChecks`) with the exact `(line, column, text)` rows
    /// the official analyzer reports for it — an empty row list for the
    /// silent cases. The rule's own faces must answer identically.
    #[test]
    fn void_checks_oracle_corpus_pins() {
        #[expect(
            clippy::type_complexity,
            reason = "a flat literal table reads better than a named struct here"
        )]
        let cases: &[(&str, &str, &[(u32, u32, &str)])] = &[
            (
                "test_assert_blockBody_returnStatement",
                "\nvoid f() {\n  assert(() {\n    return true;\n  }());\n}\n",
                &[],
            ),
            (
                "test_constructorArgument_genericParameter",
                "\nvoid f(dynamic p) {\n  A<void>.c(p);\n}\nclass A<T> {\n  T value;\n  A.c(this.value);\n}\n",
                &[(3, 13, "p")],
            ),
            (
                "test_emptyFunctionExpressionReturningFutureOrVoid",
                "\nimport 'dart:async';\n\nvoid emptyFunctionExpressionReturningFutureOrVoid(FutureOr<void> Function() f) {\n  f = () {};\n}\n",
                &[],
            ),
            (
                "test_extraPositionalArgument",
                "\nmissing_parameter_for_argument() {\n  void foo() {}\n  foo(0);\n}\n",
                &[],
            ),
            (
                "test_functionArgument_FutureOrVoidParameter_FutureOrVoidArgument",
                "\nimport 'dart:async';\nvoid f(FutureOr<void> p) {\n  m(p);\n}\nvoid m(FutureOr<void> arg) {}\n",
                &[],
            ),
            (
                "test_functionArgument_FutureOrVoidParameter_FutureVoidArgument",
                "\nimport 'dart:async';\nvoid f(Future<void> p) {\n  m(p);\n}\nvoid m(FutureOr<void> arg) {}\n",
                &[],
            ),
            (
                "test_functionArgument_FutureOrVoidParameter_dynamicArgument",
                "\nimport 'dart:async';\nvoid f(dynamic p) {\n  m(p);\n}\nvoid m(FutureOr<void> arg) {}\n",
                &[],
            ),
            (
                "test_functionArgument_FutureOrVoidParameter_nullArgument",
                "\nimport 'dart:async';\nvoid f() {\n  m(null);\n}\nvoid m(FutureOr<void> arg) {}\n",
                &[],
            ),
            (
                "test_functionArgument_voidParameter_dynamicArgument",
                "\nvoid f(dynamic p) {\n  m(p);\n}\nvoid m(void arg) {}\n",
                &[(3, 5, "p")],
            ),
            (
                "test_functionArgument_voidParameter_named",
                "\nvoid f(dynamic p) {\n  m(p: p);\n}\nvoid m({required void p}) {}\n",
                &[(3, 8, "p")],
            ),
            (
                "test_functionArgument_voidParameter_optional",
                "\nvoid f(dynamic p) {\n  m(p);\n}\nvoid m([void v]) {}\n",
                &[(3, 5, "p")],
            ),
            (
                "test_functionExpression_blockBody_returnStatement_genericContext",
                "\ngenerics_with_function() {\n  g<T>(T Function() p) => p();\n  g(() {\n    return 1;\n  });\n}\n",
                &[],
            ),
            (
                "test_functionExpression_blockBody_returnStatement_voidContext",
                "\nvoid f() {\n  void g(Function p) {}\n  g(() async {\n    return 1;\n  });\n}\n",
                &[],
            ),
            (
                "test_functionType_FutureOrVoidReturnType_Never",
                "\nimport 'dart:async';\nvoid f() {\n  foo(() {\n    fail(); // OK\n  });\n}\n\nvoid foo(FutureOr<void> Function() p) {}\nNever fail() { throw ''; }\n",
                &[],
            ),
            (
                "test_futureOrCallback",
                r#"
import 'dart:async';

void capture(FutureOr<void> Function() callback) {}

void f() {
  capture(() {
    throw "oh no";
  });
}
"#,
                &[],
            ),
            (
                "test_futureOrVoidField_assignDynamic",
                "\nimport 'dart:async';\nvoid f(A a, dynamic p) {\n  a.x = p; // OK\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
                &[],
            ),
            (
                "test_futureOrVoidField_assignFutureOrVoid",
                "\nimport 'dart:async';\nvoid f(A a, FutureOr<void> p) {\n  a.x = p;\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
                &[],
            ),
            (
                "test_futureOrVoidField_assignFutureVoid",
                "\nimport 'dart:async';\nvoid f(A a) {\n  a.x = Future.value();\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
                &[],
            ),
            (
                "test_futureOrVoidField_assignInt",
                "\nimport 'dart:async';\nvoid f(A a) {\n  a.x = 1;\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
                &[(4, 3, "a.x = 1")],
            ),
            (
                "test_futureOrVoidField_assignNull",
                "\nimport 'dart:async';\nvoid f(A a) {\n  a.x = null; // OK\n}\nclass A {\n  FutureOr<void> x;\n  A(this.x);\n}\n",
                &[],
            ),
            (
                "test_futureOrVoidFunction_blockBody_returnStatement",
                "\nimport 'dart:async';\nFutureOr<void> f() {\n  return 1;\n}\n",
                &[(4, 3, "return 1;")],
            ),
            (
                "test_futureOrVoidFunction_blockBody_returnsFuture",
                "\nimport 'dart:async';\nFutureOr<void> f() {\n  return Future.value();\n}\n",
                &[],
            ),
            (
                "test_future_dynamic",
                "\nimport 'dart:async';\n\nvoid f(FutureOr<void>? arg) {\n  Future<dynamic>? future;\n  f(future);\n}\n",
                &[],
            ),
            (
                "test_listPattern_local",
                "\nvoid f() {\n  void p;\n  [p] = <int>[7];\n  return p;\n}\n",
                &[(4, 4, "p")],
            ),
            (
                "test_listPattern_param",
                "\nvoid f(void p) {\n  [p] = <int>[7];\n}\n",
                &[(3, 4, "p")],
            ),
            (
                "test_localFunction_emptyBlockBody_matchingFutureOrVoidSignature",
                "\nimport 'dart:async';\nvoid f(FutureOr<void> Function() p) {\n  p = () {};\n}\n",
                &[],
            ),
            (
                "test_neverReturningCallbackThrows",
                "\nimport 'dart:async';\n\nNever fail() { throw ''; }\n\nvoid f() async {\n  await Future.value(5).then<void>((x) {\n    fail();\n  });\n}\n",
                &[],
            ),
            (
                "test_nonVoidFunction_assignedToVoidFunction",
                "\nvoid f(void Function() p) {\n  int g() => 1;\n  p = g;\n}\n",
                &[],
            ),
            (
                "test_recordPattern",
                "\nvoid f(void p) {\n  (p, ) = (7, );\n}\n",
                &[(3, 4, "p")],
            ),
            (
                "test_returnOfInvalidType",
                "\nvoid bug2813() {\n  return 1;\n}\n",
                &[],
            ),
            (
                "test_returnTypeVoid_Never",
                "\nvoid f(Future<int> p) {\n  p.then<void>((_) {\n    fail();\n  });\n}\n\nNever fail() { throw ''; }\n",
                &[],
            ),
            (
                "test_returnTypeVoid_throw",
                "\nvoid f(Future<int> p) {\n  p.then<void>((_) {\n    throw '';\n  });\n}\n",
                &[],
            ),
            (
                "test_setterArgument_genericParameter",
                "\nvoid f(A<void> a, dynamic p) {\n  a.f = p;\n}\nclass A<T> {\n  set f(T value) {}\n}\n",
                &[(3, 3, "a.f = p")],
            ),
            (
                "test_voidFunction_blockBody_returnStatement",
                "\nvoid f(dynamic p) {\n  return p;\n}\n",
                &[(3, 3, "return p;")],
            ),
            (
                "test_voidFunction_blockBody_returnStatement_empty",
                "\nvoid f() {\n  return;\n}\n",
                &[],
            ),
            (
                "test_voidFunction_expressionBody",
                "\nvoid f() => 7;\n",
                &[],
            ),
            (
                "doc_bad_0",
                "class A<T> {\n  T? value;\n}\n\nvoid f(A<void> a) {\n  a.value = 1;\n}\n",
                &[(6, 3, "a.value = 1")],
            ),
            (
                "doc_bad_1",
                "class A<T> {\n  void m(T p) {}\n}\n\nvoid f(A<void> a) {\n  a.m(1);\n}\n",
                &[(6, 7, "1")],
            ),
            (
                "doc_bad_2",
                "class A<T> {\n  T value;\n  void test(T arg) { }\n}\n\nvoid main() {\n  A<void> a = A<void>();\n  a.value = 1; // LINT\n  a.test(1); // LINT\n}\n",
                &[(8, 3, "a.value = 1"), (9, 10, "1")],
            ),
            (
                "doc_good_0",
                "class A<T> {\n  T? value;\n}\n\nvoid f(A<int> a) {\n  a.value = 1;\n}\n",
                &[],
            ),
            (
                "doc_good_1",
                "class A<T> {\n  T? value;\n}\n\nvoid f(A<void> a) {}\n",
                &[],
            ),
        ];
        let mut mismatches = Vec::new();
        for &(case, source, expected) in cases {
            let fixture = Fixture::parse(source);
            let lints = fixture.run_semantic(&one_rule("void_checks"));
            let actual = reported(source, &lints);
            let wanted: Vec<(u32, u32, String)> = expected
                .iter()
                .map(|&(line, col, text)| (line, col, text.to_owned()))
                .collect();
            if actual != wanted {
                mismatches.push(format!("{case}: want {wanted:?}, got {actual:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }
}
