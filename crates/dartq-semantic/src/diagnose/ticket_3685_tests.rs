//! Ticket #3685 pins: alias-preserving declared-type spelling in
//! `invalid_assignment` (census-12 shape 1.1 — the bind-side face).
//!
//! The oracle's `getDisplayString(preferTypeAlias)` reads
//! `TypeImpl.alias`, a decoration riding the instantiated type
//! (`display_string_builder.dart:379-382`), so a declared slot that
//! wrote `Foo<int>` reports `'Foo<int>'`, never the
//! `int Function(T)` expansion. `bind_declared_type` /
//! `bind_parameter_type` used to fold the alias eagerly at bind time,
//! so every downstream message spelled the expansion. Both sites now
//! keep a resolved alias head's written spelling (the
//! `fold_call_return` posture); the verdict layer still unfolds it at
//! the ask (`fold_alias_verdict`), so no judgment reads a weaker face.
//! Oracle rows cited per test.

use super::FileReport;
use super::test_rig::report_ops;

/// The `invalid_assignment` messages of one report, in report order.
fn assignment_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "invalid_assignment")
        .map(|row| row.message.to_string())
        .collect()
}

/// Shape 1.1 — the parameterized alias declared slot:
/// `typedef Foo<S> = S Function<T>(T x)`; `Foo<int> y = foo;` is
/// SILENT — the tear-off `foo`'s `int Function(T)` unifies with the
/// slot's `int Function<T>(T)` α-equivalently (`relateTypeParameters`
/// pairs the quantifiers by position), so the oracle emits no row at
/// all. The earlier expectation of one `invalid_assignment` was wrong
/// (it pinned the pre-α-recovery verdict). Oracle run (`dart analyze`
/// 3.13.7, `~/.dartq-oracle/3.13.7`, the fixture verbatim):
/// `Analyzing dq-3685... No issues found!` — upstream
/// `NonErrorResolverTest/test_genericTypeAlias_typeParameters` is
/// likewise a zero-row case. The `foo<String>("hello")` /
/// `y<String>("hello")` invocations stay clean identically.
#[test]
fn declared_alias_slot_keeps_the_written_spelling() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         int foo<T>(T x) => 3;\n\
         void test1() {\n\
           Foo<int> y = foo;\n\
           foo<String>(\"hello\");\n\
           y<String>(\"hello\");\n\
         }\n",
    );
    assert!(
        assignment_messages(&report).is_empty(),
        "the oracle emits no invalid_assignment for the α-equivalent pair: {:?}",
        report.diagnostics
    );
}

/// The parameter face: a formal bound through `bind_parameter_type`
/// keeps its written `Foo<int>` spelling as the value slot of a later
/// assignment — `int y = x` reads the parameter's declared type, which
/// the oracle renders `'Foo<int>'`, never the `int Function(T)`
/// expansion.
#[test]
fn parameter_alias_slot_keeps_the_written_spelling() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void g(Foo<int> x) {\n\
           int y = x;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        ["A value of type 'Foo<int>' can't be assigned to a variable of type 'int'."],
        "{:?}",
        assignment_messages(&report)
    );
}

/// A bare alias head (`typedef A = ...` spelled `A` with no type
/// arguments) keeps its written spelling too — `typedef A =
/// List<int>`; `A a = "s";` reports the declared slot as `'A'`, the
/// alias use the bind now preserves.
#[test]
fn bare_alias_head_keeps_the_written_spelling() {
    let report = report_ops(
        "typedef A = List<int>;\n\
         void f() {\n\
           A a = \"s\";\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        ["A value of type 'String' can't be assigned to a variable of type 'A'."],
        "{:?}",
        assignment_messages(&report)
    );
}

/// Negative control — the verdict still unfolds the kept spelling:
/// assigning a genuinely wrong value through a parameterized alias
/// reports `invalid_assignment` exactly as before (the alias head is
/// not a free pass — `fold_alias_verdict` substitutes `Foo<int>`'s
/// arguments through the typedef body before judging).
#[test]
fn wrong_value_through_alias_still_reports() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void f() {\n\
           Foo<int> y = 3;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        ["A value of type 'int' can't be assigned to a variable of type 'Foo<int>'."],
        "{:?}",
        assignment_messages(&report)
    );
}
