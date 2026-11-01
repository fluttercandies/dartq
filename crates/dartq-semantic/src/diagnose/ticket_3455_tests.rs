//! The #3455 pins — the `undefined_*` code-choice faces: a member-shaped or
//! creation-shaped reference must report its member/creation code, and
//! `undefined_identifier` stays with the bare-scope miss alone.
//!
//! Two production sites, both off the same upstream rule:
//!
//! - A generic instantiation head (`foo<int>` — the oracle's
//!   `ReadSyntax.typeInstantiation`) resolves through the unqualified
//!   lookup's `this` face: `undefined_method` naming `this`'s type where one
//!   exists (an extension body answers its `on` type), `undefined_identifier`
//!   where none does (`lookup_failure_reporter.dart`
//!   `UnqualifiedLookupDomain._readFailure`'s typeInstantiation arm).
//! - A `new`/`const` creation head whose element resolves to a NON-TYPE
//!   keeps the creation context's `creation_with_non_type` at the head
//!   token; only a head whose element lookup answers `null` takes the
//!   `undefined_identifier`-over-the-span verdict
//!   (`named_type_resolver.dart`'s `resolveConstructorTypeReference`
//!   tail).
//!
//! Oracle provenance: `dart analyze` 3.13.7 — upstream
//! `FunctionReferenceResolutionTest.test_extensionMethod_unknown`
//! (`foo<int>` inside `extension on double` reports `undefined_method` for
//! 'double'), `test_instanceMethod_unknown` (`foo<int>` inside `class A`
//! for 'A'), `test_unknownIdentifier` (`foo<int>` in a top-level function
//! keeps `undefined_identifier`), and
//! `CreationWithNonTypeTest.test_const_nonType_genericNamed`
//! (`const NonType<int>.named()` reports `creation_with_non_type` at
//! `NonType`).

use super::test_rig::{assert_silent, code_names, report_ops};

/// `foo<int>` inside an extension instance member reports
/// `undefined_method` naming the `on` type — the corpus case verbatim.
#[test]
fn type_instantiation_miss_inside_extension_names_on_type() {
    let report = report_ops(
        "extension on double {\n\
           bar() {\n\
             foo<int>;\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_method"),
        "expected undefined_method, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_identifier"),
        "unexpected undefined_identifier beside the member code, got {names:?}"
    );
}

/// `foo<int>` inside a class instance member names `this`'s type —
/// upstream's `test_instanceMethod_unknown` (`undefined_method` for 'A').
#[test]
fn type_instantiation_miss_inside_class_names_this_type() {
    let report = report_ops(
        "class A {\n\
           bar() {\n\
             foo<int>;\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_method"),
        "expected undefined_method, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_identifier"),
        "unexpected undefined_identifier beside the member code, got {names:?}"
    );
}

/// `foo<int>` where no `this` exists keeps the bare-scope face —
/// upstream's `test_unknownIdentifier` (`undefined_identifier` in a
/// top-level function body).
#[test]
fn type_instantiation_miss_without_this_stays_identifier() {
    let report = report_ops(
        "void bar() {\n\
           foo<int>;\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_identifier"),
        "expected undefined_identifier, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_method"),
        "unexpected undefined_method without a this face, got {names:?}"
    );
}

/// A `const` creation head resolving to a top-level generic FUNCTION
/// keeps the creation context's `creation_with_non_type` at the head —
/// upstream's `test_const_nonType_genericNamed`. The requalified
/// `undefined_identifier`-over-`NonType<int>` face must not fire.
#[test]
fn const_non_type_generic_named_head_reports_creation_code() {
    let report = report_ops(
        "void NonType<T>() {}\n\
         f() {\n\
           const NonType<int>.named();\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"creation_with_non_type"),
        "expected creation_with_non_type, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_identifier"),
        "unexpected undefined_identifier beside the creation code, got {names:?}"
    );
}

/// The requalification face itself is untouched: a creation head whose
/// element lookup answers `null` still reports `undefined_identifier`
/// over the written two-segment span (upstream
/// `resolveConstructorTypeReference`'s importPrefix tail).
#[test]
fn missing_qualified_creation_head_keeps_identifier_requalify() {
    let report = report_ops(
        "f() {\n\
           const Missing.new();\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_identifier"),
        "expected undefined_identifier, got {names:?}"
    );
    assert!(
        !names.contains(&"creation_with_non_type"),
        "unexpected creation_with_non_type for a null element, got {names:?}"
    );
}
