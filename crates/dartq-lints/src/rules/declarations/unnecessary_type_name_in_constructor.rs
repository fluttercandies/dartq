use super::{Check, NodeKind, RuleSpec, type_name_echo};

/// Official: `pkg/linter/lib/src/rules/unnecessary_type_name_in_constructor.dart`
/// — every constructor declaration reports at its type name (:32-43),
/// the class-name echo the grammar requires before the name part.
/// Registration is feature-gated: the primary-constructors
/// `isFeatureEnabled` bail (:36-38) — pinned on
/// [`crate::engine::Features::primary_constructors`].
///
/// dartq face: the echo is the member-name context identifier child
/// ([`type_name_echo`]; a recovered no-name head falls back to the row
/// head). Message: `diagnostic.g.dart`
/// `unnecessaryTypeNameInConstructor`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_type_name_in_constructor",
    // A factory is a `ConstructorDeclaration` upstream — the `C.name`
    // echo carries the same unnecessary type name (:32-43 covers both).
    &[NodeKind::Constructor, NodeKind::Factory],
    check,
);

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:36-38)
    }
    let node = c.node();
    let range = match type_name_echo(c, node) {
        Some(echo) => c.utf16_span(echo),
        None => crate::rules::utf16_token_span(c, c.head_token(node)),
    };
    c.report(range, &[]);
}
