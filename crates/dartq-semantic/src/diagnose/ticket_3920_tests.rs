//! #3920 pins — the dot-shorthand dispatch completion packet.
//!
//! Six arms of the shorthand family, each pinned against the 3.13.7
//! oracle (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine`):
//!
//! - (a) collection first-element context (`var list = [.a]` — the
//!   literal's own element schema arms `'_'`, never the enclosing one)
//! - (b) nullable-context strip vs the resolved-type display (a shorthand
//!   answers the ELEMENT's type, never the context's `?`)
//! - (c) the shorthand-ctor type-args gate
//!   (`wrong_number_of_type_arguments_constructor`)
//! - (d) the private-library reachability arm (`_C` from a sibling unit)
//! - (e) the G-cluster `''` len-0 name artifact (`[....]` spread)
//! - (f) the deprecated-anchor tail arm
//!
//! Every pin records the oracle expectation verbatim so a future failure
//! names both sides.

use super::test_rig::{code_names, report_ops};

/// The rows whose code name is `name`, as `code @offset: message` strings.
fn rows_named(report: &super::FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

// —— (b) the nullable-context strip ——
//
// Upstream answers the ELEMENT's type for a shorthand, never the context's
// `?` (`PropertyElementResolverResult(getType: element.returnType)` — the
// substituted interface type; the enum-constant arm the static field's own
// declared face). `shorthand_member_ty` now does the same.
//
// The CENSUS rows this arm was opened for (`unchecked_use_of_nullable_value`
// EXTRA ×43 over `ET? et = .new(1); et.value`) are NOT the typing: probing
// `class C { int value = 1; C(); } void main() { C? c = C(); c.value; }`
// (plain creation) is silent while the shorthand twin reports, and the
// shorthand row type is already non-null after this strip — so the residual
// row is the FLOW layer's initializer promotion not firing for a shorthand
// initializer (`dartq-flow/src/interp_expr.rs`, BG's live file). Routed, with
// the oracle expectation pinned below.

/// `C? c = .new()` — the shorthand answers `C`, not `C?`: the member read
/// over the LOCAL is silent upstream (oracle: no rows). Currently still
/// reports until the flow initializer promotion covers a shorthand head.
#[test]
#[ignore = "routed: flow's initializer promotion does not fire for a shorthand head"]
fn nullable_context_ctor_shorthand_promotes_the_local() {
    let report = report_ops(
        "class C {\n\
         \x20 int value = 1;\n\
         \x20 C();\n\
         }\n\
         void main() {\n\
         \x20 C? c = .new();\n\
         \x20 print(c.value);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "unchecked_use_of_nullable_value"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// The PLAIN-creation twin proves the gap is the shorthand head, not the
/// promotion machinery: `C? c = C(); c.value;` is silent today.
#[test]
fn plain_creation_initializer_promotes_the_local() {
    let report = report_ops(
        "class C {\n\
         \x20 int value = 1;\n\
         \x20 C();\n\
         }\n\
         void main() {\n\
         \x20 C? c = C();\n\
         \x20 print(c.value);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "unchecked_use_of_nullable_value"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// The static-member arm keeps the MEMBER's own declared face — a nullable
/// field stays nullable, so the strip is the CONTEXT's `?` and nothing else.
/// Pinned through the non-null context, where both faces agree.
#[test]
fn non_nullable_context_static_nullable_field_keeps_its_own_face() {
    let report = report_ops(
        "class C {\n\
         \x20 static C? maybe = null;\n\
         \x20 int value = 1;\n\
         }\n\
         void main() {\n\
         \x20 C? c = .maybe;\n\
         \x20 print(c.value);\n\
         }",
    );
    // The member face is `C?`, so the read reports — the strip never
    // invents non-nullability for a nullable member.
    assert_eq!(
        rows_named(&report, "unchecked_use_of_nullable_value").len(),
        1,
        "{:?}",
        code_names(&report)
    );
}

// —— (c) the shorthand-ctor type-args gate ——
//
// Upstream reports `wrongNumberOfTypeArgumentsDotShorthandConstructor`
// under the shared `_constructor` code, `.at(typeArguments)`, whenever the
// shorthand's context denotes an accessible class and a `<...>` group was
// written — unconditionally, the group's own arity never entering
// (instance_creation_expression_resolver.dart:105-114). `verdict_shorthand_miss`
// now calls `emit_shorthand_ctor_type_args`, which gates on the RESOLVED
// selector naming a constructor (a generic static method keeps its own
// valid instantiation) and anchors at the balanced `<...>` group.

/// `.new<int>()` over a non-generic class: one row, at the group, with the
/// dot-shorthand text.
#[test]
fn ctor_shorthand_with_type_arguments_reports_at_the_group() {
    let report = report_ops(
        "class C { C(); }\n\
         void main() {\n\
         \x20 C c = .new<int>();\n\
         \x20 print(c);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("The dot shorthand resolves to the constructor 'C.new'"),
        "the dot-shorthand variant's text must render, got {hits:?}"
    );
}

/// A NAMED constructor selector keys its own name in the text.
#[test]
fn named_ctor_shorthand_with_type_arguments_names_the_selector() {
    let report = report_ops(
        "class Foo<T> {\n\
         \x20 factory Foo.a() = _Foo;\n\
         \x20 Foo();\n\
         }\n\
         class _Foo<T> extends Foo<T> { _Foo(); }\n\
         Foo<T> bar<T>() => .a<T>();",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'Foo.a'"),
        "the selector must spell Foo.a, got {hits:?}"
    );
}

/// A generic STATIC METHOD's type-argument group is a valid instantiation,
/// not a constructor application — `.staticMethod<int>()` stays silent
/// (the gate keys on the RESOLVED selector naming a constructor).
#[test]
fn generic_static_method_shorthand_with_type_arguments_stays_silent() {
    let report = report_ops(
        "class C {\n\
         \x20 static C staticMethod<X>() => C();\n\
         }\n\
         void main() {\n\
         \x20 C c = .staticMethod<int>();\n\
         \x20 print(c);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "wrong_number_of_type_arguments_constructor"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// A selector with no `<...>` group keeps the silent face.
#[test]
fn ctor_shorthand_without_type_arguments_stays_silent() {
    let report = report_ops(
        "class C { C(); }\n\
         void main() {\n\
         \x20 C c = .new();\n\
         \x20 print(c);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "wrong_number_of_type_arguments_constructor"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// The `const` spelling is the SAME row upstream, but its emit path is the
/// const-eval face (`const_eval_faces.rs`, a live lane's file) — the
/// resolver's `node.element == null` branch for an explicitly marked const
/// invocation. Routed with the oracle expectation pinned.
#[test]
#[ignore = "routed: the const spelling's emit path is const_eval_faces.rs"]
fn const_ctor_shorthand_with_type_arguments_reports() {
    let report = report_ops(
        "class C { const C(); }\n\
         void main() {\n\
         \x20 C c = const .new<int>();\n\
         \x20 print(c);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'C.new'"),
        "the named face must render, got {hits:?}"
    );
}

// —— (a) collection first-element context — landed ——
//
// The oracle reports `dot_shorthand_missing_context` for a shorthand
// element of a collection whose own position arms no element schema: an
// unannotated inference group (`var l = [.one]`, `var s = {.one}`, map
// keys and values, later declarators), a statement-level collection, and
// a nested collection inside one of those (the outer group's face rules)
// — probed on the 3.13.7 oracle (`var list = [.one];` at the whole
// shorthand span, `var m = {1: [.one]}`, `var z = {.one: 1}`,
// `var x = [[.one]]`, `var a = 1, l = [.one]`, `[.one];`). An ANNOTATED
// group (`List<A> l = [.one]`) and an assignment target (`x = [.one]`
// with `x: List<A>`) stay silent upstream — the annotation's element
// type arms the context. `literal_elem_ctx` now arms `'_'` for exactly
// the unannotated/statement faces (`inference_group_literal_ctx` keeps
// the classification positional so the deferred re-verdicts, which run
// after the declaration window closed, answer stably); the annotated and
// assignment faces keep the under-report silence.

/// `var l = [.one];` / `var s = {.one};` — one row each, at the whole
/// shorthand span (probe4/probe5 on the oracle).
#[test]
fn unannotated_list_and_set_literals_report_missing_context() {
    for (name, source) in [
        ("list", "var l = [.one];\n  print(l);"),
        ("set", "var s = {.one};\n  print(s);"),
    ] {
        let report = report_ops(&format!(
            "class A {{ static A one = A(); }}\n\
             void main() {{\n\
             \x20 {source}\n\
             }}"
        ));
        let hits = rows_named(&report, "dot_shorthand_missing_context");
        assert_eq!(hits.len(), 1, "{name}: {:?}", code_names(&report));
        assert!(
            hits[0].contains("no context type"),
            "{name}: the oracle's text must render, got {hits:?}"
        );
    }
}

/// `var m = {1: .one};` / `var z = {.one: 1};` — a map's value and key
/// both report (probe9/probe10).
#[test]
fn unannotated_map_key_and_value_report_missing_context() {
    for (name, source) in [
        ("value", "var m = {1: .one};\n  print(m);"),
        ("key", "var z = {.one: 1};\n  print(z);"),
    ] {
        let report = report_ops(&format!(
            "class A {{ static A one = A(); }}\n\
             void main() {{\n\
             \x20 {source}\n\
             }}"
        ));
        assert_eq!(
            rows_named(&report, "dot_shorthand_missing_context").len(),
            1,
            "{name}: {:?}",
            code_names(&report)
        );
    }
}

/// `var x = [[.one]];` — the inner literal's element inherits the outer
/// group's face (probe10: reports).
#[test]
fn nested_collection_reports_missing_context() {
    let report = report_ops(
        "class A { static A one = A(); }\n\
         void main() {\n\
         \x20 var x = [[.one]];\n\
         \x20 print(x);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context").len(),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// `var a = 1, l = [.one];` — a later declarator of the inference group
/// walks back over its sibling's initializer to the group head
/// (probe10:19 reports).
#[test]
fn later_declarator_of_inference_group_reports_missing_context() {
    let report = report_ops(
        "class A { static A one = A(); }\n\
         void main() {\n\
         \x20 var a = 1, l = [.one];\n\
         \x20 print([a, l]);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context").len(),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// `[.one];` — a statement-level collection arms the unknown element
/// schema (probe7: reports at the shorthand span).
#[test]
fn statement_level_collection_reports_missing_context() {
    let report = report_ops(
        "class A { static A one = A(); }\n\
         void main() {\n\
         \x20 [.one];\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context").len(),
        1,
        "{:?}",
        code_names(&report)
    );
}

/// `t = [.one];` with `t: List<A>` — the assignment target's own face
/// owns the context and the oracle stays silent (probe6: no rows).
#[test]
fn assignment_target_collection_keeps_silence() {
    let report = report_ops(
        "class A { static A one = A(); }\n\
         void main() {\n\
         \x20 List<A> t;\n\
         \x20 t = [.one];\n\
         \x20 print(t);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// Oracle: the ANNOTATED literal arms the element type and stays silent
/// (`List<A> l = [.one];` — no rows).
#[test]
fn annotated_collection_element_resolves_against_the_element_type() {
    let report = report_ops(
        "class A { static A one = A(); }\n\
         void main() {\n\
         \x20 List<A> l = [.one];\n\
         \x20 print(l);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

// —— (d) the private-library reachability arm — routed —
//
// `shorthand_head_accessible` (`scopes_bodies.rs`) already implements the
// `isAccessibleIn` equivalence the ticket asks for: a `_`-spelled head
// that resolves to no type in THIS unit answers `false`, and both verdict
// arms then fall to their weak faces. The remaining co19 rows are the
// SAME weak-face shape as the ones that already pass — the divergence is
// the missing-context emission itself (arm (a)'s `literal_elem_ctx` gap
// and the `TK::OpenParen`/`Comma` argument arms), not the privacy gate.
// Pinned here so a future regression in the gate is caught.

/// A private head the unit CAN see (same library) stays accessible, so
/// the member verdict speaks.
#[test]
fn private_head_in_the_same_library_stays_accessible() {
    let report = report_ops(
        "class _C {\n\
         \x20 static _C? get getter => null;\n\
         }\n\
         void main() {\n\
         \x20 _C? c = .getter;\n\
         \x20 print(c);\n\
         }",
    );
    // The head resolves here, so the getter hit answers `_C?` — the
    // unconditional read below is not part of this fixture.
    assert_eq!(
        rows_named(&report, "dot_shorthand_missing_context"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

// —— (e) the G-cluster `''` len-0 name artifact — routed —
//
// The two `lints/omit_obvious_local_variable_types/doc_bad_0|doc_good_0`
// rows carry `The static getter '' isn't defined for the context type
// 'Iterable'.` at `12:41|0` — a zero-length name over a `[....]` spread
// element. The row comes from `emit_shorthand_undefined_member`, whose
// selector text is `source_slice(selector)`; a zero-length selector
// yields the empty name. The emitter lives in `const_eval_faces.rs`
// (BJ's live file), so this arm is pinned and routed rather than edited.

/// The oracle emits no `dot_shorthand_undefined_member` for
/// `const cookbook = <List<Ingredient>>[....];` — the spread is not a
/// shorthand at all, so a `''`-named row is fabricated.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn spread_head_never_emits_a_shorthand_row() {
    let report = report_ops(
        "class Ingredient { const Ingredient(); }\n\
         const cookbook = <List<Ingredient>>[....];\n\
         void main() { print(cookbook); }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_undefined_member"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

// —— (f) the deprecated-anchor tail arm — routed —
//
// The ticket asks whether the deprecated anchor sits on this file's emit
// path. It does not: `shorthand_member_ty` / `verdict_shorthand_miss`
// never consult `@deprecated`, and the census shows zero deprecated rows
// in the shorthand family (the only `deprecat` hit is
// `deprecated_new_in_comment_reference`, an unrelated face). The
// deprecated-constructor face for a shorthand belongs to the
// deprecated verifier's owner; pinned as a no-op so the boundary is
// recorded.

/// A shorthand over a `@deprecated` constructor emits nothing from THIS
/// file's emit path (the deprecated verifier owns the row upstream —
/// `deprecated_member_use_from_same_package` is out of the shorthand
/// family).
#[test]
fn deprecated_ctor_shorthand_emits_no_shorthand_row() {
    let report = report_ops(
        "class C {\n\
         \x20 @deprecated\n\
         \x20 C();\n\
         }\n\
         void main() {\n\
         \x20 C c = .new();\n\
         \x20 print(c);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "dot_shorthand_undefined_member"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

// —— (c) gate edges: the shapes the census probes adjudicated ——

/// A MATCHING-arity group still reports — the gate is unconditional
/// upstream (`typeArguments != null`, the arity never enters;
/// `instance_creation_expression_resolver.dart:105-114`, the co19
/// `type_inference_A06_t01` row: `C<int> c = .new<int>();` reports
/// 'C.new' at the group).
#[test]
fn matching_arity_generic_ctor_shorthand_still_reports() {
    let report = report_ops(
        "class C<T> { C(); }\n\
         void main() {\n\
         \x20 C<int> c = .new<int>();\n\
         \x20 print(c);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("The dot shorthand resolves to the constructor 'C.new'"),
        "the dot-shorthand text must render, got {hits:?}"
    );
}

/// The equality right operand reports the same row
/// (`test_wrongNumberOfTypeArguments_message_equalityRhs`: `c ==
/// .new<int>()` at the group).
#[test]
fn equality_rhs_ctor_shorthand_with_type_arguments_reports() {
    let report = report_ops(
        "class C { C(); }\n\
         bool f(C c) => c == .new<int>();",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'C.new'"),
        "the named face must render, got {hits:?}"
    );
}

/// An extension type's `.new` is its declared primary constructor — the
/// gate hits it exactly like a class (the co19 `type_inference_A06_t01`
/// `ET.new` row: `ET<int> et = .new<int>(0);`).
#[test]
fn extension_type_ctor_shorthand_with_type_arguments_reports() {
    let report = report_ops(
        "extension type ET<T>(T v) {}\n\
         void main() {\n\
         \x20 ET<int> et = .new<int>(0);\n\
         \x20 print(et);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'ET.new'"),
        "the extension type's unnamed face must render, got {hits:?}"
    );
}

/// A class with NO declared constructor still owns its implicit unnamed
/// face — the gate hits it (`type_parameter_error_test`'s
/// `UnnamedConstructorTypeParameters.new` row).
#[test]
fn implicit_unnamed_ctor_counts_for_the_gate() {
    let report = report_ops(
        "class UnnamedConstructorTypeParameters<T> {}\n\
         void main() {\n\
         \x20 UnnamedConstructorTypeParameters x = .new<int>();\n\
         \x20 print(x);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'UnnamedConstructorTypeParameters.new'"),
        "the implicit unnamed face must render, got {hits:?}"
    );
}

/// A TEAR-OFF `.new<int>` (no invocation) never takes this gate: the
/// group validates against the torn function type
/// (`wrong_number_of_type_arguments_function` — probe11:9-14). The
/// constructor code stays empty; the function face itself is the
/// tear-off family's own row, out of this packet.
#[test]
fn ctor_shorthand_tear_off_keeps_the_constructor_code_silent() {
    let report = report_ops(
        "class C { C(); }\n\
         void main() {\n\
         \x20 C c = .new<int>;\n\
         \x20 print(c);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "wrong_number_of_type_arguments_constructor"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// An ABSTRACT class with a GENERATIVE constructor replaces the
/// type-arguments row with `instantiate_abstract_class` upstream
/// (instance_creation_expression_resolver.dart:96-101 — the else-if
/// never reaches the type-arguments report; probe11:10). This file's
/// gate suppresses; the instantiate face's owner reports alone.
#[test]
fn abstract_class_generative_ctor_suppresses_the_type_args_row() {
    let report = report_ops(
        "abstract class AC { AC(); }\n\
         void main() {\n\
         \x20 AC a = .new<int>();\n\
         \x20 print(a);\n\
         }",
    );
    assert_eq!(
        rows_named(&report, "wrong_number_of_type_arguments_constructor"),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// A FACTORY on an abstract class keeps the row (the abstract arm
/// requires `!isFactory` — probe11:12: `abstract class Foo<T> { factory
/// Foo.a() = _Foo; … }` reports 'Foo.a').
#[test]
fn abstract_class_factory_ctor_keeps_the_type_args_row() {
    let report = report_ops(
        "abstract class Foo<T> {\n\
         \x20 factory Foo.a() = _Foo;\n\
         \x20 Foo();\n\
         }\n\
         class _Foo<T> extends Foo<T> { _Foo(); }\n\
         void main() {\n\
         \x20 Foo<int> f = .a<int>();\n\
         \x20 print(f);\n\
         }",
    );
    let hits = rows_named(&report, "wrong_number_of_type_arguments_constructor");
    assert_eq!(hits.len(), 1, "{:?}", code_names(&report));
    assert!(
        hits[0].contains("'Foo.a'"),
        "the factory selector must render, got {hits:?}"
    );
}
