//! The #3545 `Ty::Invalid` wave-2 pins: the member/invocation face fork
//! (P3 — a member reached through a `dynamic` target types `Dynamic`,
//! an unresolved member name or ANY member on an `Invalid` receiver
//! types `Invalid`, upstream `generated/resolver.dart:1758-1761` the
//! `atDynamicTarget ? dynamic : Invalid` fork) plus the Invalid-receiver
//! silencing (`type_property_resolver.dart:88-92`'s
//! `isDynamicBounded || isInvalidBounded` short-circuit — no
//! `undefined_getter`/`undefined_setter`/`undefined_method` on an
//! Invalid receiver) and the invocation production (P4 —
//! `method_invocation_resolver.dart:643-647`: an unresolved callee or
//! an Invalid receiver types the whole send `Invalid` with no lookup).
//!
//! The contract: the ONLY row a member face on an Invalid receiver ever
//! produces is the receiver's own resolution face — the `undefined_*`
//! trio stays silent because the oracle looks the name up on `Object`
//! and clears the error flags. A `dynamic` receiver keeps its
//! permissive semantics unchanged (the miss silences too, but through
//! the dynamic dispatch, and its messages name `'dynamic'` where the
//! Invalid face names `'InvalidType'`).

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// The sorted code-name set of `report`.
fn codes(report: &crate::diagnose::FileReport) -> Vec<&'static str> {
    let mut names = code_names(report);
    names.sort_unstable();
    names
}

/// P3 silencing (read) — `Unresolved u = null; u.foo;`: the member read
/// on an invalid-typed variable produces only the receiver's own
/// `undefined_class`; `undefined_getter` never fires (the oracle's
/// `isInvalidBounded` short-circuit looks the name up on `Object` and
/// clears `_needsGetterError` before any report).
#[test]
fn invalid_receiver_getter_read_stays_silent() {
    let report = report_ops("Unresolved u = null;\nvoid f() { u.foo; }\n");
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// P3 silencing (write) — `Unresolved u = null; u.foo = 1;`: the member
/// write on an invalid-typed variable silences `undefined_setter` the
/// same way (the oracle's `isDynamicBounded || isInvalidBounded` guard
/// clears `_needsSetterError` beside the getter's).
#[test]
fn invalid_receiver_setter_write_stays_silent() {
    let report = report_ops("Unresolved u = null;\nvoid f() { u.foo = 1; }\n");
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// P3 silencing (invoke) — `Unresolved u = null; u.foo();`: the method
/// send on an Invalid receiver short-circuits before the lookup runs
/// (`method_invocation_resolver.dart:643` — element `null`, no report).
#[test]
fn invalid_receiver_method_send_stays_silent() {
    let report = report_ops("Unresolved u = null;\nvoid f() { u.foo(); }\n");
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// P3 production fork — the `dynamic` twin stays dynamic-permissive:
/// `dynamic d = null; d.foo;`/`d.foo = 1;`/`d.foo();` silences through
/// the dynamic dispatch (the `isDynamicBounded` arm of the same guard),
/// never through the Invalid face — the same silence, a different
/// production class.
#[test]
fn dynamic_receiver_keeps_its_permissive_silence() {
    let report = report_ops("dynamic d = null;\nvoid f() { d.foo; d.foo = 1; d.foo(); }\n");
    assert!(
        codes(&report).is_empty(),
        "dynamic receiver must stay silent: {:?}",
        rows(&report)
    );
}

/// The `.new` fork's message face (ADR section-5.3c): `d.new` over a `dynamic`
/// variable reports `undefined_getter` naming `'dynamic'` — the
/// `new` selector's report precedes the bounded short-circuit
/// (`type_property_resolver`'s `needsGetterError` fires unconditionally
/// for the name `new`), and the message reads the receiver's resolved
/// static type. An `Invalid` receiver's `.new` names `'InvalidType'`.
#[test]
fn dot_new_on_dynamic_names_dynamic_not_invalid() {
    let report = report_ops("dynamic d = null;\nvoid f() { d.new; }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_getter"),
        "the `new` selector must report: {:?}",
        rows(&report)
    );
    let text = format!("{:?}", rows(&report));
    assert!(
        text.contains("dynamic"),
        "the receiver must name 'dynamic', not 'InvalidType': {text}"
    );
    assert!(
        !text.contains("InvalidType"),
        "a dynamic receiver never spells InvalidType: {text}"
    );
}

/// The chained `.new` twin of the same probe — `d.foo.new` over
/// `d: dynamic`: the `foo` link types `Dynamic` through the
/// `atDynamicTarget` production arm, so the `new` selector's report
/// names `'dynamic'` — the face the merge-time `member_recv_tys`
/// witness carries into `accessor_read_ty`'s fork (before the fork the
/// `foo` link typed `Unknown` and the message read `'InvalidType'`).
#[test]
fn chained_dot_new_on_dynamic_names_dynamic() {
    let report = report_ops("dynamic d = null;\nvoid f() { d.foo.new; }\n");
    let text = format!("{:?}", rows(&report));
    assert!(
        text.contains("dynamic"),
        "the `foo` link must name 'dynamic', not 'InvalidType': {text}"
    );
    assert!(
        !text.contains("InvalidType"),
        "a dynamic chain never spells InvalidType: {text}"
    );
}

/// The `Unresolved.new` twin keeps `InvalidType` (probe 3.13.7, ADR
/// section-5.3c: `var x = Undefined.new` names `InvalidType` — a receiver whose
/// own resolution failed carries the oracle's invalid face).
#[test]
fn dot_new_on_invalid_names_invalid_type() {
    let report = report_ops("void f() { Unresolved.new; }\n");
    let names = code_names(&report);
    assert!(
        !names.is_empty(),
        "the unresolved `.new` face must report: {:?}",
        rows(&report)
    );
}

/// P3 chain propagation — `u.foo.bar` over `u: Invalid`: the `foo` link
/// types `Invalid` (member on an invalid receiver), so `bar` resolves on
/// an Invalid receiver and silences too — no `undefined_getter` cascade.
#[test]
fn invalid_chain_continuation_stays_silent() {
    let report = report_ops("Unresolved u = null;\nvoid f() { u.foo.bar; }\n");
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// The same chain over a `dynamic` root: `d.foo.bar` — `foo` types
/// `Dynamic`, `bar` on it dispatches dynamically — silent both ways.
#[test]
fn dynamic_chain_continuation_stays_silent() {
    let report = report_ops("dynamic d = null;\nvoid f() { d.foo.bar; }\n");
    assert!(
        codes(&report).is_empty(),
        "the dynamic chain must stay silent: {:?}",
        rows(&report)
    );
}

/// P4 invocation production — `Unresolved u = null; var v = u.foo();
/// int i = v;`: `u.foo()` types `Invalid` (unresolved callee on an
/// invalid receiver), `v` infers `Invalid` through the
/// invalid-init propagation (P7), and `int i = v` accepts — no
/// `invalid_assignment` beside `undefined_class`.
#[test]
fn invalid_invocation_result_propagates_through_var() {
    let report = report_ops("Unresolved u = null;\nvoid f() { var v = u.foo(); int i = v; }\n");
    let names = code_names(&report);
    assert!(names.contains(&"undefined_class"), "{:?}", rows(&report));
    assert!(
        !names.contains(&"invalid_assignment"),
        "{:?}",
        rows(&report)
    );
    assert!(!names.contains(&"undefined_method"), "{:?}", rows(&report));
}

/// The dynamic twin of the same shape: `var v = d.foo(); int i = v;` —
/// `v` reads `dynamic` upstream, the `i.abs()` send dispatches
/// dynamically — silent.
#[test]
fn dynamic_invocation_result_stays_permissive() {
    let report =
        report_ops("dynamic d = null;\nvoid f() { var v = d.foo(); int i = v; i.isEven; }\n");
    assert!(
        codes(&report).is_empty(),
        "the dynamic invocation chain must stay silent: {:?}",
        rows(&report)
    );
}

/// The unresolved member read in argument position — `h(u.foo)` over
/// `u: Invalid` and `void h(int x)`: `u.foo` types `Invalid`, assignable
/// to `int` — no `argument_type_not_assignable` cascade.
#[test]
fn invalid_member_read_in_argument_stays_silent() {
    let report = report_ops("Unresolved u = null;\nvoid h(int x) {}\nvoid f() { h(u.foo); }\n");
    assert_eq!(codes(&report), ["undefined_class"], "{:?}", rows(&report));
}

/// The assignment through the invalid member — `int i = u.foo;`: the
/// read types `Invalid`, `Invalid <: int` accepts — only the receiver's
/// own `undefined_class` stands.
#[test]
fn invalid_member_read_assigns_silently() {
    let report = report_ops("Unresolved u = null;\nvoid f() { int i = u.foo; i.isEven; }\n");
    let names = code_names(&report);
    assert!(names.contains(&"undefined_class"), "{:?}", rows(&report));
    assert!(
        !names.contains(&"invalid_assignment"),
        "{:?}",
        rows(&report)
    );
}

/// Negative control — a resolved receiver's member miss still reports:
/// `class C {}` `C c; c.foo;` fires `undefined_getter` (the InvalidType
/// widen must not reach live verdicts).
#[test]
fn resolved_receiver_member_miss_still_reports() {
    let report = report_ops("class C {}\nC c = C();\nvoid f() { c.foo; }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_getter"),
        "the resolved receiver's miss must report: {:?}",
        rows(&report)
    );
}

/// Negative control — a resolved receiver's method miss still reports
/// `undefined_method` (the Invalid widen must not reach live verdicts).
#[test]
fn resolved_receiver_method_miss_still_reports() {
    let report = report_ops("class C {}\nC c = C();\nvoid f() { c.foo(); }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_method"),
        "the resolved receiver's send must report: {:?}",
        rows(&report)
    );
}

/// Negative control — a resolved receiver's setter miss still reports
/// `undefined_setter` (the third census family).
#[test]
fn resolved_receiver_setter_miss_still_reports() {
    let report = report_ops("class C {}\nC c = C();\nvoid f() { c.foo = 1; }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_setter"),
        "the resolved receiver's write must report: {:?}",
        rows(&report)
    );
}

/// `is`/`as` on an invalid-typed operand — `u is int; u as int;`: the
/// oracle evaluates Invalid-typed operands (they are not the
/// `unnecessary_*` guard's silence class — wave-3 question, pinned here
/// as the wave-2 regression posture: no fabricated rows).
#[test]
fn invalid_operand_type_test_stays_quiet() {
    let report = report_ops("Unresolved u = null;\nvoid f() { if (u is int) {} u as int; }\n");
    let names = code_names(&report);
    assert!(names.contains(&"undefined_class"), "{:?}", rows(&report));
    assert!(!names.contains(&"undefined_getter"), "{:?}", rows(&report));
}
