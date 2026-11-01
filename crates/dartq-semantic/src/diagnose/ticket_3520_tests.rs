//! #3520 pins — `constant_pattern_never_matches_value_type` must run on
//! the RECOVERED constant's type, not only on the literal-armed shapes.
//!
//! Upstream `visitConstantPattern` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:128-165)
//! evaluates the pattern's expression UNCONDITIONALLY: a parser-flagged
//! constant pattern (`const 0`, `~0`, `!false`, `const Class()`, `const
//! <int>[]`, `const {}`, `const #a`, `const int`, `const
//! bool.fromEnvironment`, `1 == 2`) still produces a DartObject whose
//! `value.type` feeds `_canBeEqual(value.type, matchedValueType)`. The
//! literal arming only ever types the `hasPrimitiveEquality` leaves
//! (int/double/String/bool/null literals and enum tails), so the
//! recovered shapes above dropped their never-matches verdict wholesale
//! — the largest miss cluster in the patterns family.
//!
//! The recovered type arrives through two seams:
//!
//! - the `const_fold`-unarmed shapes (`const Class()`, `const <int>[]`,
//!   `const {}`, `const #a`, `const int`, `const bool.fromEnvironment`,
//!   the class-static `Created` tail) re-evaluate the captured
//!   expression root in `class_static_const_tail_ty` — the oracle's
//!   `_evaluateAndReportError` on a window that is still open;
//! - the armed-but-wrong composites (`1 == 2` arms `int` at its last
//!   literal while the parsed expression is `bool`) re-read the folded
//!   `Value` row's static type at the teardown `pending_matched` join.
//!
//! The gate keeps its suppression when the oracle's own gate suppresses:
//! `!0` is a `const_eval_type_bool` fault (no DartObject), `++v` and
//! `switch() {}` evaluate to `NotConstant`, an unresolved reference is
//! the `InvalidType` early-return, and a non-primitive-equality value
//! (`Duration`) never reaches `_canBeEqual`. A pattern inside a DEAD
//! clause never reaches the verifier at all (the dead-node
//! short-circuit), which is the cluster's one extra-row direction.
//!
//! Oracle answers probed against `dart analyze` 3.13.1 and cross-checked
//! with `conformance/expected.tsv` (`invalid_const_pattern_test` /
//! `invalid_const_pattern_binary_test` rows).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// `(code, offset)` rows for the codes these pins assert on — the
/// never-matches face and the parser/verifier faces sharing its cases.
fn rows_of(report: &FileReport, source: &str) -> Vec<(&'static str, String)> {
    const FAMILY: [&str; 7] = [
        "constant_pattern_never_matches_value_type",
        "invalid_constant_const_prefix",
        "invalid_constant_pattern_unary",
        "invalid_constant_pattern_binary",
        "constant_pattern_with_non_constant_expression",
        "const_eval_type_bool",
        "unreachable_switch_case",
    ];
    let mut rows: Vec<(&'static str, String)> = report
        .diagnostics
        .iter()
        .filter(|d| FAMILY.contains(&d.code.name))
        .map(|d| {
            (
                d.code.name,
                source[d.range.offset as usize..d.range.end() as usize].to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The never-matches rows alone: `(anchored text, message)`.
fn never_rows(report: &FileReport, source: &str) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "constant_pattern_never_matches_value_type")
        .map(|d| {
            (
                source[d.range.offset as usize..d.range.end() as usize].to_string(),
                d.message.to_string(),
            )
        })
        .collect()
}

/// Diagnoses `fixture` under [`CORE_PRELUDE`]; the returned source is the
/// prelude-prefixed text the report offsets index into (the rig prepends
/// the prelude itself, so the offsets map 1:1).
fn run(fixture: &str) -> (FileReport, String) {
    (report_ops(fixture), format!("{CORE_PRELUDE}{fixture}"))
}

/// The armed path keeps reporting: `case const !false:` over an `int`
/// scrutinee carries the unary parse error AND the never-matches row —
/// `false` arms `bool`, `bool` can never equal `int`, and the `const`
/// keyword extension anchors the row at the whole `const !false` node.
/// (A bare `!false` anchors at the operand — the oracle's full-node
/// `.at(node)` anchor is the corpus's `const !false` shape.)
///
/// Oracle (`invalid_const_pattern_test` :298 —
/// `invalid_constant_pattern_unary` at `!`,
/// `constant_pattern_never_matches_value_type` at `const !false`
/// len 12, message `'int' … 'bool'`): both rows report.
#[test]
fn const_bang_false_over_int_reports_unary_and_never_matches_bool() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case const !false:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [
            (
                "constant_pattern_never_matches_value_type",
                "const !false".to_string()
            ),
            ("invalid_constant_pattern_unary", "!".to_string()),
        ],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
    let never = never_rows(&report, &source);
    assert_eq!(
        never,
        [(
            "const !false".to_string(),
            "The matched value type 'int' can never be equal to this constant of type 'bool'."
                .to_string()
        )]
    );
}

/// The `~0` shape (`BitNot` on a literal): the unary parse error reports,
/// the recovered `int` matches the `int` scrutinee, so never-matches
/// stays silent — the well-formed-compatible-constant polarity.
///
/// Oracle (`invalid_const_pattern_test` :174): `invalid_constant_pattern_unary`
/// at `~`, nothing else.
#[test]
fn bitnot_zero_over_int_reports_unary_only() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case ~0:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [("invalid_constant_pattern_unary", "~".to_string())],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
}

/// `case !0:` is the fault-side suppression: the oracle's evaluation
/// produces `const_eval_type_bool` (no DartObject), so never-matches
/// never runs even though the unary parse error and the `!` operand's
/// own `non_bool` verdict still report. The literal `0` arms `int`
/// either way, which is why this pin also proves the arming alone could
/// not decide the verdict — the eval fault is what suppresses.
///
/// Oracle: `invalid_constant_pattern_unary` at `!`,
/// `const_eval_type_bool` at `0`, `non_bool_negation_expression` at
/// `0` — and NO `constant_pattern_never_matches_value_type`.
#[test]
fn bang_zero_over_int_reports_fault_but_never_matches() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case !0:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert!(
        never_rows(&report, &source).is_empty(),
        "never-matches must stay silent on the const_eval fault: {:?}",
        report.diagnostics
    );
    let rows = rows_of(&report, &source);
    assert!(
        rows.iter().any(|r| r.0 == "invalid_constant_pattern_unary"),
        "the unary parse error still reports: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.0 == "const_eval_type_bool"),
        "the eval fault still reports: {rows:#?}"
    );
}

/// `case const 0:` — `const` before a primitive literal is the parser's
/// `invalid_constant_const_prefix`, and the recovered `int` matches the
/// `int` scrutinee: the parse error reports WITHOUT never-matches (the
/// third polarity — recovered type genuinely compatible).
///
/// Oracle (`invalid_const_pattern_test` :186 — the error anchors at the
/// expression AFTER `const`, len 1 on `0`): `invalid_constant_const_prefix`
/// at `0`, nothing else.
#[test]
fn const_prefixed_int_over_int_reports_prefix_only() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case const 0:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [("invalid_constant_const_prefix", "0".to_string())],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
}

/// The `const`-prefixed creation/collection/type-literal shapes never arm
/// the literal fold — the recovered-type eval seam owns them. Over an
/// `int` scrutinee every one reports never-matches with the expression's
/// static type, anchored at the `const` keyword (upstream `.at(node)`
/// spans `const` through the expression).
///
/// Oracle (`invalid_const_pattern_test` :316-:360, :468): `const
/// Class()` → `Class`, `const <int>[]` → `List<int>`, `const {}` →
/// `Map<dynamic, dynamic>`, `const #a` → `Symbol`, `const int` →
/// `Type`; each over `int`.
#[test]
fn const_prefixed_creations_and_literals_recover_their_static_type() {
    let fixture = "class Class { const Class(); }\n\
                   void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case const Class():\n\
                   \x20   case const <int>[]:\n\
                   \x20   case const {}:\n\
                   \x20   case const #a:\n\
                   \x20   case const int:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let never = never_rows(&report, &source);
    let anchored: Vec<(String, &str)> = never
        .iter()
        .map(|(text, msg)| (text.clone(), msg.as_str()))
        .collect();
    let want = [
        "const Class()",
        "const <int>[]",
        "const {}",
        "const #a",
        "const int",
    ];
    let got: Vec<&str> = anchored.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        got, want,
        "recovered-constant never-matches rows, all: {:?}",
        report.diagnostics
    );
    // Each message spells the recovered static type against `int`.
    let types: Vec<String> = anchored
        .iter()
        .map(|(_, msg)| {
            msg.rsplit("type '")
                .next()
                .expect("constant type in message")
                .trim_end_matches("'.")
                .to_string()
        })
        .collect();
    assert_eq!(
        types,
        [
            "Class",
            "List<int>",
            "Map<dynamic, dynamic>",
            "Symbol",
            "Type"
        ],
        "{anchored:#?}"
    );
}

/// `const` before a `double` literal over `int`: the prefix error alone —
/// the corpus's `const 0.5` shape carries no never-matches row
/// (`invalid_const_pattern_test` :198 reports
/// `invalid_constant_const_prefix` only).
///
/// Oracle (`invalid_const_pattern_test` :198 — the row anchors at the
/// `1.5` expression after `const`): `invalid_constant_const_prefix` at
/// `1.5`, nothing else.
#[test]
fn const_prefixed_double_over_int_reports_prefix_only() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case const 1.5:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [("invalid_constant_const_prefix", "1.5".to_string())],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
}

/// The armed-but-wrong composite: `case const 1 == 2:` arms `int` at the
/// last literal while the parsed expression evaluates to `bool` — the
/// oracle reads `value.type` on the DartObject, so never-matches spells
/// `bool`. Under the `const` prefix the parse error is
/// `invalid_constant_const_prefix` at the operand `1` (the
/// `invalid_constant_pattern_binary` face is the BARE binary's; the
/// `const` operand parses through the const-expression grammar where `==`
/// is legal).
///
/// Oracle (`invalid_const_pattern_binary_test` :195 — the `const 1 == 2`
/// row over a `T & int` matched face): `invalid_constant_const_prefix`
/// at `1` len 1 AND never-matches at the whole `const 1 == 2` node
/// len 12, message `'…' … 'bool'`; the same verdict over a plain `int`
/// scrutinee.
#[test]
fn const_equality_composite_recovers_bool_not_the_armed_operand() {
    let fixture = "void f(int o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case const 1 == 2:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [
            (
                "constant_pattern_never_matches_value_type",
                "const 1 == 2".to_string()
            ),
            ("invalid_constant_const_prefix", "1".to_string()),
        ],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
    let never = never_rows(&report, &source);
    assert_eq!(
        never,
        [(
            "const 1 == 2".to_string(),
            "The matched value type 'int' can never be equal to this constant of type 'bool'."
                .to_string()
        )]
    );
}

/// `case ++v:` — a non-constant shape: the parser's own
/// `constant_pattern_with_non_constant_expression` is the only row. The
/// recovered expression yields no DartObject (`NotConstant`), so
/// never-matches stays silent.
///
/// Oracle (`invalid_const_pattern_test` :180 — `case ++variable:` the row
/// anchors at the increment's operand, `variable` len 8; the corpus's
/// `o++`/`o--`/`++o`/`--o` family reads the same len-1 anchor): one
/// `constant_pattern_with_non_constant_expression` row at `v`, nothing
/// else.
#[test]
fn non_constant_shape_reports_non_constant_only() {
    let fixture = "void f(int o, int v) {\n\
                   \x20 switch (o) {\n\
                   \x20   case ++v:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert_eq!(
        rows,
        [(
            "constant_pattern_with_non_constant_expression",
            "v".to_string()
        )],
        "{rows:#?}\nall: {:?}",
        report.diagnostics
    );
}

/// The cluster's extra-row direction: a constant pattern inside a DEAD
/// clause never reaches the verifier (the oracle's dead-node
/// short-circuit) — the second `case null:` under a nullable scrutinee
/// reports `unreachable_switch_case` and NOTHING else.
///
/// Oracle (`sealed_class_switch_test` :177 family): `unreachable_switch_case`
/// on the dead clause, no `constant_pattern_never_matches_value_type`.
#[test]
fn dead_clause_constant_pattern_never_reaches_the_verifier() {
    let fixture = "class A {}\n\
                   void f(A? o) {\n\
                   \x20 switch (o) {\n\
                   \x20   case null:\n\
                   \x20   case null:\n\
                   \x20 }\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    let rows = rows_of(&report, &source);
    assert!(
        rows.iter()
            .all(|r| r.0 != "constant_pattern_never_matches_value_type"),
        "the dead clause's never-matches must stay silent: {rows:#?}\nall: {:?}",
        report.diagnostics
    );
    assert!(
        rows.iter().any(|r| r.0 == "unreachable_switch_case"),
        "the dead clause still reports unreachable_switch_case: {rows:#?}"
    );
}
