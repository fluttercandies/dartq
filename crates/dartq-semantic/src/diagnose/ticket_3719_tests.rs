//! Ticket #3719 pins: a generic container's `==` formal renders through the
//! receiver's instantiated type.
//!
//! # The wrong fact
//!
//! `check_equals_argument` looked the `==` formal up through
//! `member_formals_full` with `Receiver::Typed { ty: Ty::UNKNOWN }` — the
//! receiver-instantiation environment ([`Engine::member_recv_env`]) saw no
//! named type and answered `None`, so the declared table came back raw. A
//! `class C<T> { bool operator ==(C<T> o) }` receiver spelled `C<int>(1)`
//! therefore rendered the formal `C<T>?` where the oracle reads `C<int>?`:
//! upstream `_resolveEqual` resolves the operator on the left operand's
//! `leftType` (the instantiated `C<int>` — `typePropertyResolver.resolve`
//! in `binary_expression_resolver.dart:451-459`), and the resolved element's
//! signature carries the substitution into the formal the argument check
//! reports.
//!
//! # The contract
//!
//! - `C<int>(…) == x` — the `==` formal instantiates through the receiver's
//!   written arguments: the message spells `C<int>?`, never `C<T>?`.
//! - `C<int> x; x == …` — the declared-typed receiver instantiates the same
//!   way (the `Receiver::Typed.ty` env composes identically for a written
//!   declaration type).
//! - `C(…) == x` (raw creation) — no written arguments, no environment: the
//!   formal keeps its declared `C<T>?` spelling and the negative control is
//!   unchanged.
//! - A non-generic container `==` is untouched — the same path with an
//!   empty environment.
//!
//! Oracle provenance: `dart analyze` — the resolved member's formal type is
//! the instantiated signature face (the element model substitutes the
//! receiver's type arguments at lookup; `pkg/analyzer`'s
//! `InterfaceType.asInstanceOf` composes it for inherited hits).

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};
use super::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `argument_type_not_assignable` rows as `(offset, length, message)`,
/// ascending.
fn atna_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut at: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| {
            (
                d.range.offset,
                d.range.end() - d.range.offset,
                d.message.to_string(),
            )
        })
        .collect();
    at.sort_unstable();
    at
}

// —— the ticket's defect verbatim: a `C<int>` receiver's `==` formal renders
// —— `C<int>?`, the instantiation the oracle's resolved element carries.
#[test]
fn generic_container_equals_formal_instantiates() {
    let source = "class C<T> { C(T v); bool operator ==(C<T> o) => true; }\n\
                  void f() { if (C<int>(1) == \"s\") {} }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'C<int>?'. "
                .to_string(),
        )],
        "the `C<int>` receiver substitutes the `==` formal: {:?}",
        rows(&r)
    );
}

// —— a fitting right operand stays silent: `C<int>(2)` is a `C<int>` (the
// —— right operand's own `Created` row answers the instantiated type, and
// —— the substituted `C<int>?` formal accepts it).
#[test]
fn generic_container_equals_fitting_argument_silent() {
    let source = "class C<T> { C(T v); bool operator ==(C<T> o) => true; }\n\
                  void f() { if (C<int>(1) == C<int>(2)) {} }\n";
    let r = report_ops(source);
    assert!(
        atna_rows(&r).is_empty(),
        "`C<int> == C<int>` fits the substituted formal: {:?}",
        rows(&r)
    );
}

// —— the declared-typed receiver spells the same substitution: `C<int> x`
// —— arms the receiver environment through `Receiver::Typed.ty` exactly as
// —— a written declaration type does for an ordinary member call.
#[test]
fn declared_generic_receiver_equals_formal_instantiates() {
    let source = "class C<T> { C(T v); bool operator ==(C<T> o) => true; }\n\
                  void f(C<int> x) { if (x == \"s\") {} }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'C<int>?'. "
                .to_string(),
        )],
        "the `C<int>` declaration substitutes the `==` formal: {:?}",
        rows(&r)
    );
}

// —— negative control (stability pin, not an oracle claim): a raw `C()`
// —— creation writes no type arguments — `equality_operand_ty` keeps the
// —— bare head (the instantiate-to-bounds residual is the display face's,
// —— not the operand's), the receiver environment stays empty, and the
// —— formal keeps its declared `C<T>?` spelling — the pre-#3719 face a
// —— bare head always rendered.
#[test]
fn raw_creation_equals_formal_keeps_declared_spelling() {
    let source = "class C<T> { bool operator ==(C<T> o) => true; }\n\
                  void f() { if (C() == \"s\") {} }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'C<T>?'. "
                .to_string(),
        )],
        "a bare head writes no arguments: the formal stays declared: {:?}",
        rows(&r)
    );
}

// —— negative control: a non-generic container's `==` is unchanged — the
// —— same lookup composes an empty environment and the declared formal
// —— stands (`'D?'`, never substituted-away).
#[test]
fn non_generic_container_equals_formal_unchanged() {
    let source = "class D { bool operator ==(D o) => true; }\n\
                  void f() { if (D() == \"s\") {} }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'D?'. "
                .to_string(),
        )],
        "the non-generic `==` formal stands: {:?}",
        rows(&r)
    );
}
