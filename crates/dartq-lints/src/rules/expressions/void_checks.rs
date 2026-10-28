use dartq_semantic::model::{ArgListId, Ty};

use super::{PayloadField, Check, NodeKind, RuleSpec, query_site, same_file, slice, static_type_faces, value_run_end};
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
        NodeKind::ReturnStatement => return_face(c),
        NodeKind::Send | NodeKind::NewExpression | NodeKind::ImplicitCreation => {
            argument_face(c)
        }
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
    // The target: bare `<field>` or `this.<field>`.
    let Some((name, this_prefixed)) = assigned_name(c, &left) else {
        return;
    };
    // Shadow-correct target type: a bare name goes through the
    // same-file binding walk (a local of the same name shadows the
    // field); a `this.` target is the field itself.
    let face = if this_prefixed {
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
    if !face.is_future_or_void {
        return;
    }
    let Some(analysis) = c.semantics() else {
        return;
    };
    // The value's static type; `return;` carries none and stays
    // silent (`node.expression?.staticType` is null).
    let rows = c.collect_direct(node);
    let Some(ty) = literal_ty(c, analysis, &rows)
        .or_else(|| static_type_faces::expr_type(c, analysis, &rows))
    else {
        return;
    };
    if acceptable_when_future_or_void_expected(analysis, ty) {
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
    let (formals, arguments) = match c.kind(node) {
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
            // InstanceCreationExpression branch).
            let formals = match analysis.function_signature_of(query_site(c, kids[head])) {
                Some(sig) => sig.formals,
                None => {
                    let Some(binding) =
                        analysis.resolve_constructor(query_site(c, kids[head]), None)
                    else {
                        return;
                    };
                    let Some(formals) = binding.formals else {
                        return;
                    };
                    formals
                }
            };
            (formals, arguments)
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
            let Some(binding) = analysis.resolve_constructor(query_site(c, callee), None)
            else {
                return;
            };
            let Some(formals) = binding.formals else {
                return;
            };
            (formals, kids[args_pos])
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
        if formal_ty != Ty::Void {
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

/// The assigned name off the left run: `(name, this-prefixed)`. The
/// run reads through the access-run form (the access markers and the
/// bare-name send group tile beside the chain rows).
fn assigned_name<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<(&'a str, bool)> {
    match slice::access_run(c, rows).as_slice() {
        [single] if c.kind(*single) == NodeKind::Identifier => {
            Some((c.lexeme(c.head_token(*single)), false))
        }
        [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
            Some((c.lexeme(c.head_token(*name)), true))
        }
        _ => None,
    }
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
    // `FunctionType` row — the caller's face gate decides whether it
    // answers (the implicit_call_tearoffs return face does; the
    // void_checks `FutureOr<void>` cut rejects it).
    (matches!(kind, NodeKind::Type | NodeKind::FunctionType)).then(|| type_face(c, type_row))
}

/// The value type the literal faces answer: `null` and the scalar
/// literals (the `right_scalar_face` shape, as a [`Ty`]).
fn literal_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    rows: &[u32],
) -> Option<Ty> {
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
        Ty::Dynamic | Ty::Null | Ty::Void | Ty::Never | Ty::Unknown => true,
        Ty::Named { name, .. } => analysis.strings().resolve(name) == "Future",
        // A function-typed value is no `Future` head — it reports.
        Ty::Record { .. } | Ty::Function { .. } => false,
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
