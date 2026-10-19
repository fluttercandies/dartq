use super::{
    Check, EXTERNAL_MEMBER_BIT, Group, NodeKind, RuleSpec, constructor_name_part,
    container_name_node, modifier_keyword, type_name_echo,
};

/// Official: `pkg/linter/lib/src/rules/use_primary_constructors.dart` —
/// a class or enum without any constructor reports at the container
/// name (the default constructor is the conversion target, :76-79);
/// exactly one non-redirecting generative constructor reports at it —
/// the name part when written (:94-96), else the type-name echo
/// (:97-100). Registration is feature-gated: the primary-constructors
/// `isFeatureEnabled` bail (:35) — pinned on
/// [`crate::engine::Features::primary_constructors`].
///
/// dartq faces (:58-103): a written primary constructor is already the
/// primary form and skips (only one is grammatical); an `external`
/// constructor skips the container (the `ModSet` `external` bit); more
/// than one non-redirecting generative constructor skips (none can be
/// the root, :66-73). A redirect is the last constructor initializer
/// being `this(…)` (the `redirect` extension :109-117): the
/// [`NodeKind::Initializers`] child's last row anchored at `this` —
/// `super(…)`/`assert(…)` initializers are `Initializer` rows too but
/// anchor elsewhere, field initializers are a separate row kind.
/// Factory constructors read through the backward modifier scan (the
/// row anchors at the declared name). Enum members live under the
/// [`NodeKind::EnumBody`] wrapper. Message: `diagnostic.g.dart`
/// `usePrimaryConstructors`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_primary_constructors",
    group: Group::Standalone,
    problem: "Use a primary constructor.",
    correction: "Try using a primary constructor.",
    kinds: &[NodeKind::ClassDeclaration, NodeKind::EnumDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    if !c.unit.features.primary_constructors {
        return; // registration bail (:35)
    }
    let node = c.node();
    // A written primary constructor is the primary form (:61-67/:71-76).
    if c.collect_direct(node)
        .iter()
        .any(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
    {
        return;
    }
    let mut has_constructor = false;
    let mut root: Option<u32> = None;
    for ctor in constructors(c, node) {
        // Classes with an external constructor can't convert (:65-68).
        if dartq_ast::mod_flags(c.unit.ast, ctor) & EXTERNAL_MEMBER_BIT != 0 {
            return;
        }
        has_constructor = true;
        if !is_factory(c, ctor) && !is_redirecting(c, ctor) {
            // More than one non-redirecting generative constructor:
            // none can be the root (:69-73).
            if root.is_some() {
                return;
            }
            root = Some(ctor);
        }
    }
    if !has_constructor {
        // The default constructor is the conversion target (:76-79).
        let Some(name) = container_name_node(c, node) else {
            return;
        };
        c.report(c.utf16_span(name), &[]);
        return;
    }
    let Some(root) = root else {
        return; // no non-redirecting generative constructor (:81-84)
    };
    let range = constructor_name_part(c, root)
        .or_else(|| type_name_echo(c, root))
        .map_or_else(
            || crate::rules::utf16_token_span(c, c.head_token(root)),
            |name| c.utf16_span(name),
        );
    c.report(range, &[]);
}

/// The container's constructor rows: the body wrapper's children
/// (`DeclarationBody` for classes, `EnumBody` for enums) plus the
/// owner's own direct rows (hand-built tables nest constructors
/// directly — the `duplicate_new` dual-shape tolerance).
fn constructors(c: &Check<'_>, container: u32) -> Vec<u32> {
    let mut ctors = vec![];
    for kid in c.collect_direct(container) {
        match c.kind(kid) {
            NodeKind::Constructor => ctors.push(kid),
            NodeKind::DeclarationBody | NodeKind::EnumBody => {
                ctors.extend(
                    c.collect_direct(kid)
                        .into_iter()
                        .filter(|&row| c.kind(row) == NodeKind::Constructor),
                );
            }
            _ => {}
        }
    }
    ctors
}

/// Whether the constructor is spelled `factory` (the generative/factory
/// split :73 — no `ModSet` bit carries it; the row anchors at the name,
/// so the head modifiers sit before it).
fn is_factory(c: &Check<'_>, ctor: u32) -> bool {
    modifier_keyword(c, ctor, "factory").is_some()
}

/// Whether the last constructor initializer is `this(…)` (the `redirect`
/// extension :109-117 — `initializers.lastOrNull is
/// RedirectingConstructorInvocation`).
fn is_redirecting(c: &Check<'_>, ctor: u32) -> bool {
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Initializers)
        .and_then(|initializers| last_row(c, initializers))
        .is_some_and(|last| {
            c.kind(last) == NodeKind::Initializer && c.lexeme(c.head_token(last)) == "this"
        })
}

/// The last child row of an initializer wrapper.
fn last_row(c: &Check<'_>, initializers: u32) -> Option<u32> {
    c.collect_direct(initializers).last().copied()
}
