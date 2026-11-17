//! #4340 pins — the annotation-metadata engine seams A/B/C, on the
//! parser contracts #4338 froze.
//!
//! * Seam A — the type-variable group's metadata drain: upstream's scope
//!   walk wraps the whole variable list in the `TypeParameterScope`
//!   before visiting it (`withTypeParameterScope`, scope_context.dart
//!   :60-62), so the group's annotations resolve at the group's close
//!   with every parameter in scope — an unresolved name reports
//!   `undefined_annotation` (corpus
//!   ExtensionTypeResolutionTest/test_typeParameter_metadata_undefined,
//!   GenericFunctionTypeResolutionTest/test_metadata_typeParameter), a
//!   parameter reference falls through the resolver cascade to
//!   `invalid_annotation` (annotation_resolver.dart:386-389).
//! * Seam B — the failed-declaration-head death: `@p<A>.B.ctor()` recovers
//!   the `.` as `expected_executable` and the metadata dies with the
//!   failed unit (the fasta→analyzer translation keeps members only), so
//!   neither the name verdict nor the staged type-argument row survives
//!   to the next declaration's site drain (corpus
//!   GenericMetadataParserTest/test_typeArguments_after_prefix), while
//!   the LIVING uninstantiated shape keeps both rows
//!   (test_reference_prefixed). The `<unnamed>` synthetic the cascade
//!   splices anchors `unused_element` on its own zero-width span
//!   (test_typeArguments_after_constructorName oracle 2:13|0).
//! * Seam C — the unclosed-argument-group death: an annotation whose
//!   argument group swallows the rest of the file attaches to nothing
//!   and resolves never (corpus partial_code/annotation leftParen_*),
//!   whether the tail ends with a head-failure row or not — the staged
//!   argument rows (identifier reads, creation heads, const-set
//!   elements, switch exhaustiveness) retract with the entry.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

const UNDEFINED_ANNOTATION: &str = "Undefined name 'a' used as an annotation.";
const UNRESOLVED_ANNOTATION: &str = "Undefined name 'Unresolved' used as an annotation.";
const PREFIX_ANNOTATION: &str = "Undefined name 'p' used as an annotation.";
const INVALID_ANNOTATION: &str =
    "Annotation must be either a const variable reference or const constructor invocation.";
const NON_TYPE_AS_TYPE_ARGUMENT: &str =
    "The name 'A' isn't a type, so it can't be used as a type argument.";
const UNUSED_UNNAMED: &str = "The declaration '<unnamed>' isn't referenced.";

/// The fixture's rows of one code: messages in source order.
fn code_rows(report: &FileReport, wanted: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == wanted)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, message)| message).collect()
}

/// The source text each row of one code anchors on, in source order —
/// the span law (`at(node)` over the whole Annotation, the `<unnamed>`
/// synthetic's zero-width name) in readable form.
fn anchored(report: &FileReport, full: &str, wanted: &str) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == wanted)
        .map(|d| {
            (
                d.range.offset,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
            )
        })
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, text)| text).collect()
}

// —— Seam A: the type-variable group's metadata drain ————————————————

/// `extension type E<@Unresolved T>(int it) {}` — the extension-type
/// variable group's metadata reports on the whole Annotation node.
/// Oracle (corpus test_typeParameter_metadata_undefined):
/// `undefined_annotation 2:18|11` over `@Unresolved`.
#[test]
fn extension_type_type_variable_metadata_reports_undefined() {
    let source = "\nextension type E<@Unresolved T>(int it) {}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [UNRESOLVED_ANNOTATION],
        "rows: {:?}",
        code_rows(&report, "undefined_annotation")
    );
    assert_eq!(
        anchored(&report, &full, "undefined_annotation"),
        ["@Unresolved"]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
}

/// `Function<@a T>()? x;` with `a` defined — the GFT suffix's variable
/// group metadata resolves the name and stays silent. Oracle (corpus
/// test_metadata_typeParameter): no rows.
#[test]
fn gft_type_variable_metadata_defined_stays_silent() {
    let source = "const a = 42;\n\nFunction<@a T>()? x;\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
}

/// `f<@a E>() {}` — the plain function's variable group metadata
/// reports on the whole `@a` node (the #4338-pinned event set feeding
/// the drain; oracle `undefined_annotation` over `@a`).
#[test]
fn function_type_variable_metadata_reports_undefined() {
    let source = "f<@a E>() {}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
    assert_eq!(anchored(&report, &full, "undefined_annotation"), ["@a"]);
}

/// A method's variable group drains the same way inside a container.
#[test]
fn method_type_variable_metadata_reports_undefined() {
    let source = "class C {\n  void m<@a T>() {}\n}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
    assert_eq!(anchored(&report, &full, "undefined_annotation"), ["@a"]);
}

/// `f<@E E>() {}` — the group's scope holds the annotated variable
/// itself: the reference resolves to the `TypeParameterElement` and the
/// resolver cascade falls through to `invalid_annotation`
/// (annotation_resolver.dart:386-389), never `undefined_annotation`.
#[test]
fn type_variable_metadata_self_reference_is_invalid() {
    let source = "f<@E E>() {}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(
        code_rows(&report, "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
    assert_eq!(anchored(&report, &full, "invalid_annotation"), ["@E"]);
}

/// `f<@U T, U>() {}` — a FORWARD sibling answers too: the
/// `TypeParameterScope` wraps the whole list before any of the group's
/// metadata resolves (scope_context.dart:60-62).
#[test]
fn type_variable_metadata_forward_sibling_is_invalid() {
    let source = "f<@U T, U>() {}\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(
        code_rows(&report, "invalid_annotation"),
        [INVALID_ANNOTATION]
    );
}

/// `f<@a T, @b U>() {}` — each variable's own annotation reports, in
/// group order.
#[test]
fn type_variable_metadata_drains_per_group_entry() {
    let source = "f<@a T, @b U>() {}\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [
            "Undefined name 'a' used as an annotation.".to_string(),
            "Undefined name 'b' used as an annotation.".to_string(),
        ]
    );
}

/// `class C { C<@Foo @Bar() }` — the CONSTRUCTOR head's `<...>` group is
/// the illegal constructor type parameters: upstream's recovery never
/// parses it as a type-parameter scope (the bracket's metadata is skipped
/// with `annotation_on_type_argument`, type_info_impl.dart:1439-1449), so
/// neither name resolves. Oracle (corpus
/// test_constructorPartial3): no `undefined_annotation` rows.
#[test]
fn constructor_head_type_variable_group_is_doomed() {
    let source = "class C { C<@Foo @Bar() }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(code_rows(&report, "invalid_annotation"), [] as [String; 0]);
}

/// `class C { C(void f<@a T>()) {} }` — a group inside the constructor's
/// FORMAL list is a legal type-parameter scope (the formal list closed
/// the head window): the metadata drains normally.
#[test]
fn constructor_formal_type_variable_group_still_drains() {
    let source = "class C { C(void f<@a T>()) {} }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
}

/// `class C { factory C<@a>() => throw 0; }` — the factory name's `<...>`
/// group is the same illegal head shape as the generative constructor's
/// (a factory name is a constructor reference target upstream): the
/// metadata never resolves.
#[test]
fn factory_head_type_variable_group_is_doomed() {
    let source = "class C { factory C<@a>() => throw 0; }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
}

// —— Seam B: the failed-head death + the `<unnamed>` anchor ——————————

/// `@p<A>.B.ctor() class C {}` — the uninstantiated annotation's unit
/// failed (`expected_executable` at the `.`), the metadata died with it:
/// no `undefined_annotation`, no `non_type_as_type_argument`. Oracle
/// (corpus test_typeArguments_after_prefix#0): parser rows only.
#[test]
fn failed_head_kills_uninstantiated_annotation_verdicts() {
    let source = "@p<A>.B.ctor() class C {}\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [] as [String; 0]
    );
    assert_eq!(
        code_rows(&report, "non_type_as_type_argument"),
        [] as [String; 0]
    );
}

/// `@p.x<A> class C {}` — the LIVING uninstantiated shape keeps both
/// rows: the annotation attaches to the surviving class and its type
/// argument resolves. Oracle (corpus test_reference_prefixed#0):
/// `undefined_annotation p` + `non_type_as_type_argument A`.
#[test]
fn living_uninstantiated_annotation_keeps_both_rows() {
    let source = "@p.x<A> class C {}\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [PREFIX_ANNOTATION]
    );
    assert_eq!(
        code_rows(&report, "non_type_as_type_argument"),
        [NON_TYPE_AS_TYPE_ARGUMENT]
    );
}

/// `@p.A.ctor<B>() class C {}` — the cascade's `<unnamed>` synthetic
/// function anchors `unused_element` on its own zero-width span at the
/// `(` (the parser's splice point, #4338's
/// constructor_name_type_arguments_cascade_synthetic_anchor). Oracle:
/// `unused_element 2:13|0`.
#[test]
fn unnamed_synthetic_anchors_unused_element_at_own_span() {
    let source = "@p.A.ctor<B>() class C {}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report_ops(source);
    assert_eq!(code_rows(&report, "unused_element"), [UNUSED_UNNAMED]);
    let open = full.find("ctor<B>()").expect("fixture shape") + "ctor<B>()".len() - 2;
    let rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(
        rows,
        [(open as u32, 0)],
        "the `(` splice offset, zero width"
    );
}

// —— Seam C: the unclosed-argument-group death ——————————————————————

/// `@a( class A {}` — the unclosed argument group swallowed the rest of
/// the file; nothing resolves. Oracle (corpus
/// test_annotation_topLevel_leftParen_class): parser rows only.
#[test]
fn unclosed_group_arguments_never_resolve_class_head() {
    let source = "@a( class A {}\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_identifier"),
        [] as [String; 0]
    );
}

/// `class C { @a( const f = 0; }` — the swallowed const field head
/// stages no creation row. Oracle (corpus
/// test_annotation_classMember_leftParen_fieldConst): parser rows only.
#[test]
fn unclosed_group_arguments_never_resolve_field_const() {
    let source = "class C { @a( const f = 0; }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "creation_with_non_type"),
        [] as [String; 0]
    );
}

/// `@a( enum E { v }` — the keyword-as-identifier argument and the set
/// literal's element face die with the entry. Oracle (corpus
/// test_annotation_topLevel_leftParen_enum): parser rows only.
#[test]
fn unclosed_group_arguments_never_resolve_enum_head() {
    let source = "@a( enum E { v }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_identifier"),
        [] as [String; 0]
    );
    assert_eq!(
        code_rows(&report, "non_constant_set_element"),
        [] as [String; 0]
    );
}

/// `f() { @a( switch (x) {} }` — the LOCAL member SURVIVES the unclosed
/// argument group's recovery (the oracle keeps `undefined_annotation`
/// over the whole `@a(…)` node, the argument read's `undefined_identifier
/// 'x'`, and the `<unnamed>` unused-local row) — only the switch
/// exhaustiveness verdict stays silent: the undefined `x` types the
/// scrutinee whole `InvalidType`, and the oracle's exhaustiveness pass
/// gates off behind `hasInvalidType` (constant_verifier.dart:1095) — no
/// corpus row ever prints over `InvalidType` (every `leftParen_switch`
/// face, annotation/assert/for/if/while alike). Oracle (corpus
/// test_annotation_local_leftParen_switch): parser rows +
/// undefined_annotation + undefined_identifier x + unused_local_variable.
#[test]
fn unclosed_group_local_member_survives_invalid_scrutinee_gates_verdict() {
    let source = "f() { @a( switch (x) {} }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "non_exhaustive_switch_expression"),
        [] as [String; 0]
    );
    assert_eq!(
        code_rows(&report, "undefined_identifier"),
        ["Undefined name 'x'."]
    );
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [UNDEFINED_ANNOTATION]
    );
    assert_eq!(
        code_rows(&report, "unused_local_variable"),
        ["The value of the local variable '<unnamed>' isn't used."]
    );
}

/// The flush never over-reaches: an ATTACHED annotation's own verdict
/// survives a later unclosed group, and a defined attached annotation's
/// arguments keep resolving (upstream's null-element arm still calls
/// `_visitArguments`, annotation_resolver.dart:240-249).
#[test]
fn attached_annotation_survives_beside_doomed_one() {
    let source = "@p() class C {}\n@d( enum E { v }\n";
    let report = report_ops(source);
    assert_eq!(
        code_rows(&report, "undefined_annotation"),
        [PREFIX_ANNOTATION]
    );
    assert_eq!(
        code_rows(&report, "undefined_identifier"),
        [] as [String; 0]
    );
}
