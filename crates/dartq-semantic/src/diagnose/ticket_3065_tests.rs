//! The #3065 pins — the `unused_element_parameter` accessibility and
//! constructor-credit faces (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart):
//!
//! * `_isPubliclyAccessible`'s extension arm (:852-876) answers
//!   `enclosingElement.isPublic` outright for extension members, and
//!   `ExtensionElement.isPublic` requires a name (element.dart:3264-3267)
//!   — an unnamed extension's optional parameters report exactly like a
//!   private extension's, while a public named extension's stay spared.
//! * `_isUsedElement`'s FormalParameterElement arm (:910-982) credits a
//!   parameter as used when its constructor is invoked with the argument
//!   (`_addParametersForArguments`, :394-399) — an unannotated
//!   constructor's givens ride the call-site supply, annotation-free.
//! * The primary-constructor faces: a primary constructor's optional
//!   field formals report like a body constructor's — the collector's
//!   `ctor_formals` table carries both.

use super::test_rig::report_ops;
use super::*;

/// The `unused_element_parameter` reported parameter names, source order.
fn uep_names(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element_parameter")
        .filter_map(|d| {
            let msg = &d.message;
            let start = msg.find('\'')? + 1;
            let end = msg[start..].find('\'')? + start;
            Some(msg[start..end].to_string())
        })
        .collect()
}

/// An unnamed extension is never publicly accessible upstream — its
/// optional parameters report like a private extension's, invocation or
/// not (`extension on String { void m([int? a]) {} }` over
/// `"hello".m()`: no argument is given, `a` reports).
#[test]
fn unnamed_extension_member_parameter_reports() {
    let fixture = "extension on String {\n\
                   \x20 void m([int? a]) {}\n\
                   }\n\
                   f() => 'hello'.m();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// The same unnamed-extension face with the argument supplied — the call
/// gives `a`, so the parameter stays silent (`_addParametersForArguments`).
#[test]
fn unnamed_extension_supplied_parameter_stays_silent() {
    let fixture = "extension on String {\n\
                   \x20 void m([int? a]) {}\n\
                   }\n\
                   f() => 'hello'.m(0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// A public named extension's member IS publicly accessible upstream (the
/// extension arm answers `isPublic`) — its optional parameters never
/// report.
#[test]
fn named_public_extension_member_parameter_stays_silent() {
    let fixture = "extension Ext on String {\n\
                   \x20 void m([int? a]) {}\n\
                   }\n\
                   f() => 'hello'.m();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// A private named extension's member is not publicly accessible — the
/// unsupplied optional reports (the gate the unnamed face shares).
#[test]
fn private_extension_member_parameter_reports() {
    let fixture = "extension _E on String {\n\
                   \x20 void m([int? a]) {}\n\
                   }\n\
                   f() => 'hello'.m();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// An unannotated constructor's invocation supplies its arguments with no
/// annotation involved (`_addParametersForArguments` reads the resolved
/// parameter elements) — `_A(f: 0)` spares the field formal.
#[test]
fn unannotated_ctor_invocation_gives_the_parameter() {
    let fixture = "class _A {\n\
                   \x20 final int? f;\n\
                   \x20 _A({this.f});\n\
                   }\n\
                   f() => _A(f: 0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// A primary constructor's optional field formal reports like a body
/// constructor's — the formal table joins `ctor_formals` under the
/// unnamed face's class-name key (`class _A({this.f})` over `_A()`).
#[test]
fn primary_ctor_field_formal_reports_when_ungiven() {
    let fixture = "class _A({this.f}) {\n\
                   \x20 final int? f;\n\
                   }\n\
                   f() => _A();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["f".to_string()]);
}

/// The same primary face with the named argument given — silent.
#[test]
fn primary_ctor_field_formal_given_stays_silent() {
    let fixture = "class _A({this.f}) {\n\
                   \x20 final int? f;\n\
                   }\n\
                   f() => _A(f: 0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// A primary constructor's regular optional formal
/// (`class _A._named([int? a]);`) reports when no creation supplies it —
/// the semicolon-bodied head registers no different from a body
/// constructor's table.
#[test]
fn primary_ctor_regular_formal_reports_when_ungiven() {
    let fixture = "class _A._named([int? a]);\n\
                   f() => _A._named();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// The supplied regular formal stays silent (`_A._named(0)` gives the
/// positional slot).
#[test]
fn primary_ctor_regular_formal_given_stays_silent() {
    let fixture = "class _A._named([int? a]);\n\
                   f() => _A._named(0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}
