//! Ticket #4339 pins: the `creation_with_non_type` invocation-seam cluster —
//! the generic named-constructor head (`C<T>.named()`), the shadowed-prefix
//! head, the function head behind the creation syntax, and the unresolved
//! prefix's span.
//!
//! * A generic named-constructor head over an unresolved class
//!   (`UnresolvedClass<int>.named()` in const/new/implicit spellings)
//!   reports `creation_with_non_type` at the HEAD segment alone — the
//!   `NamedType`'s name token, never the `<...>` group the expression
//!   spelling folds (upstream `_getErrorRange` ends at `node.name.end`;
//!   named_type_resolver.dart:584-593).
//! * A head whose prefix resolves to a NON-prefix, NON-type element (a
//!   top-level function shadowing the prefix position,
//!   `const nonPrefix.Class<int>()`) reports ONLY
//!   `prefix_shadowed_by_local_declaration` at the prefix token — the
//!   NamedType resolution dead-ends at the shadow arm and no arity /
//!   creation face follows (named_type_resolver.dart:113-128). The implicit
//!   spelling keeps the member chain instead: `nonPrefix.Class<int>.named()`
//!   resolves `Class` as a getter on the function value
//!   (`undefined_getter` for the type `void Function()`).
//! * A head that resolves to a FUNCTION (`NonType<int>.named()` over
//!   `void NonType<T>() {}`) is no creation at all: the selector resolves
//!   as a method on the type-argumented function reference
//!   (`undefined_method` for the type `Function` — the instantiation
//!   failure's fallback type).
//! * An unresolved prefix head (`unresolved.Class<int>.named()`) reports
//!   `undefined_identifier` at the PREFIX segment alone — the plain
//!   identifier resolution's own span, never the qualified span
//!   (`ast_rewrite` leaves the chain a method invocation when the prefix
//!   cannot resolve).

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};
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

/// The generic named-constructor head over an unresolved class: `const`
/// spelling reports `creation_with_non_type` at `UnresolvedClass` alone.
#[test]
fn const_unresolved_generic_named_reports_head_segment() {
    let fixture = "f() {\n  const UnresolvedClass<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("creation_with_non_type", "UnresolvedClass".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("The name 'UnresolvedClass' isn't a class."))
    );
}

/// The `new` spelling: the same head-segment verdict.
#[test]
fn new_unresolved_generic_named_reports_head_segment() {
    let fixture = "f() {\n  new UnresolvedClass<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("creation_with_non_type", "UnresolvedClass".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// The implicit spelling: the same head-segment verdict (the
/// `newWithNonType` face).
#[test]
fn implicit_unresolved_generic_named_reports_head_segment() {
    let fixture = "f() {\n  UnresolvedClass<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("creation_with_non_type", "UnresolvedClass".to_string())],
        "rows: {:?}",
        rows(&report)
    );
}

/// The shadowed-prefix head (`const nonPrefix.Class<int>()` over a
/// top-level `nonPrefix` function): ONLY
/// `prefix_shadowed_by_local_declaration` at the prefix token — the
/// constructor-arity face never follows the dead-end shadow arm.
#[test]
fn const_shadowed_prefix_generic_reports_shadow_alone() {
    let fixture = "void nonPrefix() {}\nf() {\n  const nonPrefix.Class<int>();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![(
            "prefix_shadowed_by_local_declaration",
            "nonPrefix".to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The `new` spelling of the shadowed-prefix generic head: the shadow row
/// alone.
#[test]
fn new_shadowed_prefix_generic_reports_shadow_alone() {
    let fixture = "void nonPrefix() {}\nf() {\n  new nonPrefix.Class<int>();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![(
            "prefix_shadowed_by_local_declaration",
            "nonPrefix".to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The generic-NAMED spelling of the shadowed-prefix head
/// (`const nonPrefix.Class<int>.named()`): the shadow row alone.
#[test]
fn const_shadowed_prefix_generic_named_reports_shadow_alone() {
    let fixture = "void nonPrefix() {}\nf() {\n  const nonPrefix.Class<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![(
            "prefix_shadowed_by_local_declaration",
            "nonPrefix".to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The implicit shadowed-prefix chain (`nonPrefix.Class<int>.named()`):
/// the member chain resolves `Class` as a getter on the function value —
/// `undefined_getter` for `void Function()` at `Class`, no creation or
/// arity face.
#[test]
fn implicit_shadowed_prefix_generic_named_reports_getter() {
    let fixture = "void nonPrefix() {}\nf() {\n  nonPrefix.Class<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("undefined_getter", "Class".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The getter 'Class' isn't defined")
            && row.contains("for the type 'void Function()'")
    }));
}

/// The function head behind the creation syntax (`NonType<int>.named()`
/// over a generic top-level function): the selector is a method on the
/// type-argumented function reference — `undefined_method` for
/// `Function` at `named`, never `creation_with_non_type`.
#[test]
fn implicit_function_generic_named_reports_method() {
    let fixture = "void NonType<T>() {}\nf() {\n  NonType<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("undefined_method", "named".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The method 'named' isn't defined") && row.contains("for the type 'Function'")
    }));
}

/// The prefixed function head (`p.NonType<int>.named()`): the same
/// method face. The function is declared in BOTH units — the corpus's
/// self-import shape, where the bare name resolves the same function the
/// prefix scope holds.
#[test]
fn prefixed_function_generic_named_reports_method() {
    let fixture = "void NonType<T>() {}\n\
                   f() {\n\
                   \x20 p.NonType<int>.named();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("p"),
            source: "void NonType<T>() {}\n",
        }],
    );
    assert_eq!(
        code_spans(&report, &full),
        vec![("undefined_method", "named".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The method 'named' isn't defined") && row.contains("for the type 'Function'")
    }));
}

/// The prefixed non-generic function head (`p.NonType.named()`): the same
/// method face, declared in both units (the self-import shape).
#[test]
fn prefixed_function_named_reports_method() {
    let fixture = "void NonType() {}\n\
                   f() {\n\
                   \x20 p.NonType.named();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("p"),
            source: "void NonType() {}\n",
        }],
    );
    assert_eq!(
        code_spans(&report, &full),
        vec![("undefined_method", "named".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The method 'named' isn't defined") && row.contains("for the type 'Function'")
    }));
}

/// The unresolved-prefix head (`unresolved.Class<int>.named()`):
/// `undefined_identifier` at the PREFIX segment alone — the plain
/// identifier's own span, never the qualified span.
#[test]
fn unresolved_prefix_generic_named_reports_prefix_segment() {
    let fixture = "f() {\n  unresolved.Class<int>.named();\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("undefined_identifier", "unresolved".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("Undefined name 'unresolved'."))
    );
}
