//! #3630 pins — the S3 const-eval pattern cluster: the operand-constness
//! faces the five `constant_verifier`/`evaluation` decision points answer.
//!
//! Upstream homes (all reachable from `visitMapPattern` /
//! `visitConstantPattern` / `visitRelationalPattern` in
//! `pkg/analyzer/lib/src/dart/constant/constant_verifier.dart`):
//!
//! * `non_constant_relational_pattern_expression` (:410-413) — a
//!   `RelationalPattern`'s operand must be a constant; the row anchors at
//!   the deepest non-constant construct (`_evaluateAndReportError`'s
//!   `InvalidConstant` entity), not at the pattern.
//! * `non_constant_map_pattern_key` (:330-336) — each `MapPatternEntry`
//!   key is a constant context; the row anchors at the key expression.
//! * `equal_keys_in_map_pattern` (:313-355) — `visitMapPattern` collects
//!   every key's `DartObjectImpl` in a `uniqueKeys` table whose comparator
//!   is `identical` for keys without primitive equality and `==` for
//!   primitive-equality keys; the LATER key of an equal pair reports, with
//!   the first key carried as the "The first key with this value."
//!   context message.
//! * `constant_pattern_with_non_constant_expression` (:128-142) — a
//!   `ConstantPattern`'s expression must be a constant.
//! * `pattern_constant_from_deferred_library` (:770, produced by
//!   `evaluation.dart`'s `_getDeferredLibraryError` and selected by the
//!   `SwitchPatternCase` arm of `_invalidConstantForEntity`, :2130-2132) —
//!   a pattern constant reaching its value through a `deferred` import's
//!   prefix; the row anchors at the referenced name and preempts every
//!   other const verdict.
//!
//! Oracle answers below were probed against `dart analyze` 3.13.7 (the
//! pinned oracle) and cross-checked with `conformance/expected.tsv`
//! (`EqualKeysInMapPatternTest`, `NonConstantMapPatternKeyTest`,
//! `NonConstantRelationalPatternExpressionTest`,
//! `ConstantPatternWithNonConstantExpressionTest`, and the co19
//! `Patterns/map_A0*` / `relational_A04_t0*` / `constant_A06_t0*` rows).

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_with};

/// `(code, anchored source text)` rows for the five S3 codes, sorted.
fn rows(report: &FileReport, source: &str) -> Vec<(&'static str, String)> {
    const FAMILY: [&str; 5] = [
        "non_constant_relational_pattern_expression",
        "non_constant_map_pattern_key",
        "equal_keys_in_map_pattern",
        "constant_pattern_with_non_constant_expression",
        "pattern_constant_from_deferred_library",
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

/// The context messages of every `equal_keys_in_map_pattern` row, as
/// `(anchored text, message)` — the oracle's `contextMessages` face.
fn dup_context(report: &FileReport, source: &str) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "equal_keys_in_map_pattern")
        .flat_map(|d| d.context.iter().flatten())
        .map(|c| {
            (
                source[c.range.offset as usize..c.range.end() as usize].to_string(),
                c.message.to_string(),
            )
        })
        .collect()
}

/// Diagnoses `fixture` under the shared prelude; the returned source is the
/// prelude-prefixed text the report offsets index into.
fn run(fixture: &str) -> (FileReport, String) {
    (report_ops(fixture), format!("{CORE_PRELUDE}{fixture}"))
}

/// [`run`] with one sibling library mounted under a prefix. The import
/// directives must lead the unit, so the core prelude is spliced in AFTER
/// the last directive line.
fn run_with(fixture: &str, imports: &[RigImport<'_>]) -> (FileReport, String) {
    let split = fixture
        .lines()
        .take_while(|line| line.trim_start().starts_with("import "))
        .count();
    let head: String = fixture
        .lines()
        .take(split)
        .map(|line| format!("{line}\n"))
        .collect();
    let rest: String = fixture
        .lines()
        .skip(split)
        .map(|line| format!("{line}\n"))
        .collect();
    let source = format!("{head}{CORE_PRELUDE}{rest}");
    (report_ops_with(&source, imports, None), source)
}

// —— equal_keys_in_map_pattern ————————————————————————————————————————

/// `{0: 1, 0: 2}` — the `test_identical_int` shape: two int keys with the
/// same value. Oracle: `equal_keys_in_map_pattern` at the LATER `0`
/// (`3:21`, len 1), context "The first key with this value." at the first
/// `0` (`3:15`).
#[test]
fn duplicate_int_keys_report_at_the_later_key() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {0: 1, 0: 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "0".to_string())],
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        dup_context(&report, &source),
        [(
            "0".to_string(),
            "The first key with this value.".to_string()
        )]
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Two keys in a map pattern can't be equal."
    );
}

/// `{3.14: 1, 3.14: 2}` — the `test_identical_double` shape (the oracle
/// row is at `3:24`, len 4).
#[test]
fn duplicate_double_keys_report() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {3.14: 1, 3.14: 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "3.14".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

/// `{a: 1, b: 2}` over two same-valued `const` variables — the
/// `test_identical_int_viaIdentifier` shape: the keys fold to equal
/// values through the const bindings.
#[test]
fn duplicate_const_variable_keys_report() {
    let fixture = "const a = 0;\n\
                   const b = 0;\n\
                   void f(x) {\n\
                   \x20 if (x case {a: 1, b: 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "b".to_string())],
        "{:#?}",
        report.diagnostics
    );
    let context = dup_context(&report, &source);
    assert_eq!(context.len(), 1, "{context:?}");
    assert_eq!(context[0].0, "a");
}

/// Distinct int keys stay silent (`test_notIdentical_int`).
#[test]
fn distinct_int_keys_stay_silent() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {0: 1, 2: 3}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// `1` and `1.0` are not identical and not `==`-equal under the const
/// kernel (`IntState != DoubleState`), so the pair stays silent — the
/// oracle's `isIdentical2` int/double cross arm answers UNKNOWN, which the
/// `toBoolValue() == true` gate reads as "not equal".
#[test]
fn int_and_double_keys_are_not_equal() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {1: 0, 1.0: 0}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// `{(): 1, (): 2}` — the `test_recordType_primitiveEqual_empty` shape
/// (oracle `3:22`, len 2): two empty record keys with primitive equality
/// compare `==`-equal.
#[test]
fn duplicate_empty_record_keys_report() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {(): 1, (): 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "()".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

/// `{(0,): 1, (0,): 2}` — `test_recordType_primitiveEqual_positional_equal`
/// (oracle `3:24`, len 4): a positional record whose field has primitive
/// equality.
#[test]
fn duplicate_positional_record_keys_report() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {(0,): 1, (0,): 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "(0,)".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

/// `{(a: 0): 1, (a: 0): 2}` — `test_recordType_primitiveEqual_named_equal`
/// (oracle `3:26`, len 6).
#[test]
fn duplicate_named_record_keys_report() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {(a: 0): 1, (a: 0): 2}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("equal_keys_in_map_pattern", "(a: 0)".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

/// Different record shapes never collide (`test_recordType_primitiveEqual_
/// differentShape`: `(0, 1)` vs `(0,)`).
#[test]
fn different_record_shapes_stay_silent() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {(0, 1): 2, (0,): 3}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// Named record keys with different values never collide
/// (`test_recordType_primitiveEqual_named_notEqual`).
#[test]
fn different_named_record_values_stay_silent() {
    let fixture = "void f(x) {\n\
                   \x20 if (x case {(a: 0): 1, (a: 2): 3}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

// —— non_constant_map_pattern_key ————————————————————————————————————

/// `{a: 0}` over a formal parameter — the `test_formalParameter` shape:
/// the row anchors at the key (`3:15`, len 1).
#[test]
fn formal_parameter_key_reports_at_the_key() {
    let fixture = "void f(x, int a) {\n\
                   \x20 if (x case {a: 0}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("non_constant_map_pattern_key", "a".to_string())],
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Key expressions in map patterns must be constants."
    );
}

/// `{A(): 0}` over a class with a const constructor — the
/// `test_instanceCreation_noConst` shape: an IMPLICIT (non-`const`)
/// creation is not a constant expression. Oracle anchors the whole `A()`
/// (`3:15`, len 3).
#[test]
fn implicit_creation_key_reports_at_the_creation() {
    let fixture = "class A {\n\
                   \x20 const A();\n\
                   }\n\
                   void f(x) {\n\
                   \x20 if (x case {A(): 0}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("non_constant_map_pattern_key", "A()".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

/// `{const A(): 0}` — an explicit `const` creation IS a constant key and
/// stays silent.
#[test]
fn explicit_const_creation_key_stays_silent() {
    let fixture = "class A {\n\
                   \x20 const A();\n\
                   }\n\
                   void f(x) {\n\
                   \x20 if (x case {const A(): 0}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// `{a(): 0}` over a function-typed formal — the `test_rewrite_key` shape:
/// the invocation is not a constant (`3:15`, len 3).
#[test]
fn invocation_key_reports() {
    let fixture = "void f(x, bool Function() a) {\n\
                   \x20 if (x case {a(): 0}) {}\n\
                   \x20 }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [("non_constant_map_pattern_key", "a()".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

// —— non_constant_relational_pattern_expression ——————————————————————

/// `case > i` over a mutable local — the co19 `relational_A04_t01` shape:
/// the operand is not a constant (`25:12`, len 1).
#[test]
fn mutable_local_relational_operand_reports() {
    let fixture = "void f(int x) {\n\
                   \x20 int i = 0;\n\
                   \x20 switch (x) {\n\
                   \x20   case > i:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [(
            "non_constant_relational_pattern_expression",
            "i".to_string()
        )],
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The relational pattern expression must be a constant."
    );
}

/// `case == 0 + i` — the compound operand: the row anchors at the
/// non-constant LEAF (`i`), not at the whole operand expression (co19
/// `relational_A04_t01` line 30, `30:17` len 1).
#[test]
fn compound_operand_anchors_at_the_non_constant_leaf() {
    let fixture = "void f(int x) {\n\
                   \x20 int i = 0;\n\
                   \x20 switch (x) {\n\
                   \x20   case == 0 + i:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [(
            "non_constant_relational_pattern_expression",
            "i".to_string()
        )],
        "{:#?}",
        report.diagnostics
    );
}

/// `case > i++` — a postfix increment is not a constant; the oracle anchors
/// the WHOLE `i++` (`35:12`, len 3, co19 `relational_A04_t01`).
#[test]
fn postfix_increment_operand_anchors_the_whole_expression() {
    let fixture = "void f(int x) {\n\
                   \x20 int i = 0;\n\
                   \x20 switch (x) {\n\
                   \x20   case > i++:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [(
            "non_constant_relational_pattern_expression",
            "i++".to_string()
        )],
        "{:#?}",
        report.diagnostics
    );
}

/// `case > 0` — a literal operand is a constant and stays silent (co19
/// `relational_A04_t02`).
#[test]
fn literal_relational_operand_stays_silent() {
    let fixture = "void f(int x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case > 0:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// `case > a` over a `const` variable stays silent (`test_const_*`).
#[test]
fn const_variable_relational_operand_stays_silent() {
    let fixture = "const a = 0;\n\
                   void f(int x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case > a:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

// Residual (not pinned): `case > fn()` / `case > f()` — a CALL operand. The
// oracle reports `const_eval_method_invocation` (top-level function) or the
// site default (a local/formal callee, rewritten to a
// `FunctionExpressionInvocation`). This pass classifies relational operands at
// identifier-resolution time only, so a call operand (whose paren terminates
// the operand scan) reports nothing. Documented under-report.

// —— constant_pattern_with_non_constant_expression ————————————————————

// Residual (not pinned): `case A.a` over a NON-const static field. The oracle
// reports `constant_pattern_with_non_constant_expression` at `A.a`
// (`ConstantPatternWithNonConstantExpressionTest.test_class_field_notConst`,
// `7:14` len 3). The non-const member walk that backs this row is wired to the
// const-INITIALIZER face (`nonconst_member_span`), not to the pattern face, so
// this pass stays silent. Documented under-report.

/// `case A.a` over a CONST static field stays silent.
#[test]
fn const_static_field_pattern_stays_silent() {
    let fixture = "class A {\n\
                   \x20 static const a = 0;\n\
                   }\n\
                   void f(x) {\n\
                   \x20 if (x case A.a) {}\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// `case i` over a local — the `test_localVariable_notConst` shape
/// (`4:14`, len 1).
#[test]
fn local_variable_constant_pattern_reports() {
    let fixture = "void f(x) {\n\
                   \x20 int i = 0;\n\
                   \x20 if (x case i) {}\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [(
            "constant_pattern_with_non_constant_expression",
            "i".to_string()
        )],
        "{:#?}",
        report.diagnostics
    );
}

/// A parenthesized non-constant pattern anchors the inner expression
/// (`case (i)`: oracle `4:15`, len 1).
#[test]
fn parenthesized_constant_pattern_anchors_the_inner_expression() {
    let fixture = "void f(x) {\n\
                   \x20 int i = 0;\n\
                   \x20 if (x case (i)) {}\n\
                   }\n";
    let (report, source) = run(fixture);
    assert_eq!(
        rows(&report, &source),
        [(
            "constant_pattern_with_non_constant_expression",
            "i".to_string()
        )],
        "{:#?}",
        report.diagnostics
    );
}

// —— pattern_constant_from_deferred_library ——————————————————————————

/// A const through a deferred import's prefix: the `test_simple` shape —
/// the row anchors at the referenced NAME (`a.c`'s `c`, `6:12`, len 1) and
/// no other const row fires.
#[test]
fn deferred_prefix_constant_pattern_reports() {
    let fixture = "import 'package:x/a.dart' deferred as a;\n\
                   void f(int e) {\n\
                   \x20 switch (e) {\n\
                   \x20   case a.c:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run_with(
        fixture,
        &[RigImport {
            uri: "package:x/a.dart",
            prefix: Some("a"),
            source: "const int c = 0;\n",
        }],
    );
    assert_eq!(
        rows(&report, &source),
        [("pattern_constant_from_deferred_library", "c".to_string())],
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Constant values from a deferred library can't be used in patterns."
    );
}

/// The same shape without `deferred` stays silent: an ordinary prefixed
/// const is a valid constant pattern.
#[test]
fn plain_prefix_constant_pattern_stays_silent() {
    let fixture = "import 'package:x/a.dart' as a;\n\
                   void f(int e) {\n\
                   \x20 switch (e) {\n\
                   \x20   case a.c:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run_with(
        fixture,
        &[RigImport {
            uri: "package:x/a.dart",
            prefix: Some("a"),
            source: "const int c = 0;\n",
        }],
    );
    assert_eq!(rows(&report, &source), [], "{:#?}", report.diagnostics);
}

/// A deferred type literal in a constant pattern
/// (`test_simple_typeLiteral`: `case a.A:` over a deferred-imported
/// class).
#[test]
fn deferred_prefix_type_literal_reports() {
    let fixture = "import 'package:x/a.dart' deferred as a;\n\
                   void f(Object? x) {\n\
                   \x20 switch (x) {\n\
                   \x20   case a.A:\n\
                   \x20     break;\n\
                   \x20 }\n\
                   }\n";
    let (report, source) = run_with(
        fixture,
        &[RigImport {
            uri: "package:x/a.dart",
            prefix: Some("a"),
            source: "class A {}\n",
        }],
    );
    assert_eq!(
        rows(&report, &source),
        [("pattern_constant_from_deferred_library", "A".to_string())],
        "{:#?}",
        report.diagnostics
    );
}

// Residual (not pinned): `const (a.c + 1)` — the `test_nested` shape. The
// deferred reference sits inside a parenthesized compound constant; the
// capture's parenthesized-expression node does not retain the inner member
// tree this pass walks, so the row is not reached. The two simple shapes
// above (`a.c`, `a.A`) are pinned. Documented under-report.
