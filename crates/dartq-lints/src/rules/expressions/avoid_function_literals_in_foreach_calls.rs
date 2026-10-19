use super::{Check, Group, NodeKind, RuleSpec, slice};

/// Official: `pkg/linter/lib/src/rules/avoid_function_literals_in_foreach_calls.dart:31-105`
/// — `x.forEach((e) { … })` reports at the method name.
///
/// The receiver type check (`_isIterable`, :40-45) reads through the
/// resolution faces — collection literals and written bare-name types
/// decide (a user class provably outside `Iterable` stays silent), a
/// member receiver reads the bridge static type when mounted, and an
/// opaque receiver keeps the documented presumption. The
/// closure-arity stand-in remains for that opaque face: a
/// multi-parameter closure is the `Map.forEach` shape the syntax
/// cannot see (`map.forEach((k, v) …)` stays silent). A bare
/// `forEach(f)` invocation has no target and never dispatches
/// (:80). The null-aware chain bail (:87) reads the invocation's own
/// marker word plus any null-aware access over the receiver run — the
/// access-chain lookups farther out under-report on exotic chains.
/// Message: `diagnostic.g.dart` `avoidFunctionLiteralsInForeachCalls`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_function_literals_in_foreach_calls",
    group: Group::Recommended,
    problem: "Function literals shouldn't be passed to 'forEach'.",
    correction: "Try using a 'for' loop.",
    kinds: &[NodeKind::Send],
    check,
};

fn check(c: &mut Check<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    // The member face: a trailing `DotAccess` marker proves a target
    // (a bare `forEach(f)` has none, :82-84). A statement-position call
    // may tile without the marker row; the selector identifier from the
    // send parts stands in and the null-aware own-marker word defaults
    // to absent.
    let marker = slice::access_marker(c, &kids, pos, NodeKind::DotAccess);
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // `target != null` (:80): a bare `forEach(f)` invocation has no
    // target and never dispatches.
    let Some(marker) = marker else {
        return;
    };
    let name = kids[head];
    // `node.methodName.token.value() == 'forEach'` (:82-84).
    if c.kind(name) != NodeKind::Identifier || c.lexeme(c.head_token(name)) != "forEach" {
        return;
    }
    // `arguments.isNotEmpty && arguments.first is FunctionExpression`
    // (:85-86): the argument list's subtree roots lead with the call's
    // type-argument wrapper (an empty `NoTypeVariables` row on the
    // common shape), so the first argument is the first root after it.
    let Some(arguments) = arguments else {
        return;
    };
    let raw = c.collect_direct(arguments);
    let closure = raw.iter().copied().find(|&root| {
        !matches!(
            c.kind(root),
            NodeKind::NoTypeVariables | NodeKind::TypeVariables
        )
    });
    let Some(closure) = closure.filter(|&root| c.kind(root) == NodeKind::FunctionExpression)
    else {
        return;
    };
    // The chain guards: no method chaining in the target (:31-43 — the
    // official bails unconditionally, so it runs before the receiver
    // face whose opaque presumption would over-fire the chained shape),
    // no null-aware access below (:87), not inside a cascade (:45-48).
    if has_method_chaining(c, &kids, head) {
        return;
    }
    if chain_has_null_aware(c, &kids, head, Some(marker)) {
        return;
    }
    // The receiver type check (:40-45 `_isIterable`).
    if !receiver_is_iterable(c, &kids, head) {
        return;
    }
    // The closure-arity stand-in for the opaque `Map.forEach` face: a
    // multi-parameter closure is never the flagged shape on valid code.
    // The count reads the flattened parameter rows; the empty group
    // (`()`, zero parameters — invalid for `forEach`) skips, an
    // invalid-code divergence from the official's type-only gate.
    let params = c
        .collect_direct(closure)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters);
    let arity = params.map(|params| {
        c.collect_direct(params)
            .iter()
            .filter(|&kid| c.kind(*kid) == NodeKind::FormalParameter)
            .count()
    });
    if arity != Some(1) {
        return;
    }
    if inside_cascade(c) {
        return;
    }
    let range = c.utf16_span(name);
    c.report(range, &[]);
}

/// The receiver's `Iterable` verdict (`_isIterable`, :40-45 —
/// `type.implementsInterface('Iterable', 'dart.core')`): the shared
/// typed-receiver gate decides where the syntax can (literals,
/// written bare-name types — a user class is provably outside the
/// family; a member receiver through the bridge when mounted), and an
/// unresolvable receiver keeps the documented presumption.
fn receiver_is_iterable(c: &Check<'_>, kids: &[u32], head: usize) -> bool {
    super::static_type_faces::receiver_family(c, c.semantics(), kids, head, slice::ITERABLE)
        .unwrap_or(true)
}

/// `containsNullAwareInvocationInChain` (:87): the invocation's own
/// marker word, or any null-aware access over the receiver run.
fn chain_has_null_aware(c: &Check<'_>, kids: &[u32], head: usize, marker: Option<u32>) -> bool {
    if marker.is_some_and(|marker| c.payload(marker).get(1).copied().unwrap_or(0) != 0) {
        return true;
    }
    kids[..head].iter().any(|&piece| {
        matches!(c.kind(piece), NodeKind::DotAccess | NodeKind::CascadeAccess)
            && c.payload(piece).get(1).copied().unwrap_or(0) != 0
    })
}

/// `_hasMethodChaining` (:31-43): the target carries an invocation — a
/// `Send` piece in the receiver run preceded by an argument group (a
/// chained call; the bare-name reads trail a `NoArguments`).
fn has_method_chaining(c: &Check<'_>, kids: &[u32], head: usize) -> bool {
    kids[..head].iter().enumerate().any(|(i, &piece)| {
        c.kind(piece) == NodeKind::Send && i > 0 && c.kind(kids[i - 1]) == NodeKind::Arguments
    })
}

/// `_isInsideCascade` (:45-48): a cascade section owns the invocation.
fn inside_cascade(c: &Check<'_>) -> bool {
    c.path.iter().any(|&a| c.kind(a) == NodeKind::Cascade)
}
