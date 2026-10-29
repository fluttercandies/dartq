use super::{
    Check, NodeKind, RuleSpec, every_initializer_obvious, fields_written_type, utf16_token_run_span,
};

/// Official: `pkg/linter/lib/src/rules/omit_obvious_property_types.dart:48-72`
/// — a top-level or field variable list (:43-44) with a written type
/// every initializer obviously equals reports at the type (:71). The
/// matcher family is [`omit_obvious_local_variable_types`]' over the
/// field row shape (the row anchors at the keyword or type start). A
/// written `Null`/`dynamic` skips; every initializer the matcher
/// cannot name under-reports. Message: `diagnostic.g.dart`
/// `omitObviousPropertyTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "omit_obvious_property_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    // `visitFieldDeclaration` + `visitTopLevelVariableDeclaration`
    // (:50-55).
    if !matches!(c.kind(parent), NodeKind::TopLevelFields | NodeKind::Fields) {
        return;
    }
    let Some(written) = fields_written_type(c, node) else {
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
