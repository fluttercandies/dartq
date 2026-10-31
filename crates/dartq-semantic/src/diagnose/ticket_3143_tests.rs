//! #3143 — the `undefined_method` display and receiver-class faces.
//!
//! The oracle renders the method face and the read/write faces through
//! different type displays: `undefined_method` names the receiver's bare
//! element display — never the written type arguments or nullability
//! (`C<String>` renders `C`, `B?` renders `B`; upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/method_invocation_resolver.dart's
//! `receiverClassName` — only an `InterfaceType`'s element name escapes
//! the `<unknown>` default) — while `undefined_getter`/`undefined_setter`
//! keep the full `getDisplayString` render WITH type arguments but
//! WITHOUT nullability (probe: `C<String> c; c.gone;` names `C<String>`,
//! `B<int>? b; b?.gone;` names `B<int>`). The invalid-receiver split
//! follows the same asymmetry: the invocation face names `<unknown>`, the
//! read face names `InvalidType` (probe: `var x = Undefined.new;` names
//! `InvalidType`, `var x = Undefined.new();` names `<unknown>`).

use super::test_rig::report_ops;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    rows_named(report, name)
        .into_iter()
        .map(|d| d.message.to_string())
        .collect()
}

// —— The method face names the bare element display ————————————————

/// A generic receiver's method miss names `C`, never the written
/// instantiation `C<String>` (oracle: `C<String> c; c.dMethod();` reports
/// "The method 'dMethod' isn't defined for the type 'C'."); the getter
/// face on the same receiver keeps the full render (`C<int> b; b.gone;`
/// names `C<int>`).
#[test]
fn method_face_names_the_bare_element_getter_keeps_arguments() {
    let report = report_ops(
        "class C<T> {}\n\
         class B<T> {}\n\
         void main() {\n\
         \x20 C<String> c;\n\
         \x20 c.dMethod();\n\
         \x20 B<int> b;\n\
         \x20 b.gone;\n\
         }\n",
    );
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert!(
        methods[0].contains("the type 'C'."),
        "method miss names the bare element: {methods:?}"
    );
    assert!(
        !methods[0].contains("C<String>"),
        "method face drops the written arguments: {methods:?}"
    );
    let getters = messages_named(&report, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'B<int>'."),
        "getter face keeps the written arguments: {getters:?}"
    );
}

/// A nullable receiver's null-aware method miss names `B`, never `B?`
/// (corpus: `var b = new C() as B?; b?.f(...)` reports "The method 'f'
/// isn't defined for the type 'B'."),
#[test]
fn null_aware_method_face_strips_nullability() {
    let report = report_ops(
        "class B {}\n\
         void f(B? b) {\n\
         \x20 b?.g();\n\
         }\n",
    );
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert!(
        methods[0].contains("the type 'B'."),
        "null-aware method miss names the non-null face: {methods:?}"
    );
}

/// The promoted-chain settlement's re-rendered method face names the
/// settled face's bare head (`l.removeMiddle()` under `l is
/// List<String>` names `List`, never `List<String>`); the setter side of
/// the same re-render keeps the full display.
#[test]
fn settled_chain_method_face_names_the_bare_head() {
    let report = report_ops(
        "void f(List<int> l) {\n\
         \x20 if (l is List<String>) {\n\
         \x20   l.removeMiddle();\n\
         \x20 }\n\
         }\n",
    );
    let methods = messages_named(&report, "undefined_method");
    assert!(!methods.is_empty(), "the miss reports: {:?}", report.diagnostics.iter().map(|d| (d.code.name, d.message.to_string())).collect::<Vec<_>>());
    assert!(
        methods.iter().all(|m| m.contains("the type 'List'.")),
        "settled method miss names the bare head: {methods:?}"
    );
    assert!(
        methods.iter().all(|m| !m.contains("List<String>")),
        "settled method face drops the arguments: {methods:?}"
    );
}

// —— The shape-receiver displays ———————————————————————————————————

/// A function-typed receiver names `Function`, a record receiver names
/// `<unknown>` (upstream `receiverClassName` — only an `InterfaceType` or
/// `FunctionType` escapes the default,
/// method_invocation_resolver.dart).
#[test]
fn function_and_record_receivers_keep_their_shape_displays() {
    let report = report_ops(
        "void f(void Function() g, (int, int) r) {\n\
         \x20 g.bar();\n\
         \x20 r.baz();\n\
         }\n",
    );
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 2, "two method misses: {methods:?}");
    assert!(
        methods.iter().any(|m| m.contains("the type 'Function'.")),
        "function receiver names Function: {methods:?}"
    );
    assert!(
        methods.iter().any(|m| m.contains("the type '<unknown>'.")),
        "record receiver names <unknown>: {methods:?}"
    );
}

/// A static-container miss keeps naming the container's own spelling
/// (`A.staticFoo()` names `A` — the `StaticLookupDomain`'s
/// `interface.displayName`, lookup_failure_reporter.dart:417-420).
#[test]
fn static_container_miss_names_the_container() {
    let report = report_ops("class A {}\nvoid f() { A.staticFoo(); }\n");
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert!(
        methods[0].contains("the type 'A'."),
        "static miss names the container: {methods:?}"
    );
}

// —— The `new` selector's invalid-receiver split ——————————————————

/// The `new` selector's invocation face names `<unknown>` on an unresolved
/// receiver while the read face names `InvalidType` (oracle:
/// `var x = C.new();` with `C` unresolved reports "The method 'new' isn't
/// defined for the type '<unknown>'."; `var x = Undefined.new;` reports
/// "The getter 'new' isn't defined for the type 'InvalidType'.").
#[test]
fn new_selector_invocation_names_unknown_read_names_invalid_type() {
    let report = report_ops("void main() { var x = Missing.new(); }\n");
    let identifiers = messages_named(&report, "undefined_identifier");
    assert!(!identifiers.is_empty(), "the head miss reports: {identifiers:?}");
    let methods = messages_named(&report, "undefined_method");
    assert_eq!(methods.len(), 1, "one method miss: {methods:?}");
    assert!(
        methods[0].contains("the type '<unknown>'."),
        "invoked `new` names <unknown>: {methods:?}"
    );
    let reads = report_ops("void main() { var x = Missing.new; }\n");
    let getters = messages_named(&reads, "undefined_getter");
    assert_eq!(getters.len(), 1, "one getter miss: {getters:?}");
    assert!(
        getters[0].contains("the type 'InvalidType'."),
        "read `new` names InvalidType: {getters:?}"
    );
}

// —— The oracle's silences stay silent —————————————————————————————

/// A `dynamic` receiver never reports the member family; a defined member
/// never reports.
#[test]
fn dynamic_and_defined_receivers_stay_silent() {
    let report = report_ops(
        "class A { void foo() {} }\n\
         void f(dynamic x, A a) {\n\
         \x20 x.missing();\n\
         \x20 a.foo();\n\
         }\n",
    );
    assert!(
        rows_named(&report, "undefined_method").is_empty(),
        "dynamic dispatch and defined members never report: {:?}",
        report.diagnostics.iter().map(|d| d.code.name).collect::<Vec<_>>()
    );
}
