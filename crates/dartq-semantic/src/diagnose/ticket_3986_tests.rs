//! #3986 pins — local declarations consume `pending_annotations`.
//!
//! The `pending_annotations` latch used to drain only through
//! `note_decl_site` (`unused_finalization.rs`), and local declarations own
//! no `DeclKind` — `C::LocalVariableDeclaration` runs `note_local`,
//! `C::LocalFunctionDeclaration` runs `note_local_function`. The latch
//! therefore sat forever (or leaked into the next unrelated declaration's
//! drain) and the oracle's `undefined_annotation` rows for the
//! `local_ampersand_*` / `local_leftParen_*` families never emitted.
//!
//! Upstream consumes the metadata on every surviving `AnnotatedNode` —
//! `visitVariableDeclarationList` / `visitFunctionDeclaration` reach
//! `metadata.accept` → `annotation_resolver._resolve`
//! (`annotation_resolver.dart` :240-245) — so the drain now runs at
//! `begin_variables_declaration` (gated off class-member field groups) and
//! `begin_local_function_declaration` (unconditional), each BEFORE the
//! declared name binds.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rows of one code, in source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<String> {
    let mut at: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    at.sort_by_key(|(offset, _)| *offset);
    at.into_iter().map(|(_, text)| text).collect()
}

/// `f() { @a var x = 0; }` — the well-formed local annotation's
/// `undefined_annotation 'a'` now drains at `begin_variables_declaration`.
/// Oracle: `UNDEFINED_ANNOTATION 1:7 len 2` (`@a`).
#[test]
fn local_var_annotation_undefined() {
    let report = report_ops("f() { @a var x = 0; }\n");
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        ["Undefined name 'a' used as an annotation."]
    );
}

/// `f() { @ assert (true); }` — the `local_ampersand_*` representative:
/// `assert` is a reserved keyword, so the recovery inserts the synthetic
/// `''` name and the metadata event still fires. The pending `'`
/// annotation drains at the recovered `<unnamed>` variable group's
/// `begin_variables_declaration` — the oracle's `undefined_annotation ''`.
#[test]
fn local_ampersand_synthetic_drains() {
    let report = report_ops("f() { @ assert (true); }\n");
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        ["Undefined name '' used as an annotation."]
    );
}

/// `f() { @a( assert (true); }` — the `local_leftParen_*` representative:
/// `@a(` opens the argument group, the real name `a` pushes, and
/// `end_metadata` latches the whole `@a(...)` node; the recovered
/// `<unnamed>` group drains it. Oracle: `UNDEFINED_ANNOTATION 'a'` over the
/// 17-width annotation node.
#[test]
fn local_left_paren_drains() {
    let report = report_ops("f() { @a( assert (true); }\n");
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        ["Undefined name 'a' used as an annotation."]
    );
}

/// `f() { @ int g() {} }` — a local FUNCTION consumes its metadata:
/// the `pending_annotations` latch drains at
/// `begin_local_function_declaration` (unconditional — the event exists
/// only for local functions). The prelude's `int` resolves to a class whose
/// unnamed constructor is non-const, so the verdict arm reports
/// `non_constant_annotation_constructor` at the `@ int` node — proof the
/// latch emptied here rather than leaking to a later site.
#[test]
fn local_function_annotation_drains() {
    let report = report_ops("f() { @ int g() {} }\n");
    assert_eq!(
        rows_of(&report, "non_constant_annotation_constructor"),
        ["Annotation creation can only call a const constructor."]
    );
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        Vec::<String>::new()
    );
}

/// Non-regression: a class-member field group stays routed through
/// `note_decl_site` — `class C { @a var f; }` keeps the same
/// `undefined_annotation 'a'` (the `!field_group_open` gate skips the
/// local drain; the member's `FieldDeclaration` site still emits it).
#[test]
fn member_field_annotation_still_drains_via_decl_site() {
    let report = report_ops("class C { @a var f; }\n");
    assert_eq!(
        rows_of(&report, "undefined_annotation"),
        ["Undefined name 'a' used as an annotation."]
    );
}
