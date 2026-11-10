//! The #3601 pins — the equality-operand context rule for dot shorthands.
//!
//! Row-level census `dot_shorthand_undefined_member` (freshest report:
//! 430 missing / 329 extra rows). The message-only pairs — the invocation
//! template (`codes.rs`) — account for 142 of them; the positional residue
//! splits into the equality family (124 missing / 41 extra), the co19
//! static-access family (134 / 85), and small tails.
//!
//! The biggest single mechanism is `binary_expression_resolver.dart`'s
//! `_resolveEqual`: both operands are analyzed with the unknown schema, so
//! only a DIRECT dot shorthand on the right reads the left operand's static
//! type. Every shorthand nested in either operand's subtree verdicts
//! against `'_'`. dartq instead inherited the enclosing context — the
//! language-defined `bool` of an `if`/`assert` condition or a declaration
//! annotation — fabricating `... context type 'bool'` rows on left-rooted
//! positions the oracle keeps silent (`assert(.blue == color)`,
//! `equality_error_test`'s `if (.blue == color)` blocks), and stayed silent
//! where the oracle reports the empty schema
//! (`equality_selector_chain_error_test`'s left-rooted chains,
//! `equality_ctor_error_test`'s constructor spellings).
//!
//! Two neighbours stay out of this cluster: the invocation message template
//! (`codes.rs` renders the getter wording where upstream's invocation
//! spelling is `method or constructor`), and the direct-right-operand
//! left type when the left operand is a creation under a call's formal
//! (`.staticGetter` reported against `bool` in co19's
//! `Expect.isTrue(ET(1) == .staticGetter)` — the left operand's own static
//! type is misread, not the operand rule).
//!
//! Contract:
//!
//! - A shorthand that roots the LEFT operand of `==`/`!=` arms the unknown
//!   schema: the read arm stays silent (the `missing_context` sibling is
//!   unimplemented) and the invocation arm reports `'_'`.
//! - A shorthand nested in a parenthesized/conditional RIGHT operand arms
//!   the same unknown schema — upstream pushes no context onto it.
//! - A direct right-operand shorthand still reads the left operand's
//!   static type (`Object o; o == .a` keeps reporting `'Object'`).
//!
//! Oracle provenance: `dart analyze` 3.13.7 (the pinned SDK) on the
//! extracted `dot_shorthands/equality` corpus — `equality_error_test`
//! reports `'_'` for `.member() == member`, `Object` for the `(x as Object)
//! == .y` left casts, and `missing_context` for `assert(.blue == color)`.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: [`report_ops`] prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` rows as `(offset, length)` pairs,
/// ascending — the pins assert the selector span.
fn shorthand_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// The context slot every `dot_shorthand_undefined_member` row carries, in
/// report order — the firing pins assert the armed schema text.
fn shorthand_contexts(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| d.message.as_str().to_string())
        .collect()
}

// —— firing pin: a constructor shorthand that roots the LEFT operand of
// —— `==` arms the unknown schema, so the invocation arm reports `'_'`
// —— (before: the enclosing `if (` condition's `'bool'` leaked in).
#[test]
fn equality_lhs_root_invocation_reports_unknown_context() {
    let source = "class C { const C.regular(int x); }\n\
                  void f(C c) { if (.regular(1) == c) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "regular(1)"), 7)],
        "one miss row at the `regular` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_contexts(&r).iter().all(|m| m.contains("'_'")),
        "the left operand arms the unknown schema: {:?}",
        rows(&r)
    );
}

// —— firing pin: a declaration-initializer shorthand that roots the LEFT
// —— operand reports too (before: the declaration annotation `bool` was
// —— inherited or the position was silent).
#[test]
fn equality_lhs_root_in_declaration_reports_unknown_context() {
    let source = "class C { const C.regular(int x); }\n\
                  void f(C c) { bool b = .regular(1) == c; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "regular(1)"), 7)],
        "one miss row at the `regular` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_contexts(&r).iter().all(|m| m.contains("'_'")),
        "the left operand arms the unknown schema: {:?}",
        rows(&r)
    );
}

// —— firing pin: a shorthand nested in a conditional RIGHT operand arms the
// —— unknown schema (upstream pushes no context onto a non-direct operand).
#[test]
fn equality_rhs_conditional_nested_reports_unknown_context() {
    let source = "class C { const C.regular(int x); const C.other(int x); }\n\
                  void f(C c, bool b) { if (c == (b ? .regular(1) : .other(2))) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "regular(1)"), 7), (at(source, "other(2)"), 5)],
        "one miss row per conditional branch: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_contexts(&r).iter().all(|m| m.contains("'_'")),
        "the nested operand arms the unknown schema: {:?}",
        rows(&r)
    );
}

// —— silent control: a READ shorthand rooting the LEFT operand of `==` is
// —— the unknown schema's read arm — the oracle's `missing_context`
// —— sibling is unimplemented, so the row stays suppressed instead of
// —— fabricating the enclosing condition's `bool`.
#[test]
fn equality_lhs_root_read_stays_silent() {
    let source = "enum E { a }\n\
                  void f(E e) { assert(.a == e); }\n";
    let r = report_ops(source);
    assert!(
        shorthand_rows(&r).is_empty(),
        "the unknown schema's read arm stays silent: {:?}",
        rows(&r)
    );
}

// —— silent control: the same suppression for an `if` condition, whose
// —— language-defined `bool` context applies to the condition expression,
// —— never to an equality operand nested inside it.
#[test]
fn equality_lhs_root_read_in_condition_stays_silent() {
    let source = "enum E { a }\n\
                  void f(E e) { if (.a == e) {} }\n";
    let r = report_ops(source);
    assert!(
        shorthand_rows(&r).is_empty(),
        "the nested equality operand does not inherit `bool`: {:?}",
        rows(&r)
    );
}

// —— silent control: a parenthesized RIGHT operand holding a read shorthand
// —— is the same unknown-schema arm (the oracle reports `missing_context`).
#[test]
fn equality_rhs_parenthesized_read_stays_silent() {
    let source = "enum E { a }\n\
                  void f(E e) { bool b = e == (.a); }\n";
    let r = report_ops(source);
    assert!(
        shorthand_rows(&r).is_empty(),
        "a parenthesized right operand arms no context: {:?}",
        rows(&r)
    );
}

// —— silent control (the guard the whole rule must not cross): a DIRECT
// —— right-operand shorthand keeps reading the left operand's static type.
#[test]
fn equality_direct_rhs_still_reads_operand_context() {
    let source = "void f(Object o) { bool b = o == .a; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "a;"), 1)],
        "one miss row at the `a` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_contexts(&r)
            .iter()
            .all(|m| m.contains("'Object'")),
        "the direct operand keeps the left operand's context: {:?}",
        rows(&r)
    );
}
