//! #4128 — the initializer face's twin of the direct collection-operand
//! lattice: the diverse leaf joins answer the exact upper bound (never the
//! name-level `Object` collapse), and an `if`-led map entry splits at its
//! branch entries before the per-slot colon.
//!
//! The initializer face (`initializer_literal_ty` → `collection_init_ty` →
//! `init_leaf_least_from` / `init_entry_leaf_tys`) and the direct
//! is-operand face (`collection_operand_ty` → `exact_leaf_lub` /
//! `entry_leaf_tys`) are two spellings of one lattice. The direct face
//! landed the exact join first (#4012); this module pins the initializer
//! face's alignment with it, oracle-probed against `dart analyze` 3.13.7:
//!
//! * a `Null` leaf re-arms the suffix on the join of the non-null forms
//!   (`{1, null}` reads `Set<int?>`, not `Set<Object>`;
//!   `least_upper_bound.dart`'s `UP(int, Null)` = `int?`);
//! * a `?`-suffixed leaf re-arms it too, on both the primitive rung
//!   (`{1, 2.0, null}` → `Set<num?>`, `{true, 1, null}` → `Set<Object?>`)
//!   and the shared-supertype rung (`int?` + `bool` → `Object?`);
//! * a function or record leaf joins its own form and takes the suffix
//!   with a `Null` leg (`{cb, null}` → `Set<int Function(int)?>`,
//!   `{r, null}` → `Set<(int, String)?>`);
//! * map entries join per slot with the same lattice (`{1: null, 2: 'a'}`
//!   → `Map<int, String?>`, `{1: null, null: 'a'}` → `Map<int?, String?>`);
//! * an `if`-led entry is two entries — its branches join per slot
//!   (`{if (c) 1: 'a' else 2: 3}` → `Map<int, Object>`,
//!   `{if (c) 1: 'a' else 'k': 'b'}` → `Map<Object, String>`), a
//!   branchless `if` keeps its then entry
//!   (`typed_literal_resolver.dart`'s `_computeElementType` `IfElementImpl`
//!   arm applies to entries exactly as to elements).
//!
//! The shapes the lattice cannot spell stay the direct face's under-report
//! (`dynamic`): distinct function forms, distinct record shapes, a
//! parameterized head beside a bare one — never a guessed `Object` the
//! oracle's join would not print.

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::diagnose::test_rig::report_ops;
use crate::diagnose::types::FileReport;

/// Every `invalid_assignment` row's message, in report order.
fn assignment_messages(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect()
}

/// A `Null` leaf joins at the nullable leaf: `{1, null}` is `Set<int?>`
/// (`least_upper_bound.dart`'s `UP(int, Null)`), not the bare `Object`
/// the name-level fold answered.
///
/// Probed: one row, `Set<int?>` against `List<String>`.
#[test]
fn null_leg_joins_the_nullable_leaf() {
    let report = report_ops(
        "void main() {\n\
         \x20 var s = {1, null};\n\
         \x20 List<String> x = s;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'Set<int?>' can't be assigned to a variable of type 'List<String>'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The primitive rungs re-arm the suffix: `{1, 2.0, null}` joins at `num`
/// and `{true, 1, null}` at `Object`, each widened by the `Null` leg.
///
/// Probed: two rows, `Set<num?>` and `Set<Object?>` against
/// `List<String>`.
#[test]
fn primitive_joins_rearm_the_null_suffix() {
    let report = report_ops(
        "void main() {\n\
         \x20 var a = {1, 2.0, null};\n\
         \x20 List<String> x = a;\n\
         \x20 var b = {true, 1, null};\n\
         \x20 List<String> y = b;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Set<num?>' can't be assigned to a variable of type 'List<String>'.",
            "A value of type 'Set<Object?>' can't be assigned to a variable of type 'List<String>'.",
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// A nullable leaf joins its non-null form and keeps the suffix — both on
/// the class walk (`int?` + `bool` shares only `Object`, widened to
/// `Object?`) and through the map's independent slots.
///
/// Probed: `Set<Object?>` and `Map<int?, String?>` against
/// `List<String>`.
#[test]
fn nullable_leaves_join_without_dropping_the_suffix() {
    let report = report_ops(
        "void c(int? p) {\n\
         \x20 var s = {p, true};\n\
         \x20 List<String> x = s;\n\
         }\n\
         void d() {\n\
         \x20 var m = {1: null, null: 'a'};\n\
         \x20 List<String> y = m;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Set<Object?>' can't be assigned to a variable of type 'List<String>'.",
            "A value of type 'Map<int?, String?>' can't be assigned to a variable of type 'List<String>'.",
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// A function leaf keeps its own form and takes the suffix with the `Null`
/// leg (`{cb, null}` is `Set<int Function(int)?>`), the arm the
/// `Object` collapse flattened.
///
/// Probed: one row, `Set<int Function(int)?>` against `List<String>`.
#[test]
fn function_leaf_joins_its_form_and_the_null_leg() {
    let report = report_ops(
        "void f(int Function(int) cb) {\n\
         \x20 var s = {cb, null};\n\
         \x20 List<String> x = s;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Set<int Function(int)?>' can't be assigned to a variable of type 'List<String>'."
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// A record leaf joins its own shape and takes the suffix with the `Null`
/// leg (`{r, null}` is `Set<(int, String)?>`), and the record's slots stay
/// as written.
///
/// Probed: one row, `Set<(int, String)?>` against `List<String>`.
#[test]
fn record_leaf_joins_its_shape_and_the_null_leg() {
    let report = report_ops(
        "void g((int, String) r) {\n\
         \x20 var s = {r, null};\n\
         \x20 List<String> x = s;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Set<(int, String)?>' can't be assigned to a variable of type 'List<String>'."
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The preserved arms: an `Object` leaf absorbs the join
/// (`{1, new Object()}` is `Set<Object>`) and an all-`Null` slot keeps
/// `Null` (`{null: 1}` is `Map<Null, int>`).
///
/// Probed: two rows against `List<String>`.
#[test]
fn object_and_all_null_arms_keep_their_faces() {
    let report = report_ops(
        "void main() {\n\
         \x20 var a = {1, new Object()};\n\
         \x20 List<String> x = a;\n\
         \x20 var b = {null: 1};\n\
         \x20 List<String> y = b;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Set<Object>' can't be assigned to a variable of type 'List<String>'.",
            "A value of type 'Map<Null, int>' can't be assigned to a variable of type 'List<String>'.",
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// A map entry's value slot joins the same lattice: `{1: null, 2: 'a'}` is
/// `Map<int, String?>` — the key column stays `int` while the value column
/// re-arms the `Null` leg, one join per slot.
///
/// Probed: one row, `Map<int, String?>` against `List<String>`.
#[test]
fn map_value_slot_rearms_the_null_leg() {
    let report = report_ops(
        "void main() {\n\
         \x20 var m = {1: null, 2: 'a'};\n\
         \x20 List<String> x = m;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Map<int, String?>' can't be assigned to a variable of type 'List<String>'."
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// An `if`-led entry is two entries: the branchless form keeps its then
/// entry (`Map<int, String>`), the `else` form joins per slot
/// (`Map<int, String>`), and divergent value slots join at `Object`
/// (`Map<int, Object>`) instead of taking the last row's `int`.
///
/// Probed: three rows against `List<String>`.
#[test]
fn map_entry_if_splits_its_branches() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool c = true;\n\
         \x20 var a = {if (c) 1: 'a'};\n\
         \x20 List<String> x = a;\n\
         \x20 var b = {if (c) 1: 'a' else 2: 'b'};\n\
         \x20 List<String> y = b;\n\
         \x20 var d = {if (c) 1: 'a' else 2: 3};\n\
         \x20 List<String> z = d;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Map<int, String>' can't be assigned to a variable of type 'List<String>'.",
            "A value of type 'Map<int, String>' can't be assigned to a variable of type 'List<String>'.",
            "A value of type 'Map<int, Object>' can't be assigned to a variable of type 'List<String>'.",
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// Divergent KEY slots join the same way: `{if (c) 1: 'a' else 'k': 'b'}`
/// is `Map<Object, String>` — the two branch keys `int` and `String` join
/// at `Object`, never the then key's `int`.
///
/// Probed: one row, `Map<Object, String>` against `List<String>`.
#[test]
fn map_entry_if_joins_divergent_keys() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool c = true;\n\
         \x20 var m = {if (c) 1: 'a' else 'k': 'b'};\n\
         \x20 List<String> x = m;\n\
         }\n",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'Map<Object, String>' can't be assigned to a variable of type 'List<String>'."
        ],
        "{:?}",
        super::test_rig::rows(&report)
    );
}
