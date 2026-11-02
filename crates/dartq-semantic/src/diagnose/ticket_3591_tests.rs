//! The #3591 pins — the leading-dot shorthand sweeps keep silent on
//! positions the oracle 3.13.7 reports as undefined members.
//!
//! # The observed divergence
//!
//! Row-level census `dot_shorthand_undefined_member`: 478 missing rows
//! (the oracle reports, dartq stays silent) against 239 extra rows
//! (dartq fabricates a row the oracle never emits). The missing side is
//! dominated by positions the current sweep classifies as "no decidable
//! context" and therefore suppresses, while the oracle visits the
//! shorthand under a REAL context:
//!
//! - the language-defined `bool` context of a condition (`if (.x)`,
//!   `while (.x)`, the operands of `||`/`&&`/`!`) — `grammar_A05`,
//!   `bool_context_error_test`;
//! - the declared type of a body-local variable initializer
//!   (`bool b = .x`, `Object o = .x`) — the declaration pass only ran
//!   for the group's own shorthand window;
//! - the static type of an `==`/`!=` left operand when the left side is
//!   a real expression (`Object o; o == .x`) — `equality_error_test`.
//!
//! The extra side is the mirror defect of the same context rule: a
//! shorthand whose context DOES resolve (`const E x = .v1` over an enum,
//! `.new()` over a class with an unnamed constructor) is reported as an
//! undefined member because the member walk never consulted the context's
//! static namespace for that spelling.
//!
//! # The contract
//!
//! - A miss reports `dot_shorthand_undefined_member` at the selector span
//!   (`memberName` upstream), read and invocation arms alike.
//! - A resolvable shorthand — an enum constant, a static member, a
//!   constructor spelling, a language-defined member — stays silent.
//! - The row's context slot is the resolved context's display (`bool`,
//!   `Object`, `E`); the weaker `'_'`/`dynamic` faces keep their
//!   documented suppression.
//!
//! Oracle provenance: `dart analyze` 3.13.7 with `--enable-experiment=
//! dot-shorthands` — `if (.one)` / `while (.two)` report `'bool'`,
//! `Object o = .four` reports `'Object'`, `enum E { v1, v2 } ... .v1`
//! reports nothing. Conformance cases: `bool_context_error_test`,
//! `equality_error_test`, `grammar_A05`, `non_ambiguity_A02_*`.

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
/// pairs, ascending — the pins assert the selector span.
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

/// Whether `report` carries a `dot_shorthand_undefined_member` row — the
/// silent pins assert on this code alone (the fixtures' orthogonal
/// `unused_local_variable` noise stays out of the contract).
fn has_shorthand_row(report: &FileReport) -> bool {
    code_names(report).contains(&"dot_shorthand_undefined_member")
}

/// The first `dot_shorthand_undefined_member` row's message — the
/// context-slot pins assert the resolved context display.
fn shorthand_message(report: &FileReport) -> Option<&str> {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| d.message.as_str())
}

// —— control: an enum constant used as a shorthand resolves against the
// —— enum's static namespace — no row (oracle `test_dotShorthand_enum_simple`
// —— reports nothing).
#[test]
fn enum_constant_shorthand_stays_silent() {
    let r = report_ops("enum E { v1, v2 }\nconst E x1 = .v1;\n");
    assert!(
        !has_shorthand_row(&r),
        "an enum constant shorthand resolves silently: {:?}",
        rows(&r)
    );
}

// —— control: a static field hit resolves against the context type.
#[test]
fn static_field_shorthand_stays_silent() {
    let r = report_ops("class C { static int a = 1; }\nvoid f() { C c = .a; }\n");
    assert!(
        !has_shorthand_row(&r),
        "a static field shorthand resolves silently: {:?}",
        rows(&r)
    );
}

// —— control: the implicit unnamed constructor spelling resolves.
#[test]
fn unnamed_constructor_shorthand_stays_silent() {
    let r = report_ops("class C {}\nvoid f() { C c = .new(); }\n");
    assert!(
        !has_shorthand_row(&r),
        "an unnamed-constructor shorthand resolves silently: {:?}",
        rows(&r)
    );
}

// —— pin: `if (.one)` arms the language-defined `bool` context and the
// —— miss reports at the selector (oracle: `'one' ... context type 'bool'`).
#[test]
fn if_condition_arms_bool_context() {
    let source = "void f() { if (.one) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3)],
        "one miss row at the `one` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'bool'")),
        "the condition context displays as `bool`: {:?}",
        rows(&r)
    );
}

// —— pin: `while (.one)` arms the same `bool` context.
#[test]
fn while_condition_arms_bool_context() {
    let source = "void f() { while (.one) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3)],
        "one miss row at the `one` selector: {:?}",
        rows(&r)
    );
}

// —— pin: the operands of `||`/`&&` arm the `bool` context.
#[test]
fn logical_operand_arms_bool_context() {
    let source = "void f() { if (.one || .two) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3), (at(source, "two"), 3)],
        "one miss row at each `||` operand: {:?}",
        rows(&r)
    );
}

// —— pin: a bare `return .x` arms the executable's declared return face.
#[test]
fn return_arms_declared_return_type() {
    let source = "bool f() { return .one; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3)],
        "one miss row at the `one` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'bool'")),
        "the return context displays as `bool`: {:?}",
        rows(&r)
    );
}

// —— pin: a body-local variable initializer arms its written annotation.
#[test]
fn local_bool_initializer_reports() {
    let source = "void f() { bool b = .one; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3)],
        "one miss row at the `one` selector: {:?}",
        rows(&r)
    );
}

// —— pin: an `Object`-typed local initializer reports the `Object` context.
#[test]
fn local_object_initializer_reports() {
    let source = "void f() { Object o = .four; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "four"), 4)],
        "one miss row at the `four` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'Object'")),
        "the initializer context displays as `Object`: {:?}",
        rows(&r)
    );
}

// —— pin: the `==` right operand reads the left operand's static type
// —— (`Object o; bool b = o == .a`).
#[test]
fn equality_rhs_reads_left_operand_type() {
    let source = "bool f(Object o) { return o == .a; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "a;"), 1)],
        "one miss row at the `a` selector: {:?}",
        rows(&r)
    );
}

// —— pin: a list spread's element context is `Iterable`.
#[test]
fn list_spread_arms_iterable_context() {
    let source = "void f() { var l = <int>[... .one]; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "one"), 3)],
        "one miss row at the `one` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'Iterable'")),
        "a list spread context displays as `Iterable`: {:?}",
        rows(&r)
    );
}

// —— pin: a set spread's element context is also `Iterable`.
#[test]
fn set_spread_arms_iterable_context() {
    let source = "void f() { var s = <int>{... .two}; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "two"), 3)],
        "one miss row at the `two` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'Iterable'")),
        "a set spread context displays as `Iterable`: {:?}",
        rows(&r)
    );
}

// —— pin: a map spread's context is `Map` (two type arguments).
#[test]
fn map_spread_arms_map_context() {
    let source = "void f() { var m = <int, int>{1: 2, ... .three}; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "three"), 5)],
        "one miss row at the `three` selector: {:?}",
        rows(&r)
    );
    assert!(
        shorthand_message(&r).is_some_and(|m| m.contains("'Map'")),
        "a map spread context displays as `Map`: {:?}",
        rows(&r)
    );
}

// —— pin: a null-aware spread (`...?`) arms the same `Iterable` context.
#[test]
fn null_aware_spread_arms_iterable_context() {
    let source = "void f() { var l = <int>[...? .four]; }\n";
    let r = report_ops(source);
    assert_eq!(
        shorthand_rows(&r),
        vec![(at(source, "four"), 4)],
        "one miss row at the `four` selector: {:?}",
        rows(&r)
    );
}
