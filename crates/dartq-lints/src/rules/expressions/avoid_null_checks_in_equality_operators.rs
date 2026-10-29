use super::{Check, NodeKind, PayloadField, RuleSpec, slice, value_run_end};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_null_checks_in_equality_operators.dart`
/// — inside an `operator ==` whose single parameter is nullable, the
/// body walk reports every null check against that parameter: a `??`
/// with the parameter on the left (:22-27), an `==`/`!=` against the
/// `null` literal on either side (:11-18), and a `?.` access or
/// invocation whose target is the parameter (:101-107, :109-115). A
/// non-`?` parameter bails (:69-73 — that face is the analyzer's
/// `nonNullableEqualsParameter`, dart-lang/linter#2864; a written
/// `dynamic` is not `?` and bails the same way).
///
/// dartq faces over the production tiling: the expression rows
/// dispatch themselves (`BinaryExpression`, `DotAccess`) and gate at
/// the nearest enclosing method row — the operator name rides the
/// [`NodeKind::OperatorName`] sibling the member lane stamps before
/// the method row, the parameter list is the
/// [`NodeKind::FormalParameters`] child (one
/// [`NodeKind::FormalParameter`] — its name is payload word 1, the
/// written type the `Type` child). The canonical-element face runs
/// over the operand piece run (`slice::binary_operands` at the
/// operator byte split; the send-group strip reduces a bare name to
/// its identifier): parens unwrap (`(other) ?? null` reports — the
/// parenthesized operand keeps the binary's canonical element), then
/// the bare parameter identifier at the use site must survive the
/// parent-chain walk (a closer block local, closure parameter, or
/// catch parameter of the same name rejects it). The `?.` face reads
/// the [`NodeKind::DotAccess`] null-aware flag (payload word 1) and
/// reduces the receiver run the same way — an invocation's trailing
/// `Send` + argument list + selector sit right before the marker, a
/// bare property's receiver group does — so `other?.foo.bar()` fires
/// on `other?.foo` alone. Message: `diagnostic.g.dart`
/// `avoidNullChecksInEqualityOperators`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_null_checks_in_equality_operators",
    &[NodeKind::BinaryExpression, NodeKind::DotAccess],
    check,
);

fn check(c: &mut Check<'_>) {
    let Some((method, param)) = equality_parameter(c) else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::BinaryExpression => check_binary(c, method, &param),
        NodeKind::DotAccess => check_dot(c, method, &param),
        _ => {}
    }
}

/// The entry gate at the nearest enclosing method (:100-121): the
/// `==` operator name (:63), exactly one parameter (:64), and its
/// written type carrying a `?` tail (:69-73 — that face is the
/// analyzer's `nonNullableEqualsParameter`, dart-lang/linter#2864; a
/// written `dynamic` is not `?` and bails the same way). Rows outside
/// any method — or inside a method that fails the gate — stay silent.
fn equality_parameter(c: &Check<'_>) -> Option<(u32, String)> {
    for depth in 1..256 {
        let method = c.ancestor(depth)?;
        if c.kind(method) != NodeKind::Method {
            continue;
        }
        // The operator name tiles as the method row's preceding
        // sibling in the member body (the declaration lane stamps it
        // there before the method opens).
        let owner = c.unit.ast.parent(method)?;
        let kids = c.collect_direct(owner);
        let pos = kids.iter().position(|&kid| kid == method)?;
        let is_equals = pos > 0
            && c.kind(kids[pos - 1]) == NodeKind::OperatorName
            && c.lexeme(
                c.field(kids[pos - 1], PayloadField::Name)
                    .unwrap_or(crate::rules::NONE_TOKEN),
            ) == "==";
        if !is_equals {
            return None;
        }
        // `parameters.length != 1` bail (:64).
        let params = c
            .collect_direct(method)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)?;
        let parameters: Vec<u32> = c
            .collect_direct(params)
            .into_iter()
            .filter(|&kid| c.kind(kid) == NodeKind::FormalParameter)
            .collect();
        let [parameter] = parameters[..] else {
            return None;
        };
        // The parameter name: the `Name` word (the `parameter_binding`
        // face).
        let name_token = c.field(parameter, PayloadField::Name)?;
        if name_token == crate::rules::NONE_TOKEN {
            return None;
        }
        // The nullability gate (:69-73): the written type carries a `?`
        // tail; an untyped parameter (`dynamic` upstream) does not.
        let written = c
            .collect_direct(parameter)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)?;
        if !crate::rules::type_face(c, written).nullable {
            return None;
        }
        return Some((method, c.lexeme(name_token).to_owned()));
    }
    None
}

/// The binary faces (:11-27): `other ?? e`, and `other == null` /
/// `null == other` / the `!=` mirror. The report sits at the binary
/// row (its span head is the left operand's head — the official
/// `reportAtNode(node)`).
fn check_binary(c: &mut Check<'_>, method: u32, param: &str) {
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return;
    };
    // The operand runs of the flattened container (:52-59).
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let fires = match c.lexeme(op) {
        "??" => operand_is_parameter(c, &left, method, param),
        "==" | "!=" => {
            (run_is_null_literal(c, &right) && operand_is_parameter(c, &left, method, param))
                || (run_is_null_literal(c, &left) && operand_is_parameter(c, &right, method, param))
        }
        _ => false,
    };
    if fires {
        // The official reports at the whole binary expression; the
        // container row opens at the operator's tile anchor, so the
        // extent runs from the left operand's first token (the run's
        // operator-adjacent tail — a `!=` nested inside `&&` keeps the
        // outer pieces out) to the right run's last value row.
        let Some(start) = slice::operand_utf16_start(c, &left, op) else {
            return;
        };
        let end = value_run_end(c, &right, node);
        c.report(start..end, &[]);
    }
}

/// The `visitPropertyAccess`/`visitMethodInvocation` faces (:80-95): a
/// `?.` access or invocation whose target is the parameter reports at
/// the receiver — both faces read the one null-aware marker, so a
/// chain reports once (the upstream `other?.foo.bar()` shape fires on
/// `other?.foo` alone).
fn check_dot(c: &mut Check<'_>, method: u32, param: &str) {
    let node = c.node();
    // The bridge's null-aware flag (the `IsNullAware` word) is the
    // `?.` gate.
    if c.field(node, PayloadField::IsNullAware) != Some(1) {
        return;
    }
    // The receiver run: the pieces before the marker in the owner
    // slice. An adopted window parks the receiver pieces at the
    // container's parent (the `binary_operands` posture).
    let Some(parent) = c.unit.ast.parent(node) else {
        return;
    };
    let mut kids = c.collect_direct(parent);
    let Some(mut pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    if pos == 0
        && let Some(grand) = c.unit.ast.parent(parent)
    {
        let owner_kids = c.collect_direct(grand);
        let Some(container_pos) = owner_kids.iter().position(|&kid| kid == parent) else {
            return;
        };
        kids = owner_kids[..container_pos].to_vec();
        pos = kids.len();
    }
    let Some(receiver) = receiver_row(c, &kids[..pos], node) else {
        return;
    };
    if c.kind(receiver) == NodeKind::Identifier
        && c.lexeme(c.head_token(receiver)) == param
        && resolves_to_parameter(c, receiver, method, param)
    {
        let range = c.utf16_span(receiver).start..c.utf16_span(node).end;
        c.report(range, &[]);
    }
}

/// The receiver row over the pieces before a null-aware marker: the
/// access completion parks its `Send` (the marker's own span), an
/// invocation's argument list, and the selector identifier (sharing
/// the marker's head token) right before the marker — invocation or
/// bare property alike. The scan strips those wrapper pieces and
/// answers the first surviving row — a bare name reduces to its
/// identifier, a chain keeps its access row (never a parameter).
fn receiver_row(c: &Check<'_>, pieces: &[u32], marker: u32) -> Option<u32> {
    let mut end = pieces.len();
    // The marker's own `Send` (same span) and an invocation's argument
    // list park right before the marker.
    if end > 0
        && c.kind(pieces[end - 1]) == NodeKind::Send
        && c.utf16_span(pieces[end - 1]) == c.utf16_span(marker)
    {
        end -= 1;
        if end > 0
            && matches!(
                c.kind(pieces[end - 1]),
                NodeKind::Arguments | NodeKind::NoArguments
            )
        {
            end -= 1;
        }
    }
    // Wrapper pieces sit between the selector and the access send.
    while end > 0 && slice::is_send_group_piece(c, pieces[end - 1]) {
        end -= 1;
    }
    // The selector identifier shares the marker's head token.
    if end > 0
        && c.kind(pieces[end - 1]) == NodeKind::Identifier
        && c.head_token(pieces[end - 1]) == c.head_token(marker)
    {
        end -= 1;
    }
    // The receiver's own completion group.
    while end > 0 && slice::is_send_group_piece(c, pieces[end - 1]) {
        end -= 1;
    }
    pieces.get(end.checked_sub(1)?).copied()
}

/// `_isParameter` (:33-34) over the operand run: parens unwrap, the
/// send-group strip reduces a bare name to its identifier, and the
/// run's surviving tail must be the bare parameter identifier at the
/// use site (the operand completes at the run tail — the
/// `binary_operands` divergence parks earlier siblings' pieces in the
/// run head, so a head-prefix match would miss the face and a
/// whole-run match would read the wrong operand).
fn operand_is_parameter(c: &Check<'_>, run: &[u32], method: u32, param: &str) -> bool {
    let mut rows = slice::access_run(c, run);
    // `(other)` — the paren container wraps its own pieces.
    if let Some(&tail) = rows.last()
        && c.kind(tail) == NodeKind::ParenthesizedExpression
    {
        rows = slice::access_run(c, &c.collect_direct(tail));
    }
    let Some(&tail) = rows.last() else {
        return false;
    };
    c.kind(tail) == NodeKind::Identifier
        && c.lexeme(c.head_token(tail)) == param
        && resolves_to_parameter(c, tail, method, param)
}

/// `isNullLiteral` (:28, :30): the operand run's surviving tail is the
/// bare null literal — a `(null)` paren keeps the official silent (the
/// operand is the paren expression).
fn run_is_null_literal(c: &Check<'_>, run: &[u32]) -> bool {
    slice::access_run(c, run)
        .last()
        .is_some_and(|&tail| c.kind(tail) == NodeKind::LiteralNull)
}

/// The canonical-element identity face: no closer binding of the name
/// sits between the use site and the method's own parameter list. The
/// parent-chain walk rejects a block local, closure/nested-function
/// parameter, or catch parameter of the same name; reaching the
/// dispatched method confirms its parameter (exactly one, checked at
/// the entry face).
fn resolves_to_parameter(c: &Check<'_>, use_row: u32, method: u32, param_name: &str) -> bool {
    let use_start = c.byte_span(use_row).start;
    let mut current = use_row;
    loop {
        let Some(parent) = c.unit.ast.parent(current) else {
            return false;
        };
        current = parent;
        match c.kind(current) {
            NodeKind::Block | NodeKind::BlockBody => {
                if block_shadows(c, current, use_start, param_name) {
                    return false;
                }
            }
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::LocalFunctionDeclaration => {
                if params_contain(c, current, param_name) {
                    return false;
                }
            }
            NodeKind::CatchClause => {
                let ctx = dartq_parser::IdentifierContext::CatchParameter as u32;
                if c.children(current).any(|row| {
                    c.kind(row) == NodeKind::Identifier
                        && c.field(row, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(row)) == param_name
                }) {
                    return false;
                }
            }
            NodeKind::Method => {
                return current == method && params_contain(c, method, param_name);
            }
            _ => {}
        }
    }
}

/// Whether a block-local of `name` is declared before the use site
/// (the `block_binding` scan: the statement tiling keeps locals flat
/// in the owner slice).
fn block_shadows(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
    let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
    c.collect_direct(block).into_iter().any(|kid| {
        c.kind(kid) == NodeKind::VariablesDeclaration
            && c.collect_direct(kid).into_iter().any(|var| {
                let name_row = if c.kind(var) == NodeKind::Identifier {
                    Some(var)
                } else {
                    c.collect_direct(var)
                        .into_iter()
                        .find(|&piece| c.kind(piece) == NodeKind::Identifier)
                };
                name_row.is_some_and(|row| {
                    c.field(row, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(row)) == name
                        && c.byte_span(row).start < use_start
                })
            })
    })
}

/// Whether the owner's [`NodeKind::FormalParameters`] child binds
/// `name`.
fn params_contain(c: &Check<'_>, owner: u32, name: &str) -> bool {
    c.collect_direct(owner)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .any(|params| {
            c.collect_direct(params).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::FormalParameter
                    && c.field(kid, PayloadField::Name)
                        .filter(|&tok| tok != crate::rules::NONE_TOKEN)
                        .is_some_and(|tok| c.lexeme(tok) == name)
            })
        })
}
