//! #3697 pins — `assignment_to_final_local`'s write-side coverage.
//!
//! The oracle's rule (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart`
//! `AssignmentExpressionShared.checkFinalAlreadyAssigned`, :385-421 — and
//! `resolver.dart` `resolveAssignedVariablePattern`, :1429-1455 for the
//! pattern-assignment twin): a write to a `final` local or formal reports
//! unless the variable is definitely unassigned at the write. The row
//! model's gaps, and the fixes pinned here:
//!
//! 1. **Pattern-declared finals.** `final (a, b) = …`, `final [a] = …`,
//!   `final {"k": a} = …`, `for (final (v) in …)`, `case final (a)`: the
//!   `final` keyword heads the OUTERMOST pattern, so the declaration-face
//!   walk steps through the pattern's bracket structure to reach it.
//! 2. **Catch parameters are implicitly final** (`catch (e, s)`) — every
//!   write reports, no keyword required.
//! 3. **Captured writes.** A write inside a nested function body to an
//!   enclosing `final` local reports even as the variable's only write
//!   (the `potentiallyMutatedInScope` mark makes `isUnassigned` false
//!   inside the closure).
//! 4. **Sibling branches don't count as prior writes**: `if (b) { x = 1; }
//!   else { x = 2; }` assigns `x` once per path — silent.
//! 5. **Constructor initializer writes are not local writes** —
//!   `this.x`/`super.x` formals and `x = e` LHS positions resolve to the
//!   field; `field_initialized_*`/`assignment_to_final` own them.
//!
//! Oracle provenance: `dart analyze` 3.13.7 rows in
//! `conformance/expected.tsv` (co19 `LanguageFeatures/Patterns`
//! `variable_A03_*`, `variable_declaration_A03_*`/`A04_*`,
//! `execution_pattern_for_*`, `AssignmentToFinalLocalTest`, and the
//! initializer-list extras from `static_processing_A24_*` /
//! `duplicate_initializers_test`).

use super::test_rig::{code_names, report_ops, rows};
use crate::diagnose::FileReport;

/// The `assignment_to_final_local` rows as `(offset, message)` pairs,
/// ascending.
fn atfl_rows(report: &FileReport) -> Vec<(u32, String)> {
    let mut rows: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "assignment_to_final_local")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    rows.sort_unstable();
    rows
}

/// `needle`'s absolute offset inside `fixture` as the rig analyzes it (the
/// prelude shifts every fixture-local offset).
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(super::test_rig::CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

// —— pattern-declared finals (the miss side's dominant cluster) ——

/// `final (a, b) = (1, 2); a = 0;` — a parenthesized pattern-declared
/// final, plain `=` write.
///
/// Oracle (`variable_declaration_A03_t02`, `final ((a1, b1) && r1) = …;
/// a1 = 3;`): `The final variable 'a' can only be set once.` at the write
/// name.
#[test]
fn parenthesized_pattern_final_write_reports() {
    let fixture = "void f() {\n  final (a, b) = (1, 2);\n  a = 0;\n  b = 1;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (
                at(fixture, "a = 0"),
                "The final variable 'a' can only be set once.".to_string()
            ),
            (
                at(fixture, "b = 1"),
                "The final variable 'b' can only be set once.".to_string()
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// `final [a] = [0]; a = 1;` — a list pattern-declared final.
///
/// Oracle (`variable_A03_t01`, `final [int c, num d] = [1, 2]; c = 2;`).
#[test]
fn list_pattern_final_write_reports() {
    let fixture = "void f() {\n  final [int a] = [0];\n  a = 1;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "a = 1"),
            "The final variable 'a' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// `final {"k": a} = {"k": 0}; a = 1;` — a map pattern-declared final.
///
/// Oracle (`variable_A03_t01`, `final {1: e} = {1: "2"}; e = "3";`).
#[test]
fn map_pattern_final_write_reports() {
    let fixture = "void f() {\n  final {\"k\": a} = {\"k\": 0};\n  a = 1;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "a = 1"),
            "The final variable 'a' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// `for (final (int v) in xs) { v++; }` — a pattern-for-in loop variable
/// written by the `++` application inside the body.
///
/// Oracle (`execution_pattern_for_in_statement_A03_t01`, `v1++`).
#[test]
fn for_in_pattern_final_increment_reports() {
    let fixture = "void f() {\n  for (final (int v) in [1, 2]) {\n    v++;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "v++"),
            "The final variable 'v' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// A `case`-arm binder declared `final` (`case (final a, final b)`)
/// reports on writes in the body.
///
/// Oracle (`variable_A03_t02` test3, `if (r case (final a, name: final b))
/// { a = 1; }`).
#[test]
fn case_pattern_final_binder_write_reports() {
    let fixture =
        "void f(Object r) {\n  if (r case (final a, final b)) {\n    a = 1;\n    b = 2;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (
                at(fixture, "a = 1"),
                "The final variable 'a' can only be set once.".to_string()
            ),
            (
                at(fixture, "b = 2"),
                "The final variable 'b' can only be set once.".to_string()
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A compound write (`a += 1`) to a pattern-declared final reports the
/// same (the read-write intent rides the same check).
///
/// Oracle (`AssignmentToFinalLocalTest/test_localVariable_plusEq`,
/// `x += 2` on `final x = 0`).
#[test]
fn pattern_final_compound_write_reports() {
    let fixture = "void f() {\n  final (a,) = (0,);\n  a += 1;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "a += 1"),
            "The final variable 'a' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

// —— catch parameters (implicitly final) ——

/// `catch (e, s) { e = null; }` — the exception parameter is final.
///
/// Oracle (`try_catch_on_syntax_test_10` testFinal): the write reports
/// `assignment_to_final_local` at `e`.
#[test]
fn catch_exception_parameter_write_reports() {
    let fixture =
        "void f() {\n  try {\n    throw \"x\";\n  } catch (e, s) {\n    e = null;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "e = null"),
            "The final variable 'e' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// The stack-trace parameter is final the same way.
#[test]
fn catch_stack_parameter_write_reports() {
    let fixture =
        "void f() {\n  try {\n    throw \"x\";\n  } catch (e, s) {\n    s = null;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "s = null"),
            "The final variable 's' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

// —— captured writes (the closure face) ——

/// `final x; var f = () { x = 1; };` — the closure's captured write is an
/// error even though it is the variable's only write site (the variable
/// is `potentiallyMutatedInScope`, so `isUnassigned` inside the closure
/// is false). The later straight-line write reports too.
///
/// Oracle (`local_variable_assign_A01_t03`, `x1` arms): both the
/// in-closure `x1 = 42;` and the trailing `x1 = 0;` report.
#[test]
fn closure_captured_write_to_final_reports() {
    let fixture = "void f() {\n  final x;\n  var g = () {\n    x = 1;\n  };\n  x = 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![
            (
                at(fixture, "x = 1"),
                "The final variable 'x' can only be set once.".to_string()
            ),
            (
                at(fixture, "x = 0"),
                "The final variable 'x' can only be set once.".to_string()
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A write inside a nested NAMED function body is a captured write the
/// same way (the function-boundary scan is not closure-specific).
#[test]
fn nested_function_captured_write_to_final_reports() {
    let fixture = "void f() {\n  final x;\n  void g() {\n    x = 1;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "x = 1"),
            "The final variable 'x' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// A write inside a block of the SAME function is not captured — the
/// first straight-line write to `final x;` is the legal one.
#[test]
fn block_write_to_unassigned_final_stays_silent() {
    let fixture = "void f(bool b) {\n  final int x;\n  if (b) {\n    x = 1;\n  }\n  x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

// —— sibling branches (the extra side's flow face) ——

/// `if (b) { x = 1; } else { x = 2; }` — each branch is `x`'s first write
/// on its own path: silent.
///
/// Oracle (`initialize_inside_closure_test`, `assignment_with_final_
/// variables_promotion_test`'s `isInt`/`neqNull` arms): no rows.
#[test]
fn if_else_branch_writes_stay_silent() {
    let fixture = "void f(bool b) {\n  final int x;\n  if (b) {\n    x = 1;\n  } else {\n    x = 2;\n  }\n  x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// The unbraced `if/else` shape resolves identically.
#[test]
fn unbraced_if_else_branch_writes_stay_silent() {
    let fixture =
        "void f(bool b) {\n  final int x;\n  if (b)\n    x = 1;\n  else\n    x = 2;\n  x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A third write on the join still reports — after `if/else` `x` is
/// assigned on every path, so `x = 3` is not a first write.
#[test]
fn write_after_if_else_join_reports() {
    let fixture = "void f(bool b) {\n  final int x;\n  if (b) {\n    x = 1;\n  } else {\n    x = 2;\n  }\n  x = 3;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "x = 3"),
            "The final variable 'x' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

/// `switch` case clauses are alternatives the same way: a first write in
/// each of two cases is legal.
#[test]
fn switch_case_branch_writes_stay_silent() {
    let fixture = "void f(int y) {\n  final int x;\n  switch (y) {\n    case 1:\n      x = 1;\n      break;\n    case 2:\n      x = 2;\n      break;\n  }\n  x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A second write inside the SAME branch is sequential — it reports.
#[test]
fn second_write_same_branch_reports() {
    let fixture = "void f(bool b) {\n  final int x;\n  if (b) {\n    x = 1;\n    x = 2;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        atfl_rows(&report),
        vec![(
            at(fixture, "x = 2"),
            "The final variable 'x' can only be set once.".to_string()
        )],
        "{:#?}",
        rows(&report)
    );
}

// —— constructor initializer writes (the extra side's dominant cluster) ——

/// `C(this.x) : x = 0;` — the initializer LHS resolves to the FIELD, not
/// the initializing formal: `assignment_to_final_local` stays silent (the
/// `field_initialized_*` family owns the verdict).
///
/// Oracle (`test_assignedInInitializer_alreadyInitializingFormal`,
/// `duplicate_final_test_03`, `static_processing_A24_t03`'s `this: x = N`
/// rows): no `assignment_to_final_local` at the LHS name.
#[test]
fn initializer_list_write_stays_silent() {
    let fixture = "class C {\n  int x = 0;\n  C(this.x) : x = x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "the initializer LHS resolves to the field: {:#?}",
        rows(&report)
    );
}

/// A body write to a spelling that shares an initializing formal's name
/// resolves to the field member — `assignment_to_final_local` stays
/// silent (the non-final `int v` field's write is plain).
///
/// Oracle (`static_processing_A25_t01` `C3`, `this { v = 3; }`): no rows.
#[test]
fn ctor_body_write_to_formal_spelling_stays_silent() {
    let fixture = "class C {\n  int v = 0;\n  C(this.v);\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// `super.x` formals read the same: `class B extends A { B(super.x) : x =
/// 0; }` — the LHS is a field write.
#[test]
fn super_formal_initializer_write_stays_silent() {
    let fixture =
        "class A {\n  int x = 0;\n  A(this.x);\n}\nclass B extends A {\n  B(super.x) : x = 0;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

// —— negative controls ——

/// A `var` local never reports: `var a = 0; a = 1;` is a plain write.
#[test]
fn var_local_write_stays_silent() {
    let fixture = "void f() {\n  var a = 0;\n  a = 1;\n  a;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A `var` pattern-declared variable stays writable: `var (a, b) = …;
/// a = 0;` is legal.
#[test]
fn var_pattern_binder_write_stays_silent() {
    let fixture = "void f() {\n  var (a, b) = (1, 2);\n  a = 0;\n  b = 1;\n  a; b;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A non-final parameter's write never reports: `void f(int x) { x = 1; }`.
#[test]
fn parameter_write_stays_silent() {
    let fixture = "void f(int x) {\n  x = 1;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A `late final` local's write family owns its own code — the first
/// write is legal and never reports here.
#[test]
fn late_final_first_write_stays_silent() {
    let fixture = "void f() {\n  late final int x;\n  x = 1;\n  x;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}

/// A write to a FIELD (not a local) never reports here: `this` member
/// writes resolve through the member face.
#[test]
fn field_write_stays_silent() {
    let fixture = "class C {\n  final int x = 0;\n  void m() {\n    x = 1;\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "field writes resolve through the member face: {:#?}",
        rows(&report)
    );
}

/// `final (a, b)` binders keep the unused-variable face working — the
/// write-side fix must not break the read-side rows.
#[test]
fn pattern_decl_still_reads_normally() {
    let fixture = "void f() {\n  final (a, b) = (1, 2);\n  a;\n}\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"assignment_to_final_local"),
        "{:#?}",
        rows(&report)
    );
}
