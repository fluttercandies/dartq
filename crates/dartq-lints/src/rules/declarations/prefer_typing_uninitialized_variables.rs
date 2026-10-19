use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/prefer_typing_uninitialized_variables.dart:31-51`
/// — an untyped declaration (`node.type == null`) with an uninitialized
/// variable reports at the variable; fields and locals carry different
/// message texts (`...ForField` / `...ForLocalVariable`,
/// `diagnostic.g.dart:2839/2852`). The type is the child before the
/// first variable (`NoType` = absent). Core set of
/// upstream `package:lints` (`lib/core.yaml`).
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "prefer_typing_uninitialized_variables",
    group: Group::Core,
    problem: "An uninitialized variable should have an explicit type annotation.",
    correction: "Try adding a type annotation.",
    kinds: &[NodeKind::VariablesDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The for-each loop variable is a formal upstream
    // (`ForEachParts.loopVariable`), never a `VariableDeclarationList`,
    // so its production row never reports. The row tiles inside the
    // for node: the sealed `ForIn` statement face, or the element
    // face's `ForControlFlow` with the for-in flag in payload word 1
    // (`end_for_in_control_flow` patches 1; a classic `for` element
    // patches 0 and its init row keeps reporting).
    let is_for_each = c.ancestor(1).is_some_and(|parent| match c.kind(parent) {
        NodeKind::ForIn => true,
        NodeKind::ForControlFlow => c.payload(parent).get(1) == Some(&1),
        _ => false,
    });
    if is_for_each {
        return;
    }
    // `node.type != null` skips the whole declaration (:34). The written
    // type answers from three shapes: a `Type`-kind child (hand-built
    // tables), the token run after the storage keyword
    // ([`local_written_type`] — production rows anchored at the name),
    // and the type-anchored field tiling; [`declaration_written_type`]
    // folds all three, including the keyword-anchored member row whose
    // type sits between the storage keyword and the first declarator
    // name (`final int i;` — a keyword-only `var x;` has an empty gap).
    // The production `NoType` row of `var x;` leaks before the
    // declaration's window, so the first-child read alone misses the
    // keyword-only face.
    let kids = c.collect_direct(node);
    if kids.iter().any(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    }) {
        return;
    }
    let untyped = match kids.first().copied() {
        Some(first) if c.kind(first) == NodeKind::NoType => true,
        _ => super::declaration_written_type(c, node).is_none(),
    };
    if !untyped {
        return;
    }
    let is_field = c.ancestor(1).is_some_and(|parent| {
        matches!(c.kind(parent), NodeKind::Fields | NodeKind::TopLevelFields)
    });
    // Gather untyped variables under the scratch borrow, then report
    // (the report path needs `&mut Check`); allocates only on hits.
    let mut hits: Vec<u32> = Vec::new();
    for var in c.direct_children(node).iter().copied() {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        let initialized = c
            .children(var)
            .any(|ch| c.kind(ch) == NodeKind::VariableInitializer);
        if !initialized {
            hits.push(var);
        }
    }
    for var in hits {
        if is_field {
            c.report_texts(
                c.utf16_span(var),
                "An uninitialized field should have an explicit type annotation.",
                "Try adding a type annotation.",
                &[],
            );
        } else {
            let range = c.utf16_span(var);
            c.report(range, &[]);
        }
    }
}
