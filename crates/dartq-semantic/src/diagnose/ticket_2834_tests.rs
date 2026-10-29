//! The #2834 generic-function-type subtyping bounds pins: the
//! signatures' own type parameters decide through `relateTypeParameters`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
//! type_system.dart:1581-1635 — equal counts, EQUAL bounds via mutual
//! subtype, parameters unified by position). The rule is an equality,
//! not a bounds direction: a `num`-bounded signature is a subtype of an
//! `int`-bounded one in NEITHER direction. Count-only entries (an
//! interned `Ty::Function` shape records just the `<...>` group's size)
//! decide the count rule but never a bounds ask — the under-report
//! posture pinned below. The bounded pairs ride the assignment face's
//! stashed-signature junction (a formal's written annotation is the
//! only face carrying parameters WITH bounds); the byte-exact message
//! pin rides the argument face, whose display renders the parameters.

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

/// The `argument_type_not_assignable` rows of `report`, messages only.
fn arg_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `invalid_assignment` rows of `report`, messages only.
fn assignment_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect()
}

/// Equal bounds assign cleanly, whatever the parameters' spellings —
/// the positional unification (`T` against `Y` at the same index) is
/// the fresh-set fold upstream. (A bound nesting a parameterized type
/// — `Y extends List<Y>` — needs the `>>` group-close seam the parser
/// lane owns; see the #2834 report.)
#[test]
fn equal_bounds_assign_across_parameter_names() {
    let report = report_ops(
        "void f(void Function<Y extends num>() n, void Function<T extends num>() t) {\n\
         \x20   t = n;\n\
         \x20   t = t;\n\
         \x20 }",
    );
    assert!(
        assignment_messages(&report).is_empty(),
        "expected clean assignments, got {:#?}\nall rows: {:#?}",
        assignment_messages(&report),
        rows(&report)
    );
}

/// The non-inversion proof: an `int`-bounded signature assigned into a
/// `num`-bounded one reports, and the mirrored order reports too — the
/// bounds question is `isEqualTo`, never a subtype direction.
#[test]
fn differing_bounds_report_in_both_directions() {
    let report = report_ops(
        "void f(void Function<Y extends num>() n, void Function<Y extends int>() i) {\n\
         \x20   n = i;\n\
         \x20   i = n;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 2, "rows: {:#?}", rows(&report));
}

/// A bound written on exactly one side reads `dynamic` against the
/// other (`bound1 ??= DynamicTypeImpl.instance`): `dynamic` is not
/// `num` in either direction, so both orders report.
#[test]
fn unwritten_bound_reads_dynamic_against_written() {
    let report = report_ops(
        "void f(void Function<Y extends num>() n, void Function<Y>() u) {\n\
         \x20   u = n;\n\
         \x20   n = u;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 2, "rows: {:#?}", rows(&report));
}

/// The counts must match in both orders
/// (`typeParameters1.length != typeParameters2.length` → not a
/// subtype): a generic signature never fits a plain one and a plain
/// one never fits a generic. Same-count plain pairs stay clean.
#[test]
fn type_parameter_count_must_match() {
    let report = report_ops(
        "void f(void Function<Y extends num>() n, void Function() p,\n\
         \x20     void Function() p2) {\n\
         \x20   p = n;\n\
         \x20   n = p;\n\
         \x20   p = p2;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 2, "rows: {:#?}", rows(&report));
}

/// The argument face's byte-exact message: a plain top-level tear-off
/// into a generic formal reports with the parameters rendered
/// (`void Function<Y extends num>()`) — the count rule through the
/// stash the formal's annotation captured.
#[test]
fn argument_face_renders_the_type_parameters() {
    let report = report_ops(
        "void plain() {}\n\
         \x20 void takeNum(void Function<Y extends num>() cb) {}\n\
         \x20 void main() {\n\
         \x20   takeNum(plain);\n\
         \x20 }",
    );
    assert_eq!(
        arg_messages(&report),
        vec![
            "The argument type 'void Function()' can't be assigned to the parameter type 'void Function<Y extends num>()'. ".to_string()
        ],
        "rows: {:#?}",
        rows(&report)
    );
}

/// The count rule at the plain assignment face: a local's written
/// generic function-type annotation (an interned `Ty::Function` — the
/// count rides the shape, the bounds do not) against a plain shape
/// reports through `invalid_assignment` in both orders. The message
/// spelling keeps the model's render (an interned shape carries no
/// parameter names — the display seam #2834 documents), so the pins
/// assert the verdict count and the plain target spelling.
#[test]
fn count_rule_reports_at_plain_assignments() {
    let report = report_ops(
        "void f(void Function<Y extends num>() x, void Function() p) {\n\
         \x20   void Function() y = x;\n\
         \x20   void Function<Y extends num>() z = p;\n\
         \x20   void Function() w = p;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 2, "rows: {:#?}", rows(&report));
    assert!(
        messages
            .iter()
            .all(|m| m.contains("can't be assigned to a variable of type 'void Function()'")),
        "{messages:#?}"
    );
}

/// The count-only posture: two interned shapes of equal count whose
/// bounds the shape never recorded stay silent — an unknown bound is
/// not `dynamic`, and no verdict may invent one (the local annotation
/// joins no written-signature stash). The seam is documented in #2834:
/// named, bounded entries arrive only through the stash faces.
#[test]
fn count_only_bounds_never_decide() {
    let report = report_ops(
        "void f(void Function<Y extends num>() x) {\n\
         \x20   void Function<Y extends int>() y = x;\n\
         \x20 }",
    );
    assert!(
        assignment_messages(&report).is_empty(),
        "expected the under-report posture, got {:#?}\nall rows: {:#?}",
        assignment_messages(&report),
        rows(&report)
    );
}
