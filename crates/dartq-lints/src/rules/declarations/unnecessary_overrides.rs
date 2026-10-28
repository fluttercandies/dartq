use super::{Check, NodeKind, RuleSpec, member_declaration_name, operator_name};
use crate::rules::{MemberQuery, has_doc_comment, member_annotations, member_is_concrete, member_matches, same_file_ancestors, token_seq_equal, type_before};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_overrides.dart`
/// — an override whose body is exactly a super forward with the same
/// parameters reports at the method name (:52-113, the four faces).
///
/// Faces: the method/getter/setter bodies (:52-113: one statement /
/// arrow body, no comments before the `return`, super target, same-name
/// selector, arguments naming the parameters; the setter write face
/// :313-326) and the operator bodies (:245-292 — a binary forward
/// `super op other` naming the sole parameter, a prefix forward
/// `op super` with no parameters; `[]`/`[]=` index bodies match no
/// visitor upstream and stay silent). The inherited concrete member is
/// the same-file extends/with/on chain (an abstract declaration never
/// counts; cross-file stays unwired — the bridge's
/// `inherited_member` entry cannot answer the report's remaining
/// gates: `_haveSameDeclaration` (:145-152) compares `param.isCovariant`
/// and `param.defaultValueCode`, and `_addsMetadata`/`_makesPublicFromProtected`
/// (:121-125, :157-169) read `_inheritedMethod.metadata.hasProtected`,
/// none of which the model's `MemberEntry`/`Formal` rows carry — a
/// wired arm could only ever stay silent, so the missing faces are
/// the member-annotation face and the formal covariant/default-value
/// faces). The signature-equality guard (`_haveSameDeclaration`,
/// :133-155 — element types) reads written shapes: same written return
/// type (token run equality or both absent), same parameter count,
/// per-parameter kind group, name, written prefix run (type +
/// `covariant`), and default value run — differing-but-subtype
/// spellings and resolved defaults under-report. The metadata guards
/// port exactly: any annotation other than `@override` arms the
/// meaningful-override skip, a `@protected` annotation excepted when the
/// inherited member is `@protected` too (:117-131), and an override
/// without `@protected` over a `@protected` inherited member skips as
/// the visibility expansion (:92-93, :162-169) — both read the written
/// annotation names (the element-face posture). The noSuchMethod and
/// doc-comment skips (:76-79) port directly. Message:
/// `diagnostic.g.dart` `unnecessaryOverrides`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_overrides",
    &[NodeKind::Method],
    check,
);

fn check(c: &mut Check<'_>) {
    // Static skip (:56) — header ModSet bit (static is bit 3).
    const STATIC_BIT: u16 = 1 << 3;
    let node = c.node();
    if dartq_ast::mod_flags(c.unit.ast, node) & STATIC_BIT != 0 {
        return;
    }
    // The operator face names through the OperatorName row (the
    // production wrapper keeps it as the member row's left sibling);
    // methods and accessors name through the `MethodDeclaration`-context
    // identifier.
    let operator = super::operator_name(c, node);
    let Some(name_row) = operator.or_else(|| {
        c.collect_direct(node).into_iter().find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first()
                    == Some(&(dartq_parser::IdentifierContext::MethodDeclaration as u32))
        })
    }) else {
        return;
    };
    let name = c.lexeme(c.head_token(name_row)).to_owned();
    // 'noSuchMethod' is mandatory to proxify (:76).
    if name == "noSuchMethod" {
        return;
    }
    // It's ok to override to have better documentation (:79).
    if has_doc_comment(c, node) {
        return;
    }
    let get_set = super::get_set_token(c, node);
    let query = match (operator, get_set) {
        (Some(_), _) => MemberQuery::Method,
        (None, Some("get")) => MemberQuery::Getter,
        (None, Some("set")) => MemberQuery::Setter,
        (None, _) => MemberQuery::Method,
    };
    let Some(&class_row) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    }) else {
        return;
    };
    // The inherited concrete member (:81-82). The operator face names
    // through the left-sibling `OperatorName` row (production parses
    // keep it out of the member's child list — `member_matches` answers
    // the child-stamped shapes only).
    let Some(inherited) = same_file_ancestors(c, class_row, true)
        .into_iter()
        .find_map(|ancestor| {
            crate::rules::body_members(c, ancestor)
                .into_iter()
                .find(|&member| {
                    c.kind(member) == NodeKind::Method
                        && member_is_concrete(c, member)
                        && match operator {
                            Some(_) => {
                                let inherited_op = operator_name(c, member)
                                    .and_then(|row| c.payload_word(row, 0))
                                    .map(|t| c.lexeme(t));
                                let own_op = c.payload_word(name_row, 0).map(|t| c.lexeme(t));
                                inherited_op.is_some() && inherited_op == own_op
                            }
                            None => !member_matches(c, member, query, &name).is_empty(),
                        }
                })
        })
    else {
        return;
    };
    // The inherited member's written annotation names — the `@protected`
    // pairing's face (:122, :168).
    let inherited_protected = member_annotation_names(c, inherited).contains(&"protected");
    // It's ok to override to add annotations (:87, :117-131).
    if adds_metadata(c, node, inherited_protected) {
        return;
    }
    // It's ok to override to change the signature (:90, :133-155).
    let same = match operator {
        Some(_) => same_operator_declaration(c, node, inherited),
        None => same_declaration(c, node, name_row, inherited),
    };
    if !same {
        return;
    }
    // It's ok to override to make a `@protected` method public (:93,
    // :162-169).
    if !member_annotation_names(c, node).contains(&"protected") && inherited_protected {
        return;
    }
    // The body walk (:95, :52-113; operators :245-292).
    // The body walk (:95, :52-113; operators :245-292). The
    // `OperatorName` row anchors at the `operator` keyword; the operator
    // itself is payload word 0.
    let forwards = match operator {
        Some(op_row) => match c.payload_word(op_row, 0) {
            Some(token) => super_operator_forwards(c, node, c.lexeme(token)),
            None => false,
        },
        None => forwards_to_super(c, node, query, &name),
    };
    if forwards {
        // An operator reports at its own token (the `+`/`~` — the
        // `OperatorName` row anchors at the `operator` keyword, so its
        // span would over-run); a method/accessor at the name row.
        let range = match operator {
            Some(op_row) => {
                let Some(start) = c
                    .payload_word(op_row, 0)
                    .and_then(|token| c.utf16_of_token(token))
                else {
                    return;
                };
                start..start
            }
            None => c.utf16_span(name_row),
        };
        c.report(range, &[]);
    }
}

/// The written annotation names of a member (the `MetadataStar` left
/// sibling's children — the element `metadata` read's written face).
fn member_annotation_names<'a>(c: &'a Check<'a>, member: u32) -> Vec<&'a str> {
    let Some(parent) = c.unit.ast.parent(member) else {
        return Vec::new();
    };
    member_annotations(c, parent, member)
        .iter()
        .filter_map(|&meta| {
            c.collect_direct(meta).into_iter().find_map(|piece| {
                (c.kind(piece) == NodeKind::Identifier).then(|| c.lexeme(c.head_token(piece)))
            })
        })
        .collect()
}

/// `_addsMetadata` (:117-131): any annotation other than `@override`
/// arms the meaningful-override skip — a `@protected` annotation
/// excepted when the inherited member is `@protected` too (:122).
fn adds_metadata(c: &Check<'_>, node: u32, inherited_protected: bool) -> bool {
    member_annotation_names(c, node)
        .iter()
        .any(|name| match *name {
            "override" => false,
            "protected" => !inherited_protected,
            _ => true,
        })
}

/// `_haveSameDeclaration` (:133-155) over written shapes: return type,
/// then every parameter's name, kind group, receiver, prefix run
/// (written type + modifiers) and default run.
fn same_declaration(c: &Check<'_>, mine: u32, mine_name: u32, inherited: u32) -> bool {
    // The inherited method's name row (the query matched it) — the
    // shared context-stamped read.
    let Some(inherited_name) = member_declaration_name(c, inherited) else {
        return false;
    };
    // Return types: same written spelling or both absent (:138-140).
    match (
        type_before(c, mine, mine_name),
        type_before(c, inherited, inherited_name),
    ) {
        (None, None) => {}
        (Some(a), Some(b)) if token_seq_equal(c, a.0, b.0) => {}
        _ => return false,
    }
    params_equal(c, mine, inherited)
}

/// `_haveSameDeclaration` (:133-155) for an operator override: the
/// return type is the re-parsed `Type` child (the member row anchors at
/// the operator token and the `OperatorName` name rides the left
/// sibling, so `type_before`'s sibling face cannot reach it), then the
/// same per-parameter comparison.
fn same_operator_declaration(c: &Check<'_>, mine: u32, inherited: u32) -> bool {
    let operator_return = |member: u32| {
        c.collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)
    };
    match (operator_return(mine), operator_return(inherited)) {
        (None, None) => {}
        (Some(a), Some(b)) if token_seq_equal(c, a, b) => {}
        _ => return false,
    }
    params_equal(c, mine, inherited)
}

/// The parameter half of `_haveSameDeclaration` (:141-153): same count,
/// then per-parameter name, kind group, receiver, written prefix run
/// and default run.
fn params_equal(c: &Check<'_>, mine: u32, inherited: u32) -> bool {
    let (Some(mine_params), Some(inherited_params)) = (
        c.collect_direct(mine)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters),
        c.collect_direct(inherited)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters),
    ) else {
        // Both sides are accessors — no parameter lists to differ.
        return true;
    };
    let a = flat_params(c, mine_params);
    let b = flat_params(c, inherited_params);
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(p, q)| {
            p.name == q.name
                && p.group == q.group
                && p.receiver == q.receiver
                && prefix_run(c, p.name_token) == prefix_run(c, q.name_token)
                && default_run(c, p.name_token) == default_run(c, q.name_token)
        })
}

/// One parameter's comparable face: receiver lexeme (`this`/`super`/
/// `covariant`), name lexeme, kind group (0 required, 1 optional
/// positional, 2 named).
struct ParamFace<'a> {
    receiver: Option<&'a str>,
    name: &'a str,
    name_token: u32,
    group: u8,
}

/// Every parameter of a list in order, all groups.
fn flat_params<'a>(c: &'a Check<'_>, params: u32) -> Vec<ParamFace<'a>> {
    let mut out = Vec::new();
    for section in c.collect_direct(params) {
        match c.kind(section) {
            NodeKind::FormalParameter => {
                if let Some(face) = param_face(c, section, 0) {
                    out.push(face);
                }
            }
            NodeKind::OptionalFormalParameters => {
                let group: u8 = if c.lexeme(c.head_token(section)) == "{" {
                    2
                } else {
                    1
                };
                for param in c.collect_direct(section) {
                    if c.kind(param) == NodeKind::FormalParameter
                        && let Some(face) = param_face(c, param, group)
                    {
                        out.push(face);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn param_face<'a>(c: &'a Check<'_>, param: u32, group: u8) -> Option<ParamFace<'a>> {
    let payload = c.payload(param);
    let receiver = payload.first().copied().unwrap_or(crate::rules::NONE_TOKEN);
    let name_token = payload.get(1).copied()?;
    if name_token == crate::rules::NONE_TOKEN {
        return None;
    }
    let receiver = (receiver != crate::rules::NONE_TOKEN).then(|| c.lexeme(receiver));
    Some(ParamFace {
        receiver,
        name: c.lexeme(name_token),
        name_token,
        group,
    })
}

/// The written tokens before a parameter's name, back to the enclosing
/// boundary — the type-and-modifier run (`covariant int?`).
fn prefix_run<'a>(c: &'a Check<'_>, name_token: u32) -> Vec<&'a str> {
    let mut run = Vec::new();
    let mut token = name_token;
    for _ in 0..64 {
        let Some(prev) = token.checked_sub(1) else {
            break;
        };
        token = prev;
        match c.lexeme(prev) {
            "," | "(" | ")" | "{" | "}" | "[" | "]" | ";" | "=" | "<" => break,
            lex => run.push(lex),
        }
    }
    run.reverse();
    run
}

/// The written tokens after a parameter's name up to the next
/// boundary — the default value run (presence and spelling).
fn default_run<'a>(c: &'a Check<'_>, name_token: u32) -> Vec<&'a str> {
    let mut run = Vec::new();
    let mut token = name_token;
    for _ in 0..64 {
        token += 1;
        if c.unit.tokens.get(token as usize).is_none() {
            break;
        }
        match c.lexeme(token) {
            "," | ")" | "}" | "]" | ";" | "{" => break,
            lex => run.push(lex),
        }
    }
    run
}

/// The body walk (:52-113): an arrow body's expression — the childless
/// `ExpressionBody` handle keeps the expression as the member window's
/// trailing rows — or a block body's single statement, whose rows tile
/// flat into the block (the statement row itself trails the
/// expression; the `return`-comment guard :105, no guard on a bare
/// expression statement :69-71); then the face shape.
fn forwards_to_super(c: &Check<'_>, node: u32, query: MemberQuery, name: &str) -> bool {
    let Some(rows) = body_expression_rows(c, node) else {
        return false;
    };
    match query {
        MemberQuery::Method => super_call_forwards(c, &rows, name, node),
        MemberQuery::Getter => super_getter_forwards(c, &rows, name),
        MemberQuery::Setter => {
            let Some(param) = sole_parameter(c, node) else {
                return false;
            };
            super_setter_forwards(c, &rows, name, param)
        }
        MemberQuery::Variable => false,
    }
}

/// The override body's expression rows — the shared extraction of
/// [`forwards_to_super`] and the operator walk (:52-113).
fn body_expression_rows(c: &Check<'_>, node: u32) -> Option<Vec<u32>> {
    let kids = c.collect_direct(node);
    let &body = kids
        .iter()
        .find(|&&kid| matches!(c.kind(kid), NodeKind::BlockBody | NodeKind::ExpressionBody))?;
    if c.kind(body) == NodeKind::ExpressionBody {
        // The expression completes before the childless handle
        // opens (bridge `handle_expression_function_body`): the
        // member-window pieces before the handle, from the arrow
        // on — the signature rows tile before the arrow and drop
        // out.
        let arrow_end = crate::rules::byte_span_of_token(c, c.head_token(body)).end;
        let pos = kids.iter().position(|&kid| kid == body).unwrap_or(0);
        return Some(
            kids[..pos]
                .iter()
                .copied()
                .filter(|&row| c.byte_span(row).start >= arrow_end)
                .collect(),
        );
    }
    // Statement tiling: the statement rows (and their expression
    // pieces) are the body row's own children on production
    // parses — a `Block` wrapper row exists only in hand-built
    // trees.
    let block_rows = match c
        .collect_direct(body)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Block)
    {
        Some(block) => c.collect_direct(block),
        None => c.collect_direct(body),
    };
    // Exactly one statement (:52-56): the statement's own row
    // is the block window's last row, and no other statement
    // row precedes it.
    let (&last, head) = block_rows.split_last()?;
    if head
        .iter()
        .any(|&row| crate::rules::is_statement_row(c, row))
    {
        return None;
    }
    match c.kind(last) {
        NodeKind::ExpressionStatement => Some(head.to_vec()),
        NodeKind::ReturnStatement => {
            if c.comments.has_attached(c.head_token(last) as usize) {
                return None;
            }
            Some(c.collect_direct(last))
        }
        _ => None,
    }
}

/// The operator body walk (:245-292): any binary expression whose
/// operator is the declared one, with the sole parameter as the right
/// operand and the unparenthesized left operand `super` (:266-278), or
/// any prefix expression over `super` with no parameters (:280-291).
/// Index bodies (`[]`/`[]=`) match no visitor upstream and stay silent.
fn super_operator_forwards(c: &Check<'_>, node: u32, op: &str) -> bool {
    let Some(rows) = body_expression_rows(c, node) else {
        return false;
    };
    let params = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    let faces = params
        .map(|params| flat_params(c, params))
        .unwrap_or_default();
    // The binary face: exactly one parameter (:268-270). The operand
    // pieces tile inside the `BinaryExpression` row's completion window
    // (the row is the body's sole direct child), so the operands split
    // through [`binary_operands_at`]'s operator-byte partition — a bare
    // `other` operand is a `Send` tear-off group whose name is the
    // leading `Identifier` ([`access_run`] peels the group markers).
    if let [only] = faces.as_slice() {
        for &row in rows.iter() {
            if c.kind(row) != NodeKind::BinaryExpression {
                continue;
            }
            let Some(&op_token) = c.payload(row).first() else {
                continue;
            };
            if c.lexeme(op_token) != op {
                continue;
            }
            let Some((lhs, rhs)) = crate::rules::slice::binary_operands_at(c, row) else {
                continue;
            };
            let lhs_super = unparenthesized(c, &lhs)
                .is_some_and(|root| c.kind(root) == NodeKind::SuperExpression);
            let rhs_run = crate::rules::slice::access_run(c, &rhs);
            let rhs_names_param = matches!(rhs_run.as_slice(), [single]
                if c.kind(*single) == NodeKind::Identifier
                    && c.lexeme(c.head_token(*single)) == only.name);
            if lhs_super && rhs_names_param {
                return true;
            }
        }
    }
    // The prefix face: no parameters (:282-284). The `UnaryPrefix` leaf
    // anchors at the operator token (its payload carries none), the
    // operand `SuperExpression` tiles before it.
    if faces.is_empty() {
        for (index, &row) in rows.iter().enumerate() {
            if c.kind(row) != NodeKind::UnaryPrefix {
                continue;
            }
            if c.lexeme(c.head_token(row)) != op {
                continue;
            }
            if unparenthesized(c, &rows[..index])
                .is_some_and(|root| c.kind(root) == NodeKind::SuperExpression)
            {
                return true;
            }
        }
    }
    false
}

/// An operand's root row: the group's last row, unwrapping the width-0
/// paren rows (`(x)` seals at the `(` after the inner completes — the
/// `unParenthesized`/`canonicalElement` faces).
fn unparenthesized(c: &Check<'_>, rows: &[u32]) -> Option<u32> {
    let mut root = *rows.last()?;
    while c.kind(root) == NodeKind::ParenthesizedExpression {
        let window = c.children(root);
        root = window
            .end
            .checked_sub(1)
            .filter(|&last| window.start < last)?;
    }
    Some(root)
}

/// The setter's single parameter name (:314-316 — one parameter).
fn sole_parameter<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
    let params = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
    let faces = flat_params(c, params);
    match faces.as_slice() {
        [one] => Some(one.name),
        _ => None,
    }
}

/// The method face (:232-242): a super invocation of `name` whose
/// arguments name the parameters (`argumentsMatchParameters`,
/// `dart_type_utilities.dart:13-49`).
fn super_call_forwards(c: &Check<'_>, rows: &[u32], name: &str, node: u32) -> bool {
    let Some(send_pos) = rows.iter().position(|&row| c.kind(row) == NodeKind::Send) else {
        return false;
    };
    // The callee head and the argument group (flat production tiling).
    let Some((head, arguments)) = crate::rules::slice::send_parts(c, rows, send_pos) else {
        return false;
    };
    // The target is `super` and the selector names the member.
    if head == 0 || c.kind(rows[head - 1]) != NodeKind::SuperExpression {
        return false;
    }
    if c.lexeme(c.head_token(rows[head])) != name {
        return false;
    }
    // The invocation is the whole expression: nothing after the send
    // but its own access marker.
    let trailing = &rows[send_pos + 1..];
    if trailing
        .iter()
        .any(|&row| !matches!(c.kind(row), NodeKind::DotAccess | NodeKind::Qualified))
    {
        return false;
    }
    let Some(arguments) = arguments else {
        return false;
    };
    // The parameter names in order: positional group then named group.
    let params = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    let Some(params) = params else {
        return false;
    };
    let faces = flat_params(c, params);
    let positional: Vec<&str> = faces
        .iter()
        .filter(|f| f.group != 2)
        .map(|f| f.name)
        .collect();
    let named: Vec<&str> = faces
        .iter()
        .filter(|f| f.group == 2)
        .map(|f| f.name)
        .collect();
    arguments_match_parameters(c, arguments, &positional, &named)
}

/// `argumentsMatchParameters` (:13-49): each positional argument is
/// the bare identifier of the corresponding positional parameter; the
/// named arguments cover the named parameters one-to-one, each value
/// the bare identifier of its parameter.
fn arguments_match_parameters(
    c: &Check<'_>,
    arguments: u32,
    positional: &[&str],
    named: &[&str],
) -> bool {
    let rows = c.collect_direct(arguments);
    // Split the flattened rows into arguments: a named group runs from
    // its label to the marker; a positional argument is one row. The
    // bare-name send group's markers tile beside each argument root and
    // stay out of the split.
    let mut pos_args: Vec<&str> = Vec::new();
    let mut named_args: Vec<(&str, &str)> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if crate::rules::slice::is_send_group_piece(c, rows[i]) {
            i += 1;
            continue;
        }
        if c.kind(rows[i]) == NodeKind::NamedArgument {
            // The label is the identifier two tokens back; the value is
            // the root row before the marker (its send group trails).
            let Some(label) = crate::rules::slice::named_arg_name(c, &rows, i) else {
                return false;
            };
            let Some(value_row) = (0..i)
                .rev()
                .find(|&j| !crate::rules::slice::is_send_group_piece(c, rows[j]))
                .map(|j| rows[j])
            else {
                return false;
            };
            if c.kind(value_row) != NodeKind::Identifier {
                return false;
            }
            named_args.push((
                c.lexeme(c.head_token(rows[label])),
                c.lexeme(c.head_token(value_row)),
            ));
            i += 1;
            continue;
        }
        if c.kind(rows[i]) != NodeKind::Identifier {
            return false;
        }
        pos_args.push(c.lexeme(c.head_token(rows[i])));
        i += 1;
    }
    if pos_args.len() != positional.len()
        || pos_args.iter().zip(positional).any(|(a, p)| a != p)
    {
        return false;
    }
    if named_args.len() != named.len() {
        return false;
    }
    named.iter().all(|param| {
        named_args
            .iter()
            .any(|(label, value)| *label == *param && *value == *param)
    })
}

/// The getter face (:204-208): a super property read of `name` —
/// `super.name`, nothing else. The trailing send group (the bare-name
/// `parse_send` markers and leaf) tiles beside the chain rows.
fn super_getter_forwards(c: &Check<'_>, rows: &[u32], name: &str) -> bool {
    // The access marker spans the whole read; the send group trails it.
    let mut end = rows.len();
    while end > 0
        && (matches!(
            c.kind(rows[end - 1]),
            NodeKind::DotAccess | NodeKind::Qualified
        ) || crate::rules::slice::is_send_group_piece(c, rows[end - 1]))
    {
        end -= 1;
    }
    let [target, property] = &rows[..end] else {
        return false;
    };
    c.kind(*target) == NodeKind::SuperExpression
        && c.kind(*property) == NodeKind::Identifier
        && c.lexeme(c.head_token(*property)) == name
}

/// The setter face (:313-326): an assignment whose right side is the
/// bare parameter and whose left side is the super property.
fn super_setter_forwards(c: &Check<'_>, rows: &[u32], name: &str, param: &str) -> bool {
    let Some((idx, &assignment)) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| c.kind(**row) == NodeKind::Assignment)
    else {
        return false;
    };
    let Some(&op) = c.payload(assignment).first() else {
        return false;
    };
    if c.lexeme(op) != "=" {
        return false;
    }
    let Some(op_start) = c
        .unit
        .tokens
        .get(op as usize)
        .map(|t| t.byte_offset as usize)
    else {
        return false;
    };
    // The operand runs: rows left/right of the operator byte, from the
    // statement's own rows and the assignment's adopted window (the
    // right side's pieces tile before the adopted window opens).
    let window = c.children(assignment);
    let mut lhs: Vec<u32> = rows[..idx]
        .iter()
        .copied()
        .filter(|&row| c.byte_span(row).end <= op_start)
        .collect();
    lhs.extend((window.start..window.end).filter(|&row| c.byte_span(row).end <= op_start));
    let mut rhs: Vec<u32> = rows
        .iter()
        .copied()
        .filter(|&row| row != assignment && c.byte_span(row).start >= op_start)
        .collect();
    rhs.extend((window.start..window.end).filter(|&row| c.byte_span(row).start >= op_start));
    // LHS: `super.name` (an access marker and the bare-name send group
    // may trail).
    while lhs.len() > 2
        && (matches!(
            c.kind(lhs[lhs.len() - 1]),
            NodeKind::DotAccess | NodeKind::Qualified
        ) || crate::rules::slice::is_send_group_piece(c, lhs[lhs.len() - 1]))
    {
        lhs.pop();
    }
    let [target, property] = lhs.as_slice() else {
        return false;
    };
    if c.kind(*target) != NodeKind::SuperExpression
        || c.kind(*property) != NodeKind::Identifier
        || c.lexeme(c.head_token(*property)) != name
    {
        return false;
    }
    // RHS: the bare parameter identifier (the send group trails).
    while rhs.len() > 1
        && (matches!(
            c.kind(rhs[rhs.len() - 1]),
            NodeKind::DotAccess | NodeKind::Qualified
        ) || crate::rules::slice::is_send_group_piece(c, rhs[rhs.len() - 1]))
    {
        rhs.pop();
    }
    // The left selector's own send-group markers anchor right of the
    // operator byte (the width-0 `NoArguments` sits at the value head),
    // so they leak into the rhs head — they carry no face here.
    let lead = rhs
        .iter()
        .position(|&row| !crate::rules::slice::is_send_group_piece(c, row))
        .unwrap_or(rhs.len());
    rhs.drain(..lead);
    matches!(rhs.as_slice(), [only] if c.kind(*only) == NodeKind::Identifier && c.lexeme(c.head_token(*only)) == param)
}
