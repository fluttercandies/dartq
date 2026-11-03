//! Ticket #3739 pins: `const_with_type_parameters` — a type-parameter
//! type argument on a const creation's head reports at the argument leaf.
//!
//! Upstream `_checkForConstWithTypeParameters` (constant_verifier.dart
//! :591-660) runs for every const-context constructor reference the
//! verifier visits — explicit `const C<T>(...)` and the implicit
//! `const x = C<T>(...)`/`const x = C<T>.named(...)` alike — before the
//! constructor resolves, so an unresolved or non-const head still answers.
//! Each `TypeParameterElement` leaf of the written `typeArguments`
//! reports at its own span; a function-type argument's OWN declared
//! parameter (`void Function<U>`) is exempt.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// `const A<T>()` inside a generic class method — the explicit const
/// creation reports `const_with_type_parameters` at the `T` argument.
#[test]
fn explicit_const_creation_type_param_arg() {
    let fixture = "class A<X> { const A(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const A<T>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the explicit const creation's `T` argument reports"
    );
}

/// `const x = A<T>()` inside a generic class method — the implicit const
/// creation (no `const` keyword before the head) reports the same leaf.
#[test]
fn implicit_const_creation_type_param_arg() {
    let fixture = "class A<X> { const A(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const x = A<T>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the implicit const creation's `T` argument reports"
    );
}

/// `const A<List<T>>()` — a nested type-parameter leaf reports at its own
/// span (the `T` inside `List<T>`), not the `List` head.
#[test]
fn nested_type_param_arg_reports_inner_leaf() {
    let fixture = "class A<X> { const A(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const A<List<T>>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the nested `T` leaf reports at its own span"
    );
}

/// `const Missing<T>()` — an unresolved head still answers the check (the
/// verifier runs before the constructor resolves).
#[test]
fn unresolved_head_still_reports() {
    let fixture = "class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const Missing<T>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "an unresolved head's `T` argument still reports"
    );
}

/// `var x = A<T>()` — a non-const creation carries no const context, so
/// the type-parameter argument stays silent.
#[test]
fn non_const_creation_is_silent() {
    let fixture = "class A<X> { const A(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   var x = A<T>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "const_with_type_parameters").is_empty(),
        "a non-const creation's type-parameter argument reports nothing"
    );
}

/// `const A<T Function<U>()>()` — a function-type argument's OWN declared
/// parameter is exempt upstream (`allowedTypeParameters`); `U` is bound
/// by the `Function<U>` group, not the enclosing class, so the `T` leaf
/// alone reports.
#[test]
fn function_type_own_param_is_exempt() {
    let fixture = "class A<X> { const A(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const A<T Function<U>()>();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the enclosing `T` reports; the function-type's own `U` stays silent"
    );
}

/// `const A<T>.named()` — the qualified named-constructor implicit
/// creation reports the `T` argument at its own span.
#[test]
fn implicit_named_ctor_type_param_arg() {
    let fixture = "class A<X> { const A.named(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const x = A<T>.named();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the qualified implicit creation's `T` argument reports"
    );
}

/// `const A<T>()` where `T` is NOT in scope — a non-parameter type
/// argument reports nothing (the containment test gates on the in-scope
/// parameter set, never a bare name).
#[test]
fn out_of_scope_arg_is_silent() {
    let fixture = "class A<X> { const A(); }\n\
                   void m() {\n\
                   \x20 const A<int>();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert!(
        spans_of(&report, &full, "const_with_type_parameters").is_empty(),
        "a concrete type argument reports nothing"
    );
}
