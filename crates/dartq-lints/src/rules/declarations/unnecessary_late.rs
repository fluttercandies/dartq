use super::{Check, Group, NodeKind, RuleSpec};
use dartq_parser::MemberKind;

/// Official: `pkg/linter/lib/src/rules/unnecessary_late.dart:36-55` —
/// `late` on a static field or top-level variable where every variable
/// of the declaration has an initializer reports at `late`. Instance
/// fields are excluded (lazy initialization is meaningful there). The
/// `late` keyword is the declaration's span head when present
/// (`VariablesDeclaration` head-when-present contract). Message:
/// `diagnostic.g.dart` `unnecessaryLate`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "unnecessary_late",
    group: Group::Recommended,
    problem: "Unnecessary 'late' modifier.",
    correction: "Try removing the 'late'.",
    kinds: &[NodeKind::VariablesDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `late` ⟺ span head is the keyword (#174 elimination).
    let head = c.head_token(node);
    if c.lexeme(head) != "late" {
        return;
    }
    // Static fields and top-level variables only (:36-45).
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        NodeKind::TopLevelFields => {}
        NodeKind::Fields => {
            // Fields payload word 0 is the `MemberKind` discriminant:
            // only the static kind reports.
            if c.payload(parent).first() != Some(&(MemberKind::StaticField as u32)) {
                return;
            }
        }
        _ => return,
    }
    // Every variable must have an initializer (:51-54: any missing
    // initializer skips the report).
    for var in c.direct_children(node).iter().copied() {
        if c.kind(var) == NodeKind::InitializedIdentifier && !has_initializer(c, var) {
            return;
        }
    }
    let range = c.utf16_span(node);
    let start = range.start;
    c.report(start..start, &[]);
}

/// An `InitializedIdentifier` has an initializer iff a
/// `VariableInitializer` child is present (the diagnostic collapses to
/// plain absence).
fn has_initializer(c: &Check<'_>, var: u32) -> bool {
    c.children(var)
        .any(|ch| c.kind(ch) == NodeKind::VariableInitializer)
}
