//! The #3069 pins — the call-site givens the `unused_element_parameter`
//! rule reads (`_addParametersForArguments`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart:394-399)
//! for the invocations that slipped the recording:
//!
//! * the reserved `new` selector keys the givens under the class segment
//!   it hangs off — `C.new(used)` credits the `C`-keyed formal table (the
//!   collector's own `A()`/`A.new` normalization), not a `new` key no
//!   site reads;
//! * a dot-shorthand send (`.m(...)`) credits its arguments through the
//!   selector spelling (`visitDotShorthandInvocation`, :129-134) with no
//!   callee row resolved;
//! * an enum constant's positional arguments join the named ones
//!   (`visitEnumConstantDeclaration`, :146-153) — `E(0)` gives the
//!   selected constructor's first positional formal.

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

/// `C.new(0)` over an unnamed-declared constructor gives the optional
/// positional: the `new` selector keys the givens under `C`, the spelling
/// the formal table sealed on.
#[test]
fn new_selector_over_unnamed_ctor_gives_positional() {
    let fixture = "class _C {\n\
                   \x20 const _C([int? a]);\n\
                   }\n\
                   f() => _C.new(0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// The same credit over an explicitly `_C.new`-declared head — the class
/// name is that declaration's `ctor_formals` key too.
#[test]
fn new_selector_over_explicit_new_decl_gives_positional() {
    let fixture = "class _C {\n\
                   \x20 const _C.new([int? a]);\n\
                   }\n\
                   f() => _C.new(0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// Negative: `_C.new()` supplying nothing still reports the optional.
#[test]
fn new_selector_without_arguments_reports() {
    let fixture = "class _C {\n\
                   \x20 const _C([int? a]);\n\
                   }\n\
                   f() => _C.new();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// `.m(0)` gives the static method's optional positional through the
/// selector spelling — no callee row answers the dot-shorthand send.
#[test]
fn dot_shorthand_static_method_gives_positional() {
    let fixture = "class _C {\n\
                   \x20 static void m([int? a]) {}\n\
                   }\n\
                   f() => .m(0);\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// Negative: the same shorthand with nothing given still reports.
#[test]
fn dot_shorthand_static_method_without_arguments_reports() {
    let fixture = "class _C {\n\
                   \x20 static void m([int? a]) {}\n\
                   }\n\
                   f() => .m();\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// An enum constant's positional arguments join the named givens:
/// `v(0)` gives the selected constructor's optional positional.
#[test]
fn enum_const_positional_argument_gives_formal() {
    let fixture = "enum _E {\n\
                   \x20 v(0);\n\
                   \x20 const _E([int? a]);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}

/// Negative: an enum constant giving nothing still reports the optional.
#[test]
fn enum_const_without_arguments_reports() {
    let fixture = "enum _E {\n\
                   \x20 v();\n\
                   \x20 const _E([int? a]);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), vec!["a".to_string()]);
}

/// The named-selector constant's positional supply keys the selector:
/// `v.n(0)` gives `_E.n`'s formal.
#[test]
fn enum_const_named_selector_positional_gives_formal() {
    let fixture = "enum _E {\n\
                   \x20 v.n(0);\n\
                   \x20 const _E.n([int? a]);\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(uep_names(&report), Vec::<String>::new());
}
