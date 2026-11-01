//! #3584 — the invocation-layer substitution of the solved call-site
//! type arguments into the callee's formal types BEFORE the per-argument
//! assignability verdict asks. Two faces were missing the solve, and the
//! raw `T` leaf reached the verdict as-is — `int → T` is genuinely not
//! assignable, so every row was an EXTRA the oracle never emits.
//!
//! * **An enum constant with no written type-argument group infers the
//!   enum's instantiation upward** — `v2(20)` over `enum E<T>` solves
//!   `T := int` from the argument expressions and checks `20` against
//!   `int`, never the declared `T` leaf; an unsolved slot instantiates
//!   to its bound (`z('s')` under `T extends num` checks against `num`).
//!   Upstream's `checkEnumConstantDeclaration` resolves the constant as
//!   an implicit `InstanceCreationExpression` whose `GenericInferrer`
//!   solves exactly this way (pkg/analyzer
//!   type_arguments_verifier.dart:176-202).
//! * **A function-RHS alias's own signature parameters are call-site
//!   inference variables** — `typedef Foo = int Function<T>(T x)` types
//!   a field `Foo f` / a return `Foo bar()` / a local with a signature
//!   that spells `T`, and `alias_fn_sig`'s sig carries no params (the
//!   alias's OWN parameters are bound by the written use). The
//!   invocation's `exec_params` table must read the RHS's `<T>` from
//!   `alias_fn_shape` so `f<String>(x)` fixes `T := String` and `f(x)`
//!   infers upward — the solved slot substitutes the formal leaf before
//!   the verdict (upstream `FunctionTypeAliasElement.instantiate`).

use super::test_rig::report_ops;
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

// —— The enum constant's unwritten group infers upward ——————————

/// `a(1)`/`b('x')` over `enum E<T>` solve `T := int`/`T := String` from
/// the argument and check each against its own solution — silent.
/// `c<int>(1)` keeps the written-arm face. Before the fix the `T` leaf
/// reached the verdict raw and every constant reported `int → T`,
/// `String → T`.
#[test]
fn enum_const_unwritten_group_infers_from_the_argument() {
    let fixture = "\
enum E<T> {
  a(1), b('x'), c<int>(1), d('x');
  final T f;
  const E(this.f);
}
";
    let report = report_ops(fixture);
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty()
            && rows_named(&report, "const_constructor_param_type_mismatch").is_empty(),
        "every constant's own solution checks cleanly: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}

/// An unsolved slot instantiates to its bound: `z('s')` over
/// `enum N<T extends num>` solves nothing for `T` — `String` is no
/// `num` — so the constant's formal reads the bound `num` and the
/// argument reports `String → num` (the oracle's pair of faces).
/// `x(1)`/`y(1.5)` solve `T := int`/`T := double` and stay silent.
#[test]
fn enum_const_unsolved_param_instantiates_to_its_bound() {
    let fixture = "\
enum N<T extends num> {
  x(1), y(1.5), z('s');
  final T f;
  const N(this.f);
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only z reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("num"),
        "the message renders the bound num, not T: {:?}",
        rows[0].message
    );
    assert_eq!(
        rows_named(&report, "const_constructor_param_type_mismatch").len(),
        1,
        "the const-constructor twin reports with it: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A written argument still beats inference: `v<num>('abc')` over
/// `enum E<T>` substitutes `T := num` from the `<num>` group and the
/// argument reports `String → num` — the pre-existing
/// `PendingBoundedCheck::Site` arm, pinned so the inference arm never
/// shadows it.
#[test]
fn enum_const_written_group_still_substitutes() {
    let fixture = "\
enum E<T> {
  v<num>('abc');
  final T f;
  const E(this.f);
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "the written arm reports String -> num: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("num"),
        "the written num reaches the message: {:?}",
        rows[0].message
    );
}

/// A multi-parameter enum solves each slot from its own argument:
/// `p(0, 0)` over `enum P<T extends U, U extends num>` binds
/// `T := int`/`U := int` and stays silent. `q(0, 's')`'s second
/// argument reports `String → int` — the upstream ordered solve keeps
/// `U` at the first answer (`int`, from the earlier `0` argument
/// through `T extends U`) and `String` fails it.
#[test]
fn enum_const_multi_param_solves_per_slot() {
    let fixture = "\
enum P<T extends U, U extends num> {
  p(0, 0), q(0, 's');
  final T a;
  final U b;
  const P(this.a, this.b);
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only q's second argument reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int"),
        "the ordered solve keeps U at int: {:?}",
        rows[0].message
    );
}

// —— The function-RHS alias's own parameters reach exec_params ———

/// `f<String>("hi")` over `Foo f` fixes `T := String` — `String → String`
/// asks nothing. `f<String>(42)` keeps the written solution and the
/// argument reports `int → String` (the explicit fix is never clamped
/// or re-solved). The bare `f(42)`/`f("hi")` infer `T := int`/`T := String`
/// upward and stay silent. Before the fix `exec_params` was never
/// populated — `T` reached the verdict raw and `f(42)` reported the
/// EXTRA `int → T` while `f<String>(42)` reported `int → T`.
#[test]
fn alias_rhs_params_substitute_the_field_send_formals() {
    let fixture = "\
typedef Foo = int Function<T>(T x);
class A {
  Foo f = <T>(T x) => 0;
  void test() {
    f<String>(\"hi\");
    f<String>(42);
    f(42);
    f(\"hi\");
  }
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only f<String>(42) reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range, d.message.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("String"),
        "the written String reaches the message, not T: {:?}",
        rows[0].message
    );
}

/// A `Foo`-returning send invoked through `bar()<String>(…)` keeps the
/// substitution: `bar()<String>("hi")` asks nothing. The explicit
/// `<String>` latch on an invoke-of-send row (`call_args_for` keys on
/// the callee row's offset, and the parenthesized send's row is not the
/// anchor the group closes under) is a pre-existing gap outside this
/// arm's scope — `bar()<String>(42)`'s under-report is the same shape
/// upstream would report `int → String` on, deferred with the tear-off
/// family.
#[test]
fn alias_rhs_params_substitute_the_return_send_formals() {
    let fixture = "\
typedef Foo = int Function<T>(T x);
class A {
  Foo bar() => <T>(T x) => 0;
  void test() {
    bar()<String>(\"hi\");
  }
}
";
    let report = report_ops(fixture);
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "the solved String formal asks nothing of String: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}

/// A getter-shaped alias callee (`g<String>(42)`) reads the same face —
/// the oracle reports `int → String` once.
#[test]
fn alias_rhs_params_substitute_the_getter_send_formals() {
    let fixture = "\
typedef Foo = int Function<T>(T x);
Foo get g => <T>(T x) => 0;
void test() {
  g<String>(42);
  g<String>(\"hi\");
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only the wrong argument reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}
