use super::{Check, NodeKind, RuleSpec, query_site, slice};
use crate::rules::{type_face, utf16_token_span};

/// `NodeHeader.flags` modifier-mask bit for the class-family
/// `abstract` modifier (bridge `mod_bits`) — the
/// extension gate's abstract-class read.
const CLASS_ABSTRACT_BIT: u16 = 1 << 2;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/no_runtimeType_toString.dart` — a
/// `toString` invocation whose real target is a `runtimeType` access
/// reports at the method name (:67-74), and a string-interpolation
/// expression that *is* a `runtimeType` access (the implicit
/// `toString`) reports at the expression (:59-65). The access faces
/// (:113-120): `this.runtimeType`/`super.runtimeType` syntactically,
/// or a bare `runtimeType` identifier resolving to a getter. The
/// skip posture (`_canSkip`, :76-112): inside an assert, a throw, a
/// catch clause, a mixin, an abstract class, or an extension whose
/// `on` type is not a non-abstract class, the verdict suppresses.
///
/// dartq faces over the production tiling: the member-invocation
/// face over the exact `[this|super, runtimeType]` receiver pair
/// (token adjacency, the dot between) or the bare `runtimeType`
/// receiver whose binding resolves — the implicit-`this` Object
/// member or a same-file container member (the getter read);
/// locals, top-level declarations, type parameters, prefixes, and
/// unresolved names stay silent (the under-report posture).
/// Approximations (documented): the interpolation faces stay silent
/// — interpolation segments carry no AST rows (the token-layer
/// carry); a same-file top-level getter named `runtimeType`
/// under-reports (a `Decl` binding), and the extension `on` gate
/// reads the same-file declarations only, so core and cross-file
/// `on` types skip. Message: `diagnostic.g.dart`
/// `noRuntimetypeTostring`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_runtimetype_tostring",
    &[NodeKind::Send],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, _arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // A real target is required (:68-69): a bare `toString()` (the
    // implicit-`this` invocation) carries none.
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_none() {
        return;
    }
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    // Only `toString` (:67) — `toStringDeep`/`toStringShort` are not
    // the registered faces.
    if c.lexeme(selector) != "toString" {
        return;
    }
    if !is_runtime_type_access(c, analysis, &kids[..head], selector) {
        return;
    }
    // The skip posture (:76-112).
    let access = slice::access_run(c, &kids[..head]);
    let run_start = access.first().map(|&row| c.head_token(row));
    if can_skip(c, run_start, selector) {
        return;
    }
    c.report(utf16_token_span(c, selector), &[]);
}

/// The `_isRuntimeTypeAccess` read (:113-120) over the receiver run
/// before the `toString` selector: one bare `runtimeType` identifier
/// directly before the dot whose binding resolves to a getter face
/// (the implicit-`this` Object member, or a same-file container
/// member), or the `[this|super, runtimeType]` pair — syntactic
/// upstream (:108-112).
fn is_runtime_type_access(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    selector: u32,
) -> bool {
    match slice::access_run(c, run)[..] {
        [only]
            if c.kind(only) == NodeKind::Identifier
                && c.lexeme(c.head_token(only)) == "runtimeType"
                && c.head_token(only) + 2 == selector =>
        {
            matches!(
                analysis.binding_of(query_site(c, only)).map(|b| b.kind),
                Some(dartq_semantic::BindingKind::Opaque | dartq_semantic::BindingKind::Member)
            )
        }
        [target, name]
            if c.kind(name) == NodeKind::Identifier
                && c.lexeme(c.head_token(name)) == "runtimeType"
                && matches!(
                    c.kind(target),
                    NodeKind::ThisExpression | NodeKind::SuperExpression
                )
                && c.head_token(target) + 2 == c.head_token(name) =>
        {
            true
        }
        _ => false,
    }
}

/// The `_canSkip` walk (:76-112): the nearest matching ancestor
/// decides — assert, throw, catch clause, mixin, and abstract class
/// skip; an extension skips unless its `on` type names a same-file
/// non-abstract class; a concrete class keeps the verdict.
fn can_skip(c: &Check<'_>, run_start: Option<u32>, selector: u32) -> bool {
    for &row in c.path.iter().rev().skip(1) {
        match c.kind(row) {
            NodeKind::Assert
            | NodeKind::ThrowExpression
            | NodeKind::CatchClause
            | NodeKind::MixinDeclaration => return true,
            NodeKind::ClassDeclaration => {
                if dartq_ast::mod_flags(c.unit.ast, row) & CLASS_ABSTRACT_BIT != 0 {
                    return true;
                }
            }
            NodeKind::ExtensionDeclaration if !on_concrete_class(c, row) => {
                return true;
            }
            _ => {}
        }
    }
    // The production tiling: throw/assert are childless adopted
    // containers — the operand tiles beside them in the enclosing
    // window, and the container row anchors at the keyword. The
    // context row whose extent reaches the receiver run decides:
    // it starts at/before the selector and ends at the run's first
    // token (a later statement's row starts after the run and never
    // reaches back).
    if let Some(run_start) = run_start {
        let node = c.node();
        if let Some(parent) = c.unit.ast.parent(node) {
            for row in c.collect_direct(parent) {
                if matches!(c.kind(row), NodeKind::Assert | NodeKind::ThrowExpression) {
                    let range = c.unit.ast.token_range(row);
                    if range.start <= selector && range.end >= run_start {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether the extension's `on` type names a same-file non-abstract
/// class — the official `extendedElement is ClassElement &&
/// !isAbstract` read (:93-105) over the same-file declarations.
/// Cross-file, core, and mixin/enum/typedef `on` types answer
/// `false` (skip).
fn on_concrete_class(c: &Check<'_>, extension: u32) -> bool {
    // The `on` type row sits before the body (the header walk's
    // window).
    let Some(on_type) = c
        .collect_direct(extension)
        .into_iter()
        .take_while(|&kid| {
            !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody)
        })
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    else {
        return false;
    };
    let Some(name) = type_face(c, on_type).name else {
        return false;
    };
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let kids = c.collect_direct(top);
        let class_row = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::ClassDeclaration);
        let class_name = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::Identifier);
        if let (Some(class_row), Some(class_name)) = (class_row, class_name)
            && c.lexeme(c.head_token(class_name)) == name
        {
            return dartq_ast::mod_flags(c.unit.ast, class_row) & CLASS_ABSTRACT_BIT == 0;
        }
    }
    false
}
