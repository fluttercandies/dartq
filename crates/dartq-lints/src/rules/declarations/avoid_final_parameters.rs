use super::{Check, Group, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_final_parameters.dart:41-84`
/// — a parameter marked `final` reports at the keyword, except:
/// parameters of a generic function type (:63, the parent
/// `GenericFunctionType` list-level skip), function-typed-suffix
/// parameters (:76), and field/super formals (:77-78). Registration is
/// feature-gated INVERTED: the rule registers only WITHOUT primary
/// constructors (:56-59, the declaring-parameter form supersedes it) —
/// pinned on [`crate::engine::Features::primary_constructors`].
///
/// dartq faces: the list-level skip reads the owner row
/// ([`NodeKind::FunctionType`] / [`NodeKind::FunctionTypedFormalParameter`]
/// — the generic-function-type annotation collapses to the latter,
/// probe shape); the per-parameter skips read the receiver word and the
/// function-typed child. The `final` keyword is recovered from the
/// `ModSet` head (`VAR_FINAL_OR_CONST` bit, bridge `mod_bits`) by a
/// bounded scan past `required` / `covariant`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_final_parameters",
    group: Group::Standalone,
    problem: "Parameters should not be marked as 'final'.",
    correction: "Try removing the keyword 'final'.",
    kinds: &[
        NodeKind::FormalParameters,
        NodeKind::OptionalFormalParameters,
    ],
    check,
};

/// The `varFinalOrConst` `ModSet` bit (bridge `mod_bits`).
const VAR_FINAL_OR_CONST_BIT: u16 = 1 << 6;

fn check(c: &mut Check<'_>) {
    if c.unit.features.primary_constructors {
        return; // inverted registration bail (:56-59)
    }
    let node = c.node();
    // The generic-function-type face (:63): the inner parameter lists of
    // a written function type never report.
    if c.ancestor(1).is_some_and(|owner| {
        matches!(
            c.kind(owner),
            NodeKind::FunctionType | NodeKind::FunctionTypedFormalParameter
        )
    }) {
        return;
    }
    for param in c.collect_direct(node) {
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        // Function-typed suffix (:76) and field/super formals (:77-78).
        if c.collect_direct(param)
            .iter()
            .any(|&kid| c.kind(kid) == NodeKind::FunctionTypedFormalParameter)
        {
            continue;
        }
        let receiver = c
            .payload(param)
            .first()
            .copied()
            .unwrap_or(crate::rules::NONE_TOKEN);
        if receiver != crate::rules::NONE_TOKEN {
            continue;
        }
        if let Some(kw) = final_keyword(c, param) {
            c.report(crate::rules::utf16_token_span(c, kw), &[]);
        }
    }
}

/// The `final` keyword token of a `VAR_FINAL_OR_CONST`-marked
/// parameter, `None` when the written keyword is `var` / `const`.
fn final_keyword(c: &Check<'_>, param: u32) -> Option<u32> {
    if dartq_ast::mod_flags(c.unit.ast, param) & VAR_FINAL_OR_CONST_BIT == 0 {
        return None;
    }
    // The ModSet head is the first modifier; `required` / `covariant`
    // precede the storage keyword (bounded: three modifiers max).
    let mut token = c.head_token(param);
    for _ in 0..3 {
        match c.lexeme(token) {
            "final" => return Some(token),
            "required" | "covariant" => token += 1,
            _ => return None,
        }
    }
    None
}
