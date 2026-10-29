use super::{
    Check, NodeKind, PayloadField, RuleSpec, query_site, slice, static_type_faces as faces,
};
use dartq_semantic::model::Ty;

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_bool_literals_in_conditional_expressions.dart`
/// — a conditional expression whose two arms (`unParenthesized`, :43-44)
/// are both statically `bool` (`staticType == boolType`, :46-47 — exact,
/// non-nullable: `dynamic` and `bool?` arms keep the rule silent)
/// reports at the whole conditional when an arm is a `bool` literal.
/// The upstream visitor fires `reportAtNode(node)` once per literal arm
/// (:48-49); the oracle's observable output carries exactly ONE
/// diagnostic at the conditional span even when both arms are literals
/// (`b ? true : false` measures one row), so dartq pins the observable:
/// at most one emission per conditional. The registration is the
/// conditional EXPRESSION (`addConditionalExpression`, :34) —
/// if-statements are not targets.
///
/// dartq faces over the production tiling: the arms are the direct
/// children's head-token segments split at the `?`/`:` payload
/// separators (a bare send's leaf span rides its follower token, so
/// head tokens — not spans — partition; rows left of `?` are the
/// condition), each read through the paren wrappers to its root row.
/// A `true`/`false` literal answers `bool` by its own face; every
/// other arm reads the semantic type — the flow read at a bare
/// identifier (declared or promoted, so a `bool?` promoted inside
/// `if (b)` answers, as the analyzer's staticType does), or the
/// resolution face over the run (a bare send through the callee, a
/// single-name access through the member — cross-file declarations
/// included, an initializer-inferred local through the staged type
/// adoption). A missing or unknown face answers `false`: the gate
/// stays silent for arms with no decided face, and the rule reads
/// the semantic face (the wave posture: no face, no report — the
/// engine contract).
///
/// Message: `diagnostic.g.dart`
/// `avoidBoolLiteralsInConditionalExpressions`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_bool_literals_in_conditional_expressions",
    &[NodeKind::Conditional],
    check,
);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    let node = c.node();
    let Some(question) = c.field(node, PayloadField::Question) else {
        return;
    };
    let Some(colon) = c.field(node, PayloadField::Colon) else {
        return;
    };
    // The then/else arms over the direct children, split at the `?`/`:`
    // separators by head token. A bare tear-off send's wrapper pieces
    // (`NoTypeArguments`, `NoArguments`) ride the `?`/`:` separator
    // tokens, so their heads can straddle the split — keep them with
    // the arm (`head <= colon` for the then-run); `send_parts` walks
    // them back to the selector either way.
    let mut then_run = Vec::new();
    let mut else_run = Vec::new();
    for row in c.collect_direct(node) {
        let head = c.head_token(row);
        if head < question {
            continue;
        }
        if head <= colon {
            then_run.push(row);
        } else {
            else_run.push(row);
        }
    }
    // `thenExp.staticType == boolType && elseExp.staticType == boolType`
    // (:46-47): both arms must be `bool`-typed.
    if !arm_is_bool(c, analysis, &then_run) || !arm_is_bool(c, analysis, &else_run) {
        return;
    }
    // The oracle observable: ONE report at the whole conditional when
    // an arm is a `bool` literal. The upstream visitor fires per
    // literal arm (:48-49), and the measured `dart analyze` output
    // carries a single diagnostic at the conditional span — dartq pins
    // the observable, not the per-arm internal pair.
    let literal_arm = |run: &[u32]| {
        faces::run_root(c, run).is_some_and(|root| c.kind(root) == NodeKind::LiteralBool)
    };
    if !literal_arm(&then_run) && !literal_arm(&else_run) {
        return;
    }
    let Some(range) = conditional_extent(c, node, question) else {
        return;
    };
    c.report(range, &[]);
}

/// The whole conditional's UTF-16 extent: the production row opens at
/// the `?` (the condition completed before the Conditional's slot, so
/// the covered start sits past the condition's first token) and its
/// covered end can swallow an enclosing terminator — the start is the
/// condition run's earliest head, the end the else run's last real
/// row's end.
fn conditional_extent(c: &Check<'_>, node: u32, question: u32) -> Option<core::ops::Range<u32>> {
    let start = condition_start(c, node, question)
        .map(|token| crate::rules::utf16_token_span(c, token).start)?;
    let end = super::prefer_if_null_operators::conditional_end(c, node);
    Some(start..end)
}

/// The condition's first token: the condition completed before the
/// Conditional's slot, so its pieces tile as the owner's rows before
/// the node — statement-run bounded, then token-adjacency bounded back
/// to the `?` ([`slice::contiguous_tail`]). Hand-built trees carry the
/// condition rows among the node's children (`head < question`).
fn condition_start(c: &Check<'_>, node: u32, question: u32) -> Option<u32> {
    let inline: Vec<u32> = c
        .collect_direct(node)
        .into_iter()
        .filter(|&row| c.head_token(row) < question)
        .collect();
    if let Some(token) = slice::run_start_token(c, &inline) {
        return Some(token);
    }
    let parent = c.ancestor(1)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == node)?;
    let run = slice::statement_run(c, &kids[..pos]);
    slice::run_start_token(c, slice::contiguous_tail(c, &run, question))
}

/// The `staticType == boolType` face (:39-40): an exact non-nullable
/// `bool`. A `true`/`false` literal answers by its own face; a bare
/// identifier answers at its flow read (declared or promoted), and
/// every other run shape reads the resolution face (a bare send
/// through the callee, a single-name access through the member).
/// Missing or unknown faces answer `false` — the under-report
/// direction (dynamic, nullable, unresolved, inferred-`var` arms).
fn arm_is_bool(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) -> bool {
    let is_bool = |ty: Option<Ty>| {
        matches!(ty, Some(Ty::Named { name, nullable: false, .. })
            if analysis.strings().resolve(name) == "bool")
    };
    let Some(root) = faces::run_root(c, run) else {
        return false;
    };
    if c.kind(root) == NodeKind::LiteralBool {
        return true;
    }
    if c.kind(root) == NodeKind::Identifier {
        return is_bool(analysis.type_of(query_site(c, root)));
    }
    is_bool(faces::expr_type(c, analysis, run))
}
