//! The #3568 pins — `dot_shorthand_undefined_member` at the operand and
//! argument positions the declaration-initializer window reaches
//! (`resolve_decl_shorthands`'s per-position context,
//! `shorthand_ctx_at`).
//!
//! # The wrong fact
//!
//! #3433 armed every shorthand in the initializer window with the
//! declaration group's annotation — so `E e = (m == .x)` resolved `.x`
//! against `E`, and `E e = f(.x)` resolved `.x` against `E` too. The
//! position's own context — the `==` left operand's type, the argument's
//! parameter type — never armed.
//!
//! # The contract
//!
//! - `x == .a` / `x != .a`: the right operand resolves against the LEFT
//!   operand's static type (upstream `binary_expression_resolver.dart`'s
//!   `pushDotShorthandContext` on the right operand).
//! - `f(.a)`: the argument resolves against the paired parameter's
//!   declared type (`resolveArgumentsToParameters`'s formal pairing).
//! - A position with no decidable context keeps the #3433 two-arm rule:
//!   the invocation arm reports `'_'`, the read arm stays silent.
//!
//! Oracle provenance: `dart analyze` 3.13.7 truth table —
//! `dotShorthandUndefinedGetter`/`dotShorthandUndefinedInvocation` at the
//! selector span, context display in slot two.

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` rows as `(offset, length)`
/// pairs, ascending — the anchors below assert the selector span.
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

/// Whether `report` carries a `dot_shorthand_undefined_member` row.
fn has_shorthand_row(report: &FileReport) -> bool {
    code_names(report).contains(&"dot_shorthand_undefined_member")
}

// —— `==` operand, known context + member hit: `m == .a` where `m: E`
// —— and `E` declares `a` — the left operand's `E` arms the shorthand;
// —— the hit upgrades it silently (upstream's `e == .a` static-member
// —— resolution).
#[test]
fn eq_operand_member_hit_stays_silent() {
    let r = report_ops("enum E { a, b }\nvoid f() { E m = E.a; bool b = m == .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "a `==` operand hit resolves silently against the left operand: {:?}",
        rows(&r)
    );
}

// —— `==` operand, known context + miss: `m == .z` where `m: E` carries
// —— no `z` — the row lands at the `z` selector with `E` as the context
// —— display.
#[test]
fn eq_operand_member_miss_reports_at_selector() {
    let source = "enum E { a, b }\nvoid f() { E m = E.a; bool b = m == .z; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "z;"), 1)],
        "one miss row at the `z` selector: {:?}",
        rows(&r)
    );
}

// —— argument position, parameter-type context + miss: `take(.z)` where
// —— `take(E)` and `E` carries no `z` — the parameter's `E` arms the
// —— shorthand; the row lands at the `z` selector.
#[test]
fn argument_position_miss_reports_at_selector() {
    let source = "enum E { a, b }\nvoid take(E e) {}\nvoid f() { bool b = true && (take(.z) == null); }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "z)"), 1)],
        "the argument shorthand reports its parameter context: {:?}",
        rows(&r)
    );
}

// —— `==` operand, no decidable context + invocation arm: `v == .m()`
// —— where `v` carries no usable type (`var v` infers the shorthand
// —— itself, so nothing arms a context) — the invocation arm reports
// —— `'_'` at the selector.
#[test]
fn eq_operand_no_context_invocation_reports() {
    let source = "void f() { var v = null; bool b = v == .m(); }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "m("), 1)],
        "the context-free invocation reports `'_'`: {:?}",
        rows(&r)
    );
}

// —— silent twin — `==` operand with a real static member:
// —— `m == .blue` where `m: Color` and `Color` declares `blue` — the
// —— enum's static member resolves through the left operand's static
// —— type and the read arm stays silent.
#[test]
fn eq_operand_genuine_member_stays_silent() {
    let r = report_ops(
        "enum Color { red, blue }\nvoid f() { Color m = Color.red; bool b = m == .blue; }\n",
    );
    assert!(
        !has_shorthand_row(&r),
        "`Color.blue` resolves silently through the left operand: {:?}",
        rows(&r)
    );
}

// —— bare statement (no initializer): `if (m == .blue) {}` inside the
// —— executable's own body — the body-close sweep reaches the `==`
// —— operand, `Color.blue` resolves, no row.
#[test]
fn bare_statement_eq_operand_hit_stays_silent() {
    let r = report_ops(
        "enum Color { red, blue }\nvoid f() { Color m = Color.red; if (m == .blue) {} }\n",
    );
    assert!(
        !has_shorthand_row(&r),
        "a bare-statement `==` hit resolves silently: {:?}",
        rows(&r)
    );
}

// —— bare statement (no initializer): `if (m == .z) {}` — `.z` is no
// —— member of `Color`; the sweep verdicts it against the left operand's
// —— `Color` and the row lands at the `z` selector.
#[test]
fn bare_statement_eq_operand_miss_reports() {
    let source =
        "enum Color { red, blue }\nvoid f() { Color m = Color.red; if (m == .z) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "z)"), 1)],
        "a bare-statement `==` miss reports at the selector: {:?}",
        rows(&r)
    );
}

// —— corpus-shaped over-report guards (#3568 census repair): every row the
// —— sweep must NOT fabricate — an assignment target's `x = .m()`, a type-
// —— parameter formal (`C<T>`'s `T`), and a left-of-`==` shorthand —
// —— upstream reads `'_'`/`missing_context` or a context this walk cannot
// —— re-derive; all keep the under-report.
#[test]
fn over_report_guards_stay_silent() {
    for source in [
        // `x = .m()` — an assignment, never the declaration's window.
        "void f() { int x = 0; x = .m(); }\n",
        // `C(this.x)`'s formal `x: T` — `T` is `C`'s type parameter; the
        // substitution `T = C` lives in inference, not the written formal.
        "class C<T> { T x; C(this.x); }\nvoid f() { C<C> c = .new(.member()); }\n",
        // `.new() == C()` — the left shorthand's own context is `'_'`; the
        // right operand reads `'_'` too (upstream does not push the left
        // operand's type for a shorthand left).
        "class C { C(); }\nvoid f() { bool b = .new() == C(); }\n",
    ] {
        let r = report_ops(source);
        assert!(
            !has_shorthand_row(&r),
            "no fabricated `dot_shorthand_undefined_member`: {source:?} -> {:?}",
            rows(&r)
        );
    }
}


