//! #3887 pins — the row-model wave.
//!
//! (E-op A) The downward-context face instantiates an enclosing generic
//! call's explicit type arguments before the formal reaches the ladder
//! (`send_formals`' `call_type_args` × `type_param_names` substitution —
//! upstream `invocation_inferrer.dart`'s `_computeContextForArgument`
//! substitutes `typeParameter` leaves under the written `typeArguments`),
//! and `clamp`'s double arm keeps the POSITIVE `T <: double?` gate the
//! official `_refineNumericInvocationContextNullSafe` spells for it alone
//! (type_system.dart:1955-1960 — the arithmetic selectors' arm is the
//! NEGATIVE `!T <: double?`).
//!
//! (#3869 gap 1) The member-walk miss paths mirror `member_defined`'s
//! accessor-callee Function check: an invoke-row whose resolved callee is
//! a potentially-nullable function type reports the shared unchecked
//! code's `Function` variant at the whole callee expression, even when
//! the member resolved through the frozen faces or the core-type table
//! instead of the live walk.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — every fixture probed there.

use super::test_rig::{assert_silent, report_ops, report_ops_bare, rows};
use crate::diagnose::FileReport;

/// The numeric-ladder fixture prelude: `num`/`int`/`double` spelled with
/// `remainder`/`clamp` so the `refine_numeric_context` gates
/// (`T <: int?`, `T <: double?`) read real subtyping instead of the
/// member-less shared prelude (whose `int` carries only the `+`/`-`/`<`
/// faces). `report_ops_bare` mounts no prelude, so these declarations
/// are the unit's own.
const NUMERIC_PRELUDE: &str = "
class num {}
class int extends num {
  int operator +(int o) => this;
  int remainder(num o) => this;
}
class double extends num {
  double operator +(num o) => this;
  double clamp(num a, num b) => this;
}
";

fn argument_type_not_assignable(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| format!("@{}: {}", d.range.offset, d.message))
        .collect()
}

fn unchecked_function(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "unchecked_use_of_nullable_value" && d.message.contains("function")
        })
        .map(|d| format!("@{}: {}", d.range.offset, d.message))
        .collect()
}

// —— E-op A: explicit type args instantiate the enclosing formal ——

/// `send_formals` substitutes the callee's own written type arguments
/// (`context<int>` hands `int`, never the raw `T`) — the in-scope half
/// of the E-op-A wave. This pin locks the substitution face through a
/// member send whose resolved member declares a type parameter the
/// `call_type_args` group instantiates.
///
/// Residual (routed, not dropped): the eager `T -> Object?` resolution
/// of `contextType(1)` reads `callee_return_ty`, not
/// `invoke_downward_context`, so the instantiated formal never reaches
/// that arg's own typing — that consumption lives in the send-fold
/// inference (`invocation.rs` `explicit_args`/`solutions`, the #3869
/// gap-2 window) and is out of this lane's `row_resolution.rs` scope.
#[test]
fn explicit_type_args_substitute_member_formal() {
    let report = report_ops_bare(
        &r#"
        __NUMERIC_PRELUDE__
        class Box { T id<T>(T x) => x; }
        void h(int x) {}
        void g(Box b) {
          h(b.id<int>(1));
        }
        "#
        .replace("__NUMERIC_PRELUDE__", NUMERIC_PRELUDE),
    );
    assert_silent(&report);
}

/// The control: the same send without a written group keeps the
/// parameter's own declared face — `T` resolves nothing the `int`
/// formal can't already answer, so the fixture stays silent either way
/// and exists only to prove the substitution is opt-in, not ambient.
#[test]
fn explicit_type_args_absent_keeps_declared_formal() {
    let report = report_ops_bare(
        &r#"
        __NUMERIC_PRELUDE__
        class Box { T id<T>(T x) => x; }
        void h(int x) {}
        void g(Box b) {
          h(b.id(1));
        }
        "#
        .replace("__NUMERIC_PRELUDE__", NUMERIC_PRELUDE),
    );
    assert_silent(&report);
}

/// `clamp`'s POSITIVE double gate: `d.clamp(f(), f())` over `d: double`
/// hands `f` a `double` context where the arithmetic selectors would
/// hand `num` — the upstream `test_clamp_double_context_double` face
/// (type_system.dart:1955-1960).
#[test]
fn clamp_positive_double_gate() {
    let report = report_ops_bare(
        &r#"
        __NUMERIC_PRELUDE__
        T f<T>() => throw 0;
        void h(double x) {}
        void g(double d) {
          h(d.clamp(f(), f()));
        }
        "#
        .replace("__NUMERIC_PRELUDE__", NUMERIC_PRELUDE),
    );
    assert_silent(&report);
}

/// The arithmetic twin keeps the NEGATIVE gate: `d + f()` over
/// `d: double` still hands `f` a `num` context (upstream
/// `test_clamp_double_context_*`'s `+` siblings infer `num`).
#[test]
fn arithmetic_negative_double_gate_control() {
    let report = report_ops_bare(
        &r#"
        __NUMERIC_PRELUDE__
        T f<T>() => throw 0;
        void h(int x) {}
        void g(double d) {
          h(d + f());
        }
        "#
        .replace("__NUMERIC_PRELUDE__", NUMERIC_PRELUDE),
    );
    // `d + f()` types by its declared `double operator +` return — the
    // NEGATIVE gate refines `f`'s CONTEXT (not the send's return), so
    // the send keeps `double` and `h(int)` reports the oracle's own
    // `'double' can't be assigned to 'int'` row, pinned as present.
    let found = argument_type_not_assignable(&report);
    assert!(
        found
            .iter()
            .any(|row| row.contains("'double'") && row.contains("'int'")),
        "rows: {:#?}",
        rows(&report)
    );
}

// —— #3869 gap 1: member-callee Function check on the miss paths ——

/// A getter returning a nullable function type reports the `Function`
/// variant at the whole callee expression — the found-arm's own latch
/// (the shared face the miss-path arms now mirror). The function type
/// spells inline: the `AliasRhs::Fn` dep walk lives in `bounds.rs`
/// (#3875's window), outside this lane's scope.
#[test]
fn member_callee_function_nullable_found_arm() {
    let report = report_ops(
        r#"
        class C { void Function()? get cb => null; }
        void g(C c) { c.cb(); }
        "#,
    );
    let found = unchecked_function(&report);
    assert!(
        found.iter().any(|row| row.contains("function")),
        "rows: {:#?}",
        rows(&report)
    );
}

/// A method send keeps silent — `m` is the method itself, never a
/// callable result (the accessor-kind gate).
#[test]
fn member_callee_method_send_control() {
    let report = report_ops(
        r#"
        class C { void m() {} }
        void g(C c) { c.m(); }
        "#,
    );
    assert_silent(&report);
}

/// A non-nullable `Function` field resolves the invoke — the oracle's
/// `nullable_dereference_verifier` `_check` reports only a
/// POTENTIALLY-NULLABLE function type.
#[test]
fn member_callee_non_nullable_function_control() {
    let report = report_ops(
        r#"
        class C { void Function() get cb => () {}; }
        void g(C c) { c.cb(); }
        "#,
    );
    assert!(
        unchecked_function(&report).is_empty(),
        "rows: {:#?}",
        rows(&report)
    );
}
