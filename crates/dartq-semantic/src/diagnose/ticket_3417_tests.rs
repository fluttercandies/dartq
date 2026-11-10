//! #3417 pins — the type-literal receiver face (cluster C4) and the
//! prefixed built-in head (cluster C7).
//!
//! # The wrong fact
//!
//! A type-literal receiver's static type IS `Type` (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/ast/ast.dart
//! `TypeLiteral.staticType`), and the oracle resolves its members with the
//! ordinary property/invocation machinery: `Type`'s own interface first,
//! then every APPLICABLE extension of the whole import closure
//! (`extension_member_resolver.dart` `_getApplicable`). `row_resolution.rs`
//! read `Object`'s members alone as that face and probed extensions in the
//! diagnosing unit only, so every access an IMPORTED
//! `extension E on Object?` resolves reported `undefined_*` naming `Type`
//! (the `syntax_helper.dart` `expectStaticType` family, and the operator
//! twins `C > x` over an imported `operator >`).
//!
//! Two further shapes carried the same literal face where upstream does
//! not:
//!
//! - A cascade section over a MEMBER or SEND target
//!   (`C.new..foo()`, `Future.value(x)..foo()`): the section's receiver is
//!   that expression's value, never the `Type` instance — only a bare
//!   `C..foo()` target denotes the literal.
//! - An INTERFACE-typed type alias (`typedef A = C;`): upstream hands the
//!   ALIASED element to the static face
//!   (`_resolveTargetInterfaceElement`,
//!   pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:
//!   507-517; `_resolveReceiverTypeLiteral`,
//!   method_invocation_resolver.dart:168-179), so `A.sm()` resolves `C`'s
//!   statics instead of reading `A` as a `Type` instance.
//!
//! C7 is the `p.dynamic` spelling: a prefix whose own import edge exports
//! the built-in top type (`import 'dart:core' as core;`) resolves it
//! upstream (`named_type_resolver.dart`'s prefix path), while the type
//! position reported `undefined_class 'dynamic'` — the export scope never
//! carries the synthetic element.

use crate::diagnose::test_rig::{
    RigImport, assert_silent, code_names, report_ops, report_ops_imports, report_ops_with,
};

fn has(report: &crate::diagnose::FileReport, code: &str) -> bool {
    code_names(report).contains(&code)
}

// ---------------------------------------------------------------- C4 face

/// An imported `extension E on Object?` answers the instantiated type
/// literal's member (`A<int>.probe<int>()`), so nothing reports.
#[test]
fn t3417_imported_object_extension_spares_the_instantiated_literal() {
    let report = report_ops_imports(
        "class A<T> {} void test() { A<int>.probe<int>(); }",
        &[RigImport {
            uri: "package:a/ext.dart",
            prefix: None,
            source: "extension Probe on Object? { void probe<S>() {} }",
        }],
    );
    assert_silent(&report);
}

/// The same member declared on a CONCRETE `on` type never answers the
/// literal's lookup: the miss keeps its `Type` face.
#[test]
fn t3417_concrete_on_type_does_not_spare_the_literal() {
    let report = report_ops_imports(
        "class A<T> {} void test() { A<int>.probe<int>(); }",
        &[RigImport {
            uri: "package:a/ext.dart",
            prefix: None,
            source: "extension OnString on String { void probe<S>() {} }",
        }],
    );
    assert!(
        has(&report, "undefined_method"),
        "a concrete `on` type must not suppress the literal's miss, got {:?}",
        code_names(&report)
    );
}

/// An imported `extension E on Object?` declaring the operator resolves
/// `C > x` on the literal operand.
#[test]
fn t3417_imported_extension_operator_spares_the_literal_operand() {
    let report = report_ops_imports(
        "class C {} void test() { var b = C > 1; }",
        &[RigImport {
            uri: "package:a/track.dart",
            prefix: None,
            source: "extension Track on Object? { bool operator >(Object other) => true; }",
        }],
    );
    assert!(
        !has(&report, "undefined_operator"),
        "an imported `operator >` must spare the literal operand, got {:?}",
        code_names(&report)
    );
}

/// A cascade section over a SEND target (`C.make()..probe()`) resolves on
/// the send's value, never on the `Type` instance.
#[test]
fn t3417_cascade_over_a_send_target_is_not_a_type_literal() {
    let report = report_ops(
        "class C { static C make() => C(); void probe() {} } \
         void test() { C.make()..probe(); }",
    );
    assert_silent(&report);
}

/// The bare literal target keeps the `Type` face: `C..probe()` over a
/// static of `C` reports the miss naming `Type`.
#[test]
fn t3417_bare_literal_cascade_keeps_the_type_face() {
    let report = report_ops("class C { static void probe() {} } void test() { C..probe(); }");
    assert_eq!(
        code_names(&report),
        vec!["undefined_method"],
        "the bare type-literal cascade target keeps its Type face, got {:?}",
        code_names(&report)
    );
    assert!(
        report.diagnostics[0].message.contains("'Type'"),
        "expected the Type display, got {:?}",
        report.diagnostics[0].message
    );
}

/// An interface-typed alias head is the aliased class: `A.sm()` over
/// `typedef A = C;` resolves `C`'s static.
#[test]
fn t3417_interface_alias_resolves_the_aliased_statics() {
    let report =
        report_ops("class C { static void sm() {} } typedef A = C; void test() { A.sm(); }");
    assert_silent(&report);
}

/// A clean miss through an interface-typed alias still reports — the
/// aliased class's static face owns the verdict.
#[test]
fn t3417_interface_alias_miss_reports() {
    let report =
        report_ops("class C { static void sm() {} } typedef A = C; void test() { A.gone(); }");
    assert!(
        has(&report, "undefined_method"),
        "the aliased class's static miss must report, got {:?}",
        code_names(&report)
    );
}

/// A FUNCTION-typed alias keeps the literal face (`Fn.foo` over
/// `typedef Fn = void Function();` reports the getter miss).
#[test]
fn t3417_function_alias_keeps_the_literal_face() {
    let report = report_ops("typedef Fn = void Function(); void test() { Fn.foo; }");
    assert!(
        has(&report, "undefined_getter"),
        "an ungrounded function alias keeps its Type face, got {:?}",
        code_names(&report)
    );
}

// ---------------------------------------------------------------- C7 face

/// `core.dynamic` over `import 'dart:core' as core;` is the built-in top
/// type resolved through the prefix — never an undefined class.
#[test]
fn t3417_prefixed_core_dynamic_is_not_an_undefined_class() {
    let report = report_ops_with(
        "import 'dart:core' as core;\nvoid test() { core.dynamic x; x = 1; }",
        &[RigImport {
            uri: "dart:core",
            prefix: Some("core"),
            source: "",
        }],
        None,
    );
    assert!(
        !has(&report, "undefined_class"),
        "a dart:core prefix resolves the built-in top type, got {:?}",
        code_names(&report)
    );
}

/// A prefix bound to a library that does NOT export the built-in keeps the
/// miss: `core.dynamic` over a user library reports `undefined_class`.
#[test]
fn t3417_prefixed_builtin_over_a_foreign_prefix_still_reports() {
    let report = report_ops_with(
        "import 'package:a/other.dart' as core;\nvoid test() { core.dynamic x; x = 1; }",
        &[RigImport {
            uri: "package:a/other.dart",
            prefix: Some("core"),
            source: "class Q {}",
        }],
        None,
    );
    assert!(
        has(&report, "undefined_class"),
        "a non-core prefix keeps the undefined-class miss, got {:?}",
        code_names(&report)
    );
}
