use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{const_ability, utf16_token_span, variables_declaration_keyword};
use dartq_parser::MemberKind;

/// Official: `pkg/linter/lib/src/rules/prefer_const_declarations.dart:52-64`
/// — a `final` declaration list whose every variable carries an
/// initializer that is a constant as written reports at the list
/// (:62). The visited faces (:31-34) are the static field declaration,
/// the top-level variable and the local statement.
///
/// The initializer gate (:55-60) folds to the const-ability face
/// (`const_ability::is_const_able`) with the as-written cut: the
/// official `hasConstantError` answers "constant" only for expressions
/// that already are one — so a creation without the written `const`
/// (`C()`, `new C()`, `C<int>(…)`) answers false, and the
/// `TypedLiteral` arm (:58) keeps bare collection literals silent (a
/// `[]` initializer is a runtime literal). Records and parenthesized
/// expressions are not `TypedLiteral`s and ride the const-ability
/// face. Approximations (documented — the evaluator cut): a
/// written-`const` wrapper answers true without validating its
/// contents (a constant error inside stays an over-report), and the
/// constant-reference faces resolve same-file only — tear-offs
/// (`C.new`, `C.m`, a bare function name) under-report. Message:
/// `diagnostic.g.dart` `preferConstDeclarations`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_const_declarations",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `node.isConst`/`!node.isFinal` (:53-54) — the keyword must spell
    // exactly `final` (`const` and `var` never qualify).
    let Some(keyword) =
        variables_declaration_keyword(c, node).filter(|&kw| c.lexeme(kw) == "final")
    else {
        return;
    };
    // The visited faces (:31-34). Instance fields stay silent (the
    // official registers only the static `FieldDeclaration`), and the
    // for-loop faces are no `VariableDeclarationStatement`s.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        NodeKind::Fields => {
            if c.field(parent, PayloadField::MemberKind) != Some(MemberKind::StaticField as u32) {
                return;
            }
        }
        NodeKind::TopLevelFields
        | NodeKind::Block
        | NodeKind::BlockBody
        | NodeKind::LabeledStatement
        | NodeKind::SwitchCase => {}
        _ => return,
    }
    // `node.variables.every(...)` (:55-61): every variable carries an
    // initializer that is a constant as written — one miss skips the
    // report (`final x, y = 1;`). The written-`const` wrapper
    // qualifies outright (:58-59 — the keyword arm); its content may
    // tile beside the wrapper row, so the initializer kids are read
    // wrapper-first.
    let mut last_var = None;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        let Some(init) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return; // a missing initializer skips the report (:56-57)
        };
        let kids = c.collect_direct(init);
        let wrapped = kids.iter().any(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::ConstExpression | NodeKind::ConstLiteral
            )
        });
        let verdict = if wrapped {
            Some(true)
        } else {
            // The tear-off/creation expression: the LAST send owns the
            // whole access (`C<int>.new` tiles the generic `C<int>`
            // send beside the `.new` tail send — the tail is the
            // constant face).
            kids.iter()
                .rev()
                .find(|&&kid| c.kind(kid) == NodeKind::Send)
                .or_else(|| kids.last())
                .copied()
                .map(|expr| const_as_written(c, expr))
        };
        if !verdict.unwrap_or(false) {
            return;
        }
        last_var = Some(var);
    }
    let Some(last_var) = last_var else {
        return;
    };
    // `rule.reportAtNode(node)` (:62) — the list: the keyword head
    // through the last variable (the row's trailing `;` belongs to the
    // statement, not the official list node).
    let start = utf16_token_span(c, keyword).start;
    let end = c.utf16_span(last_var).end;
    c.report(start..end, &[]);
}

/// Whether the initializer is a constant as written — the official
/// `initializer != null && (is! TypedLiteral || const keyword) &&
/// !hasConstantError` face (:56-60), minus the wrapper arm (its
/// caller reads the written `const` first).
fn const_as_written(c: &Check<'_>, expr: u32) -> bool {
    match c.kind(expr) {
        // A `TypedLiteral` without the `const` keyword (:58); a
        // creation is a constant only when `const` is written, so `new`
        // and the implicit-creation spellings never are either.
        NodeKind::ListLiteral
        | NodeKind::SetOrMapLiteral
        | NodeKind::NewExpression
        | NodeKind::ImplicitCreation => false,
        NodeKind::Send => !send_invocation(c, expr) && const_ability::is_const_able(c.unit, expr),
        _ => const_ability::is_const_able(c.unit, expr),
    }
}

/// Whether the send row carries an argument list — the invocation
/// faces are runtime shapes without the written `const` (a bare `C()`
/// with a const constructor is not a constant as written).
fn send_invocation(c: &Check<'_>, send: u32) -> bool {
    let Some(parent) = c.unit.ast.parent(send) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == send) else {
        return false;
    };
    pos > 0 && c.kind(kids[pos - 1]) == NodeKind::Arguments
}
