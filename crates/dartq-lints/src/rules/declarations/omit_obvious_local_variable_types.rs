use super::{Check, NodeKind, RuleSpec, every_initializer_obvious, for_each_iterable, local_written_type, obvious_spelling, utf16_token_run_span};

/// Official: `pkg/linter/lib/src/rules/omit_obvious_local_variable_types.dart:46-90`
/// — a local variable declaration (statement or C-style for
/// initializer, :41-42/:48-51) with a written type every initializer
/// obviously equals reports at the type (:88); the for-each loop
/// variable face (:52-66) matches a collection-literal iterable whose
/// element type is the written one (:63-64). The equality folds to the
/// obvious-type faces (`initializer.hasObviousType &&
/// initializer.staticType == staticType` — `util/obvious_types.dart)`:
/// literals, `as` targets, collection literals whose elements derive
/// the written type, and constructor references whose head names it. A
/// written `Null` skips (:76); a bare identifier, `dynamic` and every
/// face the matcher cannot name under-report. Message:
/// `diagnostic.g.dart` `omitObviousLocalVariableTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "omit_obvious_local_variable_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement => {
            face_declaration(c, node);
        }
        NodeKind::ForIn => face_for_each(c, node),
        _ => {}
    }
}

/// `_visitVariableDeclarationList` (:74-89): written, not `Null`, and
/// every initializer obvious with the written spelling.
fn face_declaration(c: &mut Check<'_>, node: u32) {
    let Some(written) = local_written_type(c, node) else {
        return;
    };
    if c.lexeme(written.0) == "Null" || c.lexeme(written.0) == "dynamic" {
        return;
    }
    if !every_initializer_obvious(c, node, written) {
        return;
    }
    let Some(range) = utf16_token_run_span(c, written.0..written.1) else {
        return;
    };
    c.report(range, &[]);
}

/// The for-each face (:52-66): the loop variable's written type equals
/// the iterable's element type — the collection spelling's argument
/// run (`List`/`Set`; a map is not an iterable element source).
fn face_for_each(c: &mut Check<'_>, node: u32) {
    let Some(written) = local_written_type(c, node) else {
        return;
    };
    if c.lexeme(written.0) == "Null" || c.lexeme(written.0) == "dynamic" {
        return;
    }
    let Some(iterable) = for_each_iterable(c, node) else {
        return;
    };
    let Some(spelling) = obvious_spelling(c, iterable) else {
        return;
    };
    let length = spelling.len();
    if length < 4 {
        return;
    }
    let element = &spelling[2..length - 1];
    if (spelling[0] == "List" || spelling[0] == "Set")
        && super::spelling_matches(c, written, element)
        && let Some(range) = utf16_token_run_span(c, written.0..written.1)
    {
        c.report(range, &[]);
    }
}
