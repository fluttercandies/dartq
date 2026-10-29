use dartq_semantic::model::Ty;

use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, static_type_faces as faces,
};
use crate::rules::{MemberQuery, body_members, type_face, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_null_aware_operator_on_extension_on_nullable.dart`
/// — a null-aware access whose member resolves to an extension whose
/// extended type is nullable reports at the `?.` (the registrations
/// :39-42 are index expressions, method invocations, and property
/// accesses; the verdict :66-91 also demands the target be an
/// extension override or carry a nullable static type :98-106).
///
/// dartq faces over the production tiling: the null-aware token is the
/// access marker's payload dot (`?.`), read on the invocation send's
/// trailing marker or on a bare property access; the target face is
/// the single-name receiver's static type under the `?`-suffix read
/// (`Ty::Null` stays out — the official nullable check is the suffix,
/// which `NullType` does not carry); the extension face is the
/// same-file structural walk — the extension's `on` clause type row
/// carries the trailing `?`, and its body names the selector.
/// Approximations (documented, under-report): extension overrides,
/// index expressions, write contexts, chained receivers, and
/// cross-library extensions stay silent, as does a receiver whose
/// static type does not resolve. Message: `diagnostic.g.dart`
/// `unnecessaryNullAwareOperatorOnExtensionOnNullable`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_null_aware_operator_on_extension_on_nullable",
    &[NodeKind::Send, NodeKind::DotAccess],
    check,
);

fn check(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::Send => send_face(c, analysis),
        NodeKind::DotAccess => access_face(c, analysis),
        _ => {}
    }
}

/// The member-invocation face (`r?.m(...)`): the send's trailing
/// access marker carries the `?.` and the selector sits before the
/// argument list.
fn send_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, _arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    let Some(marker) = slice::access_marker(c, &kids, pos, NodeKind::DotAccess) else {
        return;
    };
    let selector = c.head_token(kids[head]);
    let Some(receiver) = faces::single_receiver(c, &kids[..head], selector) else {
        return;
    };
    verdict(c, analysis, receiver, selector, null_aware_dot(c, marker));
}

/// The bare property-access face (`r?.p`): the dispatched marker owns
/// the access; an invocation's trailing marker dispatches on the send
/// face instead.
fn access_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    if pos > 0 && c.kind(kids[pos - 1]) == NodeKind::Send {
        return;
    }
    let dot = c
        .field(c.node(), PayloadField::Operator)
        .filter(|&dot| c.lexeme(dot) == "?.");
    let Some(selector) = c.unit.ast.token_range(c.node()).next_back() else {
        return;
    };
    let Some(receiver) = faces::single_receiver(c, &kids[..pos], selector) else {
        return;
    };
    verdict(c, analysis, receiver, selector, dot);
}

/// The marker's access token, `None` unless null-aware (`?.`).
fn null_aware_dot(c: &Check<'_>, marker: u32) -> Option<u32> {
    let dot = c.field(marker, PayloadField::Operator)?;
    (c.lexeme(dot) == "?.").then_some(dot)
}

/// The verdict (:66-91): the target's static type is nullable and the
/// selector resolves to a same-file extension member whose `on` type
/// is nullable-marked; the report sits on the `?.` token.
fn verdict(
    c: &mut Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    receiver: u32,
    selector: u32,
    dot: Option<u32>,
) {
    let Some(dot) = dot else {
        return;
    };
    let Some(Ty::Named { nullable: true, .. }) = analysis.type_of(query_site(c, receiver)) else {
        return;
    };
    if !extension_on_nullable_member(c, c.lexeme(selector)) {
        return;
    }
    c.report(utf16_token_span(c, dot), &[]);
}

/// Whether some same-file extension declares `selector` over a
/// nullable-marked `on` type: the header's `Type` row (before the
/// body, the `supertypes` walk's window) carries the trailing `?`, and
/// the body names the member.
fn extension_on_nullable_member(c: &Check<'_>, selector: &str) -> bool {
    let Some(root) = c.unit.ast.root() else {
        return false;
    };
    for top in c.collect_direct(root) {
        if c.kind(top) != NodeKind::TopLevelDeclaration {
            continue;
        }
        let Some(ext) = c
            .collect_direct(top)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ExtensionDeclaration)
        else {
            continue;
        };
        let on_nullable = c
            .collect_direct(ext)
            .into_iter()
            .take_while(|&kid| {
                !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody)
            })
            .find(|&kid| c.kind(kid) == NodeKind::Type)
            .is_some_and(|row| type_face(c, row).nullable);
        if !on_nullable {
            continue;
        }
        // The member-name face is context-keyed: a method/getter/setter
        // names through its `MethodDeclaration`-context `Identifier`
        // (the first direct `Identifier` child can be the return type —
        // `member_matches` reads the declared-name context).
        if body_members(c, ext).into_iter().any(|member| {
            !crate::rules::member_matches(c, member, MemberQuery::Getter, selector).is_empty()
                || !crate::rules::member_matches(c, member, MemberQuery::Setter, selector)
                    .is_empty()
                || !crate::rules::member_matches(c, member, MemberQuery::Method, selector)
                    .is_empty()
        }) {
            return true;
        }
    }
    false
}
