use super::{
    Check, Group, NONE_TOKEN, NodeKind, RuleSpec, VAR_FINAL_OR_CONST_MEMBER_BIT,
    modifier_keyword,
};

/// Official: `pkg/linter/lib/src/rules/unnecessary_const_in_enum_constructor.dart`
/// — an explicit `const` on an enum constructor reports at the keyword
/// (:30-46: the constructor face requires a `BlockEnumBody` parent,
/// :31-38; the primary-constructor face requires an `EnumDeclaration`
/// parent, :39-46). Registration is feature-gated: the
/// primary-constructors `isFeatureEnabled` bail (:36-40) — pinned on
/// [`crate::engine::Features::primary_constructors`].
///
/// dartq faces: the constructor's enum-body parent is
/// [`NodeKind::EnumBody`] (bridge `begin_enum_body`); its `const` is
/// the `VAR_FINAL_OR_CONST` `ModSet` bit recovered by a backward scan
/// (the row anchors at the declared name, so the head modifiers sit
/// before it). The primary constructor carries its `const` in payload
/// word 1 (bridge `end_primary_constructor`) and its declaration kind
/// in word 0 (`DeclarationKind::Enum`). Message: `diagnostic.g.dart`
/// `unnecessaryConstInEnumConstructor`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "unnecessary_const_in_enum_constructor",
    group: Group::Standalone,
    problem: "Unnecessary 'const' keyword in an enum constructor.",
    correction: "Try removing the keyword.",
    kinds: &[NodeKind::Constructor, NodeKind::PrimaryConstructor],
    check,
};

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:36-40)
    }
    let node = c.node();
    let const_keyword = match c.kind(node) {
        NodeKind::Constructor => {
            if c.ancestor(1)
                .is_none_or(|parent| c.kind(parent) != NodeKind::EnumBody)
            {
                return; // the BlockEnumBody parent face (:31-38)
            }
            if dartq_ast::mod_flags(c.unit.ast, node) & VAR_FINAL_OR_CONST_MEMBER_BIT == 0 {
                return;
            }
            modifier_keyword(c, node, "const")
        }
        NodeKind::PrimaryConstructor => {
            if c.payload(node).first() != Some(&(dartq_parser::DeclarationKind::Enum as u32)) {
                return; // the EnumDeclaration parent face (:39-46)
            }
            c.payload(node)
                .get(1)
                .copied()
                .filter(|&token| token != NONE_TOKEN)
        }
        _ => None,
    };
    let Some(const_keyword) = const_keyword else {
        return;
    };
    c.report(crate::rules::utf16_token_span(c, const_keyword), &[]);
}
