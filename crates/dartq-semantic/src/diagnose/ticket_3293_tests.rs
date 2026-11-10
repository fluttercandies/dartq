//! #3293 pins — the `const_constructor_with_non_final_field` declaration
//! face.
//!
//! # The face
//!
//! Upstream `_checkForConstConstructorWithNonFinalField`
//! (error_verifier.dart:4098-4112, reached from the `ConstructorDeclaration`
//! / `PrimaryConstructorDeclaration` visitors at :697-712 / :1766-1790): a
//! `const` GENERATIVE constructor reports when its enclosing class carries a
//! non-final instance field — its own, its superclass chain's, or its mixins'
//! (`hasNonFinalField`, summary2/link.dart:295-322). Factories are not
//! generative; enums, mixins, and extension types are not `ClassElement`s.
//!
//! The `errorRange` is the constructor's own: a secondary head's type name
//! through its selector (`C`, `C.named`, the bare `new` head), a primary
//! head's `const` keyword through the constructor name.
//!
//! The two upstream siblings that pre-empt this row keep it silent, and
//! dartq implements neither, so this face carries their gates: a class mixin
//! adding an instance field (`const_constructor_with_mixin_with_field[s]`)
//! and an invoked non-const super constructor
//! (`const_constructor_with_non_const_super`).

use super::test_rig::{CORE_PRELUDE, code_names, report_ops};
use crate::diagnose::FileReport;

/// The fixture source as the rig analyzes it (the prelude first), so a row's
/// span reads back as its verbatim text.
fn analyzed(fixture: &str) -> String {
    format!("{CORE_PRELUDE}{fixture}")
}

/// The verbatim texts of the face's rows, in source order.
fn row_texts(source: &str, report: &FileReport) -> Vec<(u32, String)> {
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
    rows
}

/// A class with a non-final field reports at the constructor's type name.
#[test]
fn ticket_3293_unnamed_constructor_reports_at_the_type_name() {
    let fixture = "class C { int v; const C(this.v); }\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    let texts: Vec<String> = row_texts(&source, &report)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(texts, vec!["C".to_string()], "{:?}", code_names(&report));
}

/// A `final` field answers no row.
#[test]
fn ticket_3293_final_field_stays_silent() {
    let fixture = "class C { final int v; const C(this.v); }\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A const factory is not generative: no row.
#[test]
fn ticket_3293_const_factory_stays_silent() {
    let fixture = "class C { int v; const factory C() = D; }\nclass D { const D(); }\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// An enum's generative constructor is exempt (`enclosingElement is!
/// ClassElement`), const or implicit-const alike.
#[test]
fn ticket_3293_enum_stays_silent() {
    let fixture = "enum E { a; int v = 0; const E(this.v); }\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A class whose superclass only declares a non-const unnamed constructor
/// pre-empts the row (`const_constructor_with_non_const_super` owns it).
#[test]
fn ticket_3293_non_const_super_stays_silent() {
    let fixture = "class A { A(); }\nclass B extends A { int v; const B(); }\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A mixin adding an instance field pre-empts the row
/// (`const_constructor_with_mixin_with_field` owns it).
#[test]
fn ticket_3293_mixin_field_stays_silent() {
    let fixture = "mixin M { int m = 0; }\nclass C with M { int v; const C(); }\n";
    let report = report_ops(fixture);
    assert!(
        row_texts(&analyzed(fixture), &report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// The named secondary head spans the type name through its selector, even
/// when the field is declared after the constructor.
#[test]
fn ticket_3293_named_constructor_reports_through_the_selector() {
    let fixture = "class C { const C.named(); int v; }\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    let texts: Vec<String> = row_texts(&source, &report)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        vec!["C.named".to_string()],
        "{:?}",
        code_names(&report)
    );
}

/// `hasNonFinalField` climbs the superclass chain: the subclass's const
/// constructor reports over the inherited non-final field, and so does the
/// superclass's own (each class element answers its own rows).
#[test]
fn ticket_3293_inherited_field_reports() {
    let fixture = "class A { int v; const A(); }\nclass B extends A { const B(); }\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    let texts: Vec<String> = row_texts(&source, &report)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        vec!["A".to_string(), "B".to_string()],
        "{:?}",
        code_names(&report)
    );
}

/// A primary constructor's `errorRange` ends at the `const` keyword when the
/// constructor is unnamed.
#[test]
fn ticket_3293_unnamed_primary_reports_the_const_keyword() {
    let fixture = "class const C() { int v = 0; }\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    let texts: Vec<String> = row_texts(&source, &report)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        vec!["const".to_string()],
        "{:?}",
        code_names(&report)
    );
}

/// A named primary constructor spans `const` through the constructor name.
#[test]
fn ticket_3293_named_primary_reports_through_the_name() {
    let fixture = "class const C.named() { int v = 0; }\n";
    let source = analyzed(fixture);
    let report = report_ops(fixture);
    let texts: Vec<String> = row_texts(&source, &report)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        vec!["const C.named".to_string()],
        "{:?}",
        code_names(&report)
    );
}
