//! The #3016 pins — the `unused_element_parameter` non-call read and
//! redirect-factory correspondence faces (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart):
//!
//! * `visitSimpleIdentifier` (:318-326): an executable element read in a
//!   NON-call position — a method/function/constructor tear-off, or a
//!   `super.x()`/`this.x()` initializer the AST never shapes as an
//!   `InstanceCreationExpression` — credits every formal it declares.
//!   A real invocation credits only the supplied arguments
//!   (`_addParametersForArguments`, :394-399).
//! * `visitConstructorDeclaration`'s redirect arm (:102-114) +
//!   `_matchParameters` (:458-498): a redirecting factory supplies its
//!   target's parameters only where its own written list declares a
//!   corresponding formal (positional index, or same name for named).
//!   `factory T.named() = T` forwards nothing, so `T`'s optional formals
//!   still report.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unused_element_parameter` rows as the exact anchored source text,
/// source order.
fn uep_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element_parameter")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The `unused_element_parameter` row COUNT plus reported parameter names
/// as written in the messages (anchor spans are a separate face — a
/// factory's own formal may anchor on the selector head while the
/// element-level row still names the formal).
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

/// A method tear-off read credits every parameter — `a._m` resolves the
/// executable as a value, so `a` is used upstream even though no
/// invocation supplies it.
#[test]
fn method_tearoff_credits_all_parameters() {
    let fixture = "class _A {\n\
                   \x20 void _m([int? a]) {}\n\
                   }\n\
                   void f(_A a) => a._m;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(uep_spans(&report, &full), Vec::<String>::new());
}

/// A `super._named()` initializer invocation is a non-
/// `InstanceCreationExpression` read upstream — the super constructor's
/// optional formals are all used even with no argument supplied.

/// A top-level function tear-off credits its formals identically — the
/// bare `_f` read resolves the element in a non-call position.
#[test]
fn top_level_tearoff_credits_all_parameters() {
    let fixture = "void _f([int? a]) {}\n\
                   final g = _f;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(uep_spans(&report, &full), Vec::<String>::new());
}

/// A `super._named()` initializer invocation is a non-
/// `InstanceCreationExpression` read upstream — the super constructor's
/// optional formals are all used even with no argument supplied.
#[test]
fn super_named_invocation_credits_target_parameters() {
    let fixture = "class _B {\n\
                   \x20 _B._named([int? x]);\n\
                   }\n\
                   class _C extends _B {\n\
                   \x20 _C() : super._named();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(uep_spans(&report, &full), Vec::<String>::new());
}

/// A real invocation supplies only its written arguments — `_A()` with
/// no `f:` given still reports the unsupplied optional.
#[test]
fn invocation_supplies_only_written_arguments() {
    let fixture = "class _A {\n\
                   \x20 final int? f;\n\
                   \x20 _A({this.f});\n\
                   }\n\
                   f() => _A();\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(uep_spans(&report, &full), vec!["f".to_string()]);
}

/// A redirecting factory with an EMPTY written list forwards nothing —
/// `_matchParameters` matches zero pairs, so the target's optional
/// formals still report (the `factory _A.named() = _A` shape).
#[test]
fn empty_redirecting_factory_supplies_nothing() {
    let fixture = "class _A {\n\
                   \x20 final int? f;\n\
                   \x20 _A({this.f});\n\
                   \x20 factory _A.named() = _A;\n\
                   }\n\
                   f() => _A.named();\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(uep_spans(&report, &full), vec!["f".to_string()]);
}

/// A redirecting factory whose written list declares the corresponding
/// named formal supplies it — `_matchParameters` matches `f`, sparing the
/// TARGET's row — while the factory's own unsupplied `f` still reports
/// (the `ConstructorElement` arm applies to both ends upstream).
#[test]
fn redirecting_factory_matching_formal_supplies() {
    let fixture = "class _A {\n\
                   \x20 final int? f;\n\
                   \x20 _A({this.f});\n\
                   \x20 factory _A.named({int? f}) = _A;\n\
                   }\n\
                   f() => _A.named();\n";
    let report = report_ops(fixture);
    // Oracle (3.13.5): `_A`'s `f` is spared by the factory's corresponding
    // formal; the factory's own `f` still reports — exactly one row.
    assert_eq!(uep_names(&report), vec!["f".to_string()]);
}

/// A redirecting factory to a DIFFERENT target spares nothing on this
/// constructor — `factory _A.named() = _B` leaves `_A`'s own optional
/// formal unsupplied.
#[test]
fn redirect_to_other_class_supplies_nothing() {
    let fixture = "class _A {\n\
                   \x20 final int? f;\n\
                   \x20 _A({this.f});\n\
                   \x20 factory _A.named() = _B;\n\
                   }\n\
                   class _B {\n\
                   \x20 final int? g;\n\
                   \x20 _B({this.g});\n\
                   }\n\
                   f() => _A();\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    // Oracle (3.13.5): `_A`'s `f` AND `_B`'s `g` both report — the
    // `= _B` redirect spares nothing on `_A`, and `_B` is never invoked.
    assert_eq!(
        uep_spans(&report, &full),
        vec!["f".to_string(), "g".to_string()]
    );
}
