//! #4010 — the collection-literal element LUB answers the oracle's join,
//! not `dynamic`.
//!
//! The inferred declaration's initializer face (`initializer_literal_ty` →
//! `collection_init_ty` → `init_leaf_least_from`) folds heterogeneous
//! element leaves to their least upper bound: unrelated known leaves join
//! at `Object` (upstream `TypeSystem.leastUpperBound` — `int` + `bool` has
//! no common superinterface below `Object`), map entries join per-slot
//! (`Map<join(keys), join(values)>`), `int`/`double`/`num` mixes join at
//! `num`, a spread element contributes its operand's ELEMENT type
//! (`typed_literal_resolver.dart` `_computeElementType`'s iterable arm)
//! and a map spread donates its `Map<K, V>` arguments to the per-slot
//! folds. Preserved arms: a `dynamic` element keeps the literal's
//! `dynamic` argument, an empty `{}` keeps `Map<dynamic, dynamic>`, and
//! an annotated declarator keeps its written context type.

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

/// Unrelated known leaves join at `Object`: `{1, true}` infers
/// `Set<Object>` (upstream `leastUpperBound(int, bool)` is `Object` —
/// the shared-final rule), so `c is Set<Object>` is constant-true and
/// `c is Set<bool>` refuses — the joined face names `Object`, never
/// `dynamic`.
#[test]
fn heterogeneous_leaf_join_spells_object() {
    let report = report_ops(
        "void main() {\n\
         \x20 var c = {1, true};\n\
         \x20 print(c is Set<Object>);\n\
         \x20 print(c is Set<bool>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{1, true}}` joins to `Set<Object>`: the `Set<Object>` check is \
         constant-true and the `Set<bool>` check refuses; got {utc:?}"
    );
}

/// One exact leaf type keeps itself: `{1, 2}` is `Set<int>` — the
/// all-same arm the join folds through unchanged.
#[test]
fn all_same_literal_keeps_its_exact_type() {
    let report = report_ops(
        "void main() {\n\
         \x20 var c = {1, 2};\n\
         \x20 print(c is Set<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{1, 2}}` is `Set<int>`, constant-true against `Set<int>`; got {utc:?}"
    );
}

/// Map entries join per-slot: `{"a": 1, 2: "x"}` is
/// `Map<join(String, int), join(int, String)>` = `Map<Object, Object>` —
/// the oracle's `Map<A1, A2>` shape — and the joined key column refuses
/// a `Map<String, …>` ask.
#[test]
fn map_entry_slots_join_independently() {
    let report = report_ops(
        "void main() {\n\
         \x20 var m = {\"a\": 1, 2: \"x\"};\n\
         \x20 print(m is Map<Object, Object>);\n\
         \x20 print(m is Map<String, int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{\"a\": 1, 2: \"x\"}}` is `Map<Object, Object>`: constant-true \
         against itself, refused by `Map<String, int>`; got {utc:?}"
    );
}

/// The int/double mix joins at `num` — the one diverse pair the
/// primitive lattice names (`leastUpperBound(int, double)` = `num`) —
/// and the joined face refuses a narrower ask.
#[test]
fn int_double_join_spells_num() {
    let report = report_ops(
        "void main() {\n\
         \x20 var l = [1, 2.0];\n\
         \x20 print(l is List<num>);\n\
         \x20 print(l is List<double>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`[1, 2.0]` is `List<num>`: constant-true against `List<num>`, \
         refused by `List<double>`; got {utc:?}"
    );
}

/// The empty `{}` initializer keeps the map default
/// (`Map<dynamic, dynamic>` — the no-context arm), and an annotated
/// declarator keeps its written context type (the downward channel this
/// ticket does not touch).
#[test]
fn empty_and_annotated_literals_keep_their_declared_face() {
    let report = report_ops(
        "void main() {\n\
         \x20 var e = {};\n\
         \x20 print(e is Map<dynamic, dynamic>);\n\
         \x20 List<int> l = [];\n\
         \x20 print(l is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "`{{}}` stays `Map<dynamic, dynamic>` and the annotated `l` stays \
         `List<int>`; got {utc:?}"
    );
}

/// A `dynamic` element keeps the literal's `dynamic` argument (the
/// preserved under-report arm): `[d]` is `List<dynamic>`, constant-true
/// against itself.
#[test]
fn dynamic_element_stays_list_dynamic() {
    let report = report_ops(
        "void main() {\n\
         \x20 dynamic d = 3;\n\
         \x20 var x = [d];\n\
         \x20 print(x is List<dynamic>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "a `dynamic` leaf keeps `List<dynamic>`, constant-true against \
         itself; got {utc:?}"
    );
}

/// A list spread contributes its operand's ELEMENT type, not the
/// operand itself (`_computeElementType`'s iterable arm): `{...s}` over
/// a `List<int>` is `Set<int>`, refused by `Set<String>`.
#[test]
fn list_spread_contributes_its_element_type() {
    let report = report_ops(
        "void main() {\n\
         \x20 var s = [1, 2];\n\
         \x20 var x = {...s};\n\
         \x20 print(x is Set<int>);\n\
         \x20 print(x is Set<String>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{...s}}` over a `List<int>` is `Set<int>`: constant-true \
         against `Set<int>`, refused by `Set<String>`; got {utc:?}"
    );
}

/// A map spread donates its `Map<K, V>` arguments to the per-slot
/// folds (`typed_literal_resolver.dart`'s spread arm): `{...m}` over a
/// `Map<double, String>` is `Map<double, String>`, and over a
/// heterogeneous map joins per-slot to `Map<Object, Object>`.
#[test]
fn map_spread_joins_per_slot() {
    let report = report_ops(
        "void main() {\n\
         \x20 var m = {1.0: \"1\"};\n\
         \x20 var x = {...m};\n\
         \x20 print(x is Map<double, String>);\n\
         \x20 var h = {1.0: \"1\", 2: 2};\n\
         \x20 var y = {...h};\n\
         \x20 print(y is Map<Object, Object>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        2,
        "`{{...m}}` carries the spread's `Map<double, String>` and the \
         heterogeneous spread joins to `Map<Object, Object>`; got {utc:?}"
    );
}
