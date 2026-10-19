use super::{
    Check, Group, NodeKind, RuleSpec, get_set_token, is_identifier_token, parameter_name,
    parameters_bind,
};
use crate::rules::expressions::static_type_faces as faces;
use dartq_semantic::model::Ty;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_getters_setters.dart:41-97`
/// — a getter/setter pair of the same name whose bodies are the trivial
/// field wrap (getter reads a private same-class field, setter assigns
/// it from the lone parameter) reports at the getter name; neither
/// member may carry annotations, and the setter's assignment must agree
/// on type (`expression.writeType != rightHandSide.staticType` bails —
/// `ast.dart` `_checkForSimpleSetter`).
///
/// The field identity (`staticElement isGetterElement` + same enclosing
/// class, ast.dart `_checkForSimpleGetter`) is taken as a same-lexeme
/// private field declared in the same class body, with the shared
/// shadow walk: the setter's assignment target resolves through its
/// body scope, so a same-lexeme parameter captures it and the pair
/// passes (the `_checkForSimpleSetter` `writeElement` face — a
/// parameter write is not the field's implicit setter). The trivial
/// body shapes cannot carry a same-named local (the sole-statement
/// extraction), so no further shadow face exists there. The type
/// agreement reads the written spellings — field type versus setter
/// parameter type, so `String? _x` with `set x(dynamic value)` bails.
/// An unwritten field reads the initializer-inferred type instead (the
/// official `expression.writeType` resolves it — `var _x = 0;` types
/// the field `int`), so an agreement the inference refutes bails; the
/// shapes the literal map and the bridge cannot type bail in the same
/// direction (`writeType` is a resolved product — collection literals,
/// chains and unknowns stay silent). Message: `diagnostic.g.dart`
/// `unnecessaryGettersSetters`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "unnecessary_getters_setters",
    group: Group::Recommended,
    problem: "Unnecessary use of getter and setter to wrap a field.",
    correction: "Try removing the getter and setter and renaming the field.",
    kinds: &[
        NodeKind::ClassDeclaration,
        NodeKind::ExtensionTypeDeclaration,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.body case BlockClassBody body` (:55-58) — the member list.
    let Some(body) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
    else {
        return;
    };
    let members = c.collect_direct(body);
    // Build the getter/setter maps (:62-72) and the private field names.
    let mut fields: Vec<(&str, Option<u32>, Option<u32>)> = Vec::new();
    for &member in &members {
        if c.kind(member) == NodeKind::Fields {
            collect_field_names(c, member, &mut fields);
        }
    }
    for (i, &member) in members.iter().enumerate() {
        if c.kind(member) != NodeKind::Method || get_set_token(c, member) != Some("get") {
            continue;
        }
        // Names come from the context-stamped declaration name (the
        // first name-shaped child answers the written return type's
        // leaked identifier row on production parses).
        let Some(name_node) = super::member_declaration_name(c, member) else {
            continue;
        };
        let getter_name = c.lexeme(c.head_token(name_node));
        let Some(setter) = members.iter().copied().find(|&other| {
            c.kind(other) == NodeKind::Method
                && get_set_token(c, other) == Some("set")
                && super::member_declaration_name(c, other)
                    .is_some_and(|name| c.lexeme(c.head_token(name)) == getter_name)
        }) else {
            continue;
        };
        // `metadata.annotations.isEmpty` on both (:96-99) — the star
        // sibling before the member.
        if has_annotations(c, &members, i) || has_annotations_of(c, &members, setter) {
            continue;
        }
        if !is_simple_getter(c, member, &fields) || !is_simple_setter(c, setter, &fields) {
            continue;
        }
        // The getter's name reports (:101).
        let Some(start) = c.utf16_of_token(c.head_token(name_node)) else {
            continue;
        };
        c.report(start..start, &[]);
    }
}

/// The private field names declared by a `Fields` member, with each
/// field's written type row (the production `Fields`-row type sibling,
/// else the declaration's own `Type` child) and its initializer row.
fn collect_field_names<'a>(
    c: &Check<'a>,
    fields: u32,
    out: &mut Vec<(&'a str, Option<u32>, Option<u32>)>,
) {
    let group_type = c
        .collect_direct(fields)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type);
    for decl in c.collect_direct(fields) {
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
            if let Some(name_token) = crate::rules::initialized_identifier_name(c, var) {
                let lexeme = c.lexeme(name_token);
                if lexeme.starts_with('_') {
                    let initializer = c
                        .collect_direct(var)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
                    out.push((lexeme, written, initializer));
                }
            }
        }
    }
}

/// Whether the member at `index`'s left sibling is a metadata star.
fn has_annotations(c: &Check<'_>, members: &[u32], index: usize) -> bool {
    index > 0
        && c.kind(members[index - 1]) == NodeKind::MetadataStar
        && !c.collect_direct(members[index - 1]).is_empty()
}

fn has_annotations_of(c: &Check<'_>, members: &[u32], member: u32) -> bool {
    members
        .iter()
        .position(|&m| m == member)
        .is_some_and(|index| has_annotations(c, members, index))
}

/// The expression a trivial body yields, as the token run between its
/// introducer and terminator: an arrow body's run sits between the `=>`
/// (payload word 0) and the `;` (payload word 1, the row tail when the
/// word rides the absent sentinel), a `return` statement's run between
/// the row's head and tail tokens. `None` when the run is not exactly
/// one identifier token — the official's `SimpleIdentifier` face, and
/// the only shape the wrap check needs.
fn body_expression_tokens(c: &Check<'_>, kids: &[u32]) -> Option<(u32, u32)> {
    if let Some(&body) = kids
        .iter()
        .find(|&&kid| c.kind(kid) == NodeKind::ExpressionBody)
    {
        let payload = c.payload(body);
        let arrow = payload.first().copied()?;
        let end = match payload.get(1).copied() {
            Some(semi) if semi != crate::rules::NONE_TOKEN => semi,
            _ => c.unit.ast.token_range(body).end.checked_sub(1)?,
        };
        return Some((arrow, end));
    }
    let statement = sole_body_statement(c, kids)?;
    if c.kind(statement) != NodeKind::ReturnStatement {
        return None;
    }
    let range = c.unit.ast.token_range(statement);
    Some((range.start, range.end.checked_sub(1)?))
}

/// The sole statement row of a trivial block body: hand-built tables
/// nest the statements under a `Block` child; production tiles the
/// statement rows flat in the `BlockBody` (the span-only statement
/// leaves — the expression rows around them are not statement rows).
fn sole_body_statement(c: &Check<'_>, kids: &[u32]) -> Option<u32> {
    let body = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::BlockBody)?;
    let mut statements: Vec<u32> = c
        .collect_direct(body)
        .into_iter()
        .filter(|&kid| crate::rules::is_statement_row(c, kid))
        .collect();
    if statements.len() == 1 && c.kind(statements[0]) == NodeKind::Block {
        statements = c
            .collect_direct(statements[0])
            .into_iter()
            .filter(|&kid| crate::rules::is_statement_row(c, kid))
            .collect();
    }
    (statements.len() == 1).then_some(statements[0])
}

/// `isSimpleGetter` (ast.dart): the body expression (or a single
/// return's expression) is a bare private same-class field identifier.
fn is_simple_getter(
    c: &Check<'_>,
    getter: u32,
    fields: &[(&str, Option<u32>, Option<u32>)],
) -> bool {
    let kids = c.collect_direct(getter);
    let Some((head, tail)) = body_expression_tokens(c, &kids) else {
        return false;
    };
    is_bare_field_read(c, head, tail, fields)
}

/// `isSimpleSetter` (ast.dart): the body statement assigns a bare
/// private same-class field from the lone parameter, and the assignment
/// agrees on type — the field's type versus the parameter's written
/// type, each `dynamic` when unwritten
/// (`expression.writeType != rightHandSide.staticType` bails).
fn is_simple_setter(
    c: &Check<'_>,
    setter: u32,
    fields: &[(&str, Option<u32>, Option<u32>)],
) -> bool {
    let kids = c.collect_direct(setter);
    let Some(statement) = sole_body_statement(c, &kids) else {
        return false;
    };
    if c.kind(statement) != NodeKind::ExpressionStatement {
        return false;
    }
    let Some(params) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return false;
    };
    // The lone parameter: the window also carries the empty metadata
    // star, so the count is over parameter rows only.
    let param_list: Vec<u32> = c
        .collect_direct(params)
        .into_iter()
        .filter(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
            )
        })
        .collect();
    if param_list.len() != 1 {
        return false;
    }
    let Some(param_name) = parameter_name(c, param_list[0]) else {
        return false;
    };
    // The assignment: hand-built tables nest it under the statement;
    // production tiles it as the span-only statement leaf's preceding
    // sibling (the expression subtree completes before the leaf).
    let Some(body) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::BlockBody)
    else {
        return false;
    };
    let body_kids = c.collect_direct(body);
    let Some(assignment) = c
        .collect_direct(statement)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Assignment)
        .or_else(|| {
            let pos = body_kids.iter().position(|&kid| kid == statement)?;
            let prev = body_kids.get(pos.checked_sub(1)?)?;
            (c.kind(*prev) == NodeKind::Assignment).then_some(*prev)
        })
    else {
        return false;
    };
    let Some(&op) = c.payload(assignment).first() else {
        return false;
    };
    if c.lexeme(op) != "=" {
        return false;
    }
    // The assignment target: the bare field identifier immediately
    // before `=` (the token before it must not be a `.` — a prefixed
    // access is not the official's `SimpleIdentifier` target).
    let Some(lhs) = op.checked_sub(1) else {
        return false;
    };
    if !is_identifier_token(c, lhs) {
        return false;
    }
    let lhs_lexeme = c.lexeme(lhs);
    let field = fields.iter().find(|&&(name, _, _)| name == lhs_lexeme);
    let field_type = field.and_then(|&(_, written, _)| written);
    let field_initializer = field.and_then(|&(_, _, initializer)| initializer);
    if !(lhs_lexeme.starts_with('_') && field.is_some()) {
        return false;
    }
    if op >= 2 && c.lexeme(op - 2) == "." {
        return false;
    }
    // The value: the lone parameter identifier immediately after `=`,
    // sealed by the statement's `;` — the `rightElement ==
    // parameters.first` face. The target resolves through the body
    // scope: a parameter of the same name captures it (the
    // `writeElement` face — a parameter write is not the field's
    // implicit setter).
    let rhs = op + 1;
    if !is_identifier_token(c, rhs) || c.lexeme(rhs) != param_name {
        return false;
    }
    // The `;` seals the bare-identifier value — it is punctuation, not
    // an identifier token.
    if c.lexeme(op + 2) != ";" {
        return false;
    }
    // The type agreement (:ast.dart `_checkForSimpleSetter`): the
    // field's type versus the parameter's. A written pair compares
    // spellings; a written parameter stays `dynamic`-compatible only
    // against a `dynamic` field spelling; an unwritten field reads the
    // initializer-inferred type (`expression.writeType` is a resolved
    // product — `var _x = 0;` types the field `int`), and an unwritten
    // parameter is `dynamic`.
    let param_type = c.collect_direct(param_list[0]).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    });
    let agree = match (field_type, param_type) {
        (Some(field), Some(param)) => crate::rules::token_seq_equal(c, field, param),
        (Some(field), None) => {
            let range = c.unit.ast.token_range(field);
            range.len() == 1 && c.lexeme(range.start) == "dynamic"
        }
        (None, _) => match inferred_field_type(c, field_initializer) {
            Some((name, nullable)) => match param_type {
                Some(param) => written_named_matches(c, param, name, nullable),
                None => name == "dynamic" && !nullable,
            },
            None => false,
        },
    };
    agree && !parameters_bind(c, params, lhs_lexeme)
}

/// The unwritten field's type as the agreement reads it — the official
/// `expression.writeType` resolves the inferred type: an
/// initializer-less declaration is `dynamic`; the fixed Dart literal
/// types answer syntactically (`var _x = 0;` is `int`, the `null`
/// literal infers `dynamic`); a bridge-resolvable initializer answers
/// through the fold (the read's flow type, the constructed type, the
/// callee or member return type). The shapes neither source types
/// (collection and record literals, chains, unknowns) answer `None`
/// and the agreement bails — the under-report direction.
fn inferred_field_type<'a>(
    c: &'a Check<'_>,
    initializer: Option<u32>,
) -> Option<(&'a str, bool)> {
    let Some(initializer) = initializer else {
        return Some(("dynamic", false));
    };
    let run = c.collect_direct(initializer);
    // The run's root is its tail row (the flat send-chain completes
    // last); a bare-literal initializer's tail is the literal itself.
    let root = faces::run_root(c, &run)?;
    let literal = match c.kind(root) {
        NodeKind::LiteralInt => Some(("int", false)),
        NodeKind::LiteralDouble => Some(("double", false)),
        NodeKind::LiteralBool => Some(("bool", false)),
        NodeKind::StringLiteral => Some(("String", false)),
        // `var _x = null;` infers `dynamic` (the null literal's
        // inference fallback), not `Null`.
        NodeKind::LiteralNull => Some(("dynamic", false)),
        // Collection, record and const literals are context-typed —
        // the resolved static type is not decidable from the syntax.
        _ => None,
    };
    if literal.is_some() {
        return literal;
    }
    let analysis = c.semantics()?;
    let ty = match c.kind(root) {
        NodeKind::Identifier => {
            analysis.type_of(crate::rules::expressions::query_site(c, root))
        }
        NodeKind::ImplicitCreation | NodeKind::NewExpression => {
            super::constructed_type(c, analysis, root)
        }
        _ => faces::expr_type(c, analysis, &run).or_else(|| {
            // A bare `Helper()` send is an implicit class creation —
            // the callee face answers `None` on a type target; the
            // binding resolves the class name (the constructor face
            // reads `type_uses`, which a bare send's selector does not
            // publish).
            if c.kind(root) != NodeKind::Send {
                return None;
            }
            let selector = run
                .iter()
                .copied()
                .find(|&piece| c.kind(piece) == NodeKind::Identifier)?;
            let site = crate::rules::expressions::query_site(c, selector);
            analysis
                .binding_of(site)
                .filter(|binding| {
                    binding.kind == dartq_semantic::BindingKind::Decl
                        && crate::rules::is_upper_camel_case(
                            analysis.strings().resolve(binding.target_name),
                        )
                })
                .map(|binding| Ty::named(binding.target_name))
        }),
    };
    match ty {
        Some(Ty::Named {
            name,
            nullable,
            args: dartq_semantic::model::ArgListId::EMPTY,
        }) => Some((analysis.strings().resolve(name), nullable)),
        // `Ty::Null` on a read keeps the null-inference fallback's
        // verdict: the field's usable type is `dynamic`.
        Some(Ty::Null | Ty::Dynamic) => Some(("dynamic", false)),
        _ => None,
    }
}

/// Whether the written parameter row spells exactly the named type —
/// a bare name with an optional `?` (`int`, `int?`). Generic,
/// prefixed, function and record spellings never match (their
/// resolved shapes carry structure this lexical face cannot compare).
fn written_named_matches(c: &Check<'_>, param_row: u32, name: &str, nullable: bool) -> bool {
    if c.kind(param_row) != NodeKind::Type {
        return false;
    }
    let range = c.unit.ast.token_range(param_row);
    let (head, written_nullable) = match range.len() {
        1 => (range.start, false),
        2 if c.lexeme(range.end - 1) == "?" => (range.start, true),
        _ => return false,
    };
    c.lexeme(head) == name && written_nullable == nullable
}

/// Whether the token run strictly between `head` and `tail` is exactly
/// one identifier token naming a private field of this class — the
/// official `SimpleIdentifier` + same-lexeme field face over the
/// production token run.
fn is_bare_field_read(
    c: &Check<'_>,
    head: u32,
    tail: u32,
    fields: &[(&str, Option<u32>, Option<u32>)],
) -> bool {
    let single = head + 1;
    single < tail && tail - single == 1 && is_identifier_token(c, single) && {
        let lexeme = c.lexeme(single);
        lexeme.starts_with('_') && fields.iter().any(|&(name, _, _)| name == lexeme)
    }
}
