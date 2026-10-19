use super::{Check, Group, NodeKind, RuleSpec, container_name_token};
use crate::rules::{NONE_TOKEN, token_seq_equal};
use dartq_semantic::model::Ty;

/// Official: `pkg/linter/lib/src/rules/prefer_initializing_formals.dart`
/// — a constructor initializer (`: x = p`) or a constructor-body
/// assignment (`this.x = p;`) that assigns a constructor parameter to
/// the same-named instance field of this class, with no other reference
/// to the parameter, reports at the initializer (:66-159). The
/// conversion must be safe: the field must be an actual (non-static,
/// non-setter) field of this class, and the parameter type must be a
/// subtype of the field type (:114-118).
///
/// Approximations (documented): the field identity (`FieldElement` +
/// `enclosingElement == constructor's class`, :119-124) is taken as a
/// non-static field declared in the same class body (a constructor and
/// its class always share a file); the subtype check
/// (upstream Dart SDK source,
/// pkg/linter/lib/src/rules/prefer_initializing_formals.dart:163
/// `typeSystem.isSubtypeOf`) runs on the resolved declared types
/// (the constructor formal's and the field's captured [`Ty`] columns —
/// nullability and shape equality decide, function-shape columns
/// included); the written-lexeme equality stays as the fallback for
/// every unresolvable face — differing but subtype-related spellings
/// whose types do not resolve (`int` param for a `num` field) still
/// under-report. The single-reference count
/// (`_ReferenceCounter`, :208-222) counts expression-position
/// identifiers with the parameter's lexeme (dot-access name sides
/// excluded — they name members), captured reads excluded: a reference
/// inside a nested binder of that lexeme (a block local, catch
/// parameter, nested function, loop variable, case or if-case pattern
/// variable) resolves to the binder, not the parameter. Private fields
/// skip — the `private_named_parameters` face is feature-gated off.
/// Primary constructors are outside the substrate. Message:
/// `diagnostic.g.dart` `preferInitializingFormals`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_initializing_formals",
    group: Group::Recommended,
    problem: "Use an initializing formal to assign a parameter to a field.",
    correction: "Try using an initializing formal ('this.{0}') to initialize the field.",
    kinds: &[NodeKind::Constructor],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let kids = c.collect_direct(node);
    let Some(params_node) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    let params = constructor_params(c, params_node);
    // The instance fields of this class (same file by construction).
    let fields = class_fields(c);
    let initializers = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Initializers);
    // Face A: initializer-list entries `x = p` / `this.x = p` — the
    // initializer expression is the bare parameter.
    let mut candidates: Vec<(&str, &str, u32, u32)> = Vec::new();
    if let Some(initializers) = initializers {
        face_initializers(c, initializers, &mut candidates);
    }
    // Face B: body statements of the exact form `this.x = p;`. The
    // reference roots carry whether initializer entries' field-name
    // sides are excluded from the count (the initializer face).
    let mut ref_roots: Vec<(u32, bool)> = Vec::new();
    if let Some(initializers) = initializers {
        ref_roots.push((initializers, true));
    }
    if let Some(body) = kids
        .iter()
        .copied()
        .find(|&kid| matches!(c.kind(kid), NodeKind::Block | NodeKind::BlockBody))
    {
        ref_roots.push((body, false));
        face_body(c, body, &mut candidates);
    }
    finish(c, &params, &fields, &ref_roots, &candidates);
}

/// The constructor's convertible parameters: `(name, written type)` for
/// plain formals only — receiver formals (`this.`/`super.`) are already
/// initializing/super formals, function-typed formals have no comparable
/// written type.
fn constructor_params<'a>(c: &Check<'a>, params_node: u32) -> Vec<(&'a str, Option<u32>)> {
    let mut params: Vec<(&str, Option<u32>)> = Vec::new();
    for param in c.collect_direct(params_node) {
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        let payload = c.payload(param);
        if payload.first().copied().unwrap_or(NONE_TOKEN) != NONE_TOKEN {
            continue; // this.x / super.x formal
        }
        let Some(&name) = payload.get(1) else {
            continue;
        };
        if name == NONE_TOKEN {
            continue;
        }
        let written = c.collect_direct(param).into_iter().find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
            )
        });
        params.push((c.lexeme(name), written));
    }
    params
}

/// The non-static instance fields of the owner body: `(name, written
/// type)` shared per declaration.
fn class_fields<'a>(c: &Check<'a>) -> Vec<(&'a str, Option<u32>)> {
    let mut fields: Vec<(&str, Option<u32>)> = Vec::new();
    // Member discovery anchors on the class row's subtree: a subtree
    // occupies the contiguous row range from its first child slot to
    // its root (the class row completes last), so member rows sit in
    // the range under every tiling layout — the body container's
    // window can open later and the derived parent pointer can link a
    // field row to either neighbor.
    let Some(&class_row) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::ClassDeclaration)
    else {
        return fields;
    };
    for member in c.children(class_row).start..=class_row {
        if c.kind(member) != NodeKind::Fields
            || c.payload(member).first()
                != Some(&(dartq_parser::MemberKind::NonStaticField as u32))
        {
            continue;
        }
        // Production tiling: the written type tiles as a `Fields`-row
        // sibling before the declaration (the declaration row opens at
        // the name); hand-built tables nest the `Type` under it.
        let group_type = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type);
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            let written = group_type.or_else(|| {
                c.collect_direct(decl)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Type)
            });
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                // The production row is width-0 at the name, so the
                // name is the span head (see
                // [`crate::rules::initialized_identifier_name`]).
                if let Some(name_token) = crate::rules::initialized_identifier_name(c, var) {
                    fields.push((c.lexeme(name_token), written));
                }
            }
        }
    }
    fields
}

/// Face A (:71-85): each initializer entry whose expression is the
/// convertible assignment `x = p` / `this.x = p` pushes a candidate.
/// Production shape: the `Initializer` container completes last in its
/// window and its entry expression is an `Assignment` whose operand
/// pieces tile in the entry window (the parser re-reads the entry
/// through the expression engine; the `FieldInitializer` node is never
/// emitted on a production parse). The candidate carries the report
/// range — the entry window head through the assignment's end: the
/// assignment row's own span starts at the flattened operand and drops
/// the `this.` prefix.
fn face_initializers<'a>(
    c: &Check<'a>,
    initializers: u32,
    out: &mut Vec<(&'a str, &'a str, u32, u32)>,
) {
    for initializer in c.collect_direct(initializers) {
        if c.kind(initializer) != NodeKind::Initializer {
            continue;
        }
        // An `assert(...)` entry or a super/this redirection has no
        // assignment shape: the entry expression is the window's last
        // row.
        let window = c.children(initializer);
        if window.start >= window.end {
            continue;
        }
        let entry = window.end - 1;
        if c.kind(entry) != NodeKind::Assignment
            || c.payload_word(entry, 0).map(|op| c.lexeme(op)) != Some("=")
        {
            continue;
        }
        let pieces = assignment_pieces(c, initializer, entry);
        let Some((field, param, start_row, end_row)) = convertible_assignment(c, &pieces)
        else {
            continue;
        };
        out.push((field, param, start_row, end_row));
    }
}

/// The operand pieces of an `Assignment` row: the owner's subtree
/// roots below the assignment's adopted window (the leak of its
/// flattened left operand) plus the window rows themselves. Earlier
/// statements' rows stay in the run — the convertible matcher reads
/// the tail only.
fn assignment_pieces(c: &Check<'_>, owner: u32, assignment: u32) -> Vec<u32> {
    let window = c.children(assignment);
    let mut pieces: Vec<u32> = c
        .collect_direct(owner)
        .into_iter()
        .take_while(|&kid| kid != assignment)
        .filter(|&kid| kid < window.start)
        .collect();
    pieces.extend(window.start..window.end);
    pieces
}

/// Face B (:87-102): body statements of the exact form `this.x = p;` —
/// a plain `x = p;` body assignment assigns a local or parameter, not a
/// field, so only the `this`-targeted form converts. The constructor
/// body is the `BlockBody` function-body container (no inner `Block` row
/// on a production parse); a hand-built `Block` keeps its face.
fn face_body<'a>(c: &Check<'a>, body: u32, out: &mut Vec<(&'a str, &'a str, u32, u32)>) {
    // Production tiling: `BlockBody` is the function-body container and
    // its direct children are the flat statement pieces — a nested
    // `{ ... }` statement is just another `Block` child, NOT the body
    // itself, so the body's own children are always scanned. A
    // hand-built `Block` child carries its own assignment rows, so it
    // is scanned as an additional container rather than replacing the
    // body.
    let mut containers = vec![body];
    if c.kind(body) == NodeKind::BlockBody {
        containers.extend(
            c.collect_direct(body)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::Block),
        );
    }
    for container in containers {
        for assignment in c.collect_direct(container) {
            if c.kind(assignment) != NodeKind::Assignment
                || c.payload_word(assignment, 0).map(|op| c.lexeme(op)) != Some("=")
            {
                continue;
            }
            // The statement rows are childless (`ExpressionStatement` is a
            // span leaf), so the operand pieces assemble at the body
            // container.
            let pieces = assignment_pieces(c, container, assignment);
            let Some((field, param, start_row, end_row)) = convertible_assignment(c, &pieces)
            else {
                continue;
            };
            // Only the `this.x = p;` form converts in a body — a plain
            // `x = p;` body assignment assigns a local or parameter, not a
            // field.
            if c.kind(start_row) != NodeKind::ThisExpression {
                continue;
            }
            out.push((field, param, start_row, end_row));
        }
    }
}

/// The assignment's `(field name, parameter name, report start row,
/// report end row)` when it has a convertible shape and a
/// bare-identifier value. Production tiling: the value closes the run
/// as a tear-off group `[Id, wrappers, NoArguments, Send]`, and the
/// target closes at the piece fifth from the end — the bare form's
/// adopted `Send` leaf (its `[Id, wrappers, NoArguments]` leak precedes
/// it) or the `this.` form's childless `DotAccess` marker (its
/// `[this leaf, …, Send]` access run precedes it). Earlier statements'
/// rows may sit in the run, so only the tail is read. Both forms report
/// from the entry name / `this` leaf through the VALUE row (see the
/// branch notes — the official `reportAtNode` node ends at the assigned
/// value's last token); the marker only names the field. Anything else
/// (computed targets, prefixed values) is not convertible.
fn convertible_assignment<'a>(
    c: &Check<'a>,
    pieces: &[u32],
) -> Option<(&'a str, &'a str, u32, u32)> {
    let len = pieces.len();
    if len < 8 {
        return None;
    }
    let value = len - 4;
    if !matches!(
        (
            c.kind(pieces[value]),
            c.kind(pieces[value + 1]),
            c.kind(pieces[value + 2]),
            c.kind(pieces[value + 3]),
        ),
        (
            NodeKind::Identifier,
            NodeKind::NoTypeArguments | NodeKind::TypeArguments,
            NodeKind::NoArguments,
            NodeKind::Send
        )
    ) {
        return None;
    }
    let param = c.lexeme(c.head_token(pieces[value]));
    match c.kind(pieces[value - 1]) {
        // `x = p` — the leaked LHS group [Id, wrappers, NoArguments]
        // precedes the adopted Send.
        NodeKind::Send => {
            let base = value - 4;
            if !matches!(
                (
                    c.kind(pieces[base]),
                    c.kind(pieces[base + 1]),
                    c.kind(pieces[base + 2]),
                ),
                (
                    NodeKind::Identifier,
                    NodeKind::NoTypeArguments | NodeKind::TypeArguments,
                    NodeKind::NoArguments,
                )
            ) {
                return None;
            }
            let field = c.lexeme(c.head_token(pieces[base]));
            // The report end is the VALUE row: the official range is
            // `reportAtNode`'s node — the whole `ConstructorFieldInitializer`
            // (:93-104) or `AssignmentExpression` (:110-117) — ending at
            // the value's last token, and the value is pinned to a bare
            // identifier (:111 `rightHandSide is SimpleIdentifier`), so
            // `pieces[value]` is that last operand on every lintable
            // candidate. The assignment row's own span runs one token
            // past it (the bare-name send face adopts its follower, so
            // on statement faces it carries the `;`).
            Some((field, param, pieces[base], pieces[value]))
        }
        // `this.x = p` — the access run [this, …, Send] precedes the
        // marker.
        NodeKind::DotAccess => {
            if value < 6 {
                return None;
            }
            let target = value - 6;
            if c.kind(pieces[target]) != NodeKind::ThisExpression
                || c.kind(pieces[target + 4]) != NodeKind::Send
            {
                return None;
            }
            let marker = pieces[value - 1];
            let field = c.lexeme(c.head_token(marker));
            // The report end is the VALUE row (see the bare branch's
            // note): the official `AssignmentExpression` span ends at
            // the value's last token (:110-117), while the assignment
            // row's adopted window anchors on the value group's trailing
            // `Send` and its span carries the statement's `;`.
            Some((field, param, pieces[target], pieces[value]))
        }
        _ => None,
    }
}

/// The subtype verdict on the resolved declared types:
/// `Some(...)` when both the constructor formal and the class field
/// resolve with captured types — the field assigns from the parameter
/// (`typeSystem.isSubtypeOf` over the M1 lattice: equal interned
/// shapes assign, nullability decides, distinct named heads
/// suppress). `None` = any face unresolvable (no bridge, unindexed
/// class, uncaptured formal table, unknown type on either side).
fn resolved_subtype(c: &Check<'_>, param_name: &str, field_name: &str) -> Option<bool> {
    let param_ty = ctor_formal_ty(c, param_name)?;
    let field_ty = class_field_ty(c, field_name)?;
    if matches!(param_ty, Ty::Unknown) || matches!(field_ty, Ty::Unknown) {
        return None;
    }
    field_ty.assignable_from(param_ty)
}

/// The resolved declared type of the owning constructor's formal
/// `param_name` (the constructor's own captured formal table; the
/// written selector names the face, the unnamed constructor is the
/// class-name face). `None` under-reports.
fn ctor_formal_ty(c: &Check<'_>, param_name: &str) -> Option<Ty> {
    let analysis = c.semantics()?;
    let ctor = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::Constructor)?;
    let class_row = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::ClassDeclaration)?;
    let class_token = container_name_token(c, class_row)?;
    let container = analysis.strings().intern(c.lexeme(class_token));
    let selector = constructor_selector(c, ctor).map(|name| analysis.strings().intern(&name));
    let formals = analysis.ctor_formals_of(container, selector)?;
    let id = analysis.strings().intern(param_name);
    formals
        .positional
        .iter()
        .chain(&formals.named)
        .find(|formal| formal.name == id)
        .map(|formal| formal.ty)
}

/// The resolved declared type of this class's own instance field
/// `field_name` (the same-class face the AST walk already scoped —
/// the defining-container check keeps an inherited shadow out).
/// `None` under-reports.
fn class_field_ty(c: &Check<'_>, field_name: &str) -> Option<Ty> {
    let analysis = c.semantics()?;
    let class_row = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::ClassDeclaration)?;
    let class_token = container_name_token(c, class_row)?;
    let container = analysis.strings().intern(c.lexeme(class_token));
    let id = analysis.strings().intern(field_name);
    let (defining, entry) = analysis.inherited_member(container, id, false)?;
    (defining == container).then_some(entry.var_ty)
}

/// The constructor's written selector (`C.named` — the
/// `MethodDeclarationContinuation` identifier child); `None` names the
/// unnamed face.
fn constructor_selector(c: &Check<'_>, ctor: u32) -> Option<String> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier && c.payload(kid).first().copied() == Some(ctx)
        })
        .map(|kid| c.lexeme(c.head_token(kid)).to_string())
}

/// The shared gates and the report (:104-159): one candidate per field,
/// a same-named non-private instance field of this class, the resolved
/// subtype (the written-type equality fallback), and no other
/// reference to the parameter.
fn finish(
    c: &mut Check<'_>,
    params: &[(&str, Option<u32>)],
    fields: &[(&str, Option<u32>)],
    ref_roots: &[(u32, bool)],
    candidates: &[(&str, &str, u32, u32)],
) {
    for &(field_name, param_name, range_start, range_end) in candidates {
        // Multiple initializations of one field — the conversion is
        // ambiguous, none of them lints (:48-53, `_nodesToLintByField`).
        if candidates
            .iter()
            .filter(|(name, ..)| *name == field_name)
            .count()
            > 1
        {
            continue;
        }
        if field_name.starts_with('_') {
            continue; // private_named_parameters is off
        }
        // Must be assigning from the same-named parameter (upstream
        // `_checkInitializer`: `field.name != parameter.name` returns —
        // `C(int b) : p = b;` names a different parameter and the
        // conversion to `this.p` is not semantics-preserving).
        if field_name != param_name {
            continue;
        }
        let Some(&(_, param_written)) = params.iter().find(|(name, _)| *name == param_name)
        else {
            continue;
        };
        let Some(&(_, field_written)) = fields.iter().find(|(name, _)| *name == field_name)
        else {
            continue;
        };
        // The subtype gate (upstream :163): the resolved declared
        // types decide. Undecidable faces fall back to the
        // written-lexeme approximation — equal spellings are
        // subtype-equal (the documented standing face).
        match resolved_subtype(c, param_name, field_name) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                let (Some(param_type), Some(field_type)) = (param_written, field_written)
                else {
                    continue; // an unwritten type has no comparable spelling
                };
                if !token_seq_equal(c, param_type, field_type) {
                    continue;
                }
            }
        }
        let references: u32 = ref_roots
            .iter()
            .map(|&(root, initializer_face)| {
                count_expression_refs(c, root, param_name, initializer_face)
            })
            .sum();
        if references != 1 {
            continue;
        }
        // The entry window head through the entry expression's end —
        // the assignment row's own span drops the `this.` prefix.
        let start = c.utf16_span(range_start).start;
        let end = c.utf16_span(range_end).end;
        c.report(start..end, &[field_name]);
    }
}

/// The expression-position references to `name` within one constructor
/// reference root (the initializer list or the body). Only
/// `Expression`-context identifiers count. A name side of a property
/// access is a member read, never a parameter reference — its selector
/// is covered by the access's trailing `DotAccess` marker (childless in
/// the production tiling). On the initializer face the entry
/// assignment's left-hand side names the field (the official
/// `ConstructorFieldInitializer` field name), never the parameter. A
/// read captured by a nested binder of the same lexeme resolves to the
/// binder, not the parameter (the official counts element identities,
/// `_ReferenceCounter`), so it does not count.
fn count_expression_refs(c: &Check<'_>, root: u32, name: &str, initializer_face: bool) -> u32 {
    let expression_ctx = dartq_parser::IdentifierContext::Expression as u32;
    let window = c.children(root);
    let access_markers: Vec<_> = (window.start..window.end)
        .filter(|&piece| c.kind(piece) == NodeKind::DotAccess)
        .map(|piece| c.utf16_span(piece))
        .collect();
    // On the initializer face the entry assignment's left-hand side
    // names the field (the official `ConstructorFieldInitializer` field
    // name), never the parameter. The assignment's adopted child window
    // cannot draw that line — `begin_with_first_child` anchors on the
    // last completed subtree, the value group's trailing `Send` leaf,
    // which sits ABOVE the value's own rows — so the split is the
    // operator token: an identifier closing at or before its `=` is on
    // the field-name side.
    let assignment_ops: Vec<u32> = if initializer_face {
        (window.start..window.end)
            .filter(|&piece| c.kind(piece) == NodeKind::Assignment)
            .filter_map(|piece| c.payload(piece).first().copied())
            .filter(|&op| op != NONE_TOKEN)
            .filter_map(|op| c.unit.tokens.get(op as usize).map(|t| t.utf16_offset))
            .collect()
    } else {
        Vec::new()
    };
    let binders: Vec<u32> = (window.start..window.end)
        .filter(|&piece| super::scope_binds(c, piece, name))
        .collect();
    let mut count = 0;
    for id in window.start..window.end {
        if c.kind(id) != NodeKind::Identifier
            || c.payload(id).first() != Some(&expression_ctx)
            || c.lexeme(c.head_token(id)) != name
        {
            continue;
        }
        let span = c.utf16_span(id);
        if assignment_ops.iter().any(|&op| span.end <= op) {
            continue;
        }
        if access_markers
            .iter()
            .any(|access| access.start <= span.start && access.end >= span.end)
        {
            continue;
        }
        if binders
            .iter()
            .any(|&binder| super::scope_binds_at(c, binder, name, c.byte_span(id).start))
        {
            continue;
        }
        count += 1;
    }
    count
}
