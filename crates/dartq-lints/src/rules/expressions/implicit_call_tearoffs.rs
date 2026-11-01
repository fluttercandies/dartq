use dartq_semantic::model::MemberKind;

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, value_run_end, written_type_sibling,
};
use crate::rules::{TypeFace, slice};

/// Whether the rows carry a tear-off instantiation (`c<int>` /
/// `C()<int>`): a `TypeArguments` row no `Arguments` follows (a generic
/// invocation's type arguments precede its argument list).
fn is_instantiation(c: &Check<'_>, rows: &[u32]) -> bool {
    rows.iter()
        .rposition(|&row| c.kind(row) == NodeKind::TypeArguments)
        .is_some_and(|ta_pos| {
            !rows[ta_pos + 1..]
                .iter()
                .any(|&row| c.kind(row) == NodeKind::Arguments)
        })
}

/// The report end of a value-position run: [`value_run_end`], extended
/// through a trailing tear-off instantiation's type arguments — the
/// resolver's `ImplicitCallReference` spans `c<int>` / `C()<int>` whole.
fn instantiation_end(c: &Check<'_>, rows: &[u32], fallback: u32) -> u32 {
    let end = value_run_end(c, rows, fallback);
    if !is_instantiation(c, rows) {
        return end;
    }
    let ta_pos = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::TypeArguments)
        .expect("checked above");
    end.max(c.utf16_span(rows[ta_pos]).end)
}

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
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "implicit_call_tearoffs",
    &[
        NodeKind::Assignment,
        NodeKind::VariableInitializer,
        NodeKind::ReturnStatement,
        NodeKind::ExpressionBody,
        NodeKind::Send,
        NodeKind::ListLiteral,
        NodeKind::SetOrMapLiteral,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Assignment | NodeKind::VariableInitializer => context_face(c),
        NodeKind::ReturnStatement | NodeKind::ExpressionBody => return_face(c),
        NodeKind::Send => argument_face(c),
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => collection_face(c),
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
            decl.is_some_and(|decl| {
                written_row_is_function(c, declaration_type_row(c, decl))
                    || written_row_is_function(c, top_level_declaration_type_row(c, node, decl))
            })
        }
        NodeKind::Assignment => {
            let Some(op) = c.field(node, PayloadField::Operator) else {
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
    // whose type names a class with an instance `call` method. The
    // initializer's operand pieces can tile as the initializer's siblings
    // under the variable row (the send-group adoption), so the run reads
    // the node's own children plus the parent's rows inside its extent —
    // the same window [`value_rows`] assembles.
    let rows = if c.kind(node) == NodeKind::VariableInitializer {
        value_rows(c, node)
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
    let end = instantiation_end(c, &rows, node);
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
    let rows = value_rows(c, node);
    let [first, ..] = rows.as_slice() else {
        return; // `return;` carries no value
    };
    if !operand_rows_have_call(c, node, &rows) {
        return;
    }
    let start = c.utf16_span(*first).start;
    // The container rows seal past the expression's last token, so
    // the span end reads the operand rows only.
    let operands: Vec<u32> = rows
        .iter()
        .copied()
        .filter(|&row| {
            !matches!(
                c.kind(row),
                NodeKind::BinaryExpression | NodeKind::Conditional
            )
        })
        .collect();
    let end = instantiation_end(c, &operands, node);
    c.report(start..end, &[]);
}

/// The returned expression's rows: the direct children plus the owner
/// window's rows inside the row's own extent (the tiling splits the
/// value across both — `=> c` keeps the body beside the row,
/// `return c1 ?? c2;` leaves the trailing operand and the container as
/// owner siblings), in source order.
fn value_rows(c: &Check<'_>, node: u32) -> Vec<u32> {
    let mut rows = c.collect_direct(node);
    if let Some(parent) = c.unit.ast.parent(node) {
        let kids = c.collect_direct(parent);
        if kids.contains(&node) {
            let extent = c.byte_span(node);
            for kid in kids {
                if kid == node || rows.contains(&kid) {
                    continue;
                }
                let span = c.byte_span(kid);
                if span.start >= extent.start && span.end <= extent.end {
                    rows.push(kid);
                }
            }
        }
    }
    // The container's own operands ride its child window.
    let containers: Vec<u32> = rows
        .iter()
        .copied()
        .filter(|&row| {
            matches!(
                c.kind(row),
                NodeKind::BinaryExpression | NodeKind::Conditional
            )
        })
        .collect();
    for &row in &containers {
        for kid in c.collect_direct(row) {
            if !rows.contains(&kid) {
                rows.push(kid);
            }
        }
    }
    if rows.len() > 1 {
        rows.sort_by_key(|&row| {
            let span = c.byte_span(row);
            (span.start, span.end)
        });
    }
    rows
}

/// The tear-off-instantiation verdict over a send group's operand rows:
/// the last `TypeArguments` row whose tail carries no `Arguments` (an
/// invocation's type arguments are followed by its argument list — a
/// `c<T>` tear-off's are followed by `NoArguments`/nothing). The operand
/// before it judges through the shared `call` verdict; the report spans
/// `operand…<T>`.
fn instantiation_face(c: &mut Check<'_>, node: u32, rows: &[u32]) -> bool {
    let Some(ta_pos) = rows
        .iter()
        .rposition(|&row| c.kind(row) == NodeKind::TypeArguments)
    else {
        return false;
    };
    if rows[ta_pos + 1..]
        .iter()
        .any(|&row| c.kind(row) == NodeKind::Arguments)
    {
        return false;
    }
    // The operand is the send group ending before the type-argument
    // list's `<` — the argument types' own `Type`/`Identifier` rows tile
    // inside the `TypeArguments` span and stay out.
    let ta_start = c.byte_span(rows[ta_pos]).start;
    let operand: Vec<u32> = rows[..ta_pos]
        .iter()
        .copied()
        .filter(|&row| c.byte_span(row).end <= ta_start)
        .collect();
    if operand.is_empty() || !operand_rows_have_call(c, node, &operand) {
        return false;
    }
    let start = c.utf16_span(operand[0]).start;
    let end = c.utf16_span(rows[ta_pos]).end;
    c.report(start..end, &[]);
    true
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
    let Some((head, arg_row)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // The tear-off-instantiation face (:34-39): `c<T>` / `C()<T>` — the
    // operand tiles a `TypeArguments` row that no `Arguments` follows (a
    // generic invocation `f<T>(…)` carries one; a type use `List<int>`
    // owns no expression operand). The production tiling wraps the
    // instantiation in a `TypeApplication` row whose `TypeArguments`
    // piece seals after the operand's `Send` — the slice is the owner
    // window minus the trailing statement/completion rows. When this
    // dispatch's window carries the shape while the operand verdict here
    // fails (the head is the outer call, not the callable), the window
    // still owns the row through another dispatch — the value loop stays
    // out either way.
    if instantiation_face(c, node, &kids) || is_instantiation(c, &kids) {
        return;
    }
    let Some(arguments) = arg_row else {
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
        // A tear-off instantiation argument (`g(c<int>)`) is the
        // instantiation face's row — the owner window reports it whole;
        // this loop's value verdict would report the bare operand
        // beside it.
        if is_instantiation(c, value) {
            continue;
        }
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
                    && c.field(k, PayloadField::Context) == Some(ctx)
                    && c.lexeme(c.head_token(k)) == name
            });
            if named {
                return Some(kid);
            }
        }
    }
    None
}

/// The collection-element face (:34-39 through the resolver's context
/// walk): a literal whose element context is a function type reports
/// each operand carrying an instance `call` — `<void Function()>[c]`
/// reads the literal's own `TypeArguments`, `List<void Function()> f =
/// [c]` / `return [c]` reads the enclosing declared type's element
/// argument (a `List`/`Set` face; maps keep their value argument). An
/// element-untyped literal stays silent.
fn collection_face(c: &mut Check<'_>) {
    let node = c.node();
    let span = c.byte_span(node);
    // The element pieces can tile as the literal's siblings under the
    // enclosing statement — collect every row inside the literal's span.
    let mut rows = c.collect_direct(node);
    if let Some(parent) = c.unit.ast.parent(node) {
        for kid in c.collect_direct(parent) {
            if kid == node || rows.contains(&kid) {
                continue;
            }
            let kid_span = c.byte_span(kid);
            if kid_span.start >= span.start && kid_span.end <= span.end {
                rows.push(kid);
            }
        }
    }
    rows.sort_by_key(|&row| c.byte_span(row).start);
    // The literal's own type arguments (`<void Function()>[c]`): the
    // first `Type` row under the literal's `TypeArguments` child decides
    // the element context directly. The production tiling anchors the
    // `<T>` prefix as a sibling sealing at the literal's `[` (the
    // literal row covers `[…]` only), so the prefix row comes off the
    // parent's child window too.
    let ta = rows
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::TypeArguments)
        .or_else(|| {
            c.unit.ast.parent(node).and_then(|parent| {
                c.collect_direct(parent).into_iter().find(|&kid| {
                    c.kind(kid) == NodeKind::TypeArguments
                        && c.byte_span(kid).end == span.start
                })
            })
        });
    let element_is_function = if let Some(ta) = ta {
        c.collect_direct(ta)
            .into_iter()
            .find(|&arg| matches!(c.kind(arg), NodeKind::Type | NodeKind::FunctionType))
            .is_some_and(|arg| written_row_is_function(c, Some(arg)))
    } else {
        // The enclosing declared element type: `List<void Function()>` —
        // the first `FunctionType` row inside the written type's
        // `TypeArguments`. The declaration sibling scan covers a local
        // (`var l = [c]` under a `List<…>` declared target is not this
        // shape — the typed target is the return/assignment context),
        // so the face reads the enclosing written type directly: the
        // nearest enclosing declaration's written return type first.
        enclosing_collection_element_is_function(c, node)
    };
    if !element_is_function {
        return;
    }
    for segment in slice::comma_segments(c, node, &rows) {
        if segment
            .iter()
            .any(|&row| matches!(c.kind(row), NodeKind::TypeArguments))
        {
            continue;
        }
        if segment.is_empty() || !operand_rows_have_call(c, node, &segment) {
            continue;
        }
        let start = c.utf16_span(segment[0]).start;
        let end = value_run_end(c, &segment, *segment.last().unwrap_or(&node));
        c.report(start..end, &[]);
    }
}

/// Whether the enclosing context's written collection type carries a
/// function-typed element argument (`List<void Function()>`, a map's
/// value argument). The face walks the enclosing written type rows —
/// the return type of the enclosing function (`return [c]`), the
/// declared type of the enclosing variable initializer.
fn enclosing_collection_element_is_function(c: &Check<'_>, _node: u32) -> bool {
    // The literal's enclosing declaration's written type row: the
    // return type (`return [c]`) or the variable's declared type.
    let function = c.path.iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::TopLevelMethod | NodeKind::Method | NodeKind::LocalFunctionDeclaration
        )
    });
    let Some(function) = function else {
        return false;
    };
    // The declaration header carries the written return type whole: the
    // production order tiles the name first with the generic type's
    // `TypeArguments` and `Type` pieces after it (and plain types before
    // it), so the face scans every header row before the formal
    // parameters and descends the type rows — the return type is
    // function-typed when a `FunctionType` (or the bare `Function`
    // spelling) sits anywhere in them.
    let members = c.collect_direct(function);
    let formals = members
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    let header: Vec<u32> = match formals {
        Some(end) => members[..end].to_vec(),
        None => members.clone(),
    };
    let mut pending: Vec<u32> = header;
    while let Some(row) = pending.pop() {
        match c.kind(row) {
            NodeKind::FunctionType => return true,
            NodeKind::Type => {
                if bare_function_spelling(c, c.lexeme(c.head_token(row))) {
                    return true;
                }
                for kid in c.collect_direct(row) {
                    match c.kind(kid) {
                        NodeKind::FunctionType => return true,
                        NodeKind::Type | NodeKind::TypeArguments => pending.push(kid),
                        _ => {}
                    }
                }
            }
            NodeKind::TypeArguments => {
                for kid in c.collect_direct(row) {
                    match c.kind(kid) {
                        NodeKind::FunctionType => return true,
                        NodeKind::Type | NodeKind::TypeArguments => pending.push(kid),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    false
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
    // An explicit `.call` selector is the spelled tear-off, not an
    // implicit one — the resolver builds no `ImplicitCallReference` for
    // `Callable().call` (the official's visitor keys on the implicit node).
    // The `DotAccess` marker's head token IS the member name, so the
    // lexeme decides; the send group's trailing markers can also tile
    // just past the operand slice, so the owner's rows one step past
    // `rows` count too.
    let call_marked = |rows: &[u32]| {
        rows.iter().any(|&row| {
            c.kind(row) == NodeKind::DotAccess && c.lexeme(c.head_token(row)) == "call"
        })
    };
    if call_marked(rows) {
        return false;
    }
    if let Some(&last) = rows.last()
        && let Some(parent) = c.unit.ast.parent(last)
    {
        let owner = c.collect_direct(parent);
        if let Some(pos) = owner.iter().position(|&kid| kid == last) {
            for &kid in &owner[pos + 1..] {
                if c.kind(kid) == NodeKind::DotAccess && c.lexeme(c.head_token(kid)) == "call" {
                    return false;
                }
            }
        }
    }
    // A binary container riding the run (`return c1 ?? c2;`): the
    // operand pieces tile beside it — either side's verdict answers
    // (the official face reports the whole expression when the
    // resolver builds the implicit reference inside it).
    if let Some(&container) = rows
        .iter()
        .find(|&row| c.kind(*row) == NodeKind::BinaryExpression)
    {
        let Some(op) = c.field(container, PayloadField::Operator) else {
            return false;
        };
        let Some(token) = c.unit.tokens.get(op as usize) else {
            return false;
        };
        let op_start = token.byte_offset as usize;
        let mut left = Vec::new();
        let mut right = Vec::new();
        for row in rows.iter().copied().filter(|&row| row != container) {
            if c.byte_span(row).end <= op_start {
                left.push(row);
            } else {
                right.push(row);
            }
        }
        return operand_rows_have_call(c, use_node, &left)
            || operand_rows_have_call(c, use_node, &right);
    }
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
    // A bare read followed only by marker pieces (`c<int>` — the
    // tear-off's `TypeArguments` tails the name inside the operand
    // slice, before the `NoArguments` group marker) reads the name's
    // type the same way the access-run arm does; a `C()` creation's
    // `Arguments` piece is not a bare-read marker and falls through.
    if let Some(&first) = rows.first()
        && c.kind(first) == NodeKind::Identifier
    {
        let tail_is_markers = rows[1..].iter().all(|&row| {
            matches!(
                c.kind(row),
                NodeKind::NoTypeArguments
                    | NodeKind::TypeArguments
                    | NodeKind::NoArguments
                    | NodeKind::Send
            )
        });
        if tail_is_markers && !rows[1..].iter().any(|&row| c.kind(row) == NodeKind::Arguments) {
            let name = c.lexeme(c.head_token(first));
            return operand_type_has_call(c, use_node, &[first], name);
        }
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

/// The top-level tiling's written type row: the initializer's decl
/// parent is the variable's `InitializedIdentifier`, whose written
/// type rides the enclosing `TopLevelFields` row outside every wrapper
/// the sibling walks climb — the same-file binding channel answers
/// (the name carries the top-level-variable context, and
/// [`crate::rules::expressions::same_file::binding`] matches the
/// declaring field itself).
fn top_level_declaration_type_row(c: &Check<'_>, use_node: u32, decl: u32) -> Option<u32> {
    if c.kind(decl) != NodeKind::InitializedIdentifier {
        return None;
    }
    let name = crate::rules::initialized_identifier_name(c, decl)?;
    super::same_file::binding(c, use_node, c.lexeme(name))?.1
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
