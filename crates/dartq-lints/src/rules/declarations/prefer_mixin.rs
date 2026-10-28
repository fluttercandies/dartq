use super::{Check, NodeKind, RuleSpec};
use crate::rules::{NONE_TOKEN, same_file_declaration};
use dartq_ast::mod_flags;
use dartq_parser::IdentifierContext;

/// Bridge `mod_bits` class-family `mixin` — the `mixin class`
/// spelling (`isMixinClass`, upstream `element_model.dart`).
const MIXIN_CLASS_BIT: u16 = 1 << 7;

/// Official: `pkg/linter/lib/src/rules/prefer_mixin.dart:37-48` — a
/// `with` clause type whose resolved element is a plain class (neither
/// a `mixin` nor a `mixin class`) reports at the clause type (:44, the
/// written name as the `{0}` argument). The dispatch covers both
/// `with` clause carriers — the class declaration and the named mixin
/// application (`class B = A with M;`).
///
/// Approximation (documented — the same-file hierarchy face): the
/// element resolution folds to [`crate::rules::same_file_declaration`]
/// plus the generic-type-alias walk (`typedef AA = A;` — the 2.19
/// corpus `test_mixedInClass_typAlias`); a name that leaves the file
/// under-reports — the official resolves cross-file and answers
/// `MixinElement` for the SDK mixins (silence), so only the
/// cross-file plain class loses its report. Enum and extension types
/// in a with clause stay silent (neither element kind reports
/// upstream). Message: `diagnostic.g.dart` `preferMixin`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "prefer_mixin",
    &[NodeKind::ClassDeclaration, NodeKind::NamedMixinApplication],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // The `with` clause keyword: payload word 2 on both dispatch kinds
    // (bridge.rs `handle_class_with_clause` /
    // `handle_named_mixin_application_with_clause`); word 3 bounds the
    // clause at a trailing `implements`.
    let payload = c.payload(node);
    let Some(&with_kw) = payload.get(2) else {
        return;
    };
    if with_kw == NONE_TOKEN {
        return;
    }
    let implements_kw = payload.get(3).copied().unwrap_or(NONE_TOKEN);
    for kid in c.collect_direct(node) {
        if c.kind(kid) == NodeKind::DeclarationBody {
            break;
        }
        if c.kind(kid) != NodeKind::Type {
            continue;
        }
        let head = c.head_token(kid);
        if head <= with_kw
            || (implements_kw != NONE_TOKEN && c.unit.ast.token_range(kid).end > implements_kw)
        {
            continue;
        }
        let name = c.lexeme(head);
        match mixin_target(c, name) {
            // `element is MixinElement` (:42).
            Some(row) if c.kind(row) == NodeKind::MixinDeclaration => {}
            // `element.isMixinClass` (:43).
            Some(row)
                if c.kind(row) == NodeKind::ClassDeclaration
                    && mod_flags(c.unit.ast, row) & MIXIN_CLASS_BIT != 0 => {}
            // A plain class reports at the clause type (:44).
            Some(row) if c.kind(row) == NodeKind::ClassDeclaration => {
                c.report(c.utf16_span(kid), &[name]);
            }
            // Enum and extension types, and every name that leaves the
            // file, stay silent.
            _ => {}
        }
    }
}

/// The same-file declaration `name` binds to, through generic-type
/// aliases (`typedef AA = A;` — a bounded hop chain; a cycle or an
/// unresolvable link under-reports).
fn mixin_target(c: &Check<'_>, name: &str) -> Option<u32> {
    if let Some(row) = same_file_declaration(c, name) {
        return Some(row);
    }
    let mut current = name;
    for _ in 0..8 {
        let target = alias_target(c, current)?;
        if let Some(row) = same_file_declaration(c, target) {
            return Some(row);
        }
        if target == current {
            return None;
        }
        current = target;
    }
    None
}

/// The RHS name of the generic type alias named `name` (`typedef`
/// payload word 1 is the `=` token — the old function-typedef form has
/// none and aliases no class; the aliased shape is the direct `Type`
/// child, a function type has none).
fn alias_target<'a>(c: &'a Check<'_>, name: &str) -> Option<&'a str> {
    let root = c.unit.ast.root()?;
    for row in c.unit.ast.children(root) {
        if c.kind(row) != NodeKind::Typedef
            || c.payload(row).get(1).copied().unwrap_or(NONE_TOKEN) == NONE_TOKEN
        {
            continue;
        }
        let kids = c.collect_direct(row);
        let named = kids.iter().any(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && c.payload(kid).first()
                    == Some(&(IdentifierContext::TypedefDeclaration as u32))
                && c.lexeme(c.head_token(kid)) == name
        });
        if !named {
            continue;
        }
        let target = kids
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)?;
        return Some(c.lexeme(c.head_token(target)));
    }
    None
}
