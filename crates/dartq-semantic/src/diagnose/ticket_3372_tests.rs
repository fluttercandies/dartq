//! The #3372 pins: the typedef-alias arm of the build-time clause mapping.
//!
//! Oracle semantics: a typedef application is expanded at the written node
//! — the right-hand side with the alias's own parameters substituted by the
//! written arguments (their to-bounds defaults for a raw use), the use
//! site's `?` composed on top (`TypeAliasElementImpl.instantiateImpl`,
//! upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! named_type_resolver.dart:237-248). `LibraryTypes::build_with_clauses`
//! maps every written parameter bound through `clause_arg_ty`, so a typedef
//! head that failed to expand there left the parameter's
//! `ParamFacts::bound` unset: the type-variable bound rule (`bound(X) <:
//! B`, subtype.dart:490-500) then had no shape to decide with and the
//! verdict went undecided — the under-report face standing in for the real
//! bound, so a use the bound rejects stayed silent where the oracle reports
//! it.
//!
//! The pins read that rule through the mounted-ops rig: a parameter of a
//! typedef-carried bound is accepted where the expanded shape is a subtype
//! of the target (`Function` for the function-type aliases) and rejected
//! where it is not (`int`, a non-nullable `List`) — the rejection is the
//! row the unmodelled face could not produce.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::FileReport;

/// The `argument_type_not_assignable` rows of `report` in source order:
/// `(offset, length)`.
fn argument_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Byte offset of `needle` inside the fixture, shifted by the rig's core
/// prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The argument offset of `call`'s first argument — the row anchor the
/// invocation face reports at.
fn arg_at(source: &str, call: &str, prefix: &str) -> u32 {
    at(source, call) + u32::try_from(prefix.len()).expect("prefix fits")
}

/// A function-type typedef used as a parameter bound expands to the real
/// function type: `T`'s bound then satisfies `Function` (rule 13's bare
/// `Function` face) while `int` stays unreachable — the `int` row proves
/// the parameter is modelled and the bound decides, so the absent
/// `Function` row is the expanded shape's verdict, not a silent argument.
#[test]
fn ticket_3372_function_typedef_bound_feeds_the_type_variable_rule() {
    let source = r#"
typedef F<X> = void Function<Y extends X>();
class A<T extends F<num>> {
  void f(T t) {
    takesFunction(t);
    takesInt(t);
  }
}
void takesFunction(Function fn) {}
void takesInt(int i) {}
"#;
    let report = report_ops(source);
    assert_eq!(
        argument_rows(&report),
        vec![(arg_at(source, "takesInt(t);", "takesInt("), 1)],
        "the bound is the expanded function type: {:?}",
        report.diagnostics
    );
}

/// A raw alias use in the bound (`T extends F`) instantiates the alias's
/// own parameter to its to-bounds default (`instantiateToBounds`,
/// type_system.dart:719-731): the bound reads `void Function<Y extends
/// dynamic>()` — still a function type, never the unresolved face.
#[test]
fn ticket_3372_raw_typedef_bound_reads_its_to_bounds_default() {
    let source = r#"
typedef F<X> = void Function<Y extends X>();
class A<T extends F> {
  void f(T t) {
    takesFunction(t);
    takesInt(t);
  }
}
void takesFunction(Function fn) {}
void takesInt(int i) {}
"#;
    let report = report_ops(source);
    assert_eq!(
        argument_rows(&report),
        vec![(arg_at(source, "takesInt(t);", "takesInt("), 1)],
        "the raw head reads the to-bounds default: {:?}",
        report.diagnostics
    );
}

/// The `?` composition: the alias right-hand side's own suffix and the use
/// site's each make the expanded bound nullable, so `T` is not a subtype of
/// the non-nullable `List<num>` (`?` never widens a subtype question) —
/// dropping either composition would read `List<num>` and silence the row.
#[test]
fn ticket_3372_nullable_typedef_bound_keeps_the_question_mark() {
    let zero_bounded = r#"
typedef F<X> = List<X>?;
class A<T extends F<num>> {
  void f(T t) {
    takesList(t);
  }
}
void takesList(List<num> l) {}
"#;
    let report = report_ops(zero_bounded);
    assert_eq!(
        argument_rows(&report),
        vec![(arg_at(zero_bounded, "takesList(t);", "takesList("), 1)],
        "the right-hand side's `?` composes: {:?}",
        report.diagnostics
    );

    let use_site_bounded = r#"
typedef G<X> = List<X>;
class B<U extends G<num>?> {
  void g(U u) {
    takesList(u);
  }
}
void takesList(List<num> l) {}
"#;
    let report = report_ops(use_site_bounded);
    assert_eq!(
        argument_rows(&report),
        vec![(arg_at(use_site_bounded, "takesList(u);", "takesList("), 1)],
        "the use site's `?` composes: {:?}",
        report.diagnostics
    );
}

/// The registered-head regression: `List<num>` is a container, so the
/// registry arm answers first and the typedef arm never shadows it — `T`
/// keeps the real `List<num>` bound and the argument passes.
#[test]
fn ticket_3372_registered_head_keeps_the_registry_arm() {
    let source = r#"
class A<T extends List<num>> {
  void f(T t) {
    takesList(t);
  }
}
void takesList(List<num> l) {}
"#;
    let report = report_ops(source);
    assert!(
        argument_rows(&report).is_empty(),
        "the registry arm is unchanged: {:?}",
        report.diagnostics
    );
}

/// A self-referencing alias (`typedef F = F`) never registers a shape
/// (its right-hand side is a bare `Named` with no parameters), so the arm
/// misses exactly like before and the parameter keeps the unmodelled
/// bound — the under-report face suppresses both argument verdicts rather
/// than folding the alias into its own head (no double-fold, no runaway),
/// and the #3341 self-reference row stands untouched.
#[test]
fn ticket_3372_self_referential_alias_keeps_the_unmodelled_face() {
    let source = r#"
typedef F = F;
class A<T extends F> {
  void f(T t) {
    takesFunction(t);
    takesInt(t);
  }
}
void takesFunction(Function fn) {}
void takesInt(int i) {}
"#;
    let report = report_ops(source);
    assert!(
        argument_rows(&report).is_empty(),
        "an unmodelled bound suppresses both verdicts: {:?}",
        report.diagnostics
    );
    let self_reference: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_alias_cannot_reference_itself")
        .map(|d| d.range.offset)
        .collect();
    let declaration = at(source, "typedef F = F;");
    let span = u32::try_from("typedef F = F;".len()).expect("span fits");
    assert_eq!(
        self_reference.len(),
        1,
        "the #3341 row still reports: {:?}",
        report.diagnostics
    );
    assert!(
        (declaration..declaration + span).contains(&self_reference[0]),
        "the row anchors inside the alias declaration: {:?}",
        report.diagnostics
    );
}
