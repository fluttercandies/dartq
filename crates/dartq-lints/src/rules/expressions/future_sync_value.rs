use super::{Check, NodeKind, RuleSpec, slice, static_type_faces};
use crate::rules::{same_file_declaration, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/future_sync_value.dart` — a `Future.value`
/// creation whose first argument is not future-typed reports at the
/// `value` constructor-name token (`_report`, `reportAtToken`): the
/// instance type must be `Future` (`isDartAsyncFuture`), the class
/// must declare `syncValue`, and the first argument's type must be
/// neither `Future`/`FutureOr` nor `dynamic` — everything else,
/// including an unread type, reports (the visitor :44-105). The rule
/// registers below the `syncValue` constructor's introduction
/// (`constructorSince = Version(3, 10, 0)`). The faces upstream
/// registers: instance creations and dot-shorthand constructor
/// invocations (a plain `Future.value(1)` rewrites to the former
/// through constructor resolution).
///
/// dartq faces over the production tiling: the dispatch rows are the
/// three creation shapes — the `new`-keyword row, the generic implicit
/// creation (`Future<int>.value(1)`), and the invocation send (a plain
/// `Future.value(1)` stays a send here — the tiles are disjoint). The
/// `isDartAsyncFuture` + `syncValue`-exists faces fold into the
/// dart:io-style identity: the callee head is the spelled `Future`,
/// bound to nothing same-file (a user `Future` fails both upstream
/// gates), and the version gate carries the constructor's existence
/// (the 3.15 oracle's `Future` has it). The argument type reads the
/// static-type faces. Approximations (documented): the dot-shorthand
/// face (`.value(1)`) is not tiled into the dispatch here and
/// under-reports; a generic receiver (`Future<int>.value`) takes the
/// implicit-creation face; cross-file callee types the faces cannot
/// read report through the null-type posture. Message:
/// `diagnostic.g.dart` `futureSyncValue`.
/// The spec is a hand-written literal: the 3.13.7 conformance pin's
/// linter does not carry this rule (it exists only in newer upstream
/// mains), so the generated metadata table has no row to resolve — the
/// same posture as the [`crate::registry`] `META_EXEMPT` faces. The
/// texts are the rule's established messages, kept byte-identical.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "future_sync_value",
    group: crate::registry::Group::Standalone,
    problem: "For synchronous values, `Future.syncValue` is more performant.",
    correction: "Try replacing `Future.value` with `Future.syncValue`.",
    kinds: &[
        NodeKind::NewExpression,
        NodeKind::ImplicitCreation,
        NodeKind::Send,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    // `constructorSince = Version(3, 10, 0)`: the effective library
    // version below it never registers the visitor; the `syncValue`
    // constructor's existence folds into the same gate.
    if let Some(version) = c.unit.version
        && (version.major, version.minor) < (3, 10)
    {
        return;
    }
    match c.kind(c.node()) {
        NodeKind::Send => send_face(c),
        _ => creation_face(c),
    }
}

/// The invocation-send face (a plain `Future.value(1)`).
fn send_face(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    // A tear-off (`Future.value` as a reference) is no invocation.
    let Some(arguments) = arguments else {
        return;
    };
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    let marker = slice::access_marker(c, &kids, pos, NodeKind::DotAccess);
    if marker.is_none() {
        return;
    }
    let Some(receiver) = static_type_faces::single_receiver(c, &kids[..head], selector) else {
        return;
    };
    if c.lexeme(c.head_token(receiver)) != "Future"
        || c.lexeme(selector) != "value"
        // The `isDartAsyncFuture` identity: a same-file `Future`
        // binding shadows the SDK class (which lacks `syncValue` —
        // both upstream gates keep it silent).
        || crate::rules::expressions::same_file::binding(c, receiver, "Future").is_some()
    {
        return;
    }
    if first_argument_reports(c, analysis, arguments) {
        c.report(utf16_token_span(c, selector), &[]);
    }
}

/// The structured-creation face (`new Future.value(1)` and the
/// generic `Future<int>.value(1)`).
fn creation_face(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    if same_file_declaration(c, "Future").is_some() {
        return;
    }
    let Some(arguments) = c
        .collect_direct(node)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Arguments)
    else {
        return;
    };
    // `new` opens the keyword shape's span; the implicit creation
    // begins at the class name itself.
    let from = if c.kind(node) == NodeKind::NewExpression {
        c.head_token(node) + 1
    } else {
        c.head_token(node)
    };
    let args_start = c.head_token(arguments);
    let (mut class_name, mut selector) = (None, None);
    let mut token = from;
    while token < args_start {
        if is_identifier_lexeme(c, token) {
            if class_name.is_none() {
                class_name = Some(token);
            } else if token > 0 && c.lexeme(token - 1) == "." {
                selector = Some(token);
            }
        }
        token += 1;
    }
    // `constructorName.name?.token` — an unnamed creation carries no
    // token and never reports.
    if class_name.is_none_or(|t| c.lexeme(t) != "Future")
        || selector.is_none_or(|t| c.lexeme(t) != "value")
    {
        return;
    }
    if first_argument_reports(c, analysis, arguments) {
        c.report(utf16_token_span(c, selector.expect("checked above")), &[]);
    }
}

/// The first-argument face (`arguments.firstOrNull`): `Future`/
/// `FutureOr`/`dynamic` exempt, everything else — an unread type
/// included (the upstream null-staticType posture) — reports.
fn first_argument_reports(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    arguments: u32,
) -> bool {
    let pieces = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &pieces);
    let Some(range) = ranges.first() else {
        return false;
    };
    let named = c.kind(pieces[range.end - 1]) == NodeKind::NamedArgument;
    let first = if named { range.start + 1 } else { range.start };
    let last = if named { range.end - 2 } else { range.end - 1 };
    if first > last {
        return false;
    }
    // An untyped parameter's flow row answers `Unknown` — the
    // substrate's undecided posture, which the oracle reads as
    // `dynamic` (exempt); a typeless row (`None`) is the unread face
    // and reports.
    match static_type_faces::expr_type(c, analysis, &pieces[first..=last]) {
        Some(dartq_semantic::model::Ty::Named { name, .. }) => {
            let spelled = analysis.strings().resolve(name);
            !(spelled == "Future" || spelled == "FutureOr")
        }
        Some(dartq_semantic::model::Ty::Dynamic) | Some(dartq_semantic::model::Ty::Unknown) => {
            false
        }
        _ => true,
    }
}

/// Whether the token is a plain identifier (keywords excluded — `new`
/// never names the class here).
fn is_identifier_lexeme(c: &Check<'_>, token: u32) -> bool {
    c.unit
        .tokens
        .get(token as usize)
        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
}
