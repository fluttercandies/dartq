//! #3681 — composite-expression result fold (shape 1.4a, 258 rows).
//!
//! `?:`, `??=`, and switch-expression composites never stamped a result
//! `Ty`: the tail operand's own type leaked into the parenthesized group's
//! `inner_ty`, cascade receivers, and argument reads, surfacing as
//! `type_argument_not_matching_bounds` on `expectStaticType<Exactly<…>>()`.
//! The fix folds each composite into one `Value` row carrying the upstream
//! LUB — `LUB(T1,T2)` for `?:`, `LUB(NonNull(T1),T2)` for `??=` — so
//! enclosing reads see the node's own static type.
//!
//! Oracle provenance: `dart analyze` 3.13.1 — every pin below is silent
//! on the oracle.

use super::test_rig::{RigImport, assert_silent, report_ops, report_ops_imports};

/// The co19 `StaticType`/`Exactly<T>` extension the census cases use.
const EXPECT_HELPER: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
     \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
     }\n";

// —— `?:` conditional expression —————————————————————————————————

/// `int | double → num` — the commonest cluster (census rows 1–30).
#[test]
fn conditional_int_double_types_num() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(bool b) {{\n\
         \x20 var i = 1; var d = 2.0;\n\
         \x20 (b ? i : d).expectStaticType<Exactly<num>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// `Null | int → int?` — the nullable arm cluster.
#[test]
fn conditional_null_int_types_int_question() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(bool b) {{\n\
         \x20 var i = 1;\n\
         \x20 (b ? null : i).expectStaticType<Exactly<int?>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// `Iterable<int> | Iterable<double> → Iterable<num>` — the generic
/// container cluster (arg-wise LUB recursion).
#[test]
fn conditional_iterable_int_double_types_iterable_num() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(bool b) {{\n\
         \x20 Iterable<int> li = []; Iterable<double> ld = [];\n\
         \x20 (b ? li : ld).expectStaticType<Exactly<Iterable<num>>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// Negative control: same-type arms stay that type — `int | int → int`,
/// not `num`, and a wrong expectation must fire.
#[test]
fn conditional_int_int_stays_int() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(bool b) {{\n\
         \x20 var i = 1;\n\
         \x20 (b ? i : i).expectStaticType<Exactly<int>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

// —— `??=` if-null assignment ————————————————————————————————————

/// `(x ??= y)` inside parens types as `LUB(NonNull(T1), T2)` — the
/// `if_null_results` latch feeds `inner_ty` for `.member` chains.
/// `num? ??= double → num` (NonNull(num?) = num, LUB(num,double) = num).
#[test]
fn if_null_assignment_paren_types_lub() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f() {{\n\
         \x20 num? local1; double d = 2.0;\n\
         \x20 (local1 ??= d).expectStaticType<Exactly<num>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// `int? ??= int → int` — the non-null-promotion cluster: `NonNull(int?)`
/// rewrites to `int`, `LUB(int,int) = int`.
#[test]
fn if_null_assignment_non_null_promotes() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f() {{\n\
         \x20 int? x;\n\
         \x20 (x ??= 1).expectStaticType<Exactly<int>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// Bare `??=` outside parens stays silent (no `Value` row, no false
/// verdicts — the latch only feeds the paren close).
#[test]
fn if_null_assignment_bare_stays_silent() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f() {{\n\
         \x20 int? x;\n\
         \x20 x ??= 1;\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// `double? ??= int → num` — the widened-LUB cluster: `NonNull(double?)`
/// rewrites to `double`, `LUB(double,int) = num`.
#[test]
fn if_null_assignment_double_int_types_num() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f() {{\n\
         \x20 double? d;\n\
         \x20 (d ??= 1).expectStaticType<Exactly<num>>();\n\
         }}\n"
    ));
    assert_silent(&report);
}

// —— the return-context arm ————————————————————————————————————

/// A conditional in direct return position under an `async` `Future<T>`
/// declared return reaches the context arm's `FutureOr<T>` widening:
/// `future_value_type` unwraps `T` by READING the pool, and the widening
/// re-interns `FutureOr<T>` by WRITING the same pool. The unwrap used to
/// sit inside the re-intern's argument list — a method receiver is
/// evaluated before its arguments, so the unwrap's shared `pool()` read
/// hit the live `RefMut` and panicked ("RefCell already mutably
/// borrowed") on the listener-driven walk
/// (`end_conditional_expression` → `fold_conditional` →
/// `return_expression_context`). The unwrap now completes before the
/// borrow; the fold's answer is unchanged (the plain LUB `int` already
/// fits `FutureOr<int>`, so the node keeps `int`). Oracle: silent.
#[test]
fn conditional_return_async_future_context_no_pool_reentry() {
    let report = report_ops(
        "Future<int> f(bool c) async {\n\
         \x20 return c ? 1 : 2;\n\
         }\n",
    );
    assert_silent(&report);
}

// —— the mixin-constraint projection arm ————————————————————————————

/// A mixin application's `on`-constraint check projects the constraint
/// face through `Boundary::ty` (`verify_enum_member_set` →
/// `verify_mixin_application` → `instantiated_constraint`), and a FOREIGN
/// named face with written arguments re-interns its projected list into
/// the diagnosing pool (`types.rs` projecting tail). The face used to be
/// projected with the pool's shared `Ref` still live in this frame — the
/// re-intern's `borrow_mut` then panicked ("RefCell already borrowed") on
/// the whole-unit listener walk. The projection now completes before the
/// borrow, the caller-holding hoist; the verdict is unchanged (the
/// constraint `A<B>` is satisfied by the superclass `A<B>`, so the
/// application stays silent). Oracle: silent.
#[test]
fn mixin_constraint_projection_no_pool_reentry() {
    let report = report_ops_imports(
        "class C extends A<B> with M {}\n",
        &[RigImport {
            uri: "package:t/m.dart",
            prefix: None,
            source: "class A<T> {}\nclass B {}\nmixin M on A<B> {}\n",
        }],
    );
    assert_silent(&report);
}
