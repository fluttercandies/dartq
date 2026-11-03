//! The #3844 `duplicate_definition` pins for `overrides.rs`'s own emission
//! surface. The file's only `DuplicateDefinition` emitter is
//! [`Engine::emit_wildcard_dups`] — the pre-wildcard-feature `_` second
//! declaration drain (upstream `_DuplicateIdentifierScope.isWildcard`,
//! `duplicate_definition_verifier.dart:350-360`: a `_` declares no name
//! under the feature, so the scope neither collides nor reports; a
//! pre-feature unit keeps `_` an ordinary name and reports each second
//! declaration as `duplicate_definition`). Every other
//! `duplicate_definition` row — member-scope pairs, top-level
//! declarations, formal-parameter collisions, locals — is captured into
//! `FileElements::duplicates` by `collector.rs` and rendered by
//! `diagnose::duplicate_definition_diagnostics`; `overrides.rs` only reads
//! the same log to suppress its own member checks at a duplicate span.
//!
//! These pins lock the two directions the drain owns:
//! - feature ON (wildcard variables enabled): a second `_` in a block, a
//!   `for` initializer, a `catch` clause or a formal list reports nothing
//!   — `_` never claims the scope;
//! - feature OFF (pre-3.7 posture): the same shapes report one
//!   `duplicate_definition` at the SECOND `_`'s name token, never the
//!   first — upstream `checkStatement`/`checkParameters` report at the
//!   duplicate's own token.
//!
//! Oracle provenance: `dart analyze` 3.13.7.

use super::test_rig::{code_names, report_ops_wildcards, rows};

/// `duplicate_definition` rows only, in emission order.
fn dup_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| format!("@{}:{}", d.range.offset, d.message))
        .collect()
}

/// Feature ON: a block's second `_` local declares nothing — silent.
#[test]
fn wildcard_block_second_underscore_stays_silent() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 var _ = 0;\n\
         \x20 var _ = 1;\n\
         }\n",
        true,
    );
    assert!(
        dup_rows(&report).is_empty(),
        "a wildcard `_` declares no name: {:#?}",
        rows(&report)
    );
}

/// Feature OFF: the same block reports `duplicate_definition` at the
/// second `_` — the ordinary-name reading keeps the scope claim.
#[test]
fn pre_wildcards_block_second_underscore_reports() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 var _ = 0;\n\
         \x20 var _ = 1;\n\
         }\n",
        false,
    );
    let dups = dup_rows(&report);
    assert_eq!(
        dups.len(),
        1,
        "the second `_` reports once: {dups:?} / {:#?}",
        rows(&report)
    );
    assert!(
        dups[0].contains("The name '_' is already defined."),
        "duplicate_definition message: {dups:?}"
    );
}

/// Feature ON: a `for` head's second `_` initializer reports nothing —
/// the wildcard never claims the for-scope either.
#[test]
fn wildcard_for_initializer_second_underscore_stays_silent() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 for (var _ = 0, _ = 1;;) {}\n\
         }\n",
        true,
    );
    assert!(
        dup_rows(&report).is_empty(),
        "a wildcard `_` in a for head declares no name: {:#?}",
        rows(&report)
    );
}

/// Feature OFF: the for head's second `_` reports once.
#[test]
fn pre_wildcards_for_initializer_second_underscore_reports() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 for (var _ = 0, _ = 1;;) {}\n\
         }\n",
        false,
    );
    let dups = dup_rows(&report);
    assert_eq!(dups.len(), 1, "the second `_` reports once: {dups:?}");
}

/// Feature ON: a formal list's second `_` parameter reports nothing —
/// upstream `checkParameters` skips a wildcard claim entirely
/// (`_FormalParameterDuplicateIdentifierScope.isWildcard`,
/// duplicate_definition_verifier.dart:454-466).
#[test]
fn wildcard_formal_second_underscore_stays_silent() {
    let report = report_ops_wildcards("void f(int _, int _) {}\n", true);
    assert!(
        dup_rows(&report).is_empty(),
        "a wildcard `_` formal never claims the scope: {:#?}",
        rows(&report)
    );
}

/// Feature OFF: a formal list's second `_` reports once.
#[test]
fn pre_wildcards_formal_second_underscore_reports() {
    let report = report_ops_wildcards("void f(int _, int _) {}\n", false);
    let dups = dup_rows(&report);
    assert_eq!(dups.len(), 1, "the second `_` reports once: {dups:?}");
}

/// Feature ON: a `catch` clause's second `_` reports nothing — the
/// wildcard posture holds inside the catch scope too.
#[test]
fn wildcard_catch_second_underscore_stays_silent() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 try {} catch (_, _) {}\n\
         }\n",
        true,
    );
    assert!(
        dup_rows(&report).is_empty(),
        "a wildcard `_` in a catch clause declares no name: {:#?}",
        rows(&report)
    );
}

/// Feature OFF: the catch clause's second `_` reports once.
#[test]
fn pre_wildcards_catch_second_underscore_reports() {
    let report = report_ops_wildcards(
        "void f() {\n\
         \x20 try {} catch (_, _) {}\n\
         }\n",
        false,
    );
    let dups = dup_rows(&report);
    assert_eq!(dups.len(), 1, "the second `_` reports once: {dups:?}");
}

/// The member-scope half the drain must NOT shadow: a same-name instance
/// field pair still reports `duplicate_definition` through the capture
/// log (`collector.rs`'s `member_dup_claim` → `record_duplicate` —
/// `overrides.rs` never owns that row), and the wildcard drain emits no
/// second row for it. Feature ON keeps the member check identical.
#[test]
fn member_field_field_duplicate_reports_once_either_direction() {
    for wildcards in [true, false] {
        let report = report_ops_wildcards(
            "class C {\n\
             \x20 int baz = 0;\n\
             \x20 int baz = 1;\n\
             }\n",
            wildcards,
        );
        let dups = dup_rows(&report);
        assert_eq!(
            dups.len(),
            1,
            "wildcards={wildcards}: one duplicate_definition row: {dups:?}"
        );
        assert!(
            dups[0].contains("The name 'baz' is already defined."),
            "wildcards={wildcards}: the member's own name renders: {dups:?}"
        );
    }
}

/// A duplicate member's span suppresses `overrides.rs`'s own
/// `invalid_override`/`conflicting_static_and_instance` probes on that
/// span (the `elements.duplicates()` gate at `check_override` /
/// `check_non_virtual_override`): the second `foo` never draws an
/// override verdict on top of its `duplicate_definition` — the scope's
/// kept first declaration answers the face instead.
#[test]
fn duplicate_member_span_emits_no_override_row() {
    let report = report_ops_wildcards(
        "class A {\n\
         \x20 int foo() => 0;\n\
         }\n\
         class C extends A {\n\
         \x20 int foo() => 0;\n\
         \x20 int foo() => 1;\n\
         }\n",
        true,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"duplicate_definition"),
        "the member duplicate still reports: {names:?}"
    );
    assert!(
        !names.contains(&"invalid_override") && !names.contains(&"invalid_implementation_override"),
        "a duplicate span never draws the override face: {names:?}"
    );
}
