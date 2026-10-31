//! #3122 — the `invalid_assignment`/`argument_type_not_assignable` arms
//! whose roots sit in `invocation.rs`'s send close.
//!
//! * **The send-solve's instantiated return keeps its binding** — a
//!   generic callee whose upward inference solved a parameter already
//!   wrote the substituted return into the callee row
//!   (`check_arguments`'s `subst_params` arm); the function-typed-callee
//!   face must not fold that answer to the shape's `return_ty`
//!   (`void`) and starve every downstream assignability ask. Upstream
//!   types the invocation node with the inference result's return
//!   (function_expression_invocation_resolver.dart:55-78).
//! * **The effective clamp touches only the inferred solution** — a
//!   written type argument keeps its own face even when it violates the
//!   declared bound; the bound violation reports separately as
//!   `type_argument_not_matching_bounds`
//!   (type_arguments_verifier.dart:570-633). Clamping the explicit
//!   argument to its bound manufactures an `argument_type_not_assignable`
//!   pair the oracle never asks.
//! * **The enum constant's formal is the instantiated one** —
//!   `enum E<T> { a<int>(…) }` substitutes the constant's written
//!   argument for the constructor's declared `T`
//!   (`correspondingParameter.type`,
//!   error_detection_helpers.dart:72-75).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

// —— The send-solve's return binding survives the callee retype ————

/// `toF(source)` solves `X := G` and writes `F<G>` as the send's static
/// type, so `fsource` binds a function type and `fsource()` is a valid
/// invocation — the oracle emits nothing for the call. Before the fix the
/// callee retype folded `F<G>` to the shape's `void` return, `fsource`
/// bound `void`, and invoking it reported `invocation_of_non_function`.
/// (The `F<G<ITB>>` vs `F<G<…>>` pair verdict itself lives in
/// assignability.rs — another lane.)
#[test]
fn ia1_solved_send_return_feeds_the_callable_face() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
typedef G<X extends Y, Y extends A<Y>> = void Function(Y);
class A<X> {}
F<X> toF<X>(X x) => <Y extends X>() {};
void test(G source) {
  var fsource = toF(source);
  fsource();
}
void main() {}
";
    let report = report_ops(fixture);
    assert!(
        rows_named(&report, "invocation_of_non_function").is_empty()
            && rows_named(&report, "not_a_function").is_empty(),
        "fsource must bind F<G> so fsource() invokes cleanly: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}

/// A NON-generic function-typed callee keeps the shape-return retype:
/// `handler(x)` over `typedef Handler = String Function(int)` reads
/// `String`, never the whole function type — the arm the solve guard
/// must not silence (the send solve binds nothing for an unparameterized
/// callee, so the shape's own return types the call).
#[test]
fn function_typed_callee_still_yields_the_shapes_return() {
    let report = report_ops(
        "\
typedef Handler = String Function(int x);
class Widget {
  final Handler handler;
  const Widget(this.handler);
  String invoke(int x) {
    return handler(x);
  }
}
",
    );
    assert!(
        rows_named(&report, "return_of_invalid_type").is_empty(),
        "the shape's return must feed the return face: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

// —— The effective clamp reads only the inferred solution ————————

/// `g<String>("s")` writes `String` against a `num` bound: the oracle
/// reports the bound violation (`type_argument_not_matching_bounds`) and
/// keeps the WRITTEN `String` as the formal — `String → String` asks no
/// assignability question. Clamping the explicit argument to `num` would
/// manufacture a `String → num` row the oracle never emits.
#[test]
fn at2_explicit_arg_violation_stays_the_written_face() {
    let fixture = "\
void g<T extends num>(T t) {}
void main() {
  g<String>(\"s\");
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows_named(&report, "type_argument_not_matching_bounds").len(),
        1,
        "the bound violation reports once: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "the explicit formal stays the written String: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The inferred twin still clamps: an argument whose inferred solution
/// violates the bound checks against the bound's face, not the
/// inference's own answer — `f(null)` over `T extends num` reports the
/// `Null → num` pair.
#[test]
fn at2_inferred_solution_still_clamps_to_the_bound() {
    let fixture = "\
void f<T extends num>(T t) {}
void main() {
  f(null);
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "the inferred Null clamps to num and reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}

// —— The enum constant's formal is the instantiated one ——————————

/// `enum E<T> { a<int>("1") }` instantiates `T := int`: the `a`
/// constant's `String` argument checks against `int` and reports
/// `String → int` — never `String → T`.
#[test]
fn at2_enum_constant_arg_reads_the_instantiated_formal() {
    let fixture = "\
enum E<T> {
  a<int>(\"1\"),
  b<int>(2);
  const E(T t);
}
void main() {}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only the String argument reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int"),
        "the message renders the instantiated int, not T: {:?}",
        rows[0].message
    );
}
