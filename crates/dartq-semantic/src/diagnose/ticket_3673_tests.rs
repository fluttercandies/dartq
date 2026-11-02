//! Ticket #3673 pins: the `unnecessary_type_check` + `dead_code`
//! warning-family pair from census 12.
//!
//! Shape taxonomy mined from
//! `target/conformance/census-20261102-230100.report.tsv`:
//!
//! - `unnecessary_type_check` span arm: `_checkAllTypeChecks` reports
//!   `.at(node)` on the whole `IsExpression` (upstream
//!   best_practices_verifier.dart:886-890) — the operand expression's
//!   head through the written type's end. An operand whose head hides
//!   behind a call/group/creation chain (`new C<T>().f is U`,
//!   `f().g is U`) anchored at the merged row's member token, cutting
//!   the oracle's span short.
//! - `dead_code` `as`-arm: `visitAsExpression` runs
//!   `checkUnreachableNode(node.type)` AFTER the operand resolves
//!   (upstream resolver.dart:2090-2099), so `((throw 0) as T)` inside a
//!   conditional arm marks the type annotation first-dead and
//!   `flowEnd(elseExpression)` reports `[annotation.offset,
//!   elseExpression.end)` — one row covering the annotation through the
//!   arm's closer, never fragments.

use super::test_rig::{code_names, report_ops};
use super::*;

fn code_rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// Probe: `func1 is Func1` — a local function `int Function(int)`
/// against typedef `Func1` = `int Function(int)`. Oracle: constant-true.
#[test]
fn probe_typedef_fn_subtype_true() {
    let report = report_ops(
        "typedef int Func1(int a);\n\
         void main() {\n\
         \x20 int func1(int i) => -1;\n\
         \x20 func1 is Func1;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Probe: `func4 is Func1` — `int Function([int a, int b, int c])` against
/// `int Function(int)`: optional-tail drop keeps it a subtype
/// (co19 typedef/is_test).
#[test]
fn probe_typedef_optional_tail_subtype() {
    let report = report_ops(
        "typedef int Func1(int a);\n\
         typedef int Func4([int a, int b, int c]);\n\
         void main() {\n\
         \x20 int func4([int a = 0, int b = 0, int c = 0]) => -1;\n\
         \x20 func4 is Func1;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Probe: an old-style function typedef arg directly —
/// `void___int`-typed operand vs `void_` target.
#[test]
fn probe_oldstyle_typedef_arg_direct() {
    let report = report_ops(
        "typedef void void_();\n\
         typedef void void___int([int i]);\n\
         void___int make() => () {};\n\
         void main() {\n\
         \x20 make() is void_;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Control: plain same-head args.
#[test]
fn probe_generic_arg_plain_subtype() {
    let report = report_ops(
        "class C<T> {}\n\
         void main() {\n\
         \x20 C<int>() is C<num>;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Control: same typedef args both sides (reflexive).
#[test]
fn probe_generic_arg_typedef_reflexive() {
    let report = report_ops(
        "class C<T> {}\n\
         typedef void void_();\n\
         void main() {\n\
         \x20 C<void_>() is C<void_>;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Probe: `C<void___int>() is C<void_>` — the co19 optional2_test shape:
/// a created generic instance with typedef'd function-type arguments
/// tested against the same generic head with a different typedef
/// argument. `void___int = void Function([int])` <: `void_ = void
/// Function()` — oracle constant-true.
#[test]
fn probe_generic_arg_typedef_fn_subtype() {
    let report = report_ops(
        "class C<T> {}\n\
         typedef void void_();\n\
         typedef void void___int([int i]);\n\
         void main() {\n\
         \x20 C<void___int>() is C<void_>;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        ["unnecessary_type_check"],
        "{:?}",
        code_names(&report)
    );
}

/// Probe: generic-typedef target — `f13 is F13<bool>` where `f13` is
/// `List<int> Function(List<int>) Function(int)` and
/// `F13<T> = List<int> Function(List<T>) Function(int)`: NOT a subtype,
/// oracle stays silent (co19 function_type13_test extra rows).
#[test]
fn probe_generic_typedef_arg_mismatch_silent() {
    let report = report_ops(
        "typedef F13<T> = List<int> Function(List<T> x) Function(int x);\n\
         List<int> Function(List<int>) f13(int x) => throw 0;\n\
         void main() {\n\
         \x20 f13 is F13<bool>;\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// Probe: `new OneArg<String>().foo is OneArg` — the operand head hides
/// behind the `<String>` type-args group and the `()` call group; the
/// oracle anchors `unnecessary_type_check` at `new` (the whole
/// IsExpression), not at `foo` or `()`.
#[test]
fn probe_generic_creation_member_operand_anchors_at_new() {
    let source = "class OneArg<A> {\n\
                  \x20 OneArg<A> get foo => OneArg<A>();\n\
                  }\n\
                  void f() {\n\
                  \x20 new OneArg<String>().foo is OneArg;\n\
                  }\n";
    let report = report(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| {
            source[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect();
    assert_eq!(
        rows,
        ["new OneArg<String>().foo is OneArg"],
        "{:?}",
        codes(&report)
    );
}

/// Probe: `((throw 0) as int)` in a conditional else arm — the oracle's
/// `flowEnd(elseExpression)` reports `[type.offset, elseExpression.end)`
/// = `int)` (the annotation through the arm's closer), one row.
#[test]
fn probe_throw_as_named_type_dead_interval() {
    let source = "var c = true;\n\
                  void f(int x1) {\n\
                  \x20 var z = c ? x1 : ((throw 0) as int);\n\
                  }\n";
    let report = report(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| source[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string())
        .collect();
    assert_eq!(rows, ["int)"], "{:?}", report.diagnostics);
}

/// Probe: `((throw 0) as void Function(X1))` — the oracle's single row
/// covers `void Function(X1)` through the arm closer. RESIDUAL: the flow
/// layer's `type_head` latch keeps the LAST head row of a composite
/// annotation (`X1` inside the formals), so `cast_tail_row` anchors at
/// `X1` instead of `void` — the fix lives in `crates/dartq-flow` (out of
/// scope). Ignored until the flow-side latch records the first head row;
/// remove the ignore and this pin asserts the oracle row directly.
#[test]
#[ignore = "residual: dartq-flow type_head latch anchors composite type annotations at the last head row"]
fn probe_throw_as_function_type_dead_interval() {
    let source = "var c = true;\n\
                  void f<X1 extends int>(X1 x1) {\n\
                  \x20 var z = c ? x1 : ((throw 0) as void Function(X1));\n\
                  }\n";
    let report = report(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| source[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string())
        .collect();
    assert_eq!(rows, ["void Function(X1))"], "{:?}", report.diagnostics);
}

/// Probe: generic-function tear-off operand vs a typedef'd target and an
/// inline function-type annotation — `m15 is F15<T>` and
/// `m15 is List<T> Function(int, [List<int> x]) Function()` where `m15`'s
/// own type parameter `T` flows into both sides. Oracle stays silent
/// (the operand type is not a subtype of the target under the free `T`).
/// The function_type co19 family produced ~68 extra rows of this shape.
#[test]
fn probe_generic_function_tearoff_stays_silent() {
    let report = report_ops(
        "typedef F15<T> = List<T> Function(int, [List<int> x]) Function();\n\
         List<T> Function(int, [List<int> x]) m15<T>() => throw 0;\n\
         void test<T>() {\n\
         \x20 m15 is F15<T>;\n\
         \x20 m15 is List<T> Function(int, [List<int> x]) Function();\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// Probe: enhanced-enum `with`/`implements` supertypes —
/// `E.a is Enum` (the implicit `Enum` supertype every enum declares),
/// `is M` (`with M`), `is I1`/`is I2` (`implements`) all answer
/// constant-true upstream. The Enhanced-Enum co19 family produced ~27
/// miss rows where the enum head's supertype edges went unseen. The
/// `mixin M on Object` declaration draws a separate
/// `mixin_super_class_constraint_non_interface` row — a different
/// code's extra, outside this ticket's pair.
#[test]
fn probe_enhanced_enum_supertypes_constant_true() {
    let report = report_ops(
        "class I1 {}\n\
         abstract class I2 {}\n\
         mixin M on Object {}\n\
         enum E with M implements I1, I2 { a, b, c }\n\
         void main() {\n\
         \x20 E.a is Enum;\n\
         \x20 E.a is I1;\n\
         \x20 E.a is I2;\n\
         \x20 E.a is M;\n\
         }\n",
    );
    let utc: Vec<&str> = code_names(&report)
        .into_iter()
        .filter(|name| *name == "unnecessary_type_check")
        .collect();
    assert_eq!(
        utc,
        [
            "unnecessary_type_check",
            "unnecessary_type_check",
            "unnecessary_type_check",
            "unnecessary_type_check"
        ],
        "{:?}",
        code_names(&report)
    );
}

/// Probe: `((throw 0) as C1<X1>)` — a parameterized named annotation's
/// dead tail covers `C1<X1>)` (the pin's `type_head` latch keeps `C1`).
#[test]
fn probe_throw_as_generic_named_type_dead_interval() {
    let source = "class C1<X> {}\n\
                  var c = true;\n\
                  void f<X1 extends C1<X1>>(X1 x1) {\n\
                  \x20 var z = c ? x1 : ((throw 0) as C1<X1>);\n\
                  }\n";
    let report = report(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| source[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string())
        .collect();
    assert_eq!(rows, ["C1<X1>)"], "{:?}", report.diagnostics);
}
