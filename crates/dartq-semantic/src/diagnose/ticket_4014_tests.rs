//! #4014 — the two cluster-B residuals of the collection-element fold: an
//! if/else element joins its BRANCH leaves, and a spread whose operand is a
//! type parameter reads the parameter's declared bound.
//!
//! The if/else element took the last branch's row
//! (`[if (c) 1 else "s"]` answered `List<String>`); the oracle joins both
//! branches (`typed_literal_resolver.dart` `_computeElementType`'s
//! IfElementImpl arm — `leastUpperBound(thenType, elseType)`, a missing
//! `else` keeping the then leaf), so `1 | "s"` joins at `Object` and the
//! literal infers `List<Object>`. The generic-bound spread answered `None`
//! wherever the operand's head is an in-scope type parameter (`{...t}`
//! over `t: T extends Iterable<num>`), while the oracle's
//! `asInstanceOf(Iterable)` reads the bound's interface — the bound's
//! first argument answers (`Set<num>`); `T extends Map<K, V>` feeds the
//! map's per-slot folds the same way. Preserved arms: a single-branch `if`
//! keeps its one leaf, identical branches stay that type, a `dynamic`
//! branch folds to `dynamic`, and an unbounded `T` stays the under-report
//! (the oracle's spread over a boundless parameter leaves the literal
//! ambiguous — no constant-true check to fire).

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

/// Heterogeneous branches join at `Object`: `[if (cond) 1 else "s"]`
/// infers `List<Object>` (upstream `leastUpperBound(int, String)`), so
/// the `List<Object>` check is constant-true and the `List<String>`
/// check refuses — the last branch's row no longer decides. The join
/// rides the engine-side leaf read's if-element split (#4012: the
/// initializer face's arm landed here in #4014, the bind-time leg
/// through the shared `collection_operand_ty` face closes this pin).
#[test]
fn if_else_branches_join_at_object() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 var c = [if (cond) 1 else \"s\"];\n\
         \x20 print(c is List<Object>);\n\
         \x20 print(c is List<String>);\n\
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

/// A single-branch `if` keeps its one leaf: `[if (cond) 1]` is
/// `List<int>` — the no-else arm returns the then leaf unchanged.
#[test]
fn single_branch_if_keeps_its_leaf() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 var c = [if (cond) 1];\n\
         \x20 print(c is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`[if (cond) 1]` keeps `List<int>`, constant-true against itself; \
         got {utc:?}"
    );
}

/// Identical branches stay that type: `[if (cond) 1 else 2]` is
/// `List<int>` — the join of two equal leaves folds to the leaf itself.
#[test]
fn identical_if_branches_keep_their_type() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 var c = [if (cond) 1 else 2];\n\
         \x20 print(c is List<int>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`[if (cond) 1 else 2]` stays `List<int>`, constant-true against \
         itself; got {utc:?}"
    );
}

/// A `dynamic` branch folds the join to `dynamic`: `[if (cond) d else 1]`
/// is `List<dynamic>`, constant-true against itself.
#[test]
fn dynamic_if_branch_folds_to_dynamic() {
    let report = report_ops(
        "void main() {\n\
         \x20 bool cond = true;\n\
         \x20 dynamic d = 3;\n\
         \x20 var c = [if (cond) d else 1];\n\
         \x20 print(c is List<dynamic>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "a `dynamic` branch keeps `List<dynamic>`, constant-true against \
         itself; got {utc:?}"
    );
}

/// A spread over a bounded type parameter reads the bound's first
/// argument (the oracle's `asInstanceOf(Iterable)` over the parameter's
/// interface): `{...t}` with `T extends Iterable<num>` infers
/// `Set<num>`, constant-true against `Set<num>` and refused by
/// `Set<String>`.
#[test]
fn iterable_bound_spread_answers_its_first_argument() {
    let report = report_ops(
        "void f<T extends Iterable<num>>(T t) {\n\
         \x20 var s = {...t};\n\
         \x20 print(s is Set<num>);\n\
         \x20 print(s is Set<String>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{...t}}` over `T extends Iterable<num>` is `Set<num>`: \
         constant-true against `Set<num>`, refused by `Set<String>`; got \
         {utc:?}"
    );
}

/// A `List`-headed bound answers its element: `{...t}` with
/// `T extends List<String>` infers `Set<String>` — the bound's head
/// reaches `Iterable` with `String` in the first slot.
#[test]
fn list_bound_spread_answers_its_element() {
    let report = report_ops(
        "void f<T extends List<String>>(T t) {\n\
         \x20 var s = {...t};\n\
         \x20 print(s is Set<String>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{...t}}` over `T extends List<String>` is `Set<String>`, \
         constant-true against itself; got {utc:?}"
    );
}

/// A `Map`-headed bound feeds the map spread's per-slot folds:
/// `{...t}` with `T extends Map<num, String>` infers
/// `Map<num, String>`, constant-true against itself.
#[test]
fn map_bound_spread_fills_the_per_slot_folds() {
    let report = report_ops(
        "void f<T extends Map<num, String>>(T t) {\n\
         \x20 var m = {...t};\n\
         \x20 print(m is Map<num, String>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        1,
        "`{{...t}}` over `T extends Map<num, String>` is \
         `Map<num, String>`, constant-true against itself; got {utc:?}"
    );
}

/// An unbounded parameter stays the under-report: `{...t}` over a bare
/// `T` leaves the literal ambiguous (the oracle answers no constant-true
/// face there — the spread's element stays unspellable), so no
/// `unnecessary_type_check` fires on the `Set<dynamic>` ask.
#[test]
fn unbounded_type_param_spread_stays_under_report() {
    let report = report_ops(
        "void f<T>(T t) {\n\
         \x20 var s = {...t};\n\
         \x20 print(s is Set<dynamic>);\n\
         }\n",
    );
    let utc = texts(&report, "unnecessary_type_check");
    assert_eq!(
        utc.len(),
        0,
        "a spread over an unbounded `T` answers no constant-true face — \
         the `Set<dynamic>` check must NOT fire; got {utc:?}"
    );
}
