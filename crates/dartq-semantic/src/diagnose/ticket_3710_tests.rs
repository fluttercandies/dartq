//! Ticket #3710 pins: the statement-face nullability predicate's
//! `isPotentiallyNullable` recursions and the promoted-read gate that
//! keep them honest.
//!
//! Oracle: upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! `nullable_dereference_verifier.dart` `_check` silences only
//! `DynamicType || InvalidType || !isPotentiallyNullable`; `isNonNullable`
//! (`dart/element/type_system.dart:1161-1184`) recurses into a type
//! parameter's bound, recurses a `FutureOr` argument, and answers
//! `interfaces.isNotEmpty` for an extension type. The teardown operand
//! scan (`Engine::stage_operand_faces`) grounds the operand's static type
//! through `read_types`, so a promoted operand already arrives non-null —
//! and an opaque promotion the `ReadType` join cannot spell still proves
//! non-null through the staged `PromotedRead` marker.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::FileReport;

/// The prelude shifts every offset; the fixtures below are ASCII.
fn prelude_len() -> u32 {
    CORE_PRELUDE.chars().map(char::len_utf16).sum::<usize>() as u32
}

/// The absolute UTF-16 offset of `needle`'s first occurrence inside
/// `context` inside the fixture.
fn at(fixture: &str, context: &str, needle: &str) -> u32 {
    let context_start = fixture.find(context).expect("context is in the fixture");
    let within = context.find(needle).expect("needle is in the context");
    let upto = context_start + within;
    prelude_len() + fixture[..upto].chars().map(char::len_utf16).sum::<usize>() as u32
}

/// Every nullable-special-position row as `(offset, length, message)`.
fn special_position_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unchecked_use_of_nullable_value"
                && d.message.starts_with("A nullable expression")
        })
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn row(offset: u32, length: u32, message: &str) -> (u32, u32, String) {
    (offset, length, message.to_string())
}

const ITERATOR: &str = "A nullable expression can't be used as an iterator in a for-in loop.";
const SPREAD: &str = "A nullable expression can't be used in a spread.";
const YIELD_EACH: &str = "A nullable expression can't be used in a yield-each statement.";

// —— for-in iterator ————————————————————————————————————————————————

/// An unbounded `T` is bounded by the implicit `Object?` upstream, so a
/// `?`-less `T xs` iterable still reports — the `TypeParameterType` arm's
/// `bound != null && isNonNullable(bound)` over `Object?`.
#[test]
fn for_in_unbounded_type_parameter_reports() {
    let fixture = "f<T>(T xs) {\n  for (var y in xs) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in xs)", "xs"), 2, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// A promoted `T` over a nullable bound: the `xs == null` guard promotes
/// the read to the bound's non-null face, so `expression.typeOrThrow` is
/// no longer potentially nullable and the loop stays silent — the same
/// shape `for_in_promoted_type_parameter_is_silent` pins in
/// `ticket_3688_tests.rs`, re-pinned here on an unbounded parameter whose
/// implicit `Object?` bound promotes to `Object`.
#[test]
fn for_in_unbounded_type_parameter_promoted_is_silent() {
    let fixture = "f<T>(T xs) {\n  if (xs == null) return;\n  for (var y in xs) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// A container-level type parameter (`class C<T extends List?>`) reads
/// the container capture's bound, not an executable window's: the
/// operand `t` in a member's body reports through the same bound
/// recursion.
#[test]
fn for_in_container_type_parameter_nullable_bound_reports() {
    let fixture = "class C<T extends List?> {\n  void m(T t) {\n    for (var y in t) {}\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in t)", "t"), 1, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// A `FutureOr` of a nullable argument is potentially nullable upstream
/// (`isNonNullable` recurses `FutureOr<T>` into `T`), so a `?`-less
/// `FutureOr<int?>` iterable reports.
#[test]
fn for_in_future_or_nullable_argument_reports() {
    let fixture = "f(FutureOr<int?> v) {\n  for (var y in v) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in v)", "v"), 1, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// The `FutureOr` negative: a non-nullable argument is strictly
/// non-nullable upstream, so the loop is silent.
#[test]
fn for_in_future_or_non_nullable_argument_is_silent() {
    let fixture = "f(FutureOr<int> v) {\n  for (var y in v) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

// —— spread ————————————————————————————————————————————————————————

/// The extension-type arm on the spread face: an extension type with no
/// `implements` clause is potentially nullable upstream
/// (`interfaces.isNotEmpty` fails), so `...v` reports at the operand.
#[test]
fn spread_extension_type_without_implements_reports() {
    let fixture = "extension type ET<T>(List<T> _) {}\n\
                   void t(ET<int> v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "...v]", "v"), 1, SPREAD)],
        "{:?}",
        rows(&report)
    );
}

/// The extension-type negative on the spread face: an `implements`
/// clause makes the spelling strictly non-nullable, so `...v` is silent.
#[test]
fn spread_extension_type_with_implements_is_silent() {
    let fixture = "extension type ET<T>(List<T> _) implements List<T> {}\n\
                   void t(ET<int> v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// A null-guarded nullable-bound type parameter stays silent on the
/// spread face: `v` promotes to `List` before the literal, and the
/// operand's promoted `typeOrThrow` is non-nullable upstream.
#[test]
fn spread_promoted_nullable_bound_type_parameter_is_silent() {
    let fixture = "void t<L extends List?>(L v) {\n\
                   \x20 if (v == null) return;\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// A write-promoted operand: `v = q.m()` over a `dynamic` receiver
/// promotes `v` to `List<int>` (the write's non-null subtype narrowing),
/// so the operand's `typeOrThrow` is non-nullable upstream and the
/// spread is silent — the promoted-read gate must not let the declared
/// `List<int>?` face report.
#[test]
fn spread_write_promoted_operand_is_silent() {
    let fixture = "void t(List<int>? v, dynamic q) {\n\
                   \x20 v = q.m();\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

// —— yield-each ————————————————————————————————————————————————————

/// The extension-type arm on the yield-each face.
#[test]
fn yield_each_extension_type_without_implements_reports() {
    let fixture = "abstract class Stream<T> {}\n\
                   extension type SE<T>(Stream<T> _) {}\n\
                   Stream<Object?> f(SE<Object?> s) async* {\n\
                   \x20 yield* s;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "yield* s;", "s"), 1, YIELD_EACH)],
        "{:?}",
        rows(&report)
    );
}

/// An unbounded `S` operand reports on the yield-each face — the same
/// implicit-`Object?` recursion as the unbounded for-in pin.
#[test]
fn yield_each_unbounded_type_parameter_reports() {
    let fixture = "abstract class Stream<T> {}\n\
                   Stream<Object?> f<S>(S s) async* {\n\
                   \x20 yield* s;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "yield* s;", "s"), 1, YIELD_EACH)],
        "{:?}",
        rows(&report)
    );
}

/// A null-guarded nullable-bound type parameter stays silent on the
/// yield-each face — the promoted `Stream<Object?>` read is
/// non-nullable upstream.
#[test]
fn yield_each_promoted_nullable_bound_type_parameter_is_silent() {
    let fixture = "abstract class Stream<T> {}\n\
                   Stream<Object?> f<SQ extends Stream<Object?>?>(SQ sq) async* {\n\
                   \x20 if (sq == null) return;\n\
                   \x20 yield* sq;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}
