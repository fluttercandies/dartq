//! #3282 pins — the `assignment_to_final*` family's emit path.
//!
//! Upstream splits the three rows by the RECOVERY element of the write's
//! setter lookup (`AssignmentVerifier.verify`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/assignment_verifier.dart:63-87):
//!
//! * a getter whose variable is a field/variable declaration —
//!   `isOriginGetterSetter` false — reports `assignment_to_final`
//!   (`'{0}' can't be used as a setter because it's final.`);
//! * a getter that comes from an accessor declaration
//!   (`isOriginGetterSetter` true, an explicit `int get x`) reports
//!   `assignment_to_final_no_setter` naming the declaring container
//!   (`There isn't a setter named '{0}' in class '{1}'.`);
//! * a `final` LOCAL or formal reports `assignment_to_final_local`
//!   (`AssignmentExpressionShared.checkFinalAlreadyAssigned`,
//!   pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver
//!   .dart:395-425): the write is an error unless the variable is
//!   definitely unassigned there — its first assignment after a bare
//!   `final int x;` declaration — and a for-in loop's existing-variable
//!   target always reports.
//!
//! The negative twins guard the boundaries: a non-`final` write stays
//! silent, a `final` local's FIRST assignment is legal, `late final`
//! locals/fields belong to their own rows, and a `const` field write is
//! `assignment_to_const`'s row (this family stays silent rather than
//! emitting a second wrong code).

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::report_ops_bare;

/// The rows of `code`, as `(offset, message)` pairs.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, String)> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.as_str().to_owned()))
        .collect::<Vec<_>>();
    rows.sort_unstable();
    rows
}

/// The absolute offset of the `nth` (0-based) occurrence of `needle` in the
/// fixture — the rig analyzes the fixture verbatim, so the fixture-local
/// offsets are the reported ones.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .map_or(0, |found| found + needle.len());
    }
    u32::try_from(from + fixture[from..].find(needle).expect("needle occurrence"))
        .expect("fixture fits u32")
}

/// No row of the family at all — the positive faces' negative twin.
fn assert_family_silent(report: &FileReport) {
    for code in [
        "assignment_to_final",
        "assignment_to_final_local",
        "assignment_to_final_no_setter",
    ] {
        assert!(
            rows(report, code).is_empty(),
            "{code}: {:?}",
            rows(report, code)
        );
    }
}

#[test]
fn final_local_with_initializer_reports() {
    let source = "void f() {\n  final x = 0;\n  x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local"),
        vec![(
            at(source, "x = 1", 0),
            "The final variable 'x' can only be set once.".to_string()
        )]
    );
}

#[test]
fn final_parameter_reports() {
    let source = "void f(final int x) {\n  x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local"),
        vec![(
            at(source, "x = 1", 0),
            "The final variable 'x' can only be set once.".to_string()
        )]
    );
}

#[test]
fn final_local_compound_and_increment_report() {
    let source = "void f() {\n  final x = 0;\n  x += 1;\n  ++x;\n  x++;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![
            at(source, "x += 1", 0),
            at(source, "++x", 0) + 2,
            at(source, "x++", 0),
        ]
    );
}

/// The bare declaration's FIRST assignment is the legal one.
#[test]
fn final_local_first_assignment_is_legal() {
    let source = "void f() {\n  final int x;\n  x = 1;\n  x;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

/// Any earlier assignment (on any path) makes the next one report.
#[test]
fn final_local_second_assignment_reports() {
    let source = "void f(bool b) {\n  final int x;\n  if (b) x = 0;\n  x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "x = 1", 0)]
    );
}

/// A for-in loop's existing-variable target is a write per iteration, so it
/// reports unconditionally (even though the row model reads it).
#[test]
fn for_each_existing_final_target_reports() {
    let source = "void f(List<int> xs) {\n  final i;\n  for (i in xs) {\n    i; \n  }\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local"),
        vec![(
            at(source, "i in", 0),
            "The final variable 'i' can only be set once.".to_string()
        )]
    );
}

/// A declared loop variable is bound by the loop; writing it in the body is
/// the violation, at the body's name.
#[test]
fn declared_final_loop_variable_body_write_reports() {
    let source = "void f(List<int> xs) {\n  for (final i in xs) {\n    i = 4;\n  }\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_local")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "i = 4", 0)]
    );
}

#[test]
fn nonfinal_local_write_is_silent() {
    let source = "void f() {\n  var x = 0;\n  x = 1;\n  x += 2;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

/// The `late` locals belong to the late-final rows, not this family.
#[test]
fn late_final_local_write_is_silent() {
    let source = "void f() {\n  late final int x = 0;\n  x = 1;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

#[test]
fn final_instance_field_write_reports() {
    let source = "class C {\n  final int x;\n  C(this.x);\n}\n\
                  void g(C c) {\n  c.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final"),
        vec![(
            at(source, "c.x = 1", 0) + 2,
            "'x' can't be used as a setter because it's final.".to_string()
        )]
    );
}

/// The implicit-`this` spelling of the same field resolves the member face
/// too.
#[test]
fn final_instance_field_implicit_this_reports() {
    let source = "class C {\n  final int x;\n  C(this.x);\n  void m() {\n    x = 1;\n  }\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "x = 1", 0)]
    );
}

#[test]
fn static_final_field_write_reports() {
    let source = "class C {\n  static final int x = 0;\n}\n\
                  void g() {\n  C.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "C.x = 1", 0) + 2]
    );
}

/// An explicit getter is the accessor-origin face: `assignment_to_final_no_setter`
/// naming the DECLARING container.
#[test]
fn explicit_getter_write_reports_no_setter() {
    let source = "class C {\n  int get x => 0;\n}\nvoid g(C c) {\n  c.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_no_setter"),
        vec![(
            at(source, "c.x = 1", 0) + 2,
            "There isn't a setter named 'x' in class 'C'.".to_string()
        )]
    );
}

#[test]
fn static_getter_write_reports_no_setter() {
    let source = "class C {\n  static int get x => 0;\n}\nvoid g() {\n  C.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final_no_setter")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "C.x = 1", 0) + 2]
    );
}

#[test]
fn nonfinal_field_and_declared_setter_are_silent() {
    let source = "class C {\n  int a = 0;\n  int get b => 0;\n  set b(int v) {}\n}\n\
                  void g(C c) {\n  c.a = 1;\n  c.b = 2;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

/// `late final` without an initializer still induces a `name=` setter.
#[test]
fn late_final_field_without_initializer_is_silent() {
    let source = "class C {\n  late final int x;\n}\nvoid g(C c) {\n  c.x = 1;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

/// The same field WITH an initializer induces no setter.
#[test]
fn late_final_field_with_initializer_reports() {
    let source = "class C {\n  late final int x = 0;\n}\nvoid g(C c) {\n  c.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "c.x = 1", 0) + 2]
    );
}

/// A `const` field's write is `assignment_to_const`'s row — this family
/// stays silent rather than emitting a second wrong code.
#[test]
fn const_field_write_is_silent() {
    let source = "class C {\n  static const int x = 0;\n}\nvoid g() {\n  C.x = 1;\n}\n";
    assert_family_silent(&report_ops_bare(source));
}

/// A getter-only write is still not an `undefined_setter` row (the #2177
/// pin): the family claims the write instead.
#[test]
fn getter_write_stays_off_undefined_setter() {
    let source = "class C {\n  int get x => 0;\n}\nvoid g(C c) {\n  c.x = 1;\n}\n";
    let report = report_ops_bare(source);
    assert!(
        rows(&report, "undefined_setter").is_empty(),
        "{:?}",
        rows(&report, "undefined_setter")
    );
}

/// The increment applications on a `final` field assign it too — same row,
/// same anchor at the member name.
#[test]
fn final_field_increment_reports() {
    let source = "class C {\n  final int x;\n  C(this.x);\n}\n\
                  void g(C c) {\n  c.x++;\n  ++c.x;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        rows(&report, "assignment_to_final")
            .into_iter()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>(),
        vec![at(source, "c.x++", 0) + 2, at(source, "++c.x", 0) + 4]
    );
}
