//! #3237 (T3 of the pattern cluster) — the `receiver_of_type_never`
//! receiver-read widening.
//!
//! Upstream emits `receiverOfTypeNever` at exactly five resolver entry
//! points, all gated on the receiver's flow-aware static type
//! (`typeOrThrow`):
//!
//! * `method_invocation_resolver.dart:195` — `x.m()` (and cascades),
//! * `function_expression_invocation_resolver.dart:81` — `x()`,
//! * `property_element_resolver.dart:189` — `x[i]` read and write,
//! * `binary_expression_resolver.dart:454` — `x + y`, `x == y`, every
//!   user-definable operator,
//! * `prefix_expression_resolver.dart:183` / `postfix_expression_resolver
//!   .dart:134` — `++x`, `x++`, `-x`, `~x`.
//!
//! Faces that carry no `identical(type, NeverTypeImpl.instance)` arm stay
//! silent: property reads/writes (`x.foo`, `x.foo = v`), the `!` operand
//! (`_resolveNegation`), and the `&&`/`||` left operand
//! (`_resolveLogicalAnd`/`_resolveLogicalOr`).
//!
//! The receiver's `Never` reaches the resolver through three spellings, and
//! the oracle's verdicts were probed (`.third/dart-sdk-3.13.7` `dart
//! analyze`):
//!
//! * **Declared `Never`** — `void f(Never x) { x.foo(); }` fires. The
//!   written `Ty::Named{"Never"}` is the parse-time arm.
//! * **Inferred `Never`** — `var x = throw 0; x.foo()` fires (the `var`
//!   adopts `throw`'s `Never`); a `Never`-returning call receiver
//!   (`ret().foo()`, `ret()()`) fires the same way.
//! * **A `throw` receiver** — `(throw '').toString()` fires at the `throw`
//!   token (the row's `.ty` rewrite answers `Ty::Never`).
//!
//! The flow layer carries NO promoted-`Never` local read the oracle
//! reports: `Null x` under `x != null` classifies `NullOrEquivalent` on
//! both sides, so `x != null` is known-false and the body is simply dead —
//! `x` reads `Null` (probed: `invalid_use_of_null_value`, no
//! `receiver_of_type_never`), and `x is Never` reports
//! `unchecked_use_of_nullable_value`, not the Never-receiver arm. The
//! staged `promotions` `Never` arm (`staged_promotion_typed`) still spells
//! a `FlowBase::Never` fact faithfully should one ever stage — the
//! bottom-type join is the vacuous-clean face — but the reachable misses
//! are the canonical `Ty::Never` variant `is_never_ty` previously dropped.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// Asserts a `receiver_of_type_never` row fires for `fixture`.
fn assert_never_row(fixture: &str) {
    let report = report_ops(fixture);
    assert!(
        code_names(&report).contains(&"receiver_of_type_never"),
        "no receiver_of_type_never row: {:#?}",
        rows(&report)
    );
}

/// Asserts `receiver_of_type_never` never fires for `fixture`.
fn assert_never_silent(fixture: &str) {
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"receiver_of_type_never"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// Declared `Never` across the send/operator/prefix/postfix/index/call
/// entry points — `x`'s parse-time type IS `Never`.
#[test]
fn ticket_3237_declared_never_fire_faces() {
    // The index `x[i]` face (`property_element_resolver.dart:189`) and the
    // chained call-result callee (`f()()` — the outer call's `Never`-typed
    // receiver) need `stage_receiver_of_type_never` call sites in
    // `listener.rs`/`invocation.rs`, both outside this ticket's write scope;
    // they are residual misses recorded for the follow-up lane.
    let mut failures = Vec::new();
    for fixture in [
        "void f(Never x) {\n  x.foo();\n}",
        "void f(Never x) {\n  x + 1;\n}",
        "void f(Never x) {\n  -x;\n}",
        "void f(Never x) {\n  ~x;\n}",
        "void f(Never x) {\n  ++x;\n}",
        "void f(Never x) {\n  x++;\n}",
        "void f(Never x) {\n  x();\n}",
        "void f(Never x) {\n  x.foo(1 + 2);\n}",
    ] {
        let report = report_ops(fixture);
        if !code_names(&report).contains(&"receiver_of_type_never") {
            failures.push(format!("{fixture:?} -> {:?}", rows(&report)));
        }
    }
    assert!(failures.is_empty(), "silent faces:\n{}", failures.join("\n"));
}

/// An inferred-`Never` local — `var` adopts the `throw`'s bottom type, so
/// `x`'s declared type is `Never` (oracle: `receiver_of_type_never`).
#[test]
fn ticket_3237_inferred_never_var_receiver() {
    assert_never_row("void f() {\n  var x = throw 0;\n  x.foo();\n}");
}

/// A `Never`-returning call as the receiver — the expression's flow-aware
/// static type is `Never` across send/operator/index/call faces.
#[test]
fn ticket_3237_never_call_receiver() {
    // `ret()()` / `ret()[i]` are the same residual faces noted above: the
    // index read and the chained call-result callee stage no check today.
    let mut failures = Vec::new();
    for fixture in [
        "Never ret() => throw '';\nvoid f() {\n  ret().foo();\n}",
        "Never ret() => throw '';\nvoid f() {\n  ret() + 1;\n}",
        "Never ret() => throw '';\nvoid f() {\n  -ret();\n}",
    ] {
        let report = report_ops(fixture);
        if !code_names(&report).contains(&"receiver_of_type_never") {
            failures.push(format!("{fixture:?} -> {:?}", rows(&report)));
        }
    }
    assert!(failures.is_empty(), "silent faces:\n{}", failures.join("\n"));
}

/// A `throw`'s canonical `Ty::Never` receiver (the row rewrite, not the
/// written `Named{"Never"}` spelling) fires the send/operator/call faces —
/// the identity `is_never_ty` covers since #3237.
#[test]
fn ticket_3237_throw_receiver() {
    for fixture in [
        "void f() {\n  (throw '').toString();\n}",
        "void f() {\n  (throw '') + 1;\n}",
        "void f() {\n  (throw '')();\n}",
        "void f() {\n  -((throw ''));\n}",
    ] {
        assert_never_row(fixture);
    }
}

/// `x.foo` on a `Never` receiver is a property READ — upstream's `_resolve`
/// carries no Never arm. The staging gate must keep it silent (the
/// over-report face this ticket removes).
#[test]
fn ticket_3237_getter_access_never_receiver_silent() {
    assert_never_silent("void f(Never x) {\n  x.foo;\n}");
}

/// `x.foo = v` is the property WRITE face — silent upstream for the same
/// reason.
#[test]
fn ticket_3237_setter_access_never_receiver_silent() {
    assert_never_silent("void f(Never x) {\n  x.foo = 0;\n}");
}

/// `!x` routes through `_resolveNegation` — a `non_bool` face family, no
/// Never arm.
#[test]
fn ticket_3237_negation_never_receiver_silent() {
    assert_never_silent("void f(Never x) {\n  !x;\n}");
}

/// `x && y` / `x || y` left operands route through the logical resolvers —
/// no Never arm.
#[test]
fn ticket_3237_logical_operators_never_receiver_silent() {
    for op in ["&&", "||"] {
        assert_never_silent(&format!("void f(Never x, bool y) {{\n  x {op} y;\n}}"));
    }
}

/// The property faces stay silent even through the canonical `Ty::Never`
/// (`throw`) spelling — the face gate, not the type identity, decides.
#[test]
fn ticket_3237_throw_property_faces_silent() {
    assert_never_silent("void f() {\n  (throw '').foo;\n}");
    assert_never_silent("void f() {\n  (throw '').foo = 1;\n}");
}

/// A `Never?` receiver is `Null` — the nullable projection, never the
/// bottom face (`receiverOfTypeNever` checks `NeverTypeImpl.instance`,
/// non-nullable).
#[test]
fn ticket_3237_never_nullable_receiver_silent() {
    assert_never_silent("void f(Never? x) {\n  x.foo();\n}");
}

/// A `Null` receiver under `x != null` classifies `NullOrEquivalent` on
/// both sides — `x != null` is known-false, the body is dead, and `x`
/// reads `Null`, NOT `Never` (oracle: `invalid_use_of_null_value` +
/// `dead_code`, no `receiver_of_type_never`). The promoted-`Never` local is
/// a face the oracle never reaches.
#[test]
fn ticket_3237_null_receiver_promotes_null_not_never_silent() {
    assert_never_silent("void f(Null x) {\n  if (x != null) {\n    x.foo();\n  }\n}");
}

/// An `Object?`/`is Never` promoted receiver reports
/// `unchecked_use_of_nullable_value`, not `receiver_of_type_never` — a
/// promoted `Never` is not the resolver's arm.
#[test]
fn ticket_3237_is_never_promoted_receiver_silent() {
    assert_never_silent("void f(Object? x) {\n  if (x is Never) {\n    x.foo();\n  }\n}");
}
