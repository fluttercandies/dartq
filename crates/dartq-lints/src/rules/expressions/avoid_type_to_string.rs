use dartq_semantic::model::Ty;

use super::{Check, NodeKind, RuleSpec, query_site, slice, static_type_faces as faces};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_type_to_string.dart` — a `toString`
/// invocation whose receiver's static type is a subtype of `Type`
/// reports at the method name (:80-86, :98-114 — the result may be
/// minified in production). The dartq face is the member invocation
/// over an exact `Type`-typed receiver: the receiver's read row must
/// name the written `Type` (non-nullable — a `Type?` receiver is not a
/// `Type` subtype and stays silent, matching the official subtype
/// read). Approximations (documented, under-report): type-literal
/// receivers (`int.toString()`), `runtimeType` member chains, the
/// bare-name tear-off faces (:116-131), and the enclosing-class
/// `thisType` face stay silent. Message: `diagnostic.g.dart`
/// `avoidTypeToString`.
pub(crate) static SPEC: RuleSpec = crate::spec!("avoid_type_to_string", &[NodeKind::Send], check,);

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
    // The receiver-present face (:80-86): the whole-access marker.
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_none() {
        return;
    }
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    if c.lexeme(selector) != "toString" {
        return;
    }
    let Some(receiver) = faces::single_receiver(c, &kids[..head], selector) else {
        return;
    };
    // The subtype face (:105): the receiver reads as the written
    // `Type` — non-nullable (`Type?` is not a `Type` subtype).
    let Some(Ty::Named {
        name,
        nullable: false,
        ..
    }) = analysis.type_of(query_site(c, receiver))
    else {
        return;
    };
    if analysis.strings().resolve(name) != "Type" {
        return;
    }
    let range = utf16_token_span(c, selector);
    c.report(range, &[]);
}
