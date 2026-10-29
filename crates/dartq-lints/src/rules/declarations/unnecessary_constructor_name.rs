use super::{Check, NodeKind, PayloadField, RuleSpec, slice};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_constructor_name.dart`
/// — a written `.new` constructor name reports at the name token. Faces
/// (registry :35-39): the constructor declaration (:41-57), the primary
/// constructor declaration (:70-80), and the instance creation
/// (:59-62). The element guards are approximated on the declaration
/// slices: the duplicate bail counts every constructor whose written
/// name is absent or `new` (the canonical unnamed name, :43-50) —
/// non-constructor members never count — and the extension-type
/// exemption (:51-58) bails on a secondary constructor whose primary
/// constructor carries no name. The instance-creation face (:59-62)
/// reads the owner slice (`rules::slice`): the `.new` member invocation
/// over a bare receiver, `Foo.new(…)` — a constructor tear-off is no
/// instance creation. A prefixed receiver (`p.Foo.new(…)`) reads the
/// written import surface: a unit import prefix at the head makes the
/// two-segment head a prefixed type position. The official `_check` is
/// purely lexical (the name token's lexeme, no resolution gate), so the
/// erroneous `p.Missing.new(…)` reports on both sides alike — an
/// agreement face. Message: `diagnostic.g.dart`
/// `unnecessaryConstructorName`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_constructor_name",
    &[
        NodeKind::Constructor,
        NodeKind::PrimaryConstructor,
        NodeKind::Send,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::Constructor => check_declaration(c, node),
        NodeKind::PrimaryConstructor => check_primary(c, node),
        NodeKind::Send => check_creation(c),
        _ => {}
    }
}

/// The written constructor name: the identifier in the
/// method-continuation context (the class-name echo carries the method
/// context), or `None` when unnamed.
fn constructor_name(c: &Check<'_>, node: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.direct_children(node).iter().copied().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx)
    })
}

/// The written primary-constructor name (the primary-constructor
/// context identifier), or `None` when unnamed.
fn primary_name(c: &Check<'_>, node: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
    c.direct_children(node).iter().copied().find(|&kid| {
        c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx)
    })
}

/// Whether more than one constructor of the type carries the canonical
/// unnamed name (`constructors.where((c) => c.name == 'new')`, :43-50):
/// every unnamed constructor and every `new`-spelled one counts, on the
/// body's secondary constructors and the primary itself.
fn duplicate_new(c: &Check<'_>, owner: u32, primary: Option<u32>) -> bool {
    let mut count = 0usize;
    let members: Vec<u32> = c.direct_children(owner).iter().copied().collect();
    for member in members {
        match c.kind(member) {
            NodeKind::Constructor => {
                if is_unnamed_or_new(c, member) {
                    count += 1;
                }
            }
            NodeKind::DeclarationBody => {
                let ctors: Vec<u32> = c
                    .direct_children(member)
                    .iter()
                    .copied()
                    .filter(|&kid| c.kind(kid) == NodeKind::Constructor)
                    .collect();
                count += ctors
                    .into_iter()
                    .filter(|&ctor| is_unnamed_or_new(c, ctor))
                    .count();
            }
            _ => {}
        }
    }
    if let Some(primary) = primary
        && primary_name(c, primary).is_none_or(|name| c.lexeme(c.head_token(name)) == "new")
    {
        count += 1;
    }
    count > 1
}

/// No written name (the unnamed form, canonically `new`) or spelled
/// `new`.
fn is_unnamed_or_new(c: &Check<'_>, ctor: u32) -> bool {
    match constructor_name(c, ctor) {
        None => true,
        Some(name) => c.lexeme(c.head_token(name)) == "new",
    }
}

/// The owner type's written primary constructor, if any.
fn owner_primary(c: &Check<'_>) -> Option<u32> {
    let owner = c.ancestor(2)?;
    c.direct_children(owner)
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
}

fn check_declaration(c: &mut Check<'_>, node: u32) {
    let Some(name) = constructor_name(c, node) else {
        return; // unnamed constructor: `node.name == null`
    };
    if c.lexeme(c.head_token(name)) != "new" {
        return;
    }
    // The extension-type exemption (:51-58): a secondary constructor
    // whose primary constructor is unnamed needs the qualified form.
    if let Some(owner) = c.ancestor(2)
        && c.kind(owner) == NodeKind::ExtensionTypeDeclaration
        && owner_primary(c).is_some_and(|p| primary_name(c, p).is_none())
    {
        return;
    }
    // Duplicate unnamed constructors already error (:45-52).
    let body = c.ancestor(1).expect("constructor has an owner");
    if duplicate_new(c, body, owner_primary(c)) {
        return;
    }
    let Some(start) = c.utf16_of_token(c.head_token(name)) else {
        return;
    };
    c.report(start..start, &[]);
}

/// `visitPrimaryConstructorDeclaration` (:70-80): a written `.new`
/// primary-constructor name reports at the name token, unless the
/// duplicate count fires.
fn check_primary(c: &mut Check<'_>, node: u32) {
    let Some(name) = primary_name(c, node) else {
        return; // the unnamed primary: `constructorName == null`
    };
    if c.lexeme(c.head_token(name)) != "new" {
        return;
    }
    let owner = c.ancestor(1).expect("primary constructor has an owner");
    if duplicate_new(c, owner, Some(node)) {
        return;
    }
    let Some(start) = c.utf16_of_token(c.head_token(name)) else {
        return;
    };
    c.report(start..start, &[]);
}

fn check_creation(c: &mut Check<'_>) {
    // The production tiling keeps no children on the send: the `.new`
    // invocation reads the owner slice — argument list at `pos - 1`
    // (`None` is a constructor tear-off, not an instance creation),
    // the `.new` name head before that, and the bare-receiver tear-off
    // group behind the name.
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, args)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    let Some(_) = args else {
        return;
    };
    if c.kind(kids[head]) != NodeKind::Identifier || c.lexeme(c.head_token(kids[head])) != "new" {
        return;
    }
    // The bare receiver: `Foo.new(…)`.
    if let Some(group_start) = head.checked_sub(4)
        && slice::is_tear_off_group(c, &kids, group_start)
    {
        let Some(start) = c.utf16_of_token(c.head_token(kids[head])) else {
            return;
        };
        c.report(start..start, &[]);
        return;
    }
    // The prefixed face: `p.Foo.new(…)` — the head segment is an import
    // prefix of this unit (the written import surface), so the
    // two-segment head is a prefixed type position and the `.new` name
    // reports. The parse shape alone cannot distinguish the chain
    // (`local.b.new(…)` parses alike); a third segment is not a type.
    // The official `_check` is lexical, so the erroneous
    // `p.Missing.new(…)` reports on both sides alike.
    let new_token = c.head_token(kids[head]);
    if new_token < 4
        || !super::is_identifier_token(c, new_token - 2)
        || c.lexeme(new_token - 1) != "."
        || !super::is_identifier_token(c, new_token - 4)
        || c.lexeme(new_token - 3) != "."
        || new_token
            .checked_sub(5)
            .is_some_and(|prev| c.lexeme(prev) == ".")
        || !super::written_import_prefix(c, c.lexeme(new_token - 4))
    {
        return;
    }
    let Some(start) = c.utf16_of_token(new_token) else {
        return;
    };
    c.report(start..start, &[]);
}
