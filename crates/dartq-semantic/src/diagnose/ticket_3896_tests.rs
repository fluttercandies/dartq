//! #3896 pins — the `invocation.rs` packet (binder faces, the `??`
//! composite window, the index-set family, the M11 consumer face).
//!
//! Every expectation here is oracle-verified against
//! `dart analyze` 3.13.7 on a minimal fixture; the fixtures below are
//! those fixtures, reduced.
//!
//! - **Index-set keys** — a generic receiver's index argument checks
//!   against the WRITTEN instantiation (`Box<int> b; b['s']` asks `int`,
//!   never the raw `K`). Landed here: the receiver row's declared face
//!   ([`Engine::index_receiver_ty`]) is the substitution source, the same
//!   two-step read the staging site runs.
//! - **Index-set values** — `c[i] = e` checks `e` against `[]=`'s VALUE
//!   formal (`invalid_assignment`). Routed: the target-intent gate lives
//!   in `diagnose/assignability.rs` (`check_assignment` ->
//!   `index_value_write_ty`), which never sees the index fold's row as an
//!   operator target — every shape stays silent today.
//! - **`??` composite arguments** — `e1 ?? e2` types as
//!   `LUB(NonNull(T1), T2)` (`binary_expression_resolver.dart`), so
//!   `int? ?? int?` is `int?` and a non-nullable formal rejects it.
//!   Routed: `fold_binary` leaves the composite `UNKNOWN` on purpose and
//!   the deferred join (`rhs_row_ty`/`join_operand_ty`) is reachable only
//!   from `assignability.rs`'s assignment RHS — the argument face never
//!   runs it.
//! - **Binder bounds** — an explicit type argument is a SUBTYPE question
//!   upstream (`type_arguments_verifier.dart` `isSubtypeOf`), so `void`
//!   conforms to no bound but `dynamic`/`Object?`; and an F-bound whose
//!   generic function type carries its OWN binder prints (and judges)
//!   the DECLARED spelling. Routed: the verdict is
//!   `assignability.rs`'s `assignable` (assignability, not subtype), and
//!   the binder is dropped by `scopes_bodies.rs`'s `tree_ty` / the
//!   function-type render.
//! - **M11 consumer** — the same-head `infer_gather` arm assumes
//!   covariance; declared variance needs #3903's capture column.

use super::test_rig::{code_names, report_ops, rows};

/// The generic receiver's index argument substitutes the written
/// instantiation: `Box<int> b; b['s']` reports the key against `int`
/// (oracle: `The argument type 'String' can't be assigned to the
/// parameter type 'int'.`) on the read AND the write form.
#[test]
fn generic_container_index_key_substitutes_declared_parameter() {
    let report = report_ops(
        r#"
class Box<K> {
  void operator []=(K key, int v) {}
  int operator [](K key) => 0;
}

void t(Box<int> b) {
  b['s'];
  b['s'] = 1;
}
"#,
    );
    let names = code_names(&report);
    let key_rows = names
        .iter()
        .filter(|name| **name == "argument_type_not_assignable")
        .count();
    assert!(
        key_rows >= 2,
        "expected the read and the write key rows against 'int', got {:#?}",
        rows(&report)
    );
    let stated = rows(&report).join("\n");
    assert!(
        stated.contains("'String'") && stated.contains("'int'"),
        "expected the key row to spell ('String' -> 'int'), got {stated}"
    );
}

/// The index write's VALUE face: `x[0] = v` checks `v` against `[]=`'s
/// value formal, so an `int?` written into `List<int>` reports
/// `invalid_assignment` (oracle). Silent today — the target-intent gate
/// this needs is `assignability.rs`'s `check_assignment`, not this file.
#[test]
fn index_write_value_checks_against_value_formal() {
    let report = report_ops(
        r#"
void t(int? v, List<int> x) {
  x[0] = v;
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"invalid_assignment"),
        "expected invalid_assignment on the written value, got {:#?}",
        rows(&report)
    );
}

/// The `??` composite's static type is `LUB(NonNull(T1), T2)` — an `int?`
/// left side keeps the nullable face, so the argument row fires (oracle:
/// `The argument type 'int?' can't be assigned to the parameter type
/// 'int'.`). Silent today: the composite's join is
/// `assignability.rs`'s deferred `rhs_row_ty`, which the argument face
/// never reaches.
#[test]
fn null_aware_composite_argument_keeps_nullable_join() {
    let report = report_ops(
        r#"
void take(int v) {}

void t(int? a, int? b) {
  take(a ?? b);
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "expected the joined 'int?' argument row, got {:#?}",
        rows(&report)
    );
}

/// An explicit `void` type argument conforms to no bound: upstream asks
/// `isSubtypeOf(void, bound)`, false for every non-top bound (oracle
/// reports both rows). Silent today — `assignability.rs`'s `assignable`
/// answers the assignment relation.
#[test]
fn void_type_argument_never_conforms_to_non_top_bound() {
    let report = report_ops(
        r#"
void fc<T extends num>() {}
void fd<T extends Object>() {}

void t() {
  fc<void>();
  fd<void>();
}
"#,
    );
    let names = code_names(&report);
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "type_argument_not_matching_bounds")
            .count(),
        2,
        "expected both 'void' bounds rows, got {:#?}",
        rows(&report)
    );
}

/// The F-bounded call-site face: `void func1<T extends T Function<T extends
/// num>(T)>()` rejects `void Function<T extends num>(T)` — the return leg
/// compares `void` against the bound's own (shadowing) parameter, false
/// upstream. Silent today: the function-subtype return leg runs the
/// assignment relation, where `void` is accepted.
#[test]
fn f_bounded_function_type_arg_reports_return_leg() {
    let report = report_ops(
        r#"
typedef exp2 = void Function<T extends num>();
typedef exp4 = void Function<T extends num>(T);

void func1<T extends T Function<T extends num>(T)>() {}
void func3<T extends T Function<T extends num>()>() {}

void t() {
  func1<exp2>();
  func1<exp4>();
  func3<exp2>();
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .filter(|name| **name == "type_argument_not_matching_bounds")
            .count()
            >= 2,
        "expected the func1 rows, got {:#?}",
        rows(&report)
    );
}

/// The bound MESSAGE renders the DECLARED spelling: the generic function
/// type in `T Function<T extends num>(T)` carries its own binder, so the
/// call's substitution never reaches it and the oracle prints
/// `'T Function<T extends num>(T)'`. Today the binder is dropped and the
/// outer parameter substituted (`'exp2 Function(exp2)'`).
#[test]
fn f_bounded_bound_message_renders_declared_binder() {
    let report = report_ops(
        r#"
typedef exp2 = void Function<T extends num>();

void func1<T extends T Function<T extends num>(T)>() {}

void t() {
  func1<exp2>();
}
"#,
    );
    let stated = rows(&report).join("\n");
    assert!(
        stated.contains("bound 'T Function<T extends num>(T)'"),
        "expected the declared binder spelling, got {stated}"
    );
}

/// The M11 consumer face's CONTROL: the same-head `infer_gather` arm
/// treats a matching head as covariant today, so the plain COVARIANT
/// widening stays silent (correct for this shape on every future
/// reading — it can only flip if the arm breaks).
///
/// The acceptance half is gated on #3903: a DECLARED-variance head
/// (`class Cov<out T>`, or a Super-mixin whose parameter is contravariant
/// in one arm) needs the type parameter's variance token through the
/// model, the persisted payload and the signature — the collector drops
/// it at `end_type_variable`, so this arm cannot consult it yet. The
/// consumer face re-dispatches the moment that column lands; the
/// acceptance shape is the `out`/mixin matrix over the same head.
#[test]
fn gather_same_head_arm_covariant_control_stays_silent() {
    let report = report_ops(
        r#"
class Cov<T> {
  final T? value;
  Cov(this.value);
}

Cov<Object?> widen(Cov<int> a) => a;
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"return_of_invalid_type"),
        "expected the covariant head to widen (no row), got {:#?}",
        rows(&report)
    );
}
