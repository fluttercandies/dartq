//! Tickets #4333/#4334 pins: the ten surviving `const_with_type_parameters`
//! conformance divergences — the applied-instantiation family, the is/as
//! faces, and the non-const-field const-constructor row.
//!
//! * A bare generic function reference as a formal's default value whose
//!   context type binds a parameter to an in-scope type parameter reports
//!   the function-tearoff row at the reference (`visitFunctionReference`'s
//!   INFERRED instantiation, evaluation.dart:955-983) — plain formals,
//!   method formals, and constructor field formals alike.
//! * `A.new` in the same position reports `invalid_assignment` at the
//!   whole reference (the constructor tearoff instantiates the container
//!   at its implicit `dynamic` bounds; a type-parameter-mentioning
//!   context can never be satisfied).
//! * A non-const instance field initializer carrying a type-parameter
//!   tearoff row reports `const_constructor_with_field_initialized_by_
//!   non_const` at the const constructor's `const` keyword
//!   (`_validateFieldInitializers`, constant_verifier.dart:910-951).
//! * In a constant context, `e is T`/`e as T` check the WRITTEN type
//!   operand's parameter leaves (constant_verifier.dart:264-273), a
//!   function type's OWN `extends` bound included (`U extends T` reports
//!   at the `T`), and a const list's cast element whose tearoff value is
//!   not assignable to the cast target reports
//!   `list_element_type_not_assignable` (`_validateListExpression`,
//!   constant_verifier.dart:1367-1395).

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// `(code, anchored text)` pairs, sorted — the emission order of one
/// site's sibling rows is not part of the contract.
fn code_spans(report: &FileReport, full: &str) -> Vec<(&'static str, String)> {
    let mut spans: Vec<(&'static str, String)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
            )
        })
        .collect();
    spans.sort();
    spans
}

fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| d.message.to_string())
        .collect()
}

/// `void bar<T>([void Function(List<T>) p = f]) {}` — the context binds
/// `f`'s parameter to `List<T>`, which mentions `bar`'s `T`.
/// Oracle: the function-tearoff row at the `f`.
#[test]
fn default_value_applied_instantiation_nested_reports() {
    let fixture = "void f<T>(T t) => t;\n\
                   \n\
                   void bar<T>([void Function(List<T>) p = f]) {}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("const_with_type_parameters", "f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).contains(
        &"A constant function tearoff can't use a type parameter as a type argument.".to_string()
    ));
}

/// `void bar<T>([void Function(T Function()) p = f]) {}` — the context
/// slot is itself a function type whose return mentions the parameter.
#[test]
fn default_value_applied_instantiation_nested_function_reports() {
    let fixture = "void f<T>(T t) => t;\n\
                   \n\
                   void bar<T>([void Function(T Function()) p = f]) {}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("const_with_type_parameters", "f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// A method formal's default value in a generic class reports the same
/// row against the class's parameter.
#[test]
fn default_value_applied_instantiation_method_formal_reports() {
    let fixture = "void f<T>(T t) => t;\n\
                   \n\
                   class C<T> {\n\
                   \x20 void foo([void Function(T) p = f]) {}\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("const_with_type_parameters", "f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// `const C({this.p = f})` — a constructor field formal's default is a
/// constant context whose context type is the FIELD's declared type; the
/// plain default-value check never sees it.
#[test]
fn default_value_applied_instantiation_field_formal_reports() {
    let fixture = "void f<T>(T t) => t;\n\
                   \n\
                   class C<T> {\n\
                   \x20 final void Function(T) p;\n\
                   \x20 const C({this.p = f});\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("const_with_type_parameters", "f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// `void m([A<T> Function() fn = A.new]) {}` — the constructor tearoff
/// instantiates the container at `dynamic`; the `A<T>` context can never
/// be satisfied, so `invalid_assignment` reports at the whole `A.new`.
#[test]
fn default_value_constructor_tearoff_invalid_assignment_reports() {
    let fixture = "class A<T> {\n\
                   \x20 void m([A<T> Function() fn = A.new]) {}\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("invalid_assignment", "A.new".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows(&report).contains(
            &"A value of type 'A<dynamic> Function()' can't be assigned to a variable of type \
      'A<T> Function()'."
                .to_string()
        )
    );
}

/// `final x = f<U>;` in a container with a const constructor — beside the
/// (green) tearoff row at `U`, the const constructor reports the
/// non-const-field row at its `const` keyword.
#[test]
fn const_ctor_field_initialized_by_tearoff_reports() {
    let fixture = "void f<T>(T a) {}\n\
                   class A<U> {\n\
                   \x20 const A();\n\
                   \x20 final x = f<U>;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
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
    assert!(
        rows(&report).contains(
            &"Can't define the 'const' constructor because the field 'x' is initialized with a \
      non-constant value."
                .to_string()
        )
    );
}

/// `const [false is void Function(T)]` — the is-expression's written type
/// operand reports its parameter leaf (the parameter-list `T` is a use of
/// the class parameter, not a shadowing declaration).
#[test]
fn const_list_is_expression_type_param_reports() {
    let fixture = "class A<T> {\n\
                   \x20 void m() {\n\
                   \x20   const [false is void Function(T)];\n\
                   \x20 }\n\
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

/// `const [f as void Function<T>(T, [int])]` — the cast element's
/// tearoff value (`void Function<T>(T)`) is not assignable to the cast
/// target, reporting at the whole `as` expression.
#[test]
fn const_list_as_element_type_mismatch_reports() {
    let fixture = "void f<T>(T a) {}\n\
                   void g() {\n\
                   \x20 const [f as void Function<T>(T, [int])];\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![(
            "list_element_type_not_assignable",
            "f as void Function<T>(T, [int])".to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows(&report).contains(
            &"The element type 'void Function<T>(T)' can't be assigned to the list type \
      'void Function<T>(T, [int])'."
                .to_string()
        )
    );
}

/// `const A<void Function<U extends T>()>()` — a creation's type argument
/// contains a function type whose OWN parameter's `extends` bound mentions
/// the in-scope parameter: the bound leaf reports, the declaration itself
/// stays silent.
#[test]
fn const_creation_function_type_bound_leaf_reports() {
    let fixture = "class A<T> {\n\
                   \x20 const A();\n\
                   \x20 void m() {\n\
                   \x20   const A<void Function<U extends T>()>();\n\
                   \x20 }\n\
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

/// The negative gallery — none of these reports on the ticket's faces:
/// * a context whose binding slot names no type parameter (`int` — `f`'s
///   `T` infers `int`, never a type parameter);
/// * a non-generic default function;
/// * a non-const local's `is` (never a constant context);
/// * a parameter-free const `is`;
/// * a static field's initializer (statics never join the instance
///   state, so the const-constructor row never fires);
/// * a const-ctor container whose field initializer is a valid constant.
#[test]
fn unbound_or_non_const_faces_stay_silent() {
    let fixture = "void f<T>(T t) => t;\n\
                   void g(int t) => t;\n\
                   class A<T> {\n\
                   \x20 void m() {\n\
                   \x20   var y = 1 is int;\n\
                   \x20   const c = 1 is int;\n\
                   \x20 }\n\
                   \x20 void n([void Function(int) p = f]) {}\n\
                   \x20 void o([void Function(int) p = g]) {}\n\
                   }\n\
                   class B<U> {\n\
                   \x20 const B();\n\
                   \x20 final x = 1;\n\
                   \x20 static final sf = f<U>;\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let ticket_rows: Vec<_> = code_spans(&report, &full)
        .into_iter()
        .filter(|(code, _)| {
            *code == "const_with_type_parameters"
                || *code == "const_constructor_with_field_initialized_by_non_const"
                || *code == "invalid_assignment"
                || *code == "list_element_type_not_assignable"
        })
        .collect();
    assert_eq!(
        ticket_rows,
        Vec::<(&'static str, String)>::new(),
        "rows: {:?}",
        rows(&report)
    );
}
