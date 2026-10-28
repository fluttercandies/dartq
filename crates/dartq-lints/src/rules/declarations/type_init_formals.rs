use super::{PayloadField, Check, NONE_TOKEN, NodeKind, RuleSpec};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/type_init_formals.dart:37-64`
/// — an initializing formal (`this.x` / `super.x`) written with a type
/// annotation reports at the type (:37-48/:52-64), behind the element
/// re-checks: a field formal reports only when its field exists and the
/// written type equals the field's (:41-46), a super formal only when
/// the corresponding super-constructor parameter's type equals (:56-62)
/// — invalid declarations (a nonexistent field, a mismatched type) are
/// compile errors upstream and stay silent.
///
/// The field-formal gate reads the same-file class/enum body: the field
/// must be a non-static instance field of the declaring container with
/// a token-equal written type. Type spellings that differ lexically but
/// denote the same type (a typedef, a prefixed import) under-report.
/// The super-formal face is cross-file element identity (the L1
/// inheritance arm): the invoked super constructor — the written
/// `super.named(...)` initializer's selector, else the unnamed face —
/// resolves over the extends-clause supertype through the bridge, and
/// the corresponding parameter (positional formals by index, named
/// formals by name) must exist with an equal type. Only decidable faces
/// answer: a parameter absent from the table, or a no-argument named /
/// `dynamic` model type against a one-token (plus `?`) written run,
/// stays silent (the compile-error family upstream); parameterized,
/// prefixed and unknown model types are undecidable and keep the
/// lexical report — the valid-code parity posture. `this.x`/`super.x`
/// is the `FormalParameter` `receiver` payload word. Message:
/// `diagnostic.g.dart` `typeInitFormals`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "type_init_formals",
    &[NodeKind::FormalParameter],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `nodeType == null` bails (:38-39); `this.`/`super.` makes the
    // parameter an initializing formal by grammar.
    let Some(receiver) = c.field(node, PayloadField::Receiver) else {
        return;
    };
    if receiver == NONE_TOKEN {
        return;
    }
    // The written type: the Type/FunctionType/RecordType child (a
    // `NoType` child means unwritten).
    let Some(written) = c.collect_direct(node).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    }) else {
        return;
    };
    if c.lexeme(receiver) == "super" {
        // The super-formal element check (:52-64) rides the
        // corresponding super-constructor parameter — cross-file. A
        // provable mismatch (no corresponding parameter, or a decidable
        // type disagreement) is a compile error upstream and stays
        // silent; undecidable faces keep the lexical report.
        if super::super_formal_provably_unmatched(c, node, written) {
            return;
        }
        let range = c.utf16_span(written);
        c.report(range, &[]);
        return;
    }
    // The field-formal gate (:41-46): the field exists as a non-static
    // instance field of this class and the written types are equal.
    let Some(name) = c
        .field(node, PayloadField::Name)
        .filter(|&name| name != NONE_TOKEN)
    else {
        return;
    };
    let Some(field_type) = super::instance_field_written_type(c, c.lexeme(name)) else {
        return;
    };
    if !crate::rules::token_seq_equal(c, written, field_type) {
        return;
    }
    let range = c.utf16_span(written);
    c.report(range, &[]);
}
