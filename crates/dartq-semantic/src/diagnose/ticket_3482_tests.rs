//! The #3482 pins — the clause-element-kind check is a per-DECLARATION
//! verdict: `scan_header_clauses` keys each scanned header on its declaring
//! name token's offset (previously on the name itself, which collapsed
//! repeated declarations onto the LAST header and re-reported that one
//! entry at every declaration — `class C` written eighty times emitted the
//! surviving clause row seventy-nine extra times, the corpus's largest
//! duplicate family in `mixin_of_non_class` / `implements_non_class`).
//! The name-keyed substitution faces keep the element model's own
//! last-declaration collapse through `last_of_name`, so only the
//! per-declaration clause check changed shape.
//!
//! Every pin asserts the exact ROW COUNT — presence alone cannot catch the
//! multiplication.

use super::test_rig::report_ops;
use super::{Diagnostic, FileReport};
use dartq_language::Feature;
use super::test_rig::report_ops_experiments_with;

/// The rows of one clause code, in emission order.
fn rows_of<'a>(report: &'a FileReport, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect()
}

/// One declaration carrying a `with` clause over an unresolvable head —
/// the baseline count, unchanged.
#[test]
fn single_with_miss_reports_once() {
    let report = report_ops("class C extends Object with Missing {}");
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
}

/// Two declarations of the SAME name, each carrying the bad clause — the
/// per-declaration identity: each reports its own header exactly once.
/// Before the fix the second declaration's clauses were reported twice
/// (once per declaration) — four rows.
#[test]
fn two_same_name_with_misses_report_once_each() {
    let report = report_ops(
        "class C extends Object with Missing {} class C extends Object with Missing {}",
    );
    let rows = rows_of(&report, "mixin_of_non_class");
    // One row per declaration — two declarations, two distinct anchors.
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
    assert_ne!(
        rows[0].range.offset, rows[1].range.offset,
        "each declaration anchors its own entry"
    );
}

/// The corpus multiplier: eighty declarations of `class C` with the same
/// bad `with` clause report eighty rows — one per declaration — never the
/// 80 × last-header product the name-keyed map emitted.
#[test]
fn repeated_name_reports_per_declaration_not_per_name() {
    let decl = "class C extends Object with Missing {} ";
    let fixture = decl.repeat(80);
    let report = report_ops(&fixture);
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 80, "{:?}", report.diagnostics.len());
    // Every row anchors a DIFFERENT written entry — no two rows share a
    // span (the oracle never emits byte-identical duplicates).
    let mut offsets: Vec<u32> = rows.iter().map(|d| d.range.offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), 80, "distinct anchors: {:?}", offsets);
}

/// Two DIFFERENT classes each carrying the bad clause — the pin that proves
/// the key is the declaration and not "collapse all": distinct names still
/// report each header.
#[test]
fn two_distinct_classes_each_report() {
    let report = report_ops(
        "class A extends Object with Missing {} class B extends Object with Missing {}",
    );
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
}

/// The `implements` half — `issue_48371`'s multiplied family: repeated
/// declarations of the same name each report their own `implements`
/// clause entry once.
#[test]
fn repeated_implements_miss_reports_once_per_declaration() {
    let report = report_ops("class C implements Missing {} class C implements Missing {}");
    let rows = rows_of(&report, "implements_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
    assert_ne!(
        rows[0].range.offset, rows[1].range.offset,
        "each declaration anchors its own entry"
    );
}

/// A repeated enum name — `enum E with Missing` declares no `with` grammar
/// (recovery routes it through the with/implements walk): each repeated
/// declaration still reports its own `with`/`implements` entries once.
#[test]
fn repeated_enum_implements_reports_once_per_declaration() {
    let report =
        report_ops("enum E implements Missing { a } enum E implements Missing { a }");
    let rows = rows_of(&report, "implements_non_class");
    assert_eq!(rows.len(), 2, "{:?}", report.diagnostics);
}

/// The corpus count expectation, `parser-fe/primary_constructors/
/// no_class_body`: eighty `class C` declarations, every face head
/// unresolvable. The oracle emits ONE row per declaration per unresolvable
/// clause entry — 8 `with`-carrying headers in the plain-shape prefix
/// (lines 4,5,7,8,12,13,15,16) plus 32 primary-constructor headers — 40
/// `mixin_of_non_class` rows total. The count pin on the plain-shape
/// prefix alone (where this ticket's map owns the answer): exactly 8, one
/// anchor per declaration, no duplicates. Primary-constructor headers
/// (`class C() with M`) reach the same code through the clause-ledger
/// path.
#[test]
fn corpus_plain_shape_prefix_reports_eight_distinct_anchors() {
    let fixture = concat!(
        "class C;",
        "class C extends O;",
        "class C extends O implements I;",
        "class C extends O with M implements I;",
        "class C extends O with M;",
        "class C implements I;",
        "class C with M implements I;",
        "class C with M;",
        "class C<T>;",
        "class C<T> extends O<T>;",
        "class C<T> extends O<T> implements I<T>;",
        "class C<T> extends O<T> with M<T> implements I<T>;",
        "class C<T> extends O<T> with M<T>;",
        "class C<T> implements I<T>;",
        "class C<T> with M<T> implements I<T>;",
        "class C<T> with M<T>;",
    );
    let report = report_ops(fixture);
    let rows = rows_of(&report, "mixin_of_non_class");
    assert_eq!(rows.len(), 8, "{:?}", report.diagnostics);
    let mut offsets: Vec<u32> = rows.iter().map(|d| d.range.offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), 8, "distinct anchors: {offsets:?}");
}

/// The corpus-count pin — `parser-fe/primary_constructors/no_class_body`
/// verbatim (eighty `class C` declarations; `O`, `M`, `I` unresolvable).
/// The oracle emits ONE row per declaration per unresolvable clause entry —
/// upstream's `_resolveType` runs on every `ClassDeclaration` node's
/// clauses independently of the `duplicate_definition` verdict
/// (`resolution_visitor.dart:1169-1241`), so repeated names still report
/// each header's own entries: **40 `mixin_of_non_class`, 40
/// `implements_non_class`, 20 `extends_non_class`, 20
/// `mixin_with_non_class_superclass`, all at distinct anchors**. Before
/// this fix the name-keyed map re-reported the last parsed header's clauses
/// at every one of the eighty declarations — the corpus's ~200 byte-
/// identical extra rows. Primary-constructor headers (`class C() with M`)
/// reach the same codes through the `clause_miss` ledger path: their
/// headers scan "unfaithfully" for the element-kind map (the `(` token),
/// so `check_clause_element_kinds` declines and the ledger's own span-
/// recorded entries report instead — still one row per declaration.
#[test]
fn corpus_no_class_body_counts_match_the_oracle() {
    let fixture = "class C; class C extends O; class C extends O implements I; class C extends O with M implements I; class C extends O with M; class C implements I; class C with M implements I; class C with M; class C<T>; class C<T> extends O<T>; class C<T> extends O<T> implements I<T>; class C<T> extends O<T> with M<T> implements I<T>; class C<T> extends O<T> with M<T>; class C<T> implements I<T>; class C<T> with M<T> implements I<T>; class C<T> with M<T>; class C(); class C() extends O; class C() extends O implements I; class C() extends O with M implements I; class C() extends O with M; class C() implements I; class C() with M implements I; class C() with M; class C<T>(); class C<T>() extends O<T>; class C<T>() extends O<T> implements I<T>; class C<T>() extends O<T> with M<T> implements I<T>; class C<T>() extends O<T> with M<T>; class C<T>() implements I<T>; class C<T>() with M<T> implements I<T>; class C<T>() with M<T>; class const C(); class const C() extends O; class const C() extends O implements I; class const C() extends O with M implements I; class const C() extends O with M; class const C() implements I; class const C() with M implements I; class const C() with M; class const C<T>(); class const C<T>() extends O<T>; class const C<T>() extends O<T> implements I<T>; class const C<T>() extends O<T> with M<T> implements I<T>; class const C<T>() extends O<T> with M<T>; class const C<T>() implements I<T>; class const C<T>() with M<T> implements I<T>; class const C<T>() with M<T>; class C.named(); class C.named() extends O; class C.named() extends O implements I; class C.named() extends O with M implements I; class C.named() extends O with M; class C.named() implements I; class C.named() with M implements I; class C.named() with M; class C<T>.named(); class C<T>.named() extends O<T>; class C<T>.named() extends O<T> implements I<T>; class C<T>.named() extends O<T> with M<T> implements I<T>; class C<T>.named() extends O<T> with M<T>; class C<T>.named() implements I<T>; class C<T>.named() with M<T> implements I<T>; class C<T>.named() with M<T>; class const C.named(); class const C.named() extends O; class const C.named() extends O implements I; class const C.named() extends O with M implements I; class const C.named() extends O with M; class const C.named() implements I; class const C.named() with M implements I; class const C.named() with M; class const C<T>.named(); class const C<T>.named() extends O<T>; class const C<T>.named() extends O<T> implements I<T>; class const C<T>.named() extends O<T> with M<T> implements I<T>; class const C<T>.named() extends O<T> with M<T>; class const C<T>.named() implements I<T>; class const C<T>.named() with M<T> implements I<T>; class const C<T>.named() with M<T>;";
    let report = report_ops_experiments_with(
        fixture,
        &[],
        None,
        &[Feature::PrimaryConstructors],
    );
    for (code, count) in [
        ("mixin_of_non_class", 40),
        ("implements_non_class", 40),
        ("extends_non_class", 20),
        ("mixin_with_non_class_superclass", 20),
    ] {
        let rows = rows_of(&report, code);
        assert_eq!(rows.len(), count, "{code}: {:?}", report.diagnostics);
        let mut offsets: Vec<u32> = rows.iter().map(|d| d.range.offset).collect();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), count, "{code} distinct anchors: {offsets:?}");
    }
}
