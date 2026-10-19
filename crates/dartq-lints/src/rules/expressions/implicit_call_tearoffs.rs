use dartq_semantic::model::MemberKind;

use super::{
    Check, Group, NodeKind, RuleSpec, query_site, value_run_end, written_type_sibling,
};
use crate::rules::{TypeFace, slice};

/// Official: `pkg/linter/lib/src/rules/implicit_call_tearoffs.dart` —
/// the resolution-built `ImplicitCallReference` (an object with a
/// `call` member used where a function is expected) reports at the
/// node (:34-39).
///
/// The ported faces:
///
/// - the assignment/initializer site: a written function-type target
///   (the declaration's `FunctionType` row, or the bare `Function`
///   spelling unshadowed — a local target through the shadow-correct
///   binding walk) and an operand whose type names a class with an
///   instance `call` method — same-file through the class body,
///   cross-file through the bridge member walk when mounted (a
///   bare-read operand only: `this.<field>` carries no read row). The
///   operand read is shadow-correct (a local of the same name shadows
///   the field);
/// - the return site (:34-39 through the resolver's context walk): a
///   written function-type return and an operand whose type has
///   `call` — a `return e;` reports at the expression, an
///   expression-body return at the body;
/// - the argument site for a same-file top-level callee: the written
///   formal table (a `FunctionType` formal, a function-typed formal
///   shape, or the bare `Function` spelling) and a decidable operand.
///
/// The operand verdict shapes: a bare read (same-file written type,
/// then the bridge member walk), a `this.`-field (the field face, the
/// same-file class), and an instance creation (the resolved
/// container's member walk). Inferred targets (`var f = c`) and
/// cross-file/method callees stay silent — the fold models no
/// function-type `Ty` (the collector captures `Unknown`, rule 0c), so
/// the written row is the only face there. Message: `diagnostic.g.dart`
/// `implicitCallTearoffs`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "implicit_call_tearoffs",
    group: Group::Core,
    problem: "Implicit tear-off of the 'call' method.",
    correction: "Try explicitly tearing off the 'call' method.",
    kinds: &[
        NodeKind::Assignment,
        NodeKind::VariableInitializer,
        NodeKind::ReturnStatement,
        NodeKind::ExpressionBody,
        NodeKind::Send,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Assignment | NodeKind::VariableInitializer => context_face(c),
        NodeKind::ReturnStatement | NodeKind::ExpressionBody => return_face(c),
        NodeKind::Send => argument_face(c),
        _ => {}
    }
}

/// The assignment/initializer face: the context type — an assignment's
/// left target or an initializer's declared variable — written as a
/// function type.
fn context_face(c: &mut Check<'_>) {
    let node = c.node();
    // The context type: an assignment's left target or an
    // initializer's declared variable — written as a function type.
    let context_is_function = match c.kind(node) {
        NodeKind::VariableInitializer => {
            let decl = c.unit.ast.parent(node);
            decl.is_some_and(|decl| written_row_is_function(c, declaration_type_row(c, decl)))
        }
        NodeKind::Assignment => {
            let Some(op) = c.payload(node).first().copied() else {
                return;
            };
            if c.lexeme(op) != "=" {
                return;
            }
            let Some((left, _)) = slice::binary_operands(c) else {
                return;
            };
            let left = slice::access_run(c, &left);
            let Some(name) = assigned_name(c, &left) else {
                return;
            };
            target_is_function(c, node, name)
        }
        _ => false,
    };
    if !context_is_function {
        return;
    }
    // The right side: a bare read, a `this.<field>`, or a creation —
    // whose type names a class with an instance `call` method.
    let rows = if c.kind(node) == NodeKind::VariableInitializer {
        c.collect_direct(node)
    } else {
        let Some((_, right)) = slice::binary_operands(c) else {
            return;
        };
        right
    };
    if !operand_rows_have_call(c, node, &rows) {
        return;
    }
    // Report at the expression (the official node face): the rows
    // span it in source order — the run's last value row bounds the
    // end (the send-group completion wrappers finalize at the `;`).
    let start = c.utf16_span(rows[0]).start;
    let end = value_run_end(c, &rows, node);
    c.report(start..end, &[]);
}

/// The return face: a written function-type return and an operand
/// whose type has `call` reports at the returned expression (a
/// function literal's inferred return type stays silent).
fn return_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some(function) = c.path.iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::TopLevelMethod | NodeKind::Method | NodeKind::LocalFunctionDeclaration
        )
    }) else {
        return;
    };
    let Some(face) = super::void_checks::written_return_face(c, function) else {
        return;
    };
    if !face_is_function_typed(c, &face) {
        return;
    }
    let rows = c.collect_direct(node);
    let [first, ..] = rows.as_slice() else {
        return; // `return;` carries no value
    };
    if !operand_rows_have_call(c, node, &rows) {
        return;
    }
    let start = c.utf16_span(*first).start;
    let end = value_run_end(c, &rows, node);
    c.report(start..end, &[]);
}

/// The argument face for a same-file top-level callee: the written
/// formal table decides the function-typed slots (positional by
/// required order, named by label; an optional-positional formal
/// under-reports), and a decidable operand reports at the argument
/// value. A method or cross-file callee stays silent — the written
/// formal rows are not reachable through the fold (no function-type
/// `Ty`).
fn argument_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, Some(arguments))) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    if c.kind(kids[head]) != NodeKind::Identifier {
        return;
    }
    let callee = c.lexeme(c.head_token(kids[head]));
    let Some(function) = top_level_function_row(c, callee) else {
        return;
    };
    let Some(params) = c
        .collect_direct(function)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    let formals = slice::parameter_assignments::parameter_rows(c, params);
    let pieces = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &pieces);
    let mut positional = 0usize;
    for range in &ranges {
        // The formal at the dispatch slot: positional by required
        // order, named by label. A named argument flattens as
        // `name value… NamedArgument` (the marker anchors at the
        // colon).
        let (formal, value) = if c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument {
            let Some(name_idx) = slice::named_arg_name(c, &pieces, range.end - 1) else {
                continue;
            };
            let label = c.lexeme(c.head_token(pieces[name_idx]));
            let Some((row, _)) = formals.iter().find(|(row, _)| {
                slice::parameter_assignments::parameter_name(c, *row) == Some(label)
            }) else {
                continue;
            };
            (*row, &pieces[name_idx + 1..range.end - 1])
        } else {
            let Some((row, _)) = formals
                .iter()
                .filter(|(_, optional)| !*optional)
                .nth(positional)
            else {
                break;
            };
            positional += 1;
            (*row, &pieces[range.start..range.end])
        };
        if !formal_is_function_typed(c, formal) {
            continue;
        }
        if !operand_rows_have_call(c, node, value) {
            continue;
        }
        let start = c.utf16_span(value[0]).start;
        let end = value_run_end(c, value, *value.last().unwrap_or(&node));
        c.report(start..end, &[]);
    }
}

/// Whether a written type face is the function type: a `FunctionType`
/// row, or the bare `Function` spelling resolving outside the file (a
/// same-file declaration owns the name — the official element
/// identity cannot hold for it).
fn face_is_function_typed(c: &Check<'_>, face: &TypeFace<'_>) -> bool {
    face.is_function_type
        || face
            .name
            .is_some_and(|name| bare_function_spelling(c, name))
}

/// The bare `Function` spelling is the core function type only when no
/// same-file declaration owns the name.
fn bare_function_spelling(c: &Check<'_>, name: &str) -> bool {
    name == "Function" && !super::same_file::declares_type(c, name)
}

/// Whether a written type row is the function type ([`TypeFace`]'s
/// two shapes).
fn written_row_is_function(c: &Check<'_>, row: Option<u32>) -> bool {
    let Some(row) = row else {
        return false;
    };
    match c.kind(row) {
        NodeKind::FunctionType => true,
        NodeKind::Type => bare_function_spelling(c, c.lexeme(c.head_token(row))),
        _ => false,
    }
}

/// Whether a formal parameter's written type is a function type: the
/// function-typed formal shape (`void g(int x)` as a parameter), a
/// `FunctionType` child, or the bare `Function` spelling.
fn formal_is_function_typed(c: &Check<'_>, formal: u32) -> bool {
    if c.kind(formal) == NodeKind::FunctionTypedFormalParameter {
        return true;
    }
    if c.kind(formal) != NodeKind::FormalParameter {
        return false;
    }
    c.collect_direct(formal)
        .into_iter()
        .find(|&t| {
            matches!(
                c.kind(t),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
            )
        })
        .is_some_and(|row| written_row_is_function(c, Some(row)))
}

/// The same-file top-level function row carrying `name` (the
/// argument-face callee channel; a prefix-qualified or method callee
/// is out of the face).
fn top_level_function_row(c: &Check<'_>, name: &str) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
    let root = c.unit.ast.root()?;
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        for kid in c.collect_direct(top) {
            if c.kind(kid) != NodeKind::TopLevelMethod {
                continue;
            }
            let named = c.collect_direct(kid).into_iter().any(|k| {
                c.kind(k) == NodeKind::Identifier
                    && c.payload(k).first() == Some(&ctx)
                    && c.lexeme(c.head_token(k)) == name
            });
            if named {
                return Some(kid);
            }
        }
    }
    None
}

/// The operand verdict over a value's rows — the argument/return
/// faces' operand shapes: a bare read through
/// [`operand_type_has_call`], a `this.`-field through the field face
/// (the same-file class), a creation through the resolved container's
/// member walk. A bare-identifier argument can tile as the four-piece
/// tear-off group (the marker pieces are width-0 wrappers), so the
/// raw rows' leading `Identifier` stands in when the access run
/// empties.
fn operand_rows_have_call(c: &Check<'_>, use_node: u32, rows: &[u32]) -> bool {
    let run = slice::access_run(c, rows);
    if let [single] = run.as_slice()
        && c.kind(*single) == NodeKind::Identifier
    {
        let name = c.lexeme(c.head_token(*single));
        return operand_type_has_call(c, use_node, &run, name);
    }
    if let [target, name] = run.as_slice()
        && c.kind(*target) == NodeKind::ThisExpression
    {
        return crate::rules::type_face_of_field(c, c.lexeme(c.head_token(*name)))
            .and_then(|face| Some(face.name?))
            .map(|type_name| same_file_class_has_call(c, type_name))
            .unwrap_or(false);
    }
    if let Some(&first) = rows.first()
        && c.kind(first) == NodeKind::Identifier
        && rows.len() > 1
        && rows[1..]
            .iter()
            .all(|&row| slice::is_send_group_piece(c, row))
    {
        let name = c.lexeme(c.head_token(first));
        return operand_type_has_call(c, use_node, &[first], name);
    }
    creation_has_call(c, rows)
}

/// A creation operand (`C()`, `new C()`): the head resolves through
/// the bridge constructor face, and the container's member walk
/// answers the instance `call` method (cross-file classes included).
fn creation_has_call(c: &Check<'_>, run: &[u32]) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let Some(args_pos) = run
        .iter()
        .position(|&row| c.kind(row) == NodeKind::Arguments)
    else {
        return false;
    };
    let Some(callee) = run[..args_pos]
        .iter()
        .rev()
        .find_map(|&row| match c.kind(row) {
            NodeKind::Identifier => Some(row),
            // The explicit-new shape: the marker owns the callee name.
            NodeKind::ConstructorReference => c
                .collect_direct(row)
                .into_iter()
                .find(|&k| c.kind(k) == NodeKind::Identifier),
            _ => None,
        })
    else {
        return false;
    };
    let Some(binding) = analysis.resolve_constructor(query_site(c, callee), None) else {
        // An implicit creation's head carries no type-use row (the
        // parser tiles `C()` as a plain invocation, so the fold never
        // records the container) — the same-file class body answers;
        // a cross-file creation under-reports (the structural face).
        return same_file_class_has_call(c, c.lexeme(c.head_token(callee)));
    };
    analysis
        .inherited_member(binding.container, analysis.strings().intern("call"), false)
        .is_some_and(|(_, entry)| entry.kind == MemberKind::Method && !entry.is_static)
}

/// The assigned target's name off an access run: `this.<field>` or
/// bare `<field>`.
fn assigned_name<'a>(c: &'a Check<'_>, run: &[u32]) -> Option<&'a str> {
    match run {
        [single] if c.kind(*single) == NodeKind::Identifier => {
            Some(c.lexeme(c.head_token(*single)))
        }
        [target, name] if c.kind(*target) == NodeKind::ThisExpression => {
            Some(c.lexeme(c.head_token(*name)))
        }
        _ => None,
    }
}

/// The declaration's written type row (the type sibling of the
/// variable row — [`written_type_sibling`]).
fn declaration_type_row(c: &Check<'_>, decl: u32) -> Option<u32> {
    written_type_sibling(c, decl, &[NodeKind::Type, NodeKind::FunctionType])
}

/// The assignment target's context verdict, shadow-correct: the
/// innermost same-file binding's written row decides (a local
/// `void Function() g` target — the documented under-report face);
/// the same-class field face stands in when no binding answers.
fn target_is_function(c: &Check<'_>, use_node: u32, name: &str) -> bool {
    if let Some((_, written)) = super::same_file::binding(c, use_node, name) {
        return written.is_some_and(|row| written_row_is_function(c, Some(row)));
    }
    crate::rules::type_face_of_field(c, name)
        .map(|face| face_is_function_typed(c, &face))
        .unwrap_or(false)
}

/// The operand's type verdict: a class with an instance `call` method.
/// The type name reads shadow-correct (the innermost same-file
/// binding's written type — a local of the same name shadows the
/// same-class field, the documented over-report face). The class-with-
/// `call` check runs same-file first; a cross-file class name reads
/// through the bridge member walk when mounted and the operand is a
/// bare read (`this.<field>` carries no read row and stays silent —
/// the documented under-report face).
fn operand_type_has_call(c: &Check<'_>, use_node: u32, run: &[u32], name: &str) -> bool {
    let type_name = match super::same_file::binding(c, use_node, name) {
        Some((_, Some(row))) if c.kind(row) == NodeKind::Type => {
            c.lexeme(c.head_token(row)).to_owned()
        }
        Some((_, Some(row))) if c.kind(row) == NodeKind::FunctionType => return false,
        Some((_, None)) => return false,
        _ => {
            let Some(face) = crate::rules::type_face_of_field(c, name) else {
                return false;
            };
            let Some(type_name) = face.name else {
                return false;
            };
            type_name.to_owned()
        }
    };
    if same_file_class_has_call(c, &type_name) {
        return true;
    }
    // The cross-file face: the declared class body is in another file
    // (an unshadowed, non-core name) — the bridge member walk
    // resolves `call` through the graph.
    let Some(analysis) = c.semantics() else {
        return false;
    };
    if crate::rules::same_file_declaration(c, &type_name).is_some() {
        return false;
    }
    let [single] = run else {
        return false;
    };
    if c.kind(*single) != NodeKind::Identifier {
        return false;
    }
    analysis
        .member_of(query_site(c, *single), analysis.strings().intern("call"))
        .is_some_and(|binding| {
            binding.entry.kind == dartq_semantic::model::MemberKind::Method
                && !binding.entry.is_static
        })
}

/// Whether the same-file class `name` declares an instance `call`
/// method.
fn same_file_class_has_call(c: &Check<'_>, name: &str) -> bool {
    let Some(class_row) = crate::rules::same_file_declaration(c, name) else {
        return false;
    };
    crate::rules::body_members(c, class_row)
        .into_iter()
        .any(|member| {
            c.kind(member) == NodeKind::Method
                && crate::rules::get_set_token(c, member).is_none()
                && crate::rules::member_is_concrete(c, member)
                && !crate::rules::member_matches(
                    c,
                    member,
                    crate::rules::MemberQuery::Method,
                    "call",
                )
                .is_empty()
        })
}
