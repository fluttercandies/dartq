use super::{Check, NONE_TOKEN, NodeKind, RuleSpec, VAR_FINAL_OR_CONST_MEMBER_BIT, for_in_binds, parameters_bind, statement_binds};

/// Official: `pkg/linter/lib/src/rules/prefer_final_parameters.dart` —
/// a parameter that is neither `final`/`const` nor reassigned in its
/// function reports at the parameter (:65-73). Registration is
/// feature-gated INVERTED: the rule registers only WITHOUT primary
/// constructors (:48-52 — with the feature, `final` on a parameter is
/// a declaring-parameter compile-time error) — pinned on
/// [`crate::engine::Features::primary_constructors`]. The rule is
/// upstream-deprecated (3.11) but still registering — deprecated is
/// not removed. The registered faces (:54-59) are constructors,
/// methods, and function expressions — and a top-level/local function
/// declaration reports THROUGH the last face: the analyzer's
/// `FunctionDeclaration` wraps a registered `FunctionExpression`
/// child, so its parameters are visited (oracle 3.13.1,
/// `invalid_case_patterns.dart` 16:8 `dynamic o` and siblings); our
/// tiling names the wrapped row `TopLevelMethod`/`LocalFunction-
/// Declaration`, so both join the kinds list. Field/super
/// formals are final by definition (:67-70).
///
/// dartq faces: the storage keyword is the `VAR_FINAL_OR_CONST`
/// `ModSet` bit recovered by a bounded forward scan (the receiver word
/// answers the field/super skip); the wildcard skip folds
/// `declaredElement.isWildcardVariable` (:71) to the lexeme under the
/// engine's `wildcard_variables` bit (types.rs precedent); the
/// `!body.isPotentiallyMutatedInScope` guard (:72-73) folds to the
/// element-identity write walk below — every unqualified write in the
/// function scope mutates (`_recordUnqualifiedWrite`,
/// analyzer/src/dart/resolver/scope_analyzer.dart:1094-1103:
/// assignment LHS, `++`/`--` prefix/postfix operands, assigned
/// variable patterns), and a nearer binder of the same lexeme (the
/// shared shadow-check face's binder set) captures the write first.
/// Message: `diagnostic.g.dart` `preferFinalParameters`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_final_parameters",
    &[
        NodeKind::Constructor,
        NodeKind::Method,
        NodeKind::FunctionExpression,
        // The FunctionDeclaration-wraps-FunctionExpression face: a
        // top-level (`TopLevelMethod`) or local
        // (`LocalFunctionDeclaration`) function's parameters report
        // like a closure's.
        NodeKind::TopLevelMethod,
        NodeKind::LocalFunctionDeclaration,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.unit.features.primary_constructors {
        return; // inverted registration bail (:48-52)
    }
    let node = c.node();
    let Some(params) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return;
    };
    for param in parameter_rows(c, params) {
        // `isFinal || isConst` (:65-66) — `var` does not skip.
        if storage_keyword(c, param).is_some_and(|kw| matches!(c.lexeme(kw), "final" | "const"))
        {
            continue;
        }
        // A field/super formal is final without the modifier (:67-70).
        let receiver = c.payload(param).first().copied().unwrap_or(NONE_TOKEN);
        if receiver != NONE_TOKEN {
            continue;
        }
        let Some(name) = c
            .payload(param)
            .get(1)
            .copied()
            .filter(|&token| token != NONE_TOKEN)
        else {
            continue;
        };
        let name = c.lexeme(name);
        // `isWildcardVariable` (:71) under the engine's wildcard bit.
        if name == "_" && c.unit.features.wildcard_variables {
            continue;
        }
        // `!isPotentiallyMutatedInScope` (:72-73) — the official walks
        // `node.body` only, so the query skips the parameter list; the
        // production tiling may hang the body rows (a `=>` expression's
        // pieces) directly on the container rather than inside the
        // `ExpressionBody` wrapper, so the scan covers every direct
        // child below it.
        let mutated = c
            .collect_direct(node)
            .into_iter()
            .filter(|&kid| kid != params)
            .any(|kid| mutates(c, kid, name));
        if mutated {
            continue;
        }
        c.report(c.utf16_span(param), &[name]);
    }
}

/// The container's parameter rows: the direct plain formals plus the
/// optional group's rows (the analyzer's `parameters.parameters` list
/// includes both nests).
fn parameter_rows(c: &Check<'_>, params: u32) -> Vec<u32> {
    let mut rows = vec![];
    for kid in c.collect_direct(params) {
        match c.kind(kid) {
            NodeKind::FormalParameter => rows.push(kid),
            NodeKind::OptionalFormalParameters => {
                rows.extend(
                    c.collect_direct(kid)
                        .into_iter()
                        .filter(|&row| c.kind(row) == NodeKind::FormalParameter),
                );
            }
            _ => {}
        }
    }
    rows
}

/// The `var`/`final`/`const` keyword token of a `VAR_FINAL_OR_CONST`-
/// marked parameter (`None` when unmarked; bounded: `required` /
/// `covariant` precede the storage keyword).
fn storage_keyword(c: &Check<'_>, param: u32) -> Option<u32> {
    if dartq_ast::mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_MEMBER_BIT == 0 {
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

/// Whether any unqualified write to `name` happens in the container's
/// scope (the `isPotentiallyMutatedInScope` fold). The write faces are
/// the scope analyzer's `_recordUnqualifiedWrite` set: an assignment
/// whose left operand is the bare name, a `++`/`--` whose operand is
/// the bare name, and an assigned variable pattern. A nearer binder of
/// the same lexeme (a nested function's parameters, a block's local
/// declarations, catch parameters, loop variables, case pattern
/// variables) captures the write first — the shared shadow-check
/// face's binder set.
fn mutates(c: &Check<'_>, node: u32, name: &str) -> bool {
    match c.kind(node) {
        NodeKind::Assignment => {
            // The adopted container is childless on the production
            // tiling — the operands re-split off the owner siblings on
            // the operator token. The nested-write arm still recurses
            // the right run's pieces (a write inside a call argument).
            let Some((left, right)) = crate::rules::slice::binary_operands_at(c, node) else {
                return false;
            };
            // The bare-name lhs: the identifier immediately before the
            // operator token (a member-write target's `.name` tokens sit
            // between the receiver identifier and the operator).
            let op = c.payload(node).first().copied();
            // `this.x = …` is a *qualified* field write — the scope
            // analyzer's `_recordUnqualifiedWrite` only counts the bare
            // name. The left run can carry earlier siblings' rows (a
            // `=>` body keeps the owner prefix inside it), so the
            // unqualified test is the last operand identifier closing
            // at `=` with no `.` before it — the `this.` selector's
            // name token sits right after the dot.
            let lhs_writes = op.is_some_and(|op| {
                crate::rules::slice::access_run(c, &left)
                    .last()
                    .is_some_and(|&lhs| {
                        c.kind(lhs) == NodeKind::Identifier
                            && c.unit.ast.token_range(lhs).end == op
                            && c.lexeme(c.head_token(lhs)) == name
                            && c.head_token(lhs)
                                .checked_sub(1)
                                .is_none_or(|prev| c.lexeme(prev) != ".")
                    })
            });
            if lhs_writes {
                return true;
            }
            right.into_iter().any(|kid| mutates(c, kid, name))
        }
        NodeKind::UnaryPrefix | NodeKind::UnaryPostfix => {
            // The operator row is a leaf sealing last in the owner
            // slice; the bare operand's tear-off group
            // (`Identifier, NoTypeArguments, NoArguments, Send`)
            // occupies the four owner siblings right before it (the
            // same completion-order posture for `++p` and `p++`).
            let operator = match c.kind(node) {
                NodeKind::UnaryPrefix => c.lexeme(c.head_token(node)),
                _ => c.lexeme(c.unit.ast.token_range(node).end - 1),
            };
            if !matches!(operator, "++" | "--") {
                return false;
            }
            let Some(parent) = c.unit.ast.parent(node) else {
                return false;
            };
            let siblings = c.collect_direct(parent);
            let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
                return false;
            };
            let Some(start) = pos.checked_sub(4) else {
                return false;
            };
            crate::rules::slice::is_tear_off_group(c, &siblings, start)
                && c.lexeme(c.head_token(siblings[start])) == name
        }
        NodeKind::PatternAssignment => {
            // The childless marker anchors over the `=` (payload word
            // 0); the target pattern's pieces are the owner siblings
            // before it, up to the `=` byte — the statement run
            // bounded at the previous statement row.
            let Some(equals) = c.payload(node).first().copied() else {
                return false;
            };
            let Some(eq_start) = c
                .unit
                .tokens
                .get(equals as usize)
                .map(|t| t.byte_offset as usize)
            else {
                return false;
            };
            let Some(parent) = c.unit.ast.parent(node) else {
                return false;
            };
            let siblings = c.collect_direct(parent);
            let Some(pos) = siblings.iter().position(|&sibling| sibling == node) else {
                return false;
            };
            crate::rules::slice::statement_run(c, &siblings[..pos])
                .into_iter()
                .filter(|&row| c.byte_span(row).end <= eq_start)
                .any(|row| {
                    c.kind(row) == NodeKind::AssignedVariablePattern
                        && c.lexeme(c.head_token(row)) == name
                })
        }
        NodeKind::AssignedVariablePattern => c.lexeme(c.head_token(node)) == name,
        // Nested function scopes: the own parameters bind first.
        NodeKind::FunctionExpression
        | NodeKind::LocalFunctionDeclaration
        | NodeKind::NamedFunctionExpression => {
            let own_params = c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
            !own_params.is_some_and(|params| parameters_bind(c, params, name))
                && c.collect_direct(node)
                    .into_iter()
                    .any(|kid| mutates(c, kid, name))
        }
        // Block statements: a binding statement shadows itself only
        // (its trailing expression rows cannot write the bound name in
        // legal code — the initializer runs before the binding).
        NodeKind::Block => c
            .collect_direct(node)
            .into_iter()
            .any(|kid| !statement_binds(c, kid, name) && mutates(c, kid, name)),
        NodeKind::CatchClause => {
            let binds = c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                .is_some_and(|params| parameters_bind(c, params, name));
            !binds
                && c.collect_direct(node)
                    .into_iter()
                    .any(|kid| mutates(c, kid, name))
        }
        // Loop heads shadow their own variables only; the body stays
        // in scope.
        NodeKind::ForStatement => {
            let parts = c
                .collect_direct(node)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::ForParts);
            let head_shadowed = parts.is_some_and(|parts| binds_parts(c, parts, name));
            c.collect_direct(node).into_iter().any(|kid| {
                Some(kid) != parts.filter(|_| head_shadowed) && mutates(c, kid, name)
            })
        }
        NodeKind::ForIn | NodeKind::ForControlFlow => {
            !for_in_binds(c, node, name)
                && c.collect_direct(node)
                    .into_iter()
                    .any(|kid| mutates(c, kid, name))
        }
        // Case patterns bind new variables across the case body.
        NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
            let binds = c.collect_direct(node).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::DeclaredVariablePattern
                    && c.payload(kid)
                        .first()
                        .is_some_and(|&v| v != NONE_TOKEN && c.lexeme(v) == name)
            });
            !binds
                && c.collect_direct(node)
                    .into_iter()
                    .any(|kid| mutates(c, kid, name))
        }
        _ => c
            .collect_direct(node)
            .into_iter()
            .any(|kid| mutates(c, kid, name)),
    }
}

/// Whether a `ForParts` row's own variable declarations bind `name`
/// (the `path_shadowed` `ForStatement` face).
fn binds_parts(c: &Check<'_>, parts: u32, name: &str) -> bool {
    c.collect_direct(parts)
        .into_iter()
        .any(|stmt| statement_binds(c, stmt, name))
}
