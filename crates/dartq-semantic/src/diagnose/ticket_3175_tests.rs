//! #3175 — the type-literal receiver face on plain (non-cascade) member
//! reads.
//!
//! A receiver head that spells a TYPE — the `dynamic` keyword, `Never`, or
//! an ungrounded type alias (a function/record target; upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/ast/ast.dart `TypeLiteral.staticType`,
//! the same domain the cascade sections' `Receiver::Container` gate
//! serves) — denotes the `Type` INSTANCE: the member lookup is an instance
//! lookup on `Type`, the alias's own statics never answer, the `Object`
//! face resolves, and a clean miss reports the access-intent trio naming
//! `Type` (probe 3.13.7: `Cb.foo` over `typedef Cb = void Function()`
//! reports "The getter 'foo' isn't defined for the type 'Type'.", the
//! invoke twin "The method 'bar' isn't defined for the type 'Type'.",
//! `Never.bar()` and `dynamic.bar()` the same). The alias invoke twin is
//! the invocation walk's own face (`report_alias_selector_face`), so these
//! pins cover the keyword invokes instead. A VALUE receiver of
//! function/record type — the head spells a formal, local, field, or
//! getter — keeps the shape face's own display (`cb.gone` names `void
//! Function()`), and a dynamic-typed value dispatches dynamically and
//! stays silent.

use super::*;
use crate::diagnose::test_rig::report_ops;

fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.message.to_string())
        .collect()
}

/// An ungrounded alias head's read names `Type` — never the aliased
/// function type's display (`Cb.foo<int>` reports "The getter 'foo' isn't
/// defined for the type 'Type'.").
#[test]
fn alias_literal_read_names_type() {
    let report = report_ops(
        "typedef Cb = void Function();\n\
         var a = Cb.foo<int>;\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(
        getters[0], "The getter 'foo' isn't defined for the type 'Type'.",
        "the literal's read names Type: {getters:?}"
    );
}

/// The same face on a plain read without selector arguments (`Cb.gone`).
#[test]
fn alias_literal_plain_read_names_type() {
    let report = report_ops(
        "typedef Cb = void Function();\n\
         void f() { Cb.gone; }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(
        getters[0], "The getter 'gone' isn't defined for the type 'Type'.",
        "the plain read names Type: {getters:?}"
    );
}

/// The invoke twin reports `undefined_method` naming `Type` (`Cb.bar()`).
#[test]
fn alias_literal_invoke_names_type() {
    let report = report_ops(
        "typedef Cb = void Function();\n\
         void f() { Cb.bar(); }\n",
    );
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert_eq!(
        methods[0], "The method 'bar' isn't defined for the type 'Type'.",
        "the invoke names Type: {methods:?}"
    );
}

/// The `dynamic` keyword head is the type literal too (`dynamic.foo` over
/// a static-extension bait reports the `Type` face).
#[test]
fn dynamic_keyword_head_names_type() {
    let report = report_ops(
        "extension E on dynamic {\n\
         \x20 static int get foo => 0;\n\
         }\n\
         void f() {\n\
         \x20 dynamic.foo;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(
        getters[0], "The getter 'foo' isn't defined for the type 'Type'.",
        "the dynamic literal names Type: {getters:?}"
    );
}

/// `Never` spelled as the receiver head is the type literal (`Never.gone`
/// names `Type`, never `Never`).
#[test]
fn never_keyword_head_names_type() {
    let report = report_ops("void f() { Never.gone; }\n");
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(
        getters[0], "The getter 'gone' isn't defined for the type 'Type'.",
        "the Never literal names Type: {getters:?}"
    );
}

/// The keyword heads' invoke twin has no invocation-side face, so this face
/// owns it: `Never.bar()` reports `undefined_method` naming `Type` once.
#[test]
fn never_keyword_invoke_names_type() {
    let report = report_ops("void f() { Never.bar(); }\n");
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert_eq!(
        methods[0], "The method 'bar' isn't defined for the type 'Type'.",
        "the Never invoke names Type: {methods:?}"
    );
}

/// `dynamic.bar()` — the same single `Type` row.
#[test]
fn dynamic_keyword_invoke_names_type() {
    let report = report_ops("void f() { dynamic.bar(); }\n");
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert_eq!(
        methods[0], "The method 'bar' isn't defined for the type 'Type'.",
        "the dynamic invoke names Type: {methods:?}"
    );
}

/// A VALUE receiver of function type keeps the shape face's own display —
/// the head spells a formal, not a type (`cb.gone` names `void
/// Function()`).
#[test]
fn value_receiver_keeps_the_shape_display() {
    let report = report_ops(
        "void f(void Function() cb) {\n\
         \x20 cb.gone;\n\
         }\n",
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert_eq!(
        getters[0], "The getter 'gone' isn't defined for the type 'void Function()'.",
        "the value face keeps its display: {getters:?}"
    );
}

/// The `Object` face resolves on the `Type` instance (`Cb.hashCode` stays
/// silent).
#[test]
fn object_member_on_alias_literal_stays_silent() {
    let report = report_ops(
        "typedef Cb = void Function();\n\
         void f() { Cb.hashCode; }\n",
    );
    assert!(
        messages_named(&report, "undefined_getter").is_empty(),
        "the Object face resolves: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A dynamic-typed VALUE dispatches dynamically and stays silent — the
/// literal face is the keyword head's alone (`x.foo` over `dynamic x`).
#[test]
fn dynamic_typed_value_stays_silent() {
    let report = report_ops(
        "void f(dynamic x) {\n\
         \x20 x.foo;\n\
         }\n",
    );
    assert!(
        messages_named(&report, "undefined_getter").is_empty(),
        "a dynamic value dispatches: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
