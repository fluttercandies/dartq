//! The #3799 pins: the `use_of_void_result` operand gate reads the void
//! SPELLING through the alias fold, not the dedicated [`Ty::Void`]
//! variant alone (upstream `checkForUseOfVoidResult`,
//! error_detection_helpers.dart:245-258 — the operand's `staticType` is
//! `VoidTypeImpl`, which a typedef-`void` use also answers). A typedef-
//! folded `void` reaches the operand faces as the alias head `Named{V}` —
//! or the RHS projection `Named{void}` — so every consumption site that
//! pre-gated on the dedicated variant under-reported the alias face:
//!
//! - `x == null` / `null == x` on `V x` (the `fold_binary` left check and
//!   `check_equals_argument`'s right-operand short-circuit) stayed silent
//!   where the oracle reports both operands;
//! - `3 + x`, `x!`, `if (x)`, `!x` and the for-in iterable face
//!   (`for (Object? _ in List<V>)`) pre-gated the same way;
//! - `for (V x in List<V>)` read `Named{V}` as a NON-void loop variable
//!   and over-reported the iterable — upstream's `variableType is
//!   VoidType` folds the alias and stays silent.
//!
//! # The contract
//!
//! - Every `void`-spelling operand — dedicated variant, spelled `void`,
//!   or typedef-folded `void` — reports `use_of_void_result` wherever the
//!   oracle does.
//! - A typedef-`void` loop variable or `==` formal is still `void`
//!   upstream, so the faces keyed on "is the OTHER side void" (the for-in
//!   loop-variable gate, the `==` formal carve-out) fold the alias the
//!   same way — never firing on a pair the oracle accepts.
//! - Non-`void` operands keep the no-op; the span dedup still answers one
//!   row per consumed node.

use crate::diagnose::test_rig::{report_ops, rows};

/// The oracle's `useOfVoidResult` message spelling (messages.yaml:19638).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts the fixture's `use_of_void_result` rows sit at exactly
/// `expected` `(offset, length)` pairs (offsets relative to the fixture's
/// own text, `CORE_PRELUDE` shifted). Other diagnostics a fixture's shape
/// invites (`unused_local_variable`, `dead_code`, type-check faces) are
/// tolerated.
fn assert_void_rows(
    report: &crate::diagnose::FileReport,
    fixture: &str,
    expected: &[(usize, u32)],
) {
    let mut got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    got.sort_unstable();
    let prelude = crate::diagnose::test_rig::CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

// —— `==` operand: the typedef-`void` local fires on both sides
// —— (probed oracle 3.13.7: `x == null` reports `x`, `null == x` reports
// —— `x`) — the alias head folds to `VoidTypeImpl` upstream.
#[test]
fn ticket_3799_equality_both_operands_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  x == null;\n  null == x;\n}";
    let report = report_ops(fixture);
    let left = fixture.find("x == null").unwrap();
    let right = fixture.rfind("x;").unwrap();
    assert_void_rows(&report, fixture, &[(left, 1), (right, 1)]);
}

// —— The spelled-`void` control beside it: same pair, same rows (the
// —— fold keeps the dedicated-variant face identical).
#[test]
fn ticket_3799_equality_both_operands_spelled_void() {
    let fixture = "void top() {}\nvoid main() {\n  void x = top();\n  x == null;\n  null == x;\n}";
    let report = report_ops(fixture);
    let left = fixture.find("x == null").unwrap();
    let right = fixture.rfind("x;").unwrap();
    assert_void_rows(&report, fixture, &[(left, 1), (right, 1)]);
}

// —— Binary operator argument: `3 + x` on `V x` short-circuits to
// —— `use_of_void_result` at the operand (upstream
// —— `checkForArgumentTypeNotAssignableForArgument`'s void arm).
#[test]
fn ticket_3799_binary_argument_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  3 + x;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("x;").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— `!`-postfix operand: `x!` on `V x` consumes the value — the oracle
// —— reports `use_of_void_result` at the operand-plus-`!` span.
#[test]
fn ticket_3799_postfix_bang_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  x!;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("x!").unwrap();
    assert_void_rows(&report, fixture, &[(at, 2)]);
}

// —— Condition face: `if (x)` on `V x` reports the operand — the
// —— junction/condition path reads the same spelled gate.
#[test]
fn ticket_3799_condition_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  if (x) {}\n}";
    let report = report_ops(fixture);
    let at = fixture.find("(x)").unwrap() + 1;
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— `!`-prefix operand: `!x` on `V x` reports the operand.
#[test]
fn ticket_3799_negation_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  !x;\n}";
    let report = report_ops(fixture);
    let at = fixture.find("x;").unwrap();
    assert_void_rows(&report, fixture, &[(at, 1)]);
}

// —— `&&` operands: the left fires unconditionally, the right under the
// —— junction — `V`-typed operands fold to the same face.
#[test]
fn ticket_3799_junction_typedef_void() {
    let fixture = "typedef V = void;\nV top() => throw 0;\nvoid main() {\n  V x = top();\n  V y = top();\n  x && y;\n}";
    let report = report_ops(fixture);
    let left = fixture.find("x &&").unwrap();
    let right = fixture.find("y;").unwrap();
    assert_void_rows(&report, fixture, &[(left, 1), (right, 1)]);
}

// —— For-in iterable: `List<V>`'s element folds to `void` upstream, so a
// —— non-void loop variable reports the iterable (upstream's
// —— `sequenceElementType is VoidType && variableType is! VoidType`).
#[test]
fn ticket_3799_for_in_typedef_void_element() {
    let fixture = "typedef V = void;\nvoid main(List<V> values) {\n  for (Object? _ in values) {}\n}";
    let report = report_ops(fixture);
    let at = fixture.find("values) {}").unwrap();
    assert_void_rows(&report, fixture, &[(at, "values".len() as u32)]);
}

// —— The negative twin: a typedef-`void` LOOP VARIABLE stays silent —
// —— `for (V _ in values)` folds `V` to `VoidTypeImpl` upstream and the
// —— `variableType is! VoidType` arm fails.
#[test]
fn ticket_3799_for_in_typedef_void_loop_var_silent() {
    let fixture = "typedef V = void;\nvoid main(List<V> values) {\n  for (V _ in values) {}\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[]);
}

// —— `==` formal carve-out: `bool operator ==(V o)` accepts the typedef-
// —— `void` actual — the formal folds to `VoidTypeImpl` the same way the
// —— operand does.
#[test]
fn ticket_3799_equality_typedef_formal_carve_out() {
    let fixture = "typedef V = void;\nclass C { bool operator ==(V o) => true; }\nV top() => throw 0;\nvoid main() {\n  C c = C();\n  V x = top();\n  c == x;\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[]);
}

// —— Non-void operand stays silent: the widened gate never fires on a
// —— genuine `int` operand.
#[test]
fn ticket_3799_non_void_operands_stay_silent() {
    let fixture = "typedef V = void;\nvoid main() {\n  int x = 1;\n  x == null;\n  null == x;\n  3 + x;\n  if (true) {}\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[]);
}

// —— #3822: the if-null join's FUNCTION LUB lifts the return type instead
// —— of collapsing to the bare `Function` head. Upstream
// —— `LeastUpperBoundHelper.getLeastUpperBoundOfFunctionTypes` joins the
// —— returns (`C1<int>` / `C2<int>` → `A`) and takes the GLB of each
// —— formal. The probe rides the `Exactly<T>` extension bound: with the
// —— right join the written type argument conforms, with the bare
// —— `Function` collapse it does not (the census `type_argument_not_
// —— matching_bounds` extras).
const BOUND_FIXTURE_HEAD: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
       void expectStaticType<R extends Exactly<T>>() {}\n\
     }\n\
     class A {}\n\
     class C1<T> implements A {}\n\
     class C2<T> implements A {}\n";

fn assert_no_bound_rows(report: &crate::diagnose::FileReport, fixture: &str) {
    let got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert!(got.is_empty(), "fixture={fixture:?}\nrows={:?}", rows(report));
}

// —— `??=` over two function-typed operands: `C1<int> Function()` and
// —— `C2<int> Function()` join to `A Function()`, so the `Exactly<A
// —— Function()>` argument conforms.
#[test]
fn ticket_3822_if_null_assignment_function_return_lift() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}C2<int> Function() make() => () => C2<int>();\n\
         void main() {{\n\
         \x20 C1<int> Function()? f;\n\
         \x20 (f ??= make())..expectStaticType<Exactly<A Function()>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    assert_no_bound_rows(&report, &fixture);
}

// —— The formal column takes the greatest lower bound: `void
// —— Function(A)` and `void Function(C1<int>)` join as
// —— `void Function(C1<int>)`.
#[test]
fn ticket_3822_if_null_assignment_function_formal_glb() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}void Function(C1<int>) make() => (C1<int> x) {{}};\n\
         void main() {{\n\
         \x20 void Function(A)? f;\n\
         \x20 (f ??= make())..expectStaticType<Exactly<void Function(C1<int>)>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    assert_no_bound_rows(&report, &fixture);
}

// —— An incompatible formal shape keeps the bare `Function` fallback (the
// —— official LUB answers the `Function` head when the arity or the
// —— required split differs) — and the `Exactly<Function>` argument still
// —— conforms to it.
#[test]
fn ticket_3822_if_null_assignment_function_shape_mismatch_falls_back() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}int Function(int) make() => (int x) => x;\n\
         void main() {{\n\
         \x20 C1<int> Function()? f;\n\
         \x20 (f ??= make())..expectStaticType<Exactly<Function>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    assert_no_bound_rows(&report, &fixture);
}

// —— Upstream `_functionType` (least_upper_bound.dart:757-769) DROPS a
// —— surplus optional positional from the joined signature instead of
// —— bailing: `void Function(int, [String])` / `void Function(int)` join
// —— as `void Function(int)`. Bailing would answer the bare `Function`
// —— and the `Exactly<void Function(int)>` argument would not conform.
#[test]
fn ticket_3822_function_lub_drops_surplus_optional_positional() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}void Function(int, [String]) g = (int x, [String y]) {{}};\n\
         void main() {{\n\
         \x20 void Function(int)? f;\n\
         \x20 (f ??= g)..expectStaticType<Exactly<void Function(int)>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    assert_no_bound_rows(&report, &fixture);
}

// —— The same walk matches named formals BY NAME and drops an unmatched
// —— optional one (:715-753): `void Function({int a, String b})` /
// —— `void Function({int a})` join as `void Function({int a})`.
#[test]
fn ticket_3822_function_lub_intersects_named_formals() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}void Function({{int a, String b}}) g = ({{int a = 0, String b = ''}}) {{}};\n\
         void main() {{\n\
         \x20 void Function({{int a}})? f;\n\
         \x20 (f ??= g)..expectStaticType<Exactly<void Function({{int a}})>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    assert_no_bound_rows(&report, &fixture);
}

// —— The bound-suppression gate (a) must not eat a REAL mismatch: a
// —— fully-solved bound still reports. Probed oracle 3.13.7 on the
// —— canonical form — `int i; i.expectStaticType<Exactly<String>>()`
// —— reports the bounds row while `Exactly<int>` is silent.
#[test]
fn ticket_3822_solved_bound_mismatch_still_reports() {
    let fixture = format!(
        "{BOUND_FIXTURE_HEAD}void main() {{\n\
         \x20 int i = 0;\n\
         \x20 i.expectStaticType<Exactly<String>>();\n\
         \x20 i.expectStaticType<Exactly<int>>();\n\
         }}"
    );
    let report = report_ops(&fixture);
    let hits: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    assert_eq!(hits.len(), 1, "rows={:?}", rows(&report));
}
