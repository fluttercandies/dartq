//! #3948 pins — the annotation verdict's two faces and the formal
//! default-value context channel.
//!
//! Three faces landed together (the brief's items 1–3):
//!
//! * **The kind arm** (item 1, `check_annotation` /
//!   `emit_pending_annotation`): an annotation that resolves but is no
//!   valid annotation target reports the oracle's kind row —
//!   `invalid_annotation` (`@int`, `@f`, `@x` non-const, `@C` named-ctor
//!   miss), `non_constant_annotation_constructor` (`@C` / `@C()` on a
//!   non-const constructor), `no_annotation_constructor_arguments` (`@C`
//!   on a const unnamed constructor with no `(`) — upstream
//!   `annotation_resolver.dart` `_resolve` +
//!   `constant_verifier.dart` `visitAnnotation`.
//!
//! * **Consumption-keyed suppression** (item 2): an annotation verdict
//!   emits only when a surviving declaration consumes the metadata — the
//!   pending entry rides `PendingAnnotation.node` (latched at
//!   `end_metadata`) and emits at `note_decl_site`'s drain, so an
//!   annotation dropped with a discarded declaration reports nothing.
//!
//! * **The formal default-value channel** (item 3): a formal's own `=` /
//!   `:` separator inside an open formal list donates the formal's
//!   declared face — `literal_ctx_ty` reads `frame.var_types` for a
//!   `LocalKind::Parameter` binding left of the separator.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rows of one code, in source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<String> {
    let mut at: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    at.sort_by_key(|(offset, _)| *offset);
    at.into_iter().map(|(_, text)| text).collect()
}

// —— item 1: the kind arm ——————————————————————————————————————————

/// `@int` resolves to a class type whose unnamed constructor is the
/// implicit (non-const) default — upstream's `_classGetter` answers the
/// unnamed `ConstructorElement`, and `constant_verifier` reports
/// `non_constant_annotation_constructor` for its `!isConst`.
#[test]
fn annotation_int_class_nonconst_reports() {
    let report = report_ops("@int class C {}\n");
    assert_eq!(
        rows_of(&report, "non_constant_annotation_constructor"),
        ["Annotation creation can only call a const constructor."]
    );
}

/// `@C` on a class whose only constructor is const and takes no `(` —
/// `constant_verifier`'s `argumentList == null` arm reports
/// `no_annotation_constructor_arguments`.
#[test]
fn annotation_const_class_no_args_reports() {
    let report = report_ops("class C { const C(); }\n@C class D {}\n");
    assert_eq!(
        rows_of(&report, "no_annotation_constructor_arguments"),
        ["Annotation creation must have arguments."]
    );
}

/// `@C()` on the same const unnamed constructor stays silent — a const
/// constructor invocation with an argument list.
#[test]
fn annotation_const_class_with_args_silent() {
    let report = report_ops("class C { const C(); }\n@C() class D {}\n");
    assert_eq!(
        rows_of(&report, "non_constant_annotation_constructor"),
        Vec::<String>::new()
    );
    assert_eq!(
        rows_of(&report, "no_annotation_constructor_arguments"),
        Vec::<String>::new()
    );
    assert_eq!(rows_of(&report, "invalid_annotation"), Vec::<String>::new());
}

/// `@x` on a const top-level variable with no `(` stays silent — the
/// `VariableElement`/`PropertyAccessorElement` arm.
#[test]
fn annotation_const_top_var_silent() {
    let report = report_ops("const x = 1;\n@x class C {}\n");
    assert_eq!(rows_of(&report, "invalid_annotation"), Vec::<String>::new());
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        Vec::<String>::new()
    );
}

/// `@x` on a non-const top-level variable reports `invalid_annotation`.
#[test]
fn annotation_nonconst_top_var_invalid() {
    let report = report_ops("final x = 1;\n@x class C {}\n");
    assert_eq!(
        rows_of(&report, "invalid_annotation"),
        ["Annotation must be either a const variable reference or const constructor invocation."]
    );
}

/// `@f` on a top-level function — the fall-through `invalidAnnotation`.
#[test]
fn annotation_top_fn_invalid() {
    let report = report_ops("void f() {}\n@f class C {}\n");
    assert_eq!(
        rows_of(&report, "invalid_annotation"),
        ["Annotation must be either a const variable reference or const constructor invocation."]
    );
}

/// `@C.named` where `named` is no constructor reports `invalid_annotation`
/// — `_classGetter`'s `getNamedConstructor` recovery misses.
#[test]
fn annotation_named_ctor_miss_invalid() {
    let report = report_ops("class C { const C(); }\n@C.named class D {}\n");
    assert_eq!(
        rows_of(&report, "invalid_annotation"),
        ["Annotation must be either a const variable reference or const constructor invocation."]
    );
}

/// `@C.named()` on a const named constructor stays silent.
#[test]
fn annotation_named_ctor_const_silent() {
    let report = report_ops("class C { const C.named(); }\n@C.named() class D {}\n");
    assert_eq!(rows_of(&report, "invalid_annotation"), Vec::<String>::new());
    assert_eq!(
        rows_of(&report, "non_constant_annotation_constructor"),
        Vec::<String>::new()
    );
}

/// `@C.named` (no `(`) on a const named constructor reports
/// `no_annotation_constructor_arguments`.
#[test]
fn annotation_named_ctor_const_no_args_reports() {
    let report = report_ops("class C { const C.named(); }\n@C.named class D {}\n");
    assert_eq!(
        rows_of(&report, "no_annotation_constructor_arguments"),
        ["Annotation creation must have arguments."]
    );
}

// —— item 2: consumption-keyed suppression ——————————————————————————

/// `@a` on a dropped `class` head reports nothing — the pending entry
/// never drains (the recovery discarded the declaration), matching the
/// oracle's zero rows.
#[test]
fn annotation_dropped_class_suppresses() {
    let report = report_ops("@a(\n");
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        Vec::<String>::new()
    );
    assert_eq!(rows_of(&report, "invalid_annotation"), Vec::<String>::new());
}

/// `@a` before a surviving `final` variable consumes the metadata — the
/// verdict still emits (the declaration's consumption), now against the
/// resolved element (`_localVariable`'s invalid arm on the non-const
/// `final`), matching the oracle's `invalid_annotation`.
#[test]
fn annotation_left_paren_final_consumed_invalid() {
    let report = report_ops("@a(\nfinal a = 0;\n");
    assert_eq!(
        rows_of(&report, "invalid_annotation"),
        ["Annotation must be either a const variable reference or const constructor invocation."]
    );
}

/// `@a` before a surviving `const` variable — the consumed verdict's
/// `_localVariable` arm answers silent on `isConst` (the oracle's zero
/// rows).
#[test]
fn annotation_left_paren_const_consumed_silent() {
    let report = report_ops("@a(\nconst a = 0;\n");
    assert_eq!(rows_of(&report, "invalid_annotation"), Vec::<String>::new());
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        Vec::<String>::new()
    );
}

// —— item 3: the formal default-value context channel ————————————————

/// The acceptance pin's closure-parameter default: `= const ['foo']`
/// inside `[List<int> x = …]` donates `List<int>`'s element face, so the
/// `'foo'` element reports `list_element_type_not_assignable`.
#[test]
fn formal_default_list_context_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 var g = ([List<int> x = const ['foo']]) {};\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "list_element_type_not_assignable"),
        ["The element type 'String' can't be assigned to the list type 'int'."]
    );
}
