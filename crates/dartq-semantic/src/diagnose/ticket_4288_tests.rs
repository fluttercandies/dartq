//! #4288 pins: the record-vs-record assignability verdict inside a typed
//! list literal's element check.
//!
//! `<(int, int)>[(3, 'z')]` parsed both sides correctly — the written
//! `<(int, int)>` group folds to `Ty::Record` through
//! `literal_group_args`/`parse_record_type_arg` (#4279/#4036), and the
//! record literal element pushes its own `Ty::Record` row — but the
//! element check's transparent-wrapper peel treated the record literal's
//! `( … )` as a parenthesized group, peeled it, and stranded the row
//! search on the fields' drained residue: `list_element_type_not_assignable`
//! stayed silent where the oracle reports. The peel now declines a `(`
//! group whose `)` carries the record row ([`RowShape::Record`]), the
//! fallback row answers `Ty::Record`, and the existing record arm of
//! [`Engine::assignable`] decides positional arity + per-slot,
//! named-field set + per-slot, and the nullable flags — the same verdict
//! record-typed VARIABLES already exercised.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the same shapes (the
//! matrix below mirrors the probe file's rows verbatim).

use super::FileReport;
use super::test_rig::report_ops;

/// The `list_element_type_not_assignable` messages, in source order.
fn element_texts(report: &FileReport) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "list_element_type_not_assignable")
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// Positional arity and per-slot order decide: a truncated or extended
/// record and a per-slot type mismatch each report the oracle's row.
#[test]
fn record_element_arity_and_slot_mismatches_report() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <(int, int)>[(1,)];\n\
         \x20 final b = <(int, int)>[(1, 2, 3)];\n\
         \x20 final c = <(int, int)>[(1, 'x')];\n\
         \x20 final d = <(String, int)>[(1, 2)];\n\
         \x20 final e = <(int,)>[(1, 'x')];\n\
         \x20 final f = <(int, int)>[()];\n\
         }\n",
    );
    assert_eq!(
        element_texts(&report),
        vec![
            "The element type '(int,)' can't be assigned to the list type \
             '(int, int)'."
                .to_string(),
            "The element type '(int, int, int)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
            "The element type '(int, int)' can't be assigned to the list \
             type '(String, int)'."
                .to_string(),
            "The element type '(int, String)' can't be assigned to the \
             list type '(int,)'."
                .to_string(),
            "The element type '()' can't be assigned to the list type \
             '(int, int)'."
                .to_string(),
        ],
    );
}

/// The named-field set decides by name set + per-slot type: a missing or
/// extra field and a slot-type mismatch report; a permuted written order
/// of the same set stays silent (record-type identity is a name set).
#[test]
fn record_element_named_field_set_decides() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <(int, {String s})>[(1,)];\n\
         \x20 final b = <(int, {String s})>[(1, s: 'x', t: 2)];\n\
         \x20 final c = <(int, {String s})>[(1, s: 5)];\n\
         \x20 final d = <(int, {String s})>[(s: 'x')];\n\
         \x20 final e = <({int a, String b})>[(a: 1, b: 2)];\n\
         \x20 final ok1 = <(int, {String s})>[(1, s: 'x')];\n\
         \x20 final ok2 = <({int a, String b})>[(b: 'x', a: 1)];\n\
         }\n",
    );
    assert_eq!(
        element_texts(&report),
        vec![
            "The element type '(int,)' can't be assigned to the list type \
             '(int, {String s})'."
                .to_string(),
            "The element type '(int, {String s, int t})' can't be assigned \
             to the list type '(int, {String s})'."
                .to_string(),
            "The element type '(int, {int s})' can't be assigned to the \
             list type '(int, {String s})'."
                .to_string(),
            "The element type '({String s})' can't be assigned to the list \
             type '(int, {String s})'."
                .to_string(),
            "The element type '({int a, int b})' can't be assigned to the \
             list type '({int a, String b})'."
                .to_string(),
        ],
        "the two matching spellings stay silent"
    );
}

/// Nested record fields recurse through the same verdict, a nullable
/// declared element accepts the non-nullable literal and `null`, and a
/// parenthesized record literal reports at the whole element extent.
#[test]
fn record_element_nested_nullable_and_wrapped() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <((int, int), String)>[((1, 'x'), 'a')];\n\
         \x20 final b = <((int, int), String)>[((1, 2), 'a')];\n\
         \x20 final c = <(int, int)?>[(1, 'x')];\n\
         \x20 final ok1 = <(int, int)?>[(1, 2)];\n\
         \x20 final ok2 = <(int, int)?>[null];\n\
         \x20 final d = <(int, int)>[((1, 'x'))];\n\
         \x20 final e = <()>[()];\n\
         }\n",
    );
    assert_eq!(
        element_texts(&report),
        vec![
            "The element type '((int, String), String)' can't be assigned \
             to the list type '((int, int), String)'."
                .to_string(),
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)?'."
                .to_string(),
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
        ],
    );
    // The wrapped record reports at the whole element extent — the
    // oracle's `.at(element)` covers `((1, 'x'))` (10 columns), not the
    // record row's bare `)`.
    let wrapped = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "list_element_type_not_assignable"
                && d.message.contains("'(int, String)'")
                && d.message.contains("'(int, int)'.")
        })
        .last()
        .expect("the wrapped record row");
    assert_eq!(wrapped.range.length, 10, "{wrapped:?}");
}

/// Field-type assignability rides the ordinary lattice: `double` into
/// `int` refuses, `double`/`int` into `num` accepts, and the untyped
/// literal under a `List<(int, int)>` context asks the same question.
#[test]
fn record_element_field_lattice_and_untyped_context() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <(int, int)>[(1, 2.5)];\n\
         \x20 final b = <(num, num)>[(1, 2.5)];\n\
         \x20 final c = <(num, num)>[(1, 2)];\n\
         \x20 List<(int, int)> d = [(1, 'x')];\n\
         \x20 List<(int, int)> ok = [(1, 2)];\n\
         }\n",
    );
    assert_eq!(
        element_texts(&report),
        vec![
            "The element type '(int, double)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
        ],
    );
}

/// A spread element contributes its iterable's element type — a
/// `List<(int, String)>` spread into `<(int, int)>[…]` asks the same
/// record-vs-record verdict at the operand.
#[test]
fn record_element_spread_operand_asks_the_verdict() {
    let report = report_ops(
        "void main() {\n\
         \x20 final rs = <(int, String)>[(1, 'x')];\n\
         \x20 final ok = <(int, int)>[(1, 2)];\n\
         \x20 final a = <(int, int)>[...rs];\n\
         \x20 final b = <(int, int)>[...ok];\n\
         }\n",
    );
    assert_eq!(
        element_texts(&report),
        vec![
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)'."
                .to_string(),
        ],
    );
}
