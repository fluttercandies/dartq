//! #3274 pins — the context type feeding an instance creation's type
//! arguments.
//!
//! Upstream resolves every `InstanceCreationExpression` under the enclosing
//! expression's context type (`visitInstanceCreationExpression` runs the
//! constructor's type parameters through the inference context), so
//! `LinkedHashSet<int> s = LinkedHashSet()` infers `LinkedHashSet<int>`
//! downward. dartq folded the creation from its argument-side constraints
//! alone: an omitted argument list solved nothing and the instance typed
//! `LinkedHashSet<dynamic>`, so every write/return/initializer face
//! reported a spurious `invalid_assignment` (or its `return_of_invalid_type`
//! / `field_initializer_not_assignable` sibling).
//!
//! The pins below cover every context channel the fix feeds — declaration
//! initializer, assignment target (parameter, parenthesized operand,
//! cascade head), host return (`return` and `=>` bodies, method and top-level
//! forms, a closure literal's `ctx_ret`), constructor-initializer field, and
//! both conditional operands — plus the negative: with no context the
//! creation stays `LinkedHashSet<dynamic>`.

use crate::diagnose::test_rig::{code_names, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The write/return/initializer codes the un-inferred creation type raised
/// (the corpus audit's EXTRA set).
const INVALID: [&str; 4] = [
    "invalid_assignment",
    "return_of_invalid_type",
    "return_of_invalid_type_from_closure",
    "field_initializer_not_assignable",
];

/// The invalid-type codes of one report, in report order.
fn invalid_codes(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
        .into_iter()
        .filter(|code| INVALID.contains(code))
        .collect()
}

/// Every pin requires the report to carry no invalid-type row at all.
fn assert_silent(report: &FileReport) {
    assert!(invalid_codes(report).is_empty(), "{:?}", rows(report));
}

/// A generic class with no member face: the creation's own type is the
/// only fact a verdict can read.
const BOX: &str = "class Box<T> {}\n";

/// A `dart:collection`-shaped set head (the corpus spelling): a generic
/// class with the cascade target member the corpus cases use.
const SET: &str = "class LinkedHashSet<E> {\n  void addAll(Iterable<E> other) {}\n}\n";

/// A `dart:collection`-shaped map head (two parameters).
const MAP: &str = "class LinkedHashMap<K, V> {}\n";

/// The canonical shape: the declaration's written annotation is the
/// creation's context (`Box<int> b = Box()` infers `<int>`).
#[test]
fn ticket_3274_declared_initializer_infers() {
    let report = report_ops(&format!(
        "{BOX}\
         void f() {{\n\
         \x20 Box<int> b = Box();\n\
         }}"
    ));
    assert_silent(&report);
}

/// The corpus spelling of the same channel: `dart:collection`'s
/// `LinkedHashSet<int> s = LinkedHashSet()`.
#[test]
fn ticket_3274_linked_hash_set_declaration_infers() {
    let report = report_ops(&format!(
        "{SET}\
         void f() {{\n\
         \x20 LinkedHashSet<int> s = LinkedHashSet();\n\
         }}"
    ));
    assert_silent(&report);
}

/// An assignment to a declared parameter: the target's write type is the
/// context (`test_assignment` in the corpus).
#[test]
fn ticket_3274_assignment_target_infers() {
    let report = report_ops(&format!(
        "{SET}\
         void f(LinkedHashSet<int> s) {{\n\
         \x20 s = LinkedHashSet();\n\
         }}"
    ));
    assert_silent(&report);
}

/// A parenthesized creation inherits the position of its group
/// (`test_assignment_withParentheses`).
#[test]
fn ticket_3274_parenthesized_assignment_infers() {
    let report = report_ops(&format!(
        "{SET}\
         void f(LinkedHashSet<int> s) {{\n\
         \x20 s = ((LinkedHashSet()));\n\
         }}"
    ));
    assert_silent(&report);
}

/// A cascade head reads the assignment target as its context
/// (`test_assignment_withCascade`).
#[test]
fn ticket_3274_cascade_head_infers() {
    let report = report_ops(&format!(
        "{SET}\
         void f(LinkedHashSet<int> s) {{\n\
         \x20 s = LinkedHashSet()..addAll([1, 2, 3]);\n\
         }}"
    ));
    assert_silent(&report);
}

/// A bare `return` statement: the host function's declared return is the
/// context (`test_returnStatement_functionDeclaration`).
#[test]
fn ticket_3274_return_statement_infers() {
    let report = report_ops(&format!(
        "{SET}\
         LinkedHashSet<int> f() {{\n\
         \x20 return LinkedHashSet();\n\
         }}"
    ));
    assert_silent(&report);
}

/// An arrow body: the same host-return channel spelled with `=>`
/// (`test_functionExpression_functionDeclaration`).
#[test]
fn ticket_3274_arrow_body_infers() {
    let report = report_ops(&format!(
        "{MAP}\
         LinkedHashMap<int, int> f() => LinkedHashMap();"
    ));
    assert_silent(&report);
}

/// The method forms of both return channels
/// (`test_functionExpression_methodDeclaration`,
/// `test_returnStatement_methodDeclaration`).
#[test]
fn ticket_3274_method_return_and_arrow_body_infer() {
    let report = report_ops(&format!(
        "{SET}{MAP}\
         class C {{\n\
         \x20 LinkedHashSet<int> a() {{\n\
         \x20   return LinkedHashSet();\n\
         \x20 }}\n\
         \x20 LinkedHashMap<int, int> b() => LinkedHashMap();\n\
         }}"
    ));
    assert_silent(&report);
}

/// A constructor initializer list: the target field's declared type is the
/// context (`test_constructorInitializer` — `field_initializer_not_assignable`
/// before the fix).
#[test]
fn ticket_3274_constructor_initializer_infers() {
    let report = report_ops(&format!(
        "{MAP}\
         class Foo {{\n\
         \x20 LinkedHashMap<int, double> a;\n\
         \x20 Foo() : a = LinkedHashMap();\n\
         }}"
    ));
    assert_silent(&report);
}

/// The conditional's else operand inherits the whole conditional's context
/// (`test_conditionalRight` — the corpus asymmetry this closes).
#[test]
fn ticket_3274_conditional_else_infers() {
    let report = report_ops(&format!(
        "{MAP}\
         void f(LinkedHashMap<int, int> a, LinkedHashMap<int, int> b) {{\n\
         \x20 a = true ? b : LinkedHashMap();\n\
         }}"
    ));
    assert_silent(&report);
}

/// The then operand reads the same context (`test_conditionalLeft`, the
/// silent half of the corpus pair — a regression guard on the climb).
#[test]
fn ticket_3274_conditional_then_infers() {
    let report = report_ops(&format!(
        "{MAP}\
         void f(LinkedHashMap<int, int> a, LinkedHashMap<int, int> b) {{\n\
         \x20 a = true ? LinkedHashMap() : b;\n\
         }}"
    ));
    assert_silent(&report);
}

/// A closure literal's `return`: the function-typed argument slot's return
/// is the context (`test_returnStatement_functionExpression` — the
/// `ctx_ret` channel).
#[test]
fn ticket_3274_closure_return_infers() {
    let report = report_ops(&format!(
        "{SET}\
         void g(LinkedHashSet<int> Function() f) {{}}\n\
         void h() {{\n\
         \x20 g(() {{\n\
         \x20   return LinkedHashSet();\n\
         \x20 }});\n\
         }}"
    ));
    assert_silent(&report);
}

/// The negative guard: with no context the creation keeps
/// `LinkedHashSet<dynamic>` — the second write proves the inferred
/// variable's face (`LinkedHashSet<dynamic>` does not fit
/// `LinkedHashSet<String>`), so the feed never invented an argument.
#[test]
fn ticket_3274_no_context_keeps_dynamic() {
    let report = report_ops(&format!(
        "{SET}\
         void f() {{\n\
         \x20 var s = LinkedHashSet();\n\
         \x20 LinkedHashSet<String> t = s;\n\
         }}"
    ));
    assert_eq!(
        invalid_codes(&report),
        ["invalid_assignment"],
        "{:?}",
        rows(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.name == "invalid_assignment"
                && diagnostic.message.contains("LinkedHashSet<dynamic>")),
        "{:?}",
        rows(&report)
    );
}
