//! #4012 — the cluster-B engine-side twin: a DIRECT collection-literal
//! operand (the `is` check's operand IS the literal) folds its leaves
//! through the exact join, and an if-led element joins its BRANCH leaves.
//!
//! The direct operand rode `collection_operand_ty` → `least_leaf_ty` and
//! folded on the name-level lattice: heterogeneous known leaves that do
//! not lift answered `dynamic` (the oracle's `Object` — upstream
//! `TypeSystem.leastUpperBound`'s top rung for known leaves), a `Null`
//! leg answered `dynamic` (the oracle's nullable join), and the
//! spread-only map-decided literal (`{...m}`) answered the
//! `Map<dynamic, dynamic>` default where the oracle reads the spread's
//! `Map<K, V>` arguments — the same conventions the initializer face
//! (`init_leaf_least_from` / `init_entry_leaf_tys`) landed in #4010,
//! now shared through the exact join. The if-led element
//! took its last in-span row (`[if (cond) 1 else "s"]` read `String`);
//! the oracle joins the branches (`typed_literal_resolver.dart`
//! `_computeElementType`'s IfElementImpl arm —
//! `leastUpperBound(thenType, elseType)`, a missing `else` keeping the
//! then leaf), at the is-operand face here and at the bind-time leg the
//! closing pin of #4014 tracks.

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// Heterogeneous known leaves join at `Object` at the direct operand:
/// `{1, false}` reads `Set<Object>` (upstream
/// `leastUpperBound(int, bool)` — no common superinterface below
/// `Object`), so the `Set<Object>` check is constant-true and the
/// `Set<bool>` check refuses — the operand face no longer answers the
/// `dynamic` under-report.
#[test]
fn direct_literal_leaves_join_at_object() {
    let report = report_ops(
        "void main() {\n\
         \x20 print({1, false} is Set<Object>);\n\
         \x20 print({1, false} is Set<bool>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{1, false}}` joins its leaves at `Set<Object>`: constant-true \
         against `Set<Object>`, refused by `Set<bool>`; got {utc:?}"
    );
}

/// A spread-only map-decided literal keeps the spread's slot types, and
/// a spread segment inside an entry literal donates its `Map<K, V>`
/// arguments to the per-slot folds: `{...int_map}` reads
/// `Map<int, int>` (constant-true against itself, refused by
/// `Map<String, int>`) and `{...int_map, 3: 4}` joins the spread's
/// slots with the written entry's — the map default's
/// `Map<dynamic, dynamic>` no longer swallows the donation.
#[test]
fn spread_map_operand_keeps_its_slot_types() {
    let report = report_ops(
        "void main() {\n\
         \x20 Map<int, int> int_map = {1: 2};\n\
         \x20 print({...int_map} is Map<int, int>);\n\
         \x20 print({...int_map} is Map<String, int>);\n\
         \x20 print({...int_map, 3: 4} is Map<int, int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "`{{...int_map}}` donates `Map<int, int>` slots and \
         `{{...int_map, 3: 4}}` joins them per-slot: the two \
         `Map<int, int>` asks fire, `Map<String, int>` refuses; got {utc:?}"
    );
}

/// The bind-time leg rides the same face: `var m = {...int_map}` binds
/// the donation (`Map<int, int>`), so the later is-check is
/// constant-true — the variable's inferred slot answers the oracle's
/// spread map, not the `dynamic` default.
#[test]
fn spread_map_bind_leg_keeps_the_slots() {
    let report = report_ops(
        "void main() {\n\
         \x20 Map<int, int> int_map = {1: 2};\n\
         \x20 var m = {...int_map};\n\
         \x20 print(m is Map<int, int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`var m = {{...int_map}}` binds `Map<int, int>`, constant-true \
         against itself; got {utc:?}"
    );
}

/// Heterogeneous branches join at `Object` at the direct operand:
/// `[if (cond) 1 else "s"]` reads `List<Object>` (upstream
/// `leastUpperBound(int, String)`), so the `List<Object>` check is
/// constant-true and the `List<String>` check refuses — the last
/// branch's in-span row no longer decides the element.
#[test]
fn if_else_branches_join_at_object_direct() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 print([if (cond) 1 else \"s\"] is List<Object>);\n\
         \x20 print([if (cond) 1 else \"s\"] is List<String>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`[if (cond) 1 else \"s\"]` joins the branches to `List<Object>`: \
         constant-true against `List<Object>`, refused by `List<String>`; \
         got {utc:?}"
    );
}

/// Identical branches keep their type and a single-branch `if` keeps
/// its one leaf: `[if (cond) 1 else 2]` is `List<int>` (the join of two
/// equal leaves folds to the leaf) and `[if (cond) 1]` is `List<int>`
/// (the no-else arm returns the then leaf), each constant-true against
/// itself and refused off-type.
#[test]
fn identical_and_single_branch_leaves_keep_their_type() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 print([if (cond) 1 else 2] is List<int>);\n\
         \x20 print([if (cond) 1 else 2] is List<String>);\n\
         \x20 print([if (cond) 1] is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "identical branches stay `List<int>` and a single-branch `if` \
         keeps its leaf: the two `List<int>` asks fire, `List<String>` \
         refuses; got {utc:?}"
    );
}

/// A `Null` leg joins at the nullable leaf: `[1, null]` reads
/// `List<int?>` (upstream `leastUpperBound(int, Null)` = `int?`), so
/// the `List<int?>` check is constant-true and the `List<int>` check
/// refuses — the nullable join the name-level lattice kept
/// `dynamic`.
#[test]
fn null_leaf_joins_at_the_nullable_leaf() {
    let report = report_ops(
        "void main() {\n\
         \x20 print([1, null] is List<int?>);\n\
         \x20 print([1, null] is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`[1, null]` joins at `List<int?>`: constant-true against \
         `List<int?>`, refused by `List<int>`; got {utc:?}"
    );
}
