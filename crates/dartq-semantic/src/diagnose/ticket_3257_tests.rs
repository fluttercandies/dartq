//! #3257 pins — the arity / named-set / requiredness arm of
//! [`Engine::override_broken`] (recon C6).
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `subtype.dart:338-430` `_isFunctionSubtypeOf`: the two formal tables
//!   walk in one pass — a declared required positional needs a required
//!   super positional, a declared optional positional needs any super
//!   positional, and a declared named parameter needs a same-named super
//!   one whose `required` flag it may only relax. Trailing declared
//!   required parameters (`required` named ones included) and any leftover
//!   super parameter fail the subtype question.
//! - `correct_override.dart:36-39` `isCorrectOverrideOf`: that subtype
//!   question over the two member function types is the whole verdict.
//!
//! The corpus faces these pins mirror:
//! `diagnostics/invalid_override/InvalidOverrideTest` `test_method_required`,
//! `test_method_positional_optional`, `test_method_positional_optionalAnd
//! Required`, `test_method_positional_optionalAndRequired2`,
//! `test_method_named_fewerNamedParameters`,
//! `test_method_named_missingNamedParameter`, `test_method_namedParamType`,
//! `test_method_optionalParamType`.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope, and `#[path]`
//! keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{assert_silent, report_ops};

/// The `invalid_override` rows of one report, verbatim, in report order.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

// —— positional arity ————————————————————————————————

/// `test_method_required`: a newly required positional rejects the super's
/// shorter calls (`a_required > b_required`).
#[test]
fn added_required_positional_breaks() {
    let report = report_ops(
        "class A { m(int a) {} }\n\
         class B extends A { m(int a, int b) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int, int)') isn't a valid override of \
          'A.m' ('dynamic Function(int)')."]
    );
}

/// The mirror direction: a dropped positional cannot accept the super's
/// longer calls (`a.positional.len() < b.positional.len()`).
#[test]
fn dropped_positional_breaks() {
    let report = report_ops(
        "class A { m(int a, int b) {} }\n\
         class B extends A { m(int a) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int)') isn't a valid override of \
          'A.m' ('dynamic Function(int, int)')."]
    );
}

/// `test_method_positional_optional`: dropping one optional positional is
/// still a dropped positional — the super's two-argument call no longer
/// fits.
#[test]
fn dropped_optional_positional_breaks() {
    let report = report_ops(
        "class A { m([int a, int b]) {} }\n\
         class B extends A { m([int a]) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function([int])') isn't a valid override of \
          'A.m' ('dynamic Function([int, int])')."]
    );
}

/// `test_method_positional_optionalAndRequired`: the required head plus a
/// dropped optional tail.
#[test]
fn dropped_optional_tail_breaks() {
    let report = report_ops(
        "class A { m(int a, int b, [int c, int d]) {} }\n\
         class B extends A { m(int a, int b, [int c]) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int, int, [int])') isn't a valid \
          override of 'A.m' ('dynamic Function(int, int, [int, int])')."]
    );
}

/// `test_method_positional_optionalAndRequired2`: a required positional
/// promoted into the optional group — the declared list is one shorter, so
/// the super's four-argument call has no slot.
#[test]
fn required_promoted_into_optional_group_breaks() {
    let report = report_ops(
        "class A { m(int a, int b, [int c, int d]) {} }\n\
         class B extends A { m(int a, [int c, int d]) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int, [int, int])') isn't a valid \
          override of 'A.m' ('dynamic Function(int, int, [int, int])')."]
    );
}

/// An ADDED optional positional is the valid-override widening (`Buf
/// toUint8List([Buf? bytes])` over `Buf toUint8List()`).
#[test]
fn added_optional_positional_is_silent() {
    let report = report_ops(
        "class A { m(int a) {} }\n\
         class B extends A { m(int a, [int? b]) {} }",
    );
    assert_silent(&report);
}

/// Optional-vs-required boundary, declared side heavier: the super accepts
/// `m()` through its optional parameter, the declaration does not.
#[test]
fn optional_to_required_positional_breaks() {
    let report = report_ops(
        "class A { m([int a]) {} }\n\
         class B extends A { m(int a) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int)') isn't a valid override of \
          'A.m' ('dynamic Function([int])')."]
    );
}

/// Optional-vs-required boundary, declared side lighter: the declaration
/// accepts every call the super's required head allows. The optional slot
/// carries a default so the fixture raises no
/// `missing_default_value_for_parameter` row of its own.
#[test]
fn required_to_optional_positional_is_silent() {
    let report = report_ops(
        "class A { m(int a) {} }\n\
         class B extends A { m([int a = 0]) {} }",
    );
    assert_silent(&report);
}

/// Both directions of the optional counting in one head: a super two-param
/// list with a required head stays satisfied by a one-required declared
/// list, though the lengths coincide.
#[test]
fn positional_lengths_equal_required_counts_differ_break() {
    let report = report_ops(
        "class A { m(int a, [int b]) {} }\n\
         class B extends A { m(int a, int b) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function(int, int)') isn't a valid override of \
          'A.m' ('dynamic Function(int, [int])')."]
    );
}

// —— named set and requiredness ————————————————————————

/// `test_method_named_fewerNamedParameters`: a dropped named parameter
/// rejects calls the super accepts.
#[test]
fn dropped_named_parameter_breaks() {
    let report = report_ops(
        "class A { m({int a = 0, int b = 0}) {} }\n\
         class B extends A { m({int a = 0}) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function({int a})') isn't a valid override of \
          'A.m' ('dynamic Function({int a, int b})')."]
    );
}

/// `test_method_named_missingNamedParameter`: a same-size set with a
/// renamed member — the super's `b` has no slot.
#[test]
fn renamed_named_parameter_breaks() {
    let report = report_ops(
        "class A { m({int a = 0, int b = 0}) {} }\n\
         class B extends A { m({int a = 0, int c = 0}) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function({int a, int c})') isn't a valid \
          override of 'A.m' ('dynamic Function({int a, int b})')."]
    );
}

/// An ADDED optional named parameter is the valid-override widening.
#[test]
fn added_optional_named_parameter_is_silent() {
    let report = report_ops(
        "class A { m({int a = 0}) {} }\n\
         class B extends A { m({int a = 0, int? b}) {} }",
    );
    assert_silent(&report);
}

/// An ADDED `required` named parameter rejects every call without it.
#[test]
fn added_required_named_parameter_breaks() {
    let report = report_ops(
        "class A { m() {} }\n\
         class B extends A { m({required int a}) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function({required int a})') isn't a valid \
          override of 'A.m' ('dynamic Function()')."]
    );
}

/// `required` may only tighten onto a parameter that is required in the
/// super: an optional super named parameter relaxed to `required` rejects
/// the super's no-argument call.
#[test]
fn tightened_named_requiredness_breaks() {
    let report = report_ops(
        "class A { m({int? a}) {} }\n\
         class B extends A { m({required int? a}) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function({required int? a})') isn't a valid \
          override of 'A.m' ('dynamic Function({int? a})')."]
    );
}

/// The reverse relaxation is the valid-override widening.
#[test]
fn relaxed_named_requiredness_is_silent() {
    let report = report_ops(
        "class A { m({required int a}) {} }\n\
         class B extends A { m({int a = 0}) {} }",
    );
    assert_silent(&report);
}

// —— parameter faces inside the arity-matched shapes ——————————

/// `test_method_namedParamType`: same named set, narrowed optional type —
/// the face runs contravariantly over the named slot.
#[test]
fn narrowed_named_parameter_type_breaks() {
    let report = report_ops(
        "abstract class A { m({int? a}); }\n\
         class B implements A { m({String? a}) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function({String? a})') isn't a valid override \
          of 'A.m' ('dynamic Function({int? a})')."]
    );
}

/// `test_method_optionalParamType`: same arity, narrowed optional
/// positional type.
#[test]
fn narrowed_optional_positional_type_breaks() {
    let report = report_ops(
        "abstract class A { m([int? a]); }\n\
         class B implements A { m([String? a]) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function([String?])') isn't a valid override of \
          'A.m' ('dynamic Function([int?])')."]
    );
}

/// The widened parameter face on an arity-matched shape stays silent.
#[test]
fn widened_optional_positional_type_is_silent() {
    let report = report_ops(
        "abstract class A { m([int? a]); }\n\
         class B implements A { m([num? a]) {} }",
    );
    assert_silent(&report);
}

/// `test_method_positional_optional` verbatim: untyped parameters render
/// `dynamic`, and the optional tail keeps its brackets in BOTH signature
/// slots of the row.
#[test]
fn corpus_positional_optional_renders_dynamic_and_brackets() {
    let report = report_ops(
        "class A { m([a, b]) {} }\n\
         class B extends A { m([a]) {} }",
    );
    assert_eq!(
        override_rows(&report),
        ["'B.m' ('dynamic Function([dynamic])') isn't a valid override of \
          'A.m' ('dynamic Function([dynamic, dynamic])')."]
    );
}

/// An identical signature in every column (arity, names, requiredness,
/// types) is the trivial override.
#[test]
fn identical_signature_is_silent() {
    let report = report_ops(
        "class A { m(int a, {int? b, required int c}) {} }\n\
         class B extends A { m(int a, {int? b, required int c}) {} }",
    );
    assert_silent(&report);
}
