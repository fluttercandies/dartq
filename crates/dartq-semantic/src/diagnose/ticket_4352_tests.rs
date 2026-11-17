//! #4352 pins — the primary constructor's `errorRange` under an
//! augmentation of the declaring class.
//!
//! # The defect
//!
//! A named primary constructor's errorRange recomposes from the head
//! anchor (`(`/`.`) plus the declaring class's name token. The name token
//! was read as the file's LAST same-name Container site, but the const
//! face's flush re-runs at every same-name declaration close — including
//! a later `augment class <same name>`'s — when that last site is the
//! augmentation's own name, sitting PAST the original header's head: the
//! named-primary span subtracted `end - start` across the two
//! declarations and underflowed (the three co19 Augmentations crashes),
//! while the unnamed variant silently anchored at the augmentation's
//! name and dropped its `const` row.
//!
//! # The oracle
//!
//! Upstream anchors at the declaration OWNING the head — the
//! `PrimaryConstructorDeclaration` node carries its own `typeName`, and
//! `errorRange` spans `beginToken` (the `const` keyword when written,
//! else that own type name) through `constructorName ?? beginToken`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart,
//! `PrimaryConstructorDeclarationImpl.errorRange`). The owning site here
//! is the last same-name Container site whose name token ends at or
//! before the head — inside one header the name always precedes the
//! `(`/`.` head token, and a later augmentation's site starts past it.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, code_names, report_ops};
use crate::diagnose::FileReport;

/// The fixture source as the rig analyzes it (the prelude first), so a
/// row's span reads back as its verbatim text.
fn analyzed(fixture: &str) -> String {
    format!("{CORE_PRELUDE}{fixture}")
}

/// The verbatim texts of the face's rows, in source order.
fn row_texts(source: &str, report: &FileReport) -> Vec<String> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "const_constructor_with_non_final_field")
        .map(|row| {
            let start = row.range.offset as usize;
            let end = start + row.range.length as usize;
            (row.range.offset, source[start..end].to_string())
        })
        .collect();
    rows.sort_by_key(|(offset, _)| *offset);
    rows.into_iter().map(|(_, text)| text).collect()
}

/// The `complete_declarations_A01_t25` C3 shape: a named primary with a
/// body, the non-final field in the ORIGINAL class, and a later
/// augmenting class closing over the same name. The face flush at the
/// augmentation's close must not underflow the owning header's span, and
/// a `const`-less header reports nothing.
#[test]
fn ticket_4352_named_primary_with_augmenting_class_does_not_underflow() {
    let fixture = "class C3.foo(int v) {\n  int v;\n  this : v = v;\n}\n\
augment class C3 {\n  augment C3.foo(int v);\n}\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The `augmenting_constructors_A09_t09` C2 shape: a semicolon-terminated
/// named primary completed by a body-bearing augmentation, the non-final
/// field joining from the augmenting class. Same flush, same requirement.
#[test]
fn ticket_4352_incomplete_named_primary_completed_by_augmentation() {
    let fixture = "class C2.foo(int v);\n\
augment class C2 {\n  int? _v;\n  augment C2.foo(int v) {\n    _v = v;\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A `const` named primary anchors at the OWNING header — the `const`
/// keyword through the constructor name — even though the flush runs at
/// the later augmentation's close (the underflowing span before #4352).
#[test]
fn ticket_4352_const_named_primary_anchors_at_the_owning_header() {
    let fixture = "class const C2.foo(int v);\n\
augment class C2 {\n  int v;\n  augment C2.foo(int v) {}\n}\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    assert_eq!(
        row_texts(&source, &report),
        vec!["const C2.foo".to_string()],
        "{:?}",
        code_names(&report)
    );
}

/// The unnamed variant never underflowed but silently dropped its row:
/// the span anchored at the augmentation's name token, whose lexeme is
/// not `const`. The owning header's `const` keyword carries the row.
#[test]
fn ticket_4352_const_unnamed_primary_keeps_its_const_row() {
    let fixture = "class const C1(int v) {}\n\
augment class C1 {\n  int v;\n  augment C1(int v);\n}\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    assert_eq!(
        row_texts(&source, &report),
        vec!["const".to_string()],
        "{:?}",
        code_names(&report)
    );
}
