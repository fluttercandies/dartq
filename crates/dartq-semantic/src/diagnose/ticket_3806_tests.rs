//! The #3806 pins — `dot_shorthand_undefined_member` context faces that the
//! `undefined_getter` census lane reached through
//! `crates/dartq-semantic/src/diagnose/scopes_bodies.rs` (`shorthand_ctx_at`).
//!
//! # The two wrong facts
//!
//! 1. **The comparison node answered for the left operand.** `fold_binary`
//!    drains `==`'s operand rows and pushes one `Value` row spanning the
//!    whole binary (typed `bool`). That row's range STARTS at the left
//!    operand's head, so when the head is a callee
//!    (`print(C(1) == .a)`, `take(C(1) == .a)`) the staged span is the
//!    callee's own call span — which ends exactly at the operator and so
//!    passed the adjacency test. The shorthand then resolved against
//!    `'bool'`: 40 census extras, every one of them a passing upstream test
//!    (`equality_A01_t04`, `constant_expression_A03/A04/A05`,
//!    `test_equality_indexExpression`, …).
//! 2. **A relational pattern's operand read the unknown schema.** `case ==
//!    <parenthesized>` resolved its nested shorthand against `'_'` (the
//!    read arm's whole-span `dot_shorthand_missing_context`) where upstream
//!    arms the `==` invocation's parameter face `Object`: 29 missing rows
//!    plus the 29 paired `missing_context` extras on
//!    `equality_error_test` / `equality_A02_t05…t08`.
//!
//! # The contract (oracle `dart analyze` 3.13.7)
//!
//! - `x == .a`: the right operand resolves against the LEFT operand's
//!   static type — never against the comparison's own `bool`
//!   (`print(C(1) == .a)` reports `'C'`, `false == .a` reports `'bool'`).
//! - `e case == .a` (a DIRECT operand): the matched value's static type
//!   (`int` for an `int` scrutinee).
//! - `e case == (.a)` and `e case == (c ? .a : .b)` (a NESTED operand):
//!   `'Object'`, the `Object.==(Object)` parameter — for every scrutinee
//!   family (`int`, enum, extension type, `Object`).
//!
//! Fixture provenance: each shape was measured against the oracle before the
//! pin (`case == (.x)` → `'Object'`; `case == .x` → the scrutinee head;
//! `C(1) == .x` → `'C'`), and the same file through both engines.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

/// The `dot_shorthand_undefined_member` rows of `report`, in the rig's
/// `code @offset: message` shape.
fn shorthand(report: &FileReport) -> Vec<String> {
    rows(report)
        .into_iter()
        .filter(|row| row.starts_with("dot_shorthand_undefined_member "))
        .collect()
}

/// The single shorthand row of a fixture that yields exactly one.
fn only_shorthand(report: &FileReport) -> String {
    let mut found = shorthand(report);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one dot_shorthand_undefined_member row: {:?}",
        rows(report)
    );
    found.remove(0)
}

// —— DS-1: an argument-position `==` operand resolves against the left
// —— callee's type, not against the comparison's own `bool`. The drained
// —— composite row is what answers today.
#[test]
fn equality_operand_in_argument_position_reads_the_left_callee() {
    let r = report_ops(
        "class C { int x; C(this.x); }\n\
         void take(bool b) {}\n\
         void main() { take(C(1) == .missing); }\n",
    );
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'C'."),
        "the left operand is the creation's `C`, never the comparison's `bool`: {row}"
    );
}

// —— DS-1, the statement-position twin (`print(C(1) == .a)` in the census):
// —— a call whose result feeds a non-`bool` sink must not turn the context
// —— into the enclosing expression's type either.
#[test]
fn equality_operand_in_a_void_argument_position_reads_the_left_callee() {
    let r = report_ops(
        "class C { int x; C(this.x); }\n\
         void print2(Object? o) {}\n\
         void main() { print2(C(1) == .missing); }\n",
    );
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'C'."),
        "a non-`bool` sink never supplies the operand's context: {row}"
    );
}

// —— DS-1, the declaration form that already worked — pinned so the fix
// —— cannot trade one position for another.
#[test]
fn equality_operand_in_a_declaration_reads_the_left_callee() {
    let r = report_ops(
        "class C { int x; C(this.x); }\n\
         void main() { var v = C(1) == .missing; }\n",
    );
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'C'."),
        "the declaration initializer keeps the left operand's face: {row}"
    );
}

// —— DS-1's suppression guard: a GENUINE `bool` left operand keeps its
// —— type. The comparison-row filter must reject the comparison node only,
// —— never a real operand that happens to be `bool`.
#[test]
fn equality_operand_with_a_bool_left_operand_keeps_bool() {
    let r = report_ops("void main() { var v = false == .missing; }\n");
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'bool'."),
        "a `bool` left operand still arms `bool`: {row}"
    );
}

// —— DS-2: a parenthesized relational-pattern operand arms the `==`
// —— parameter face `Object` — not the unknown schema (whose read arm
// —— reports the whole-span `missing_context` the oracle never emits).
#[test]
fn relational_pattern_parenthesized_operand_reads_object() {
    let r = report_ops(
        "class C { const C(int x); }\n\
         void main() { C c = const C(0); if (c case == (.missing)) {} }\n",
    );
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'Object'."),
        "a parenthesized pattern operand reads the `==` parameter face: {row}"
    );
    assert!(
        !rows(&r)
            .iter()
            .any(|row| row.starts_with("dot_shorthand_missing_context ")),
        "the nested-operand arm must not fall back to `missing_context`: {:?}",
        rows(&r)
    );
}

// —— DS-2: both branches of a conditional inside a pattern operand carry
// —— the same `Object` face (`equality_error_test`'s `case == (cond ?
// —— .red : .green)` shape).
#[test]
fn relational_pattern_conditional_branches_read_object() {
    let r = report_ops(
        "class C { const C(int x); }\n\
         bool k = true;\n\
         void main() { C c = const C(0); if (c case == (k ? .m1 : .m2)) {} }\n",
    );
    let found = shorthand(&r);
    assert_eq!(
        found.len(),
        2,
        "both branches report one row each: {:?}",
        rows(&r)
    );
    for row in &found {
        assert!(
            row.ends_with("isn't defined for the context type 'Object'."),
            "each conditional branch reads `Object`: {row}"
        );
    }
}

// —— DS-2's other half: a DIRECT operand (`case == .a`) keeps the matched
// —— value's static type. The pattern guard must not flatten both shapes
// —— into one face.
#[test]
fn relational_pattern_direct_operand_reads_the_matched_type() {
    let r = report_ops(
        "enum E { a }\n\
         void main() { E e = E.a; if (e case == .missing) {} }\n",
    );
    let row = only_shorthand(&r);
    assert!(
        row.ends_with("isn't defined for the context type 'E'."),
        "the direct operand reads the scrutinee's matched type: {row}"
    );
}
