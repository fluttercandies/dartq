use dartq_semantic::model::Ty;

use super::{Check, Group, NodeKind, RuleSpec, query_site, same_file};
use crate::rules::{byte_span_of_token, slice};

/// Official: `pkg/linter/lib/src/rules/no_literal_bool_comparisons.dart:44-62`
/// — a boolean literal on either side of `==`/`!=`/`|`/`||`/`&`/`&&`/`^`
/// whose OTHER operand is a non-nullable `bool` reports at the literal
/// (:55-60). The other operand's type is resolution. The written-type
/// face over the same-file channel: a bare identifier (or its
/// invocation) bound to a written `bool` — parameter, local, field,
/// top-level variable — or to a written `bool Function(…)` (the
/// invocation face, the return type read), a bool literal on both
/// sides, and a member access `r.m` whose receiver's type carries a
/// non-nullable `bool` member `m` — the bridge member type when
/// mounted (any receiver the fold typed, same-file or cross-file),
/// `this.<member>` through the enclosing class, or the receiver's
/// same-file written class. A written `bool?` stays silent exactly
/// like the official (:41, the `isNonNullable` gate); inferred-`bool`
/// locals (`var x = true`) under-report — the fold publishes no
/// inferred type (the documented inference family). Message:
/// `diagnostic.g.dart` `noLiteralBoolComparisons`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "no_literal_bool_comparisons",
    group: Group::Standalone,
    problem: "Unnecessary comparison to a boolean literal.",
    correction: "Remove the comparison and use the negation operator `!` if necessary.",
    kinds: &[NodeKind::BinaryExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op) = c.payload(node).first().copied() else {
        return;
    };
    if !matches!(c.lexeme(op), "==" | "!=" | "|" | "||" | "&" | "&&" | "^") {
        return;
    }
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let report = if let Some(literal) = bool_literal(c, &left) {
        other_is_bool(c, &right).then_some(literal)
    } else if let Some(literal) = bool_literal(c, &right) {
        other_is_bool(c, &left).then_some(literal)
    } else {
        None
    };
    if let Some(literal) = report {
        c.report(c.utf16_span(literal), &[]);
    }
}

/// The bool-literal operand of a side, when the side is one.
fn bool_literal(c: &Check<'_>, rows: &[u32]) -> Option<u32> {
    rows.iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::LiteralBool)
}

/// The other-side verdict: a non-nullable `bool` over the written-type
/// faces (:41 — the `isNonNullable` gate).
fn other_is_bool(c: &Check<'_>, rows: &[u32]) -> bool {
    // A bool literal on the other side is a bool (`true == true`).
    if rows.iter().any(|&row| c.kind(row) == NodeKind::LiteralBool) {
        return true;
    }
    let run = slice::access_run(c, rows);
    match run.as_slice() {
        // A bare name: the binding's written type (`bool`, or a
        // function type returning `bool` — the invocation face).
        [only] if c.kind(*only) == NodeKind::Identifier => written_is_bool(c, *only),
        // An invocation of a bare name: the callee's written return
        // type (`fn() == true`).
        [callee, arguments]
            if c.kind(*callee) == NodeKind::Identifier
                && c.kind(*arguments) == NodeKind::Arguments =>
        {
            written_is_bool(c, *callee)
        }
        // A member access over a bare receiver: the same-file class
        // member's written type (`c.x == true`).
        _ => member_face(c, rows),
    }
}

/// The binding face of the identifier `name_row`: `true` when its
/// written type is a non-nullable `bool`, or a function type whose
/// return type is a non-nullable `bool`.
fn written_is_bool(c: &Check<'_>, name_row: u32) -> bool {
    let name = c.lexeme(c.head_token(name_row));
    let Some((_, type_row)) = same_file::binding(c, c.node(), name) else {
        return false;
    };
    let Some(type_row) = type_row else {
        return false;
    };
    non_null_bool_type(c, type_row)
}

/// Whether the written type row is a non-nullable `bool` (or, for
/// function types, has a non-nullable `bool` return type).
fn non_null_bool_type(c: &Check<'_>, type_row: u32) -> bool {
    match c.kind(type_row) {
        NodeKind::Type => {
            let head = c.head_token(type_row);
            c.lexeme(head) == "bool"
                && c.byte_span(type_row).end == byte_span_of_token(c, head).end
        }
        NodeKind::FunctionType => c
            .collect_direct(type_row)
            .into_iter()
            .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::VoidType))
            .is_some_and(|return_row| non_null_bool_type(c, return_row)),
        _ => false,
    }
}

/// The member face over the raw operand rows: a `DotAccess` marker's
/// member name against a receiver whose type carries a non-nullable
/// `bool` member — the bridge member type when mounted (the receiver's
/// resolved declared type, same-file or cross-file), then `this.<m>`
/// through the enclosing class, then the receiver's same-file written
/// class.
fn member_face(c: &Check<'_>, rows: &[u32]) -> bool {
    let Some(marker) = rows
        .iter()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::DotAccess)
    else {
        return false;
    };
    let Some(name_index) = slice::access_name_index(c, rows, marker) else {
        return false;
    };
    let name = c.lexeme(c.head_token(rows[name_index]));
    // The bridge member type decides first: the receiver's declared
    // type read (`u.flag`) resolves across files.
    if let Some(analysis) = c.semantics()
        && let Some(receiver) = slice::bare_receiver(c, rows, name_index)
        && let Some(ty) =
            analysis.member_type(query_site(c, receiver), analysis.strings().intern(name))
        && let Ty::Named {
            name: ty_name,
            nullable: false,
            ..
        } = ty
    {
        return analysis.strings().resolve(ty_name) == "bool";
    }
    // `this.<member>`: the enclosing class-like scope's member.
    let run = slice::access_run(c, rows);
    if let [target, member] = run.as_slice()
        && c.kind(*target) == NodeKind::ThisExpression
    {
        let Some(class_row) = c.path.iter().rev().find_map(|&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration | NodeKind::MixinDeclaration
            )
            .then_some(a)
        }) else {
            return false;
        };
        let Some(class_name) = crate::rules::class_declaration_name(c, class_row) else {
            return false;
        };
        return member_is_bool(c, class_name, c.lexeme(c.head_token(*member)));
    }
    let Some(receiver) = slice::bare_receiver(c, rows, name_index) else {
        return false;
    };
    let receiver_name = c.lexeme(c.head_token(receiver));
    let Some(class_name) =
        same_file::written_type(c, receiver, receiver_name).map(str::to_owned)
    else {
        return false;
    };
    member_is_bool(c, &class_name, name)
}

/// Whether the same-file class `class_name` declares a member `name`
/// whose written type is a non-nullable `bool` (a field or a getter).
fn member_is_bool(c: &Check<'_>, class_name: &str, name: &str) -> bool {
    let Some(class_row) = crate::rules::same_file_declaration(c, class_name) else {
        return false;
    };
    for member in crate::rules::body_members(c, class_row) {
        match c.kind(member) {
            NodeKind::Fields => {
                let mut written = None;
                for kid in c.collect_direct(member) {
                    match c.kind(kid) {
                        NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType => {
                            written = Some(kid);
                        }
                        NodeKind::Identifier
                            if c.payload(kid).first()
                                == Some(
                                    &(dartq_parser::IdentifierContext::FieldDeclaration as u32),
                                )
                                && c.lexeme(c.head_token(kid)) == name =>
                        {
                            return written.is_some_and(|row| non_null_bool_type(c, row));
                        }
                        _ => {}
                    }
                }
            }
            NodeKind::Method if crate::rules::get_set_token(c, member) == Some("get") => {
                let Some(name_row) = c.collect_direct(member).into_iter().find(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.payload(kid).first()
                            == Some(
                                &(dartq_parser::IdentifierContext::MethodDeclaration as u32),
                            )
                }) else {
                    continue;
                };
                if c.lexeme(c.head_token(name_row)) != name {
                    continue;
                }
                if let Some((type_row, _)) = crate::rules::type_before(c, member, name_row)
                    && non_null_bool_type(c, type_row)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
