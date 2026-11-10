//! #3823 pins — the `unchecked_use_of_nullable_value` face.
//!
//! Census `census-20261104-020335` (oracle 3.13.7): 380 rows =
//! 188 missing / 192 extra across 145 cases. The near-balanced split means
//! the DECISION is broadly right and the per-shape arms are wrong; the
//! pins below cover the arms the engine's statement-operand teardown walk
//! ([`super::engine_core`]'s `stage_operand_faces`) owns.
//!
//! Oracle contract (`dart analyze` 3.13.7): `type_property_resolver.dart`
//! :82-235 selects the message by the access node's own shape and asks
//! `isPotentiallyNullable` of the RECEIVER EXPRESSION's static type;
//! `nullable_dereference_verifier.dart` :40-60 picks the code and swaps to
//! `invalid_use_of_null_value` for a `Null` receiver; `for_resolver.dart`
//! :71-88, `resolver.dart` :4180-4195 and `yield_statement_resolver.dart`
//! :163-168 are the statement-operand asks, each anchored `.at(expression)`.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::types::FileReport;

/// messages.yaml's spread operand row (`uncheckedUseOfNullableValueInSpread`).
const SPREAD_MSG: &str = "A nullable expression can't be used in a spread.";
/// messages.yaml's for-in row (`uncheckedUseOfNullableValueAsIterator`).
const ITERATOR_MSG: &str = "A nullable expression can't be used as an iterator in a for-in loop.";
/// The member-invocation family's shared tail (method/operator variants).
const INVOKE_MSG: &str = "can't be unconditionally invoked";

/// Every `unchecked_use_of_nullable_value` row's `(offset, length, message)`
/// in report order.
fn unchecked_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Asserts the unchecked rows sit at exactly `expected`
/// `(fixture_offset, length, message_fragment)` triples (`CORE_PRELUDE`
/// shifted inside).
#[track_caller]
fn assert_unchecked(report: &FileReport, fixture: &str, expected: &[(usize, u32, &str)]) {
    let prelude = CORE_PRELUDE.len() as u32;
    let mut got: Vec<(u32, u32, String)> = unchecked_rows(report);
    got.sort_by_key(|(offset, _, _)| *offset);
    let want: Vec<(u32, u32, &str)> = expected
        .iter()
        .map(|(offset, length, prefix)| (prelude + *offset as u32, *length, *prefix))
        .collect();
    assert_eq!(
        got.len(),
        want.len(),
        "fixture={fixture:?}\nrows={:#?}",
        rows(report)
    );
    for ((offset, length, message), (want_offset, want_length, prefix)) in
        got.iter().zip(want.iter())
    {
        assert_eq!(
            (*offset, *length),
            (*want_offset, *want_length),
            "fixture={fixture:?}\nrows={:#?}",
            rows(report)
        );
        assert!(
            message.contains(prefix),
            "fixture={fixture:?}: message {message:?} lacks {prefix:?}\nrows={:#?}",
            rows(report)
        );
    }
}

/// A `void`-typed for-in iterable reports the iterator face at the operand
/// (`for_resolver.dart:71-74` — the verifier's `.at(iterable)` ask fires on
/// the operand expression, never the `for` keyword).
///
/// Probed: `ERROR|COMPILE_TIME_ERROR|UNCHECKED_USE_OF_NULLABLE_VALUE|…|3|17|1|A
/// nullable expression can't be used as an iterator in a for-in loop.`
#[test]
fn void_for_in_iterable_reports_iterator_face() {
    let fixture = "Object? f(void x) {\n\
         \x20 for (var y in x) {}\n\
         \x20 return null;\n\
         }\n";
    let report = report_ops(fixture);
    assert_unchecked(&report, fixture, &[(36, 1, ITERATOR_MSG)]);
}

/// A ParenthesizedExpression operand types as its INNER expression
/// (`ParenthesizedExpression.staticType` is the child's) while the oracle's
/// `.at(expression)` anchor covers the parens — `for (var v in (x))`
/// reports `(x)`, length 3, never the inner `x`.
///
/// Probed (the same shape with `for (var v in (p))` over a `void p`):
/// `…|8|17|3|A nullable expression can't be used as an iterator in a
/// for-in loop.`
#[test]
fn void_for_in_parenthesized_iterable_reports_at_the_group() {
    let fixture = "Object? f(void x) {\n\
         \x20 for (var v in (x)) {}\n\
         \x20 return null;\n\
         }\n";
    let report = report_ops(fixture);
    assert_unchecked(&report, fixture, &[(36, 3, ITERATOR_MSG)]);
}

/// A nullable spread operand (`[...c.lp]` over a `List<int>?` public field,
/// unpromotable) reports the spread face at the operand
/// (`resolver.dart:4188-4193`, gated on `!node.isNullAware`).
///
/// Probed: `…|7|15|4|A nullable expression can't be used in a spread.`
#[test]
fn nullable_spread_operand_reports_spread_face() {
    let fixture = "class C {\n\
         \x20 List<int>? lp;\n\
         }\n\
         Object? f(C c) {\n\
         \x20 var a = [...c.lp];\n\
         \x20 return a;\n\
         }\n";
    let report = report_ops(fixture);
    assert_unchecked(&report, fixture, &[(60, 4, SPREAD_MSG)]);
}

/// An explicitly null-aware spread (`...?`) is the `!node.isNullAware`
/// false arm — the oracle stays silent, so the face must not fire
/// (`resolver.dart:4188-4193`).
///
/// Probed: no `unchecked_use_of_nullable_value` row.
#[test]
fn null_aware_spread_stays_silent() {
    let fixture = "class C {\n\
         \x20 List<int>? lp;\n\
         }\n\
         Object? f(C c) {\n\
         \x20 var a = [...?c.lp];\n\
         \x20 return a;\n\
         }\n";
    let report = report_ops(fixture);
    assert_unchecked(&report, fixture, &[]);
}

/// The unary void operand through a parenthesized group (`-(x)` over a
/// `void` parameter): upstream routes the prefix `-` through the type
/// property resolver's nullable arm and resolves the `unary-` METHOD, so
/// the row is the method face anchored at the operator token
/// (`prefix_expression_resolver.dart:152-227`).
///
/// Probed: `…|9|11|1|The method 'unary-' can't be unconditionally invoked
/// because the receiver can be 'null'.`
#[test]
fn void_parenthesized_negation_reports_method_face() {
    let fixture = "Object? f(void x) {\n\
         \x20 return -(x);\n\
         }\n";
    let report = report_ops(fixture);
    assert_unchecked(&report, fixture, &[(29, 1, INVOKE_MSG)]);
}
