use super::{Check, Group, NodeKind, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_init_to_null.dart:59-88`
/// — a variable whose initializer is the `null` literal reports at the
/// declaration; const/final declarations are exempt (`!node.isConst &&
/// !node.isFinal`, :62-63), the declared type must be nullable
/// (`typeSystem.isNullable`, :41/:65 — `int x = null;` is a compile
/// error upstream), and the formal-parameter default-value faces
/// (:74-87) carry the same nullable gate.
///
/// The nullable gate reads the declaration's tokens
/// ([`super::declaration_written_type`] + the tail-`?`/`dynamic`
/// spell): a written non-nullable type bails, no written type infers
/// `dynamic` (nullable). A type-alias spelling of a nullable type
/// (`MyNull?` aside) under-reports. The super-formal sub-check
/// (`superConstructorParameter.defaultValueCode`, :78-84) remains
/// resolution-bound and treated like the regular form. The keyword gate
/// reads the declaration's `varFinalOrConst` token (the
/// `VariablesDeclaration` `modifier` word; the parameter's init range is
/// its `init_start`/`init_end` words). Message: `diagnostic.g.dart`
/// `avoidInitToNull`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_init_to_null",
    group: Group::Recommended,
    problem: "Redundant initialization to 'null'.",
    correction: "Try removing the initializer.",
    kinds: &[NodeKind::InitializedIdentifier, NodeKind::FormalParameter],
    check,
};

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::InitializedIdentifier => check_variable(c),
        NodeKind::FormalParameter => check_parameter(c),
        _ => {}
    }
}

fn check_variable(c: &mut Check<'_>) {
    // The initializer is the `VariableInitializer` child; its operand is
    // the null literal.
    let initializer = c
        .collect_direct(c.node())
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
    let Some(initializer) = initializer else {
        return;
    };
    let Some(value) = first_child(c, initializer) else {
        return;
    };
    if c.kind(value) != NodeKind::LiteralNull {
        return;
    }
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    // `!node.isConst && !node.isFinal` (:62-63): the declaration
    // keyword is the VariablesDeclaration `modifier` word;
    // a written type (no keyword) initializes the same way.
    if c.kind(parent) == NodeKind::VariablesDeclaration
        && let Some(&keyword) = c.payload(parent).first()
        && keyword != NONE_TOKEN
        && matches!(c.lexeme(keyword), "final" | "const")
    {
        return;
    }
    // `isNullable(declaredElement.type)` (:41/:65): a written
    // non-nullable type is a compile error upstream — `int x = null;`
    // stays silent; no written type infers `dynamic` (nullable).
    if c.kind(parent) == NodeKind::VariablesDeclaration
        && let Some(run) = super::declaration_written_type(c, parent)
        && !super::written_run_nullable(c, run)
    {
        return;
    }
    let range = c.utf16_span(c.node());
    c.report(range, &[]);
}

fn check_parameter(c: &mut Check<'_>) {
    // `{int x = null}` — the default value is delimited by the
    // `init_start`/`init_end` payload words);
    // exactly the `null` token qualifies under either end convention.
    let payload = c.payload(c.node());
    let Some(&start) = payload.get(2) else {
        return;
    };
    if start == NONE_TOKEN {
        return;
    }
    let end = payload.get(3).copied().unwrap_or(start);
    if c.lexeme(start) != "null" || end.saturating_sub(start) > 1 {
        return;
    }
    // `isNullable(declaredElement.type)` (:87): the written type
    // decides; absent (`{x = null}`) infers `dynamic` (nullable).
    if let Some(written) = c.collect_direct(c.node()).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    }) {
        let range = c.unit.ast.token_range(written);
        if !super::written_run_nullable(c, (range.start, range.end)) {
            return;
        }
    }
    // `reportAtNode` on the parameter reports `name..default end`
    // (upstream highlight `[!x = null!]` / `[!this.a = null!]` — the
    // written type is excluded, the `this`/`super` receiver word
    // included): payload[0] = receiver, [1] = name, [2..3] = the
    // default's init range (end-exclusive).
    let name_or_receiver = payload
        .first()
        .copied()
        .filter(|&token| token != NONE_TOKEN)
        .or_else(|| payload.get(1).copied())
        .unwrap_or(start);
    let end_byte = c
        .unit
        .tokens
        .get(end.saturating_sub(1) as usize)
        .map(|token| token.byte_offset + token.byte_len)
        .unwrap_or(0);
    let end = c.unit.lines.byte_to_utf16(c.unit.source, end_byte);
    let start = c
        .unit
        .tokens
        .get(name_or_receiver as usize)
        .map(|token| token.utf16_offset)
        .unwrap_or(0);
    c.report(start..end, &[]);
}
