use super::{
    Check, Group, NodeKind, RuleSpec, VAR_FINAL_OR_CONST_MEMBER_BIT, formal_parameter_rows,
    same_class_field_type,
};
use crate::rules::{NONE_TOKEN, body_members, initialized_identifier_name};
use dartq_ast::mod_flags;

/// Official:
/// `pkg/linter/lib/src/rules/use_declaring_parameters.dart` — a
/// primary constructor's parameter that declares (or is assigned to)
/// its field reports at the parameter name. Registration is
/// feature-gated: the primary-constructors `isFeatureEnabled` bail
/// (:35) — pinned on
/// [`crate::engine::Features::primary_constructors`]. Faces (:43-57):
/// a `this.f` field formal checks (:61-71 — no written type, or the
/// field's type equals the parameter's); a parameter without a
/// storage keyword assigned to a same-named field (modulo a leading
/// underscore, :148-159) by a body initializer or a body-block
/// assignment checks (:76-94); super formals and storage-keyword
/// parameters skip; a body-less primary has no declaring candidates
/// (:50-54). Message: `diagnostic.g.dart` `useDeclaringParameters`.
///
/// Approximations: the element type equalities (:67/:90) fold to the
/// written type spellings for field formals (the same-file field
/// read) and to the name match for assignments — a same-named field
/// of a different type is a compile error on valid input; the field
/// existence gate folds to the same-file field scan (cross-file
/// under-reports).
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_declaring_parameters",
    group: Group::Standalone,
    problem: "Use a declaring parameter.",
    correction: "Try using a declaring parameter.",
    kinds: &[NodeKind::PrimaryConstructor],
    check,
};

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:35)
    }
    let node = c.node();
    let Some(class_row) = c.path[..c.path.len() - 1].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
        )
    }) else {
        return;
    };
    // `node.body` (:44) — the paired member row; a body-less primary
    // skips the non-declaring branch (:50-54).
    let body = body_members(c, class_row)
        .into_iter()
        .find(|&member| c.kind(member) == NodeKind::PrimaryConstructorBody);
    for param in formal_parameter_rows(c, node) {
        // The receiver word answers the field/super formal split.
        let receiver = c.payload(param).first().copied().unwrap_or(NONE_TOKEN);
        if receiver != NONE_TOKEN {
            if c.lexeme(receiver) == "this" {
                check_field_formal(c, param, class_row);
            }
            continue; // super formals skip (:48)
        }
        // `constFinalOrVarKeyword == null` (:49).
        if storage_keyword(c, param).is_some() {
            continue;
        }
        let Some(body) = body else {
            continue;
        };
        check_non_declaring(c, param, body, class_row);
    }
}

/// `_checkFieldFormalParameter` (:61-71): `parameter.type == null` or
/// the field's type equals the parameter's — the equality folds to
/// the written spellings (the same-file field read; the field's
/// element is the parameter's by construction when the spellings
/// match, and the `field != null` gate skips unknown names).
fn check_field_formal(c: &mut Check<'_>, param: u32, class_row: u32) {
    let Some(&name) = c.payload(param).get(1) else {
        return;
    };
    if name == NONE_TOKEN {
        return;
    }
    let written = written_type(c, param);
    let Some(written) = written else {
        c.report(crate::rules::utf16_token_span(c, name), &[]); // no type
        return;
    };
    let Some(field) = same_class_field_type(c, class_row, c.lexeme(name)) else {
        return; // `field != null` (:66)
    };
    if c.unit.source[c.byte_span(written)] == c.unit.source[c.byte_span(field)] {
        c.report(crate::rules::utf16_token_span(c, name), &[]);
    }
}

/// `_checkNonDeclaringParameter` (:76-94): the parameter assigned to a
/// same-named field by a body initializer or a body-block assignment.
fn check_non_declaring(c: &mut Check<'_>, param: u32, body: u32, class_row: u32) {
    let Some(&name_token) = c.payload(param).get(1) else {
        return;
    };
    if name_token == NONE_TOKEN {
        return; // `name != null` (:87)
    }
    let name = c.lexeme(name_token);
    // `_findAssignedField` (:100-144).
    let Some(field_name) = assigned_field_name(c, body, name) else {
        return;
    };
    // `_namesMatch` (:114/:132) then the `FieldElement` gate — the
    // type equality folds to the name match (a same-named field of a
    // different type is a compile error on valid input).
    if !names_match(name, field_name) || !field_exists(c, class_row, field_name) {
        return;
    }
    c.report(crate::rules::utf16_token_span(c, name_token), &[]);
}

/// The assigned field's name, or `None`: a primary-body initializer
/// whose expression is exactly the parameter identifier (the whole
/// expression, :111), or a body-block assignment whose right side is
/// the parameter (:125-128).
fn assigned_field_name<'a>(c: &'a Check<'_>, body: u32, name: &str) -> Option<&'a str> {
    for piece in c.collect_direct(body) {
        if c.kind(piece) != NodeKind::Initializers {
            continue;
        }
        for initializer in c.collect_direct(piece) {
            if c.kind(initializer) != NodeKind::Initializer {
                continue;
            }
            // `field = expr` tiles the `Assignment` row as the
            // initializer's last child (`super()`/`assert()` carry a
            // call chain instead).
            let Some(assignment) = c
                .collect_direct(initializer)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Assignment)
            else {
                continue;
            };
            if let Some(field) = assigned_field_in(c, assignment, name) {
                return Some(field);
            }
        }
    }
    let block_body = c
        .collect_direct(body)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::BlockBody)?;
    // The production `BlockBody` parents its statement rows directly
    // (no intermediate `Block`), and the flat send-chain tiling keeps
    // the `Assignment` tail as a direct `BlockBody` child alongside
    // the `ExpressionStatement` marker — the statement's own window is
    // empty.
    for piece in c.collect_direct(block_body) {
        if c.kind(piece) == NodeKind::Assignment
            && let Some(field) = assigned_field_in(c, piece, name)
        {
            return Some(field);
        }
    }
    None
}

/// The `_i = i` / `this.i = i` field name when the assignment's right
/// side is exactly the parameter (`names_match` modulo a leading
/// underscore). The adopted-operator container keeps only the right
/// run in its window; `binary_operands_at` re-splits left/right on
/// the operator token.
fn assigned_field_in<'a>(c: &'a Check<'_>, assignment: u32, name: &str) -> Option<&'a str> {
    let (left, right) = crate::rules::slice::binary_operands_at(c, assignment)?;
    // The right side is exactly the parameter identifier: a bare `i`
    // send chain whose only `Identifier` row is `i` itself.
    let identifiers: Vec<u32> = right
        .iter()
        .copied()
        .filter(|&piece| c.kind(piece) == NodeKind::Identifier)
        .collect();
    let [rhs] = identifiers.as_slice() else {
        return None;
    };
    if !names_match(c.lexeme(c.head_token(*rhs)), name) {
        return None;
    }
    // The write target names the field: the last identifier of the
    // left run (`i` or `this.i`).
    left.iter()
        .rev()
        .flat_map(|&kid| {
            if c.kind(kid) == NodeKind::Identifier {
                vec![kid]
            } else {
                c.collect_direct(kid)
            }
        })
        .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        .map(|kid| c.lexeme(c.head_token(kid)))
}

/// `_namesMatch` (:148-159): the names are equal modulo one leading
/// underscore on either side.
fn names_match(parameter: &str, field: &str) -> bool {
    parameter == field
        || (parameter.starts_with('_') && &parameter[1..] == field)
        || (field.starts_with('_') && parameter == &field[1..])
}

/// The same-class field `name` (the `FieldElement` gate).
fn field_exists(c: &Check<'_>, class_row: u32, name: &str) -> bool {
    body_members(c, class_row).iter().any(|&member| {
        c.kind(member) == NodeKind::Fields
            && c.children(member).any(|descendant| {
                c.kind(descendant) == NodeKind::InitializedIdentifier
                    && initialized_identifier_name(c, descendant)
                        .is_some_and(|token| c.lexeme(token) == name)
            })
    })
}

/// The parameter's written type row, if any (`parameter.type`).
fn written_type(c: &Check<'_>, param: u32) -> Option<u32> {
    c.collect_direct(param).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    })
}

/// The `var`/`final`/`const` keyword token of a
/// `VAR_FINAL_OR_CONST`-marked parameter (`None` when unmarked;
/// bounded: `required`/`covariant` precede the storage keyword).
fn storage_keyword(c: &Check<'_>, param: u32) -> Option<u32> {
    if mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_MEMBER_BIT == 0 {
        return None;
    }
    let mut token = c.head_token(param);
    for _ in 0..3 {
        match c.lexeme(token) {
            "var" | "final" | "const" => return Some(token),
            "required" | "covariant" => token += 1,
            _ => return None,
        }
    }
    None
}
