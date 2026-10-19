use super::{Check, Group, NodeKind, RuleSpec, slice};
use crate::rules::expressions::{query_site, static_type_faces as faces};
use dartq_semantic::model::Ty;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_types_on_closure_parameters.dart`
/// — a closure whose context type is a function type reports each
/// parameter that carries a written `NamedType` at the type (:60-62 —
/// the `type is NamedType` gate leaves a generic function type
/// annotation silent), and a function-typed-suffix parameter at the
/// parameter (:54-57). The context type is the official
/// `approximateContextType` (upstream Dart SDK source,
/// pkg/linter/lib/src/rules/extensions.dart:452-520) — a resolution
/// product. The faces, in the official decision order:
/// `list.map((int e) => …)` resolves through the callee's signature —
/// the corresponding formal's declared type (the `ArgumentList` arm,
/// :474-477); `f = (…) => …` resolves through the target's static
/// type (the `AssignmentExpression` arm, :494-496, bare identifiers —
/// the written same-file face first, then the flow-row type); the
/// variable-declaration initializer reads the written type (the
/// `VariableDeclarationList.type` arm, :500-502). Unresolvable
/// contexts under-report (the documented posture). Message:
/// `diagnostic.g.dart` `avoidTypesOnClosureParameters`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_types_on_closure_parameters",
    group: Group::Standalone,
    problem: "Unnecessary type annotation on a function expression parameter.",
    correction: "Try removing the type annotation.",
    kinds: &[NodeKind::FunctionExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if !context_is_function_type(c, node) {
        return;
    }
    let Some(params) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    for param in c.collect_direct(params) {
        match c.kind(param) {
            // The function-typed suffix reports at the parameter and
            // stops (:54-57). The production shape wraps the suffix row
            // in a `FormalParameter` (`(int x, void g(int y))` — the
            // wrapper spans `void g`).
            NodeKind::FormalParameter
                if c.collect_direct(param)
                    .into_iter()
                    .any(|kid| c.kind(kid) == NodeKind::FunctionTypedFormalParameter) =>
            {
                c.report(c.utf16_span(param), &[]);
            }
            NodeKind::FormalParameter => {
                // A written `NamedType` face reports at the type
                // (:60-62 — the `type is NamedType` gate): a generic
                // function type annotation (`void Function() cb`)
                // never qualifies, `dynamic` never qualifies.
                let type_row = c
                    .collect_direct(param)
                    .into_iter()
                    .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::VoidType));
                let Some(type_row) = type_row else {
                    continue;
                };
                if c.kind(type_row) == NodeKind::Type
                    && c.lexeme(c.head_token(type_row)) == "dynamic"
                {
                    continue;
                }
                c.report(c.utf16_span(type_row), &[]);
            }
            _ => {}
        }
    }
}

/// The lexical context face: through the transparent initializer chain,
/// the enclosing variable declaration's or the assignment target's
/// written type is a [`NodeKind::FunctionType`]. Any other intermediate
/// row (an argument list, a cascade section) cuts the chain — the
/// closure is no longer the position the written type governs.
fn context_is_function_type(c: &Check<'_>, closure: u32) -> bool {
    let len = c.path.len();
    for i in (0..len.saturating_sub(1)).rev() {
        let ancestor = c.path[i];
        match c.kind(ancestor) {
            // `void Function(int) f = (…) …;` — the written type tiles
            // in the declaring owner's slice before the declaration (the
            // head shape, the block_binding walk); a `FunctionType` head
            // governs every initializer (:500-502).
            NodeKind::VariablesDeclaration => {
                if i == 0 {
                    return false;
                }
                let owner = c.path[i - 1];
                let mut written: Option<u32> = None;
                for kid in c.collect_direct(owner) {
                    if kid == ancestor {
                        return written
                            .is_some_and(|row| c.kind(row) == NodeKind::FunctionType);
                    }
                    if matches!(
                        c.kind(kid),
                        NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
                    ) {
                        written = Some(kid);
                    }
                }
                return false;
            }
            // `f = (…) …;` — the target's written type governs
            // (:494-496, `ancestor.staticType`). The production
            // `Assignment` row spans the right side only (the bridge
            // opens it at the operand boundary) with the operator
            // token in payload word 0 — the target is the identifier
            // token immediately before it.
            NodeKind::Assignment => {
                let Some(&eq) = c.payload(ancestor).first() else {
                    return false;
                };
                // The bare-identifier target face: the token before the
                // operator (a compound `+=` target qualifies the same).
                let is_identifier_target = eq.checked_sub(1).is_some_and(|target| {
                    c.unit
                        .tokens
                        .get(target as usize)
                        .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Identifier)
                });
                if !is_identifier_target {
                    return false;
                }
                match crate::rules::expressions::same_file::binding(
                    c,
                    closure,
                    c.lexeme(eq - 1),
                ) {
                    Some((_, Some(type_row))) => {
                        return c.kind(type_row) == NodeKind::FunctionType;
                    }
                    _ => {}
                }
                // The resolved-target face (`f = (…) => …` with no
                // same-file written type): the target's static type
                // answers (`target.staticType`) — a bare target
                // through its flow row, a member target (`c.onFoo = …`)
                // through the receiver's member face.
                let Some(semantics) = c.semantics() else {
                    return false;
                };
                let Some(target) = eq
                    .checked_sub(1)
                    .and_then(|token| c.unit.tokens.get(token as usize))
                else {
                    return false;
                };
                let site = dartq_source::SourceRange {
                    offset: target.byte_offset,
                    length: target.byte_len,
                };
                let period = eq.checked_sub(2).map(|token| c.lexeme(token));
                if period == Some(".") {
                    let Some(receiver) = eq
                        .checked_sub(3)
                        .and_then(|token| c.unit.tokens.get(token as usize))
                    else {
                        return false;
                    };
                    let name = semantics.strings().intern(c.lexeme(eq - 1));
                    return matches!(
                        semantics.member_type(
                            dartq_source::SourceRange {
                                offset: receiver.byte_offset,
                                length: receiver.byte_len,
                            },
                            name,
                        ),
                        Some(Ty::Function { .. })
                    );
                }
                return matches!(semantics.type_of(site), Some(Ty::Function { .. }));
            }
            // The SDK-callback face (`list.map((int e) => …)`): the
            // governing context type is the corresponding formal's
            // declared type (the `ArgumentList` arm, :474-477 —
            // positional by dispatch order, named by label).
            NodeKind::Arguments => {
                let Some(semantics) = c.semantics() else {
                    return false;
                };
                return matches!(
                    argument_context_ty(c, semantics, ancestor, closure),
                    Some(Ty::Function { .. })
                );
            }
            // The transparent initializer chain (the wrappers between a
            // closure and the declaration its position belongs to).
            NodeKind::InitializedIdentifier
            | NodeKind::VariableInitializer
            | NodeKind::ParenthesizedExpression
            | NodeKind::ConstExpression
            | NodeKind::ConstLiteral
            | NodeKind::Conditional => {}
            _ => return false,
        }
    }
    false
}

/// The corresponding formal's declared type at the invocation whose
/// argument list is `args` — positional by dispatch order, named by
/// label (the same slot walk the argument faces use). `None` when the
/// send does not resolve or the closure occupies no slot — the
/// under-report direction.
fn argument_context_ty(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    args: u32,
    closure: u32,
) -> Option<Ty> {
    let parent = c.unit.ast.parent(args)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == args)?;
    // The invocation send tiles right after the argument list.
    if siblings
        .get(pos + 1)
        .copied()
        .is_none_or(|send| c.kind(send) != NodeKind::Send)
    {
        return None;
    }
    let Some((head, Some(_))) = slice::send_parts(c, &siblings, pos + 1) else {
        return None;
    };
    let sig = match analysis.function_signature_of(query_site(c, siblings[head])) {
        Some(sig) => sig,
        // An explicit `receiver.name` callee carries no bare-reference
        // row — the member face composes off the trailing `DotAccess`
        // marker (upstream answers the same member face off the
        // invocation target's static type).
        None => member_callee_sig(c, analysis, &siblings, pos + 1)?,
    };
    // The closure's dispatch slot over the argument list's own pieces.
    let pieces = c.collect_direct(args);
    let mut positional = 0usize;
    for range in slice::arguments(c, &pieces) {
        let last = range.end - 1;
        if c.kind(pieces[last]) == NodeKind::NamedArgument {
            let Some(name_idx) = slice::named_arg_name(c, &pieces, last) else {
                continue;
            };
            if pieces[name_idx + 1..last].contains(&closure) {
                let label = analysis
                    .strings()
                    .intern(c.lexeme(c.head_token(pieces[name_idx])));
                return sig.formals.named_formal(label).map(|formal| formal.ty);
            }
        } else if pieces[range.clone()].contains(&closure) {
            return sig
                .formals
                .positional
                .get(positional)
                .map(|formal| formal.ty);
        }
        positional += 1;
    }
    None
}

/// The explicit `receiver.name` callee's signature: the `DotAccess`
/// marker trailing the send carries the access — its payload's first
/// token is the dot, the member name follows it, and the receiver run
/// tiles before. `None` under-reports (no marker, unmodeled receiver,
/// non-method member).
fn member_callee_sig<'s>(
    c: &Check<'_>,
    analysis: &'s dartq_semantic::Analysis<'_>,
    siblings: &[u32],
    send: usize,
) -> Option<dartq_semantic::model::FnSig<'s>> {
    let marker = slice::access_marker(c, siblings, send, NodeKind::DotAccess)?;
    let dot = c.payload(marker).first().copied()?;
    let selector = dot + 1;
    let receiver_run: Vec<u32> = siblings
        .iter()
        .copied()
        .filter(|&piece| c.unit.ast.token_range(piece).end <= dot)
        .collect();
    let receiver = faces::single_receiver(c, &receiver_run, selector)?;
    let binding = analysis.member_of(
        query_site(c, receiver),
        analysis.strings().intern(c.lexeme(selector)),
    )?;
    dartq_semantic::model::FnSig::of_member(binding.entry)
}
