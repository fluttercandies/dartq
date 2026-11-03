//! Ticket #3688 pins (census-12 lane I): the four nullable "special
//! position" faces that share `unchecked_use_of_nullable_value` but carry
//! their own message — a for-in iterator, a spread operand, a condition,
//! and a yield-each operand.
//!
//! Oracle: upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! `nullable_dereference_verifier.dart` `_check` gates on
//! `receiverType is DynamicType || InvalidType || !isPotentiallyNullable`;
//! `isPotentiallyNullable` is `!isNonNullable`, and `isNonNullable`
//! (`dart/element/type_system.dart`) recurses into a type parameter's bound
//! while answering `interfaces.isNotEmpty` for an extension type. So a
//! `?`-less spelling is still potentially nullable when its bound is
//! (`T extends Iterable<int>?`, an unbounded `T` bounded by `Object?`) or
//! when it names an extension type with no `implements` clause. Every call
//! site reports `.at(expression)`.
//!
//! Two of the four are decided here; the other two need the teardown join:
//!
//! * **Condition** — the anchor is listener-owned, and both misses were
//!   anchor defects (an `AsExpression` reported at its operand, a
//!   parenthesized group at its `)`), fixed in `listener.rs`. Active pins.
//! * **Iterator / spread / yield-each** — the rows themselves are decided by
//!   the teardown operand scan (`Engine::stage_operand_faces`), which is
//!   promotion-aware because the operand is grounded through
//!   `Engine::read_types` (filled by the flow replay at the teardown join).
//!   The listener cannot answer them: its rows are the parse-time declared
//!   faces, so emitting there reports a null-guarded operand the oracle
//!   keeps silent — measured, not assumed:
//!   `f<T extends List?>(T v) { if (v == null) return; ...<Object>[...v]; }`
//!   fires from the listener and is silent through the teardown scan. The
//!   fix therefore belonged in the scan's `operand_face_ty_nullable`
//!   predicate (`engine_core.rs`), not here — #3710 extended it to the
//!   `isNonNullable` recursions (type-parameter bound, `FutureOr`
//!   argument, extension-type interfaces) behind the promoted-read gate,
//!   and the five positive pins for those faces are now active against
//!   it. Their already-correct faces (`void`, the `?` spellings) and
//!   their negative controls are active, and
//!   [`for_in_promoted_type_parameter_is_silent`] is the guard that fails if
//!   the predicate is extended without the promoted-bound handling.

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
/// The four messages are the only rows whose text starts this way; the
/// operator/receiver rows the same fixtures also carry share the code but
/// not the text.
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
const CONDITION: &str = "A nullable expression can't be used as a condition.";

// —— for-in iterator ————————————————————————————————————————————————

/// `T extends Iterable<int>?` is a `?`-less spelling whose bound is
/// potentially nullable, so the loop reports at the iterable
/// (`for_resolver.dart` `_checkForEachParts` →
/// `nullableDereferenceVerifier.expression`). The invalid-type verdict for
/// the same operand is a separate code and is not asserted here.
#[test]
fn for_in_type_parameter_nullable_bound_reports() {
    let fixture = "f<T extends Iterable<int>?>(T xs) {\n  for (var y in xs) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in xs)", "xs"), 2, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// An extension type with no `implements` clause is potentially nullable
/// upstream (`isNonNullable` answers `interfaces.isNotEmpty`), so the loop
/// reports even though the spelling carries no `?` — the co19
/// `Extension-types/dynamic_semantics_extension_type_A06_t02` shape.
#[test]
fn for_in_extension_type_without_implements_reports() {
    let fixture = "extension type ET1<T>(Iterable<T> _) {}\n\
                   f(ET1<int> it1) {\n  for (var i in it1) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in it1)", "it1"), 3, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// The negative control for the extension-type arm: the same declaration
/// with the coverage claim in place is strictly non-nullable upstream, so
/// the loop is silent.
#[test]
fn for_in_extension_type_with_implements_is_silent() {
    let fixture = "extension type ET2<T>(Iterable<T> _) implements Iterable<T> {}\n\
                   f(ET2<int> it2) {\n  for (var i in it2) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// The `void` operand (upstream `isPotentiallyNullable`'s `VoidType` arm —
/// the eleven `void/void_type_usage_test` iterator rows): the teardown scan
/// already owns this face, so the row must appear exactly once.
#[test]
fn for_in_void_operand_reports_once() {
    let fixture = "f(void v) {\n  for (var y in v) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "in v)", "v"), 1, ITERATOR)],
        "{:?}",
        rows(&report)
    );
}

/// A null-guarded operand is promoted before the loop, so the oracle's
/// `expression.typeOrThrow` is no longer potentially nullable and the loop
/// is silent. This is the verdict the declared-face predicate extension must
/// keep: it fails the moment the bound arm is bolted on without the
/// promoted-bound handling the teardown join alone can provide.
#[test]
fn for_in_promoted_type_parameter_is_silent() {
    let fixture = "f<T extends Iterable<int>?>(T xs) {\n\
                   \x20 if (xs == null) return;\n  for (var y in xs) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// The plain negative control: a non-nullable iterable never reports.
#[test]
fn for_in_non_nullable_iterable_is_silent() {
    let fixture = "f(Iterable<int> xs) {\n  for (var y in xs) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

// —— spread ————————————————————————————————————————————————————————

/// An unbounded `T` is bounded by the implicit `Object?`, so `...v` reports
/// at the operand (`generated/resolver.dart` `visitSpreadElement` →
/// `nullableDereferenceVerifier.expression`).
#[test]
fn spread_unbounded_type_parameter_reports() {
    let fixture = "void t<T>(T v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "...v]", "v"), 1, SPREAD)],
        "{:?}",
        rows(&report)
    );
}

/// A nullable bound on a `?`-less spelling reports the same way.
#[test]
fn spread_nullable_bound_type_parameter_reports() {
    let fixture = "void t<L extends List?>(L v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "...v]", "v"), 1, SPREAD)],
        "{:?}",
        rows(&report)
    );
}

/// The `void` spread operand (`StaticSemantic_A10_t01`'s `...x4`): owned by
/// the teardown scan, must stay single.
#[test]
fn spread_void_operand_reports_once() {
    let fixture = "void t(void v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "...v]", "v"), 1, SPREAD)],
        "{:?}",
        rows(&report)
    );
}

/// The negative control: a non-nullable operand is silent.
#[test]
fn spread_non_nullable_operand_is_silent() {
    let fixture = "void t(List<int> v) {\n  var y = <Object>[...v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

/// The null-aware control: `...?v` is the author's own null admission and
/// the verifier is never asked (`visitSpreadElement`'s `!node.isNullAware`
/// arm), so even a `?`-suffixed operand stays silent.
#[test]
fn spread_null_aware_operand_is_silent() {
    let fixture = "void t(List<int>? v) {\n  var y = <Object>[...?v];\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

// —— yield-each ————————————————————————————————————————————————————

/// The yield-each operand's nullable face
/// (`yield_statement_resolver.dart` `_resolve` →
/// `nullableDereferenceVerifier.expression`), anchored at the operand.
#[test]
fn yield_each_nullable_bound_type_parameter_reports() {
    let fixture = "abstract class Stream<T> {}\n\
                   Stream<Object?> f<SQ extends Stream<Object?>?>(SQ sq) async* {\n\
                   \x20 yield* sq;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "yield* sq;", "sq"), 2, YIELD_EACH)],
        "{:?}",
        rows(&report)
    );
}

/// The negative control: a non-nullable bound is silent.
#[test]
fn yield_each_non_nullable_stream_is_silent() {
    let fixture = "abstract class Stream<T> {}\n\
                   Stream<Object?> f<SQ extends Stream<Object?>>(SQ sq) async* {\n\
                   \x20 yield* sq;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}

// —— condition ————————————————————————————————————————————————————

/// `x as bool? ? a : b`: the oracle reports `.at(expression)` over the whole
/// `AsExpression` node — `x as bool?` (ten columns), not the operand `x`
/// (`bool_expression_verifier.dart:65`). The `AsExpression`'s `endToken` is
/// the written cast type's last token.
#[test]
fn condition_as_expression_reports_whole_cast_span() {
    let fixture = "void f() {\n  dynamic x, y, z;\n  x as bool? ? (x + y) : z;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(at(fixture, "x as bool?", "x as bool?"), 10, CONDITION)],
        "{:?}",
        rows(&report)
    );
}

/// The parenthesized form reports at the `ParenthesizedExpression` — the
/// whole `(x as bool?)` (twelve columns), not its `)`.
#[test]
fn condition_parenthesized_as_expression_reports_group_span() {
    let fixture = "void f() {\n  dynamic x, y, z;\n  (x as bool?) ? (x + y) : z;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![row(
            at(fixture, "(x as bool?) ?", "(x as bool?)"),
            12,
            CONDITION
        )],
        "{:?}",
        rows(&report)
    );
}

/// The promotion negative control: `b` is legitimately nullable, but the
/// `b == null` guard promotes it to non-nullable `bool` before the second
/// `if`, so the oracle's `expression.typeOrThrow` is non-nullable and the
/// condition is silent.
#[test]
fn condition_promoted_local_is_silent() {
    let fixture = "void f(bool? b) {\n  if (b == null) return;\n  if (b) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        special_position_rows(&report),
        vec![],
        "{:?}",
        rows(&report)
    );
}
