use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::last_child;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/prefer_function_declarations_over_variables.dart:40-58`
/// — a variable initialized with a function literal reports. Fields and
/// top-level variables only when final (:42-49); locals unless the
/// variable is potentially mutated in its scope (:50-55).
///
/// The mutation face (`isPotentiallyMutatedInScope`, :53) is the
/// in-rule write scan over the enclosing function body: any unqualified
/// write bound to the variable after the declaration passes — an
/// assignment target (`=`, `??=`, compound), a `++`/`--` operand, or a
/// pattern-assignment variable target (`(f, _) = ...`). The element
/// identity is the scope walk: a write inside a subtree that re-binds
/// the name (a block local, catch parameter, nested function, loop
/// variable, case or if-case pattern variable) resolves to the inner
/// binder and does not count. Loop-part rebinding (`for (f in ...)`) is
/// a declaration, not a write, and never suppresses, mirroring the
/// official's declaration face. An initializer-inferred write type
/// keeps the documented conservative family. Message:
/// `diagnostic.g.dart` `preferFunctionDeclarationsOverVariables`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_function_declarations_over_variables",
    &[NodeKind::InitializedIdentifier],
    check,
);

fn check(c: &mut Check<'_>) {
    let Some(initializer) = c
        .collect_direct(c.node())
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
    else {
        return;
    };
    // The value expression is the initializer's LAST direct child (it
    // completes last); a lambda's absent-return marker
    // (`NoTypeVariables`) may precede the `FunctionExpression` inside
    // the completion window, so the first-child read is not the value.
    let Some(value) = last_child(c, initializer) else {
        return;
    };
    if c.kind(value) != NodeKind::FunctionExpression {
        return;
    }
    // No enclosing function body ⟹ field/top-level: final only (:42-49).
    let Some(declaration) = c.ancestor(1) else {
        return;
    };
    if c.kind(declaration) != NodeKind::VariablesDeclaration {
        return;
    }
    let is_member = c
        .ancestor(2)
        .is_some_and(|owner| matches!(c.kind(owner), NodeKind::Fields | NodeKind::TopLevelFields));
    if is_member {
        // The storage keyword: production parses store only `late` in
        // the payload word, so the shared head/payload/backward-walk
        // read decides (`final handler = ...;` at top level answers
        // `final`).
        let keyword = crate::rules::variables_declaration_keyword(c, declaration);
        if keyword.is_none_or(|kw| c.lexeme(kw) != "final") {
            return;
        }
    } else if is_potentially_mutated(c) {
        // `isPotentiallyMutatedInScope` (:53) — a later write to the
        // name in the enclosing function body keeps it a variable.
        return;
    }
    let range = c.utf16_span(c.node());
    c.report(range, &[]);
}

/// Whether any unqualified write to this declaration's name sits at or
/// after the declaration inside the enclosing function body (the
/// nearest `BlockBody` ancestor — the official's `FunctionBody` scope).
fn is_potentially_mutated(c: &Check<'_>) -> bool {
    let Some(name_token) = crate::rules::initialized_identifier_name(c, c.node()) else {
        return false;
    };
    let name = c.lexeme(name_token);
    let after = c.byte_span(c.node()).end;
    let Some(body) = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&ancestor| c.kind(ancestor) == NodeKind::BlockBody)
    else {
        return false;
    };
    is_mutated_after(c, body, name, after)
}

/// The write scan over `root`'s subtree: a write bound to the bare name
/// at or after `after` (byte offset — the declaration's own initializer
/// is before it). Subtrees that end before `after` prune; a subtree
/// that re-binds the name prunes whole ([`scope_binds`]), except an
/// if-case whose binding reaches the then branch only.
fn is_mutated_after(c: &Check<'_>, root: u32, name: &str, after: usize) -> bool {
    let kids = c.collect_direct(root);
    for (index, &kid) in kids.iter().enumerate() {
        if c.byte_span(kid).end <= after {
            continue;
        }
        let writable = c.byte_span(kid).start >= after;
        if writable && writes_name(c, kid, name) {
            return true;
        }
        if writable && unary_writes_name(c, &kids, index, name) {
            return true;
        }
        if writable
            && c.kind(kid) == NodeKind::PatternAssignment
            && pattern_writes_name(c, kid, name)
        {
            return true;
        }
        // An if-case binder captures its then branch only; the else
        // branch (and later siblings at this level) still bind ours.
        if c.kind(kid) == NodeKind::IfStatement && super::if_case_binds(c, kid, name) {
            let branch_kids = c.collect_direct(kid);
            if let Some(cond_pos) = branch_kids
                .iter()
                .position(|&branch| c.kind(branch) == NodeKind::ParenthesizedCondition)
            {
                for &branch in branch_kids.iter().skip(cond_pos + 2) {
                    if is_mutated_after(c, branch, name, after) {
                        return true;
                    }
                }
                continue;
            }
        }
        if scope_binds_other(c, kid, name, c.node()) {
            continue; // the subtree's writes bind the inner declaration
        }
        if is_mutated_after(c, kid, name, after) {
            return true;
        }
    }
    false
}

/// [`super::scope_binds`] with the scanned declaration itself excluded:
/// the enclosing block's own `var f = () {}` statement is the binder the
/// walk measures mutations of, not a shadowing rebind — only a
/// *different* `InitializedIdentifier` row prunes the subtree.
fn scope_binds_other(c: &Check<'_>, node: u32, name: &str, declared: u32) -> bool {
    if c.kind(node) != NodeKind::Block {
        return super::scope_binds(c, node, name);
    }
    c.collect_direct(node).into_iter().any(|stmt| {
        c.kind(stmt) == NodeKind::VariablesDeclaration
            && c.collect_direct(stmt).into_iter().any(|decl| {
                decl != declared
                    && c.kind(decl) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, decl)
                        .is_some_and(|n| c.lexeme(n) == name)
            })
            || c.kind(stmt) != NodeKind::VariablesDeclaration
                && super::statement_binds(c, stmt, name)
    })
}

/// The write faces: an assignment whose left operand is the bare name
/// (the op payload word may be any of `=` / `??=` / compound). The
/// adopted `Assignment` container is childless in the production
/// tiling — its operand pieces tile as owner siblings, re-split on the
/// operator token by [`crate::rules::slice::binary_operands_at`].
fn writes_name(c: &Check<'_>, node: u32, name: &str) -> bool {
    if c.kind(node) != NodeKind::Assignment {
        return false;
    }
    let Some(op) = c.field(node, PayloadField::Operator) else {
        return false;
    };
    let Some((left, _)) = crate::rules::slice::binary_operands_at(c, node) else {
        return false;
    };
    // The bare-name lhs: the identifier immediately before the
    // operator token (a member-write target's `.name` tokens sit
    // between the receiver identifier and the operator).
    crate::rules::slice::access_run(c, &left)
        .last()
        .is_some_and(|&lhs| {
            c.kind(lhs) == NodeKind::Identifier
                && c.unit.ast.token_range(lhs).end == op
                && c.lexeme(c.head_token(lhs)) == name
        })
}

/// The `++`/`--` face: the operator row is a leaf sealing last in the
/// owner slice, so the bare operand's tear-off group
/// (`Identifier, NoTypeArguments, NoArguments, Send`) occupies the four
/// owner siblings right before it — the same completion-order posture
/// for `++f` and `f++` (`parameter_assignments`'s unary face).
fn unary_writes_name(c: &Check<'_>, kids: &[u32], index: usize, name: &str) -> bool {
    let node = kids[index];
    let operator = match c.kind(node) {
        NodeKind::UnaryPrefix => c.lexeme(c.head_token(node)),
        NodeKind::UnaryPostfix => c.lexeme(c.unit.ast.token_range(node).end - 1),
        _ => return false,
    };
    if operator != "++" && operator != "--" {
        return false;
    }
    let Some(start) = index.checked_sub(4) else {
        return false;
    };
    crate::rules::slice::is_tear_off_group(c, kids, start)
        && c.lexeme(c.head_token(kids[start])) == name
}

/// The pattern-assignment face: the childless `PatternAssignment`
/// marker anchors over the `=` (the row's `Equals` word), and the target
/// pattern's pieces are the owner siblings before it, up to the `=`
/// byte — a write when an `AssignedVariablePattern` names the bare
/// name (`(f, _) = ...`).
fn pattern_writes_name(c: &Check<'_>, node: u32, name: &str) -> bool {
    let Some(equals) = c.field(node, PayloadField::Equals) else {
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
            c.kind(row) == NodeKind::AssignedVariablePattern && c.lexeme(c.head_token(row)) == name
        })
}
