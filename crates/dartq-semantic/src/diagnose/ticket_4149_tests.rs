//! Ticket #4149 pins: the constant-context tearoff face of
//! `const_with_type_parameters` and the single-file rig's unit row.
//!
//! * An applied type-argument group with no call — `f<U>` (a function
//!   reference) or `A<T>` before `.new`/`.named` (a constructor
//!   reference) — inside a constant context reports each type-parameter
//!   leaf of the written group with the site-specific message
//!   (`visitConstructorReference`/`visitFunctionReference`,
//!   constant_verifier.dart:202-264 — oracle probes 2026-11-16).
//! * The constant contexts: a `const` local initializer, a formal's
//!   default value, and an instance field's initializer inside a
//!   container that declares a const constructor (`final` and `var`
//!   alike; statics stay out).
//! * The single-file `async_return_tests` rig registers its defining
//!   unit — `declared_outside_core`'s negatives answer exactly like the
//!   engine flow.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// `(code, anchored text)` pairs in source order.
fn code_spans(report: &FileReport, full: &str) -> Vec<(&'static str, String)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
            )
        })
        .collect()
}

fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| d.message.to_string())
        .collect()
}

/// `const c = A<T>.new;` — a constant-context constructor tearoff reports
/// at the type-parameter leaf with the constructor-tearoff message.
/// Oracle: `CONST_WITH_TYPE_PARAMETERS` at the `T` ("A constant
/// constructor tearoff can't use a type parameter as a type argument.").
#[test]
fn const_local_constructor_tearoff_reports() {
    let fixture = "class A<T> {\n\
                   \x20 void m() {\n\
                   \x20   const c = A<T>.new;\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![
            ("unused_local_variable", "c".to_string()),
            ("const_with_type_parameters", "T".to_string()),
        ],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows(&report).contains(
            &"A constant constructor tearoff can't use a type parameter as a type argument."
                .to_string()
        )
    );
}

/// `const c = A<List<T>>.new;` — the nested leaf reports at its own span.
#[test]
fn const_local_constructor_tearoff_nested_leaf() {
    let fixture = "class A<T> {\n\
                   \x20 void m() {\n\
                   \x20   const c = A<List<T>>.new;\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![
            ("unused_local_variable", "c".to_string()),
            ("const_with_type_parameters", "T".to_string()),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `const c = f<U>;` — a constant-context function tearoff reports with
/// the function-tearoff message. Oracle: at the `U`.
#[test]
fn const_local_function_tearoff_reports() {
    let fixture = "void f<X>(X a) {}\n\
                   class A<U> {\n\
                   \x20 void m() {\n\
                   \x20   const c = f<U>;\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![
            ("unused_local_variable", "c".to_string()),
            ("const_with_type_parameters", "U".to_string()),
        ],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).contains(
        &"A constant function tearoff can't use a type parameter as a type argument.".to_string()
    ));
}

/// `void m([fn = A<T>.new]) {}` — a formal's default value is a constant
/// context; the tearoff row reports there too.
#[test]
fn default_value_constructor_tearoff_reports() {
    let fixture = "class A<T> {\n\
                   \x20 void m([fn = A<T>.new]) {}\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("const_with_type_parameters", "T".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// `class A<T> { const A(); final x = f<U>; }` — an instance field's
/// initializer inside a container with a const constructor is a constant
/// context (`final` and `var` alike; the probe pins the `final` shape),
/// and the tearoff's non-constness reports the const constructor's own
/// row at its `const` keyword (#4333's
/// `check_nonconst_field_const_ctor_face`).
#[test]
fn const_ctor_class_field_function_tearoff_reports() {
    let fixture = "void f<X>(X a) {}\n\
                   class A<U> {\n\
                   \x20 const A();\n\
                   \x20 final x = f<U>;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let mut spans = code_spans(&report, &full);
    spans.sort();
    assert_eq!(
        spans,
        vec![
            (
                "const_constructor_with_field_initialized_by_non_const",
                "const".to_string()
            ),
            ("const_with_type_parameters", "U".to_string()),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// The negative gallery — none of these is a constant context or a
/// type-parameter use, so all stay silent on this face:
/// * `var y = A<T>.new;` — a non-const local initializer;
/// * `final x = f<U>;` in a class with NO const constructor;
/// * `static final sf = f<U>;` inside a const-ctor class (statics never
///   join the instance state);
/// * `const c = A<int>.new;` — a concrete argument is a valid constant;
/// * `const c = A<int>;` — a bare type literal, never visited upstream.
#[test]
fn non_const_or_concrete_tearoffs_stay_silent() {
    let fixture = "void f<X>(X a) {}\n\
                   class A<T> {\n\
                   \x20 void m() {\n\
                   \x20   var y = A<T>.new;\n\
                   \x20   const c1 = A<int>.new;\n\
                   \x20   const c2 = A<int>;\n\
                   \x20 }\n\
                   }\n\
                   class B<U> {\n\
                   \x20 final x = f<U>;\n\
                   }\n\
                   class C<U> {\n\
                   \x20 const C();\n\
                   \x20 static final sf = f<U>;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let const_rows: Vec<_> = code_spans(&report, &full)
        .into_iter()
        .filter(|(code, _)| *code == "const_with_type_parameters")
        .collect();
    assert_eq!(
        const_rows,
        Vec::<(&'static str, String)>::new(),
        "rows: {:?}",
        rows(&report)
    );
}

// —— #4147 re-adjudication: the enum static-vs-instance face ———————————
//
// Oracle probes (2026-11-16, pinned 3.13.7): a HEALTHY enum is fully
// silent on `conflicting_static_and_instance` — an enum constant is a
// static PROBE only (`_checkEnumStatic`'s `_getInterfaceMember` drops
// every static member, so a constant never collides with itself or a
// sibling). A constant whose name IS an implicit `Enum`/`Object`
// instance face reports once with the enum-qualified instance member
// (`enum Collide { index }` → "…have instance member 'Collide.index'…").

/// `enum E { v }` (plus the #3120-pin usage shapes) reports nothing —
/// the healthy face the #4119 collection flagged as over-reported.
#[test]
fn healthy_enum_constants_never_collide() {
    let fixture = "enum E { v }\n\
                   class A extends E2 {}\n\
                   class E2 {}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let conflicts: Vec<_> = code_spans(&report, &full)
        .into_iter()
        .filter(|(code, _)| *code == "conflicting_static_and_instance")
        .collect();
    assert_eq!(
        conflicts,
        Vec::<(&'static str, String)>::new(),
        "rows: {:?}",
        rows(&report)
    );
}

/// `enum Collide { index }` — a constant over the implicit `Enum.index`
/// getter reports once with the enum-qualified instance member spelling.
/// Oracle: "Class 'Collide' can't define static member 'index' and have
/// instance member 'Collide.index' with the same name." at the constant.
#[test]
fn enum_constant_over_implicit_index_collides() {
    let fixture = "enum Collide { index }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("conflicting_static_and_instance", "index".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert_eq!(
        rows(&report),
        [
            "Class 'Collide' can't define static member 'index' and have instance member \
          'Collide.index' with the same name."
                .to_string()
        ]
    );
}
