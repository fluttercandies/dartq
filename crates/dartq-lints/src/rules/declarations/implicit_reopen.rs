use super::{Check, RuleSpec, class_bits};
use crate::rules::{SupertypeClause, supertypes, utf16_token_span};
use dartq_ast::NodeKind;

/// Official: `pkg/linter/lib/src/rules/implicit_reopen.dart:42-130` — a
/// class (declaration or type alias) whose direct same-library
/// superclass carries a stronger restriction than the subclass writes
/// reports at its name: a `base` subclass of a `final` superclass
/// reports `final` (:58-65), a `base` or modifier-less subclass of an
/// `interface` superclass reports `interface` (:67-88). A `@reopen`
/// annotation (:49), a `sealed` subclass (:50) and a `mixin class`
/// (:51) all skip; a supertype declared outside the library skips
/// (:56). The superclass flags are the element's EFFECTIVE flags — the
/// analyzer induces modifiers onto `sealed` declarations from their own
/// supertypes (`instance_member_inferrer.dart` `_setInducedModifier`),
/// which is what makes the induced chains of
/// `implicit_reopen_test.dart` `ImplicitReopenInducedModifierTest`
/// report; the same-file face is [`super::hierarchy_row`] and
/// [`super::hierarchy_flags`].
///
/// Approximations (documented — the element-model cut): the supertype
/// must resolve to a same-file class-like row (`supertype.library !=
/// library` :56 — a cross-library supertype never reports); a `@reopen`
/// spelled through a prefix (`meta.reopen`) never matches the written
/// annotation name, so it reports where the official stays silent. The
/// experimental `RuleState` is registry metadata, not a gate: upstream
/// registers the rule unconditionally and enablement is name-only.
/// Message: `diagnostic.g.dart` `implicitReopen` (:1525-1540).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "implicit_reopen",
    &[NodeKind::ClassDeclaration, NodeKind::NamedMixinApplication],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `element.metadata.hasReopen` (:49).
    if has_reopen_annotation(c, node) {
        return;
    }
    let flags = dartq_ast::mod_flags(c.unit.ast, node);
    // `element.isSealed` (:50), `element.isMixinClass` (:51).
    if flags & (class_bits::SEALED | class_bits::MIXIN) != 0 {
        return;
    }
    // `element.supertype` (:54-55): the `extends` target, or the mixin
    // application's first type.
    let Some(supertype_name) = extends_name(c, node) else {
        return;
    };
    // `supertype.library != library` (:56): a name that leaves the file
    // is the cross-library face.
    let Some(supertype) = super::hierarchy_row(c, supertype_name) else {
        return;
    };
    let Some(super_flags) = super::hierarchy_flags(c, supertype) else {
        return;
    };
    // `element.isBase` / `element.hasNoModifiers` (:58-89): the
    // written-modifier face.
    let reason = if flags & class_bits::BASE != 0 {
        if super_flags & class_bits::FINAL != 0 {
            Some("final")
        } else if super_flags & class_bits::INTERFACE != 0 {
            Some("interface")
        } else {
            None
        }
    } else if flags
        & (class_bits::INTERFACE | class_bits::BASE | class_bits::SEALED | class_bits::FINAL)
        == 0
    {
        (super_flags & class_bits::INTERFACE != 0).then_some("interface")
    } else {
        None
    };
    let Some(reason) = reason else {
        return;
    };
    // `rule.reportAtToken(nameToken)` (:102-107).
    let Some(name) = super::container_name_node(c, node) else {
        return;
    };
    let class_name = c.lexeme(c.head_token(name)).to_owned();
    let supertype_name = supertype_name.to_owned();
    c.report(
        utf16_token_span(c, c.head_token(name)),
        &["class", &class_name, &supertype_name, reason],
    );
}

/// The direct superclass name: the `extends` clause of a class
/// declaration, the first type row of a mixin application.
fn extends_name<'a>(c: &'a Check<'_>, row: u32) -> Option<&'a str> {
    let supertypes = supertypes(c, row);
    if c.kind(row) == NodeKind::NamedMixinApplication {
        return supertypes.first().map(|(name, _)| *name);
    }
    supertypes
        .into_iter()
        .find(|(_, clause)| *clause == SupertypeClause::Extends)
        .map(|(name, _)| name)
}

/// The written `@reopen` face (`element.metadata.hasReopen`): the
/// declaration's `MetadataStar` left sibling names an unprefixed
/// `reopen` annotation. Production trees keep the star under the unit to
/// the top-level wrapper's left (`parseTopLevelDeclarationImpl` runs
/// `parseMetadataStar` before the prelude opens), so both placements are
/// read; a prefixed spelling (`meta.reopen`) never matches.
fn has_reopen_annotation(c: &Check<'_>, row: u32) -> bool {
    let names_reopen = |star: u32| {
        c.collect_direct(star).into_iter().any(|meta| {
            c.kind(meta) == NodeKind::Metadata
                && c.collect_direct(meta).into_iter().any(|piece| {
                    c.kind(piece) == NodeKind::Identifier
                        && c.lexeme(c.head_token(piece)) == "reopen"
                })
        })
    };
    let Some(parent) = c.unit.ast.parent(row) else {
        return false;
    };
    // (a) the star beside the row itself (member placement, hand-built
    // trees).
    let siblings = c.collect_direct(parent);
    if let Some(pos) = siblings.iter().position(|&sib| sib == row)
        && siblings[..pos]
            .iter()
            .rev()
            .find(|&&sib| c.kind(sib) == NodeKind::MetadataStar)
            .is_some_and(|&star| names_reopen(star))
    {
        return true;
    }
    // (b) the star beside the top-level wrapper (the production
    // top-level placement).
    if c.kind(parent) == NodeKind::TopLevelDeclaration
        && let Some(outer) = c.unit.ast.parent(parent)
    {
        let wrappers = c.collect_direct(outer);
        if let Some(pos) = wrappers.iter().position(|&sib| sib == parent)
            && wrappers[..pos]
                .iter()
                .rev()
                .find(|&&sib| c.kind(sib) == NodeKind::MetadataStar)
                .is_some_and(|&star| names_reopen(star))
        {
            return true;
        }
    }
    false
}
