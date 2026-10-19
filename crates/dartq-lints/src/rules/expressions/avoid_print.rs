use super::{Check, Group, NodeKind, RuleSpec, query_site, slice};
use crate::rules::utf16_token_span;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/avoid_print.dart` — an invocation whose
/// method name resolves to the dart:core `print` function reports at
/// the method name (:39-43), unless the call sits inside an
/// `if (kDebugMode)` then branch (`_isDebugOnly`, :52-68 — the
/// ancestor walk stops at the enclosing function body) or a comment
/// reference; a bare `print` identifier passed as an argument reports
/// at the identifier (`_validateArgument`, :70-80 — the debug walk
/// does not apply on this face, and the comment reference is the only
/// exemption).
///
/// dartq faces over the production tiling: the bare-invocation
/// callee face — the selector is the bare identifier `print` and
/// carries no workspace binding (the static-type wave's core-name
/// posture: a resolvable same-file, local, type-parameter, or
/// imported workspace declaration shadows the core function and
/// stays silent, the under-report direction; the synthetic SDK face
/// the implicit dart:core import arms at the resolved bare name is
/// the core symbol itself — the `is_sdk_binding` cut keeps it
/// reporting; the shadow join validates the matched reference row's
/// own name, so only a row that IS this `print` use answers the
/// shadow). The tear-off argument face rides the
/// bare-name argument's own four-piece send group dispatch, gated on
/// the group's top-level membership in an `Arguments` row — the
/// `SimpleIdentifier`-argument posture; every other bare use
/// (`print;`, `var x = print;`) is never visited upstream and stays
/// silent. Approximations (documented): member and prefixed callees
/// (`a.print()`, `p.print()`) and member tear-off arguments
/// (`f(C.print)` — the trailing access marker) resolve through the
/// member model this rule does not carry; comment references
/// (no faces from comments) stay silent. A flattened binary-operand
/// argument (`f(x + print)`) still reports — the adopted rows keep
/// the group shape — where upstream's `expression is
/// SimpleIdentifier` skips (the documented over-report corner). The
/// `kDebugMode` exemption gates on the written name — the upstream
/// face resolves package:flutter's foundation getter, which no
/// imported model carries — so a shadowing `kDebugMode` value
/// under-reports. Message: `diagnostic.g.dart` `avoidPrint`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "avoid_print",
    group: Group::Standalone,
    problem: "Don't invoke 'print' in production code.",
    correction: "Try using a logging framework.",
    kinds: &[NodeKind::Send],
    check,
};

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some((head, arguments)) = slice::send_parts(c, &kids, pos) else {
        return;
    };
    if arguments.is_none() {
        // The tear-off argument face (:70-80): this row is the
        // bare-name group's own marker. It reports only when the
        // group stands at the top level of an `Arguments` row — the
        // `SimpleIdentifier`-argument posture (`_validateArgument`
        // takes the argument expression; other bare uses never
        // visit). A trailing access marker is a member tear-off
        // (`f(C.print)` — not a `SimpleIdentifier`, silent). The
        // debug-only walk does not apply here (:70-80 carries only
        // the comment-reference exemption, and comments produce no
        // rows in this substrate).
        let argument_position = c
            .ancestor(1)
            .is_some_and(|parent| c.kind(parent) == NodeKind::Arguments)
            && slice::is_tear_off_group(c, &kids, head)
            && slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_none();
        if !argument_position {
            return;
        }
        let selector_row = kids[head];
        let selector = c.head_token(selector_row);
        if c.lexeme(selector) != "print" {
            return;
        }
        if shadowed(c, analysis, selector_row) {
            return;
        }
        c.report(utf16_token_span(c, selector), &[]);
        return;
    }
    // The bare-callee face: a member receiver (`a.print()`) resolves
    // through the member model this rule does not carry.
    if slice::access_marker(c, &kids, pos, NodeKind::DotAccess).is_some() {
        return;
    }
    let selector_row = kids[head];
    let selector = c.head_token(selector_row);
    if c.lexeme(selector) != "print" {
        return;
    }
    // The dart:core face (:39-40): an unshadowed bare name — the
    // reference row at the callee site must BE the `print` use itself
    // (the row's own name validates the join) for a user declaration
    // to shadow the core function (the `unnecessary_unawaited` gate).
    if shadowed(c, analysis, selector_row) {
        return;
    }
    // The debug-only exemption (:52-68).
    if debug_only(c) {
        return;
    }
    c.report(utf16_token_span(c, selector), &[]);
}

/// Whether the reference row joined at `selector_row`'s site is the
/// `print` use itself bound to a USER declaration — the shadow face.
/// The row's own name validates the join: a reference row naming any
/// other identifier is not this use's resolution (the offset join is
/// positional, so a foreign-name row answers `false` and the core
/// function's unshadowed verdict stands). The synthetic SDK face is
/// the core `print` symbol itself (the engine graph's implicit
/// dart:core import lands the resolved bare name on an unindexed
/// `<dart:core>` row), so the [`dartq_semantic::Analysis::is_sdk_binding`]
/// cut keeps it reporting — only a workspace declaration shadows.
fn shadowed(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, selector_row: u32) -> bool {
    analysis
        .binding_of(query_site(c, selector_row))
        .is_some_and(|binding| {
            !analysis.is_sdk_binding(&binding)
                && binding.target_name == analysis.strings().intern("print")
        })
}

/// The `_isDebugOnly` walk (:52-68): the call sits in a then branch
/// whose condition is the bare name `kDebugMode` at any depth, and
/// the enclosing function body never comes first — the body bail
/// keeps a call inside a closure under the branch from exempting.
fn debug_only(c: &Check<'_>) -> bool {
    let path = c.path;
    for i in (0..path.len().saturating_sub(1)).rev() {
        let row = path[i];
        let below = path[i + 1];
        match c.kind(row) {
            NodeKind::IfStatement => {
                // Production tiling: the then-arm pieces tile flat
                // beside the ParenthesizedCondition, so branch
                // membership is token geometry — `below` sits in the
                // then branch when it starts at/after the condition's
                // end and no `else` token lies between.
                let kids = c.collect_direct(row);
                let Some(cond) = kids
                    .iter()
                    .copied()
                    .find(|&k| c.kind(k) == NodeKind::ParenthesizedCondition)
                else {
                    continue;
                };
                let cond_end = c.unit.ast.token_range(cond).end;
                let below_start = c.unit.ast.token_range(below).start;
                let in_then = below_start >= cond_end
                    && !(cond_end..below_start).any(|t| c.lexeme(t) == "else");

                if in_then {
                    // The condition's pieces tile in the IfStatement's
                    // own window (only the degenerate Send container
                    // parents to the ParenthesizedCondition) — the
                    // bare-name face is the cond-range piece run: one
                    // Identifier, wrappers only beside it.
                    let cond_range = c.unit.ast.token_range(cond);
                    let mut name = None;
                    let bare = c
                        .collect_direct(row)
                        .into_iter()
                        .filter(|&kid| {
                            kid != cond
                                && c.unit.ast.token_range(kid).start >= cond_range.start
                                && c.unit.ast.token_range(kid).end <= cond_range.end
                        })
                        .all(|kid| match c.kind(kid) {
                            NodeKind::Identifier if name.is_none() => {
                                name = Some(kid);
                                true
                            }
                            NodeKind::NoTypeArguments | NodeKind::NoArguments => true,
                            _ => false,
                        });
                    if bare && name.is_some_and(|n| c.lexeme(c.head_token(n)) == "kDebugMode") {
                        return true;
                    }
                }
            }
            // The function-body bail (:62-66): `Block`-bodied and
            // arrow bodies — the only body rows that own expression
            // pieces.
            NodeKind::BlockBody | NodeKind::ExpressionBody => return false,
            _ => {}
        }
    }
    false
}
