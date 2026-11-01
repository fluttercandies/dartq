//! The #3519 pins — the guarded-pattern scope faces.
//!
//! # The faces
//!
//! * `pattern_type_mismatch_in_irrefutable_context`: every node of a
//!   `PatternVariableDeclarationStatement`'s irrefutable pattern asks
//!   `matched <: required`, the initializer's static type descending
//!   through record slots / list elements / map values / object getter
//!   columns / the cast target. Oracle-verified anchors (3.13.7): a typed
//!   leaf reports at `T`'s start through the name (`var (int a) = 's'` at
//!   the `int a` span), a record node at `(`, a list at `<`/`[`, a map at
//!   `<`/`{`, an object node at the type head; parenthesized nodes are
//!   transparent.
//! * Relational operand scope (`case > x:`): the operand resolves in the
//!   scope enclosing the pattern — a same-head pattern variable reports
//!   `referenced_before_declaration`, a provably non-constant operand
//!   `non_constant_relational_pattern_expression`.
//! * Guard/shared scope: `when` writes to a guarded pattern variable
//!   report `pattern_variable_assignment_inside_guard`; a body read of a
//!   shared-body group's inconsistently joined variable reports
//!   `invalid_pattern_variable_in_shared_case_scope` (all three official
//!   reason spellings share the one `uniqueName`).
//!
//! Every expectation was checked against the oracle
//! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`); each pin names
//! the corpus row it encodes.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// The rows of `code` in `report` as `@offset` spans, fixture-relative
/// (the rig prepends [`super::test_rig::CORE_PRELUDE`], so the raw
/// report offset shifts by the prelude's UTF-16 length — every
/// expectation below counts from the fixture's first byte).
fn offsets_of(report: &super::FileReport, code: &str) -> Vec<String> {
    let base = super::test_rig::CORE_PRELUDE.len() as u32;
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| format!("@{}+{}", d.range.offset - base, d.range.length))
        .collect()
}

// —— face 1: pattern_type_mismatch_in_irrefutable_context ——

/// `test_declaredVariablePattern_valueSupertype`: the leaf `int a`'s
/// span, `num` not assignable to `int`.
#[test]
fn irrefutable_typed_leaf() {
    let report = report_ops(
        "void f(num x) {\n  var (int a) = x;\n}\n",
    );
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@23+5".to_string()],
        "{:#?}",
        rows(&report)
    );
}

/// `test_declaredVariablePattern_valueSubtype` /
/// `test_declaredVariablePattern_valueDynamic`: assignable matched types
/// and `dynamic` stay silent.
#[test]
fn irrefutable_typed_leaf_agreeing() {
    for source in [
        "void f(int x) {\n  var (num a) = x;\n}\n",
        "void f(dynamic x) {\n  var (int a) = x;\n}\n",
        "void f(String x) {\n  var (int a) = 1;\n}\n",
    ] {
        let report = report_ops(source);
        assert!(
            !code_names(&report)
                .contains(&"pattern_type_mismatch_in_irrefutable_context"),
            "{source:?} rows: {:#?}",
            rows(&report)
        );
    }
}

/// `test_recordPattern_notRecord` / `test_recordPattern_record_differentShape`:
/// the record node reports `(Object?,)`-shape at `(`.
#[test]
fn irrefutable_record() {
    let report = report_ops("void f(Object x) {\n  var (a,) = x;\n}\n");
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@25+4".to_string()],
        "{:#?}",
        rows(&report)
    );
}

/// `test_listPattern_notList` / `test_listPattern_differentList`: the
/// list node reports `List<Object?>` / `List<int>` at `[`/`<`.
#[test]
fn irrefutable_list() {
    let report = report_ops("void f(Object x) {\n  var [a] = x;\n}\n");
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@25+3".to_string()],
        "{:#?}",
        rows(&report)
    );
    let report = report_ops("void f(List<Object> x) {\n  var <int>[a] = x;\n}\n");
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@31+8".to_string()],
        "{:#?}",
        rows(&report)
    );
}

/// `test_mapPattern_notMap`: the map node reports `Map<int, String>` at
/// the type-argument group's `<`.
#[test]
fn irrefutable_map() {
    let report = report_ops("void f(Object x) {\n  var <int, String>{0: a} = x;\n}\n");
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@25+19".to_string()],
        "{:#?}",
        rows(&report)
    );
}

/// `test_objectPattern_differentClass` / `doc_0`: the object node reports
/// its written head at the head's start.
#[test]
fn irrefutable_object() {
    let report = report_ops("void f(Object x) {\n  var String(length: a) = x;\n}\n");
    assert_eq!(
        offsets_of(&report, "pattern_type_mismatch_in_irrefutable_context"),
        vec!["@25+17".to_string()],
        "{:#?}",
        rows(&report)
    );
}

// —— face 2: relational operand scope ——

/// `test_notConst_formalParameter` / `test_notConst_topLevelVariable` /
/// `test_variables_scope`: the non-constant operand reports at its own
/// span; a same-head pattern variable adds `referenced_before_declaration`.
#[test]
fn relational_operand_nonconst() {
    let report = report_ops("void f(x, int a) {\n  if (x case > a) {}\n}\n");
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|c| **c == "non_constant_relational_pattern_expression")
            .count(),
        1,
        "{:#?}",
        rows(&report)
    );
    let report = report_ops("final a = 0;\nvoid f(x) {\n  if (x case > a) {}\n}\n");
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|c| **c == "non_constant_relational_pattern_expression")
            .count(),
        1,
        "{:#?}",
        rows(&report)
    );
    // `const` spellings stay silent (local, top-level, literal).
    for source in [
        "void f(x) {\n  const a = 0;\n  if (x case > a) {}\n}\n",
        "const a = 0;\nvoid f(x) {\n  if (x case > a) {}\n}\n",
        "void f(x) {\n  if (x case > 0) {}\n}\n",
    ] {
        let report = report_ops(source);
        assert!(
            !code_names(&report)
                .contains(&"non_constant_relational_pattern_expression"),
            "{source:?} rows: {:#?}",
            rows(&report)
        );
    }
}

/// `test_variables_scope` (if_element / if_statement / switch_expression /
/// switch_statement): `case [int a, == a]` reports BOTH codes at `a`.
#[test]
fn relational_operand_self_reference() {
    let report = report_ops(
        "const a = 0;\nvoid f(Object? x) {\n  switch (x) {\n    case [int a, == a] when a > 0:\n      a;\n  }\n}\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"non_constant_relational_pattern_expression"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        names.contains(&"referenced_before_declaration"),
        "{:#?}",
        rows(&report)
    );
}

// —— face 3a: pattern_variable_assignment_inside_guard ——

/// `doc_0` / `test_expression_assignment` family: the write inside the
/// guard reports at the target; a `then`-body write and a plain-local
/// write stay silent.
#[test]
fn guard_assignment() {
    for source in [
        "void f(int x) {\n  if (x case var a when (a = 1) > 0) {}\n}\n",
        "void f(int x) {\n  if (x case var a when (a += 1) > 0) {}\n}\n",
        "void f(int x) {\n  if (x case var a when (a++) > 0) {}\n}\n",
        "void f(int x) {\n  if (x case var a when (++a) > 0) {}\n}\n",
        "void f(int x) {\n  if (x case int a || int a when (a = 1) > 0) {}\n}\n",
    ] {
        let report = report_ops(source);
        assert_eq!(
            code_names(&report)
                .iter()
                .filter(|c| **c == "pattern_variable_assignment_inside_guard")
                .count(),
            1,
            "{source:?} rows: {:#?}",
            rows(&report)
        );
    }
    // A write in the `then` body, and a write to a non-pattern local
    // inside the guard, both stay silent.
    for source in [
        "void f(int x) {\n  if (x case var a when a > 0) {\n    a = 0;\n  }\n}\n",
        "void f(int x) {\n  var b = 0;\n  if (x case var a when (b = 1) > 0) {}\n}\n",
    ] {
        let report = report_ops(source);
        assert!(
            !code_names(&report)
                .contains(&"pattern_variable_assignment_inside_guard"),
            "{source:?} rows: {:#?}",
            rows(&report)
        );
    }
}

// —— face 3b: invalid_pattern_variable_in_shared_case_scope ——

/// `doc_0` / `declareFirst` / `declareSecond`: a variable declared in
/// some but not all heads reports `not_all_cases` at the body read.
#[test]
fn shared_scope_not_all_cases() {
    let report = report_ops(
        "void f(Object? x) {\n  switch (x) {\n    case int a when a > 0:\n    case 0:\n      a;\n  }\n}\n",
    );
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|c| **c == "invalid_pattern_variable_in_shared_case_scope")
            .count(),
        1,
        "{:#?}",
        rows(&report)
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_pattern_variable_in_shared_case_scope")
        .expect("row");
    assert!(
        row.message.contains("is available in some, but not all cases"),
        "{}",
        row.message
    );
}

/// `doc_1` / `doc_2` / `hasDefault` / `hasLabel`: a `default:` member or
/// a member label reports the label reason.
#[test]
fn shared_scope_has_label() {
    for (source, want) in [
        (
            "void f(Object? x) {\n  switch (x) {\n    case int a when a > 0:\n    default:\n      a;\n  }\n}\n",
            "is not available because there is a label or 'default' case",
        ),
        (
            "void f(Object? x) {\n  switch (x) {\n    case int a when a > 0:\n    someLabel:\n    case 0:\n      a;\n  }\n}\n",
            "is not available because there is a label or 'default' case",
        ),
    ] {
        let report = report_ops(source);
        let row = report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "invalid_pattern_variable_in_shared_case_scope")
            .unwrap_or_else(|| panic!("{source:?} rows: {:#?}", rows(&report)));
        assert!(row.message.contains(want), "{} — {source:?}", row.message);
    }
}

/// `doc_3` / `doc_4` / `differentFinality` / `differentTypes`: same name
/// in every head but divergent type or finality reports the NORM reason.
#[test]
fn shared_scope_different_finality_or_type() {
    for source in [
        "void f(Object? x) {\n  switch (x) {\n    case int a when a < 0:\n    case num a when a > 0:\n      a;\n  }\n}\n",
        "void f(Object? x) {\n  switch (x) {\n    case final int a when a < 0:\n    case int a when a > 0:\n      a;\n  }\n}\n",
    ] {
        let report = report_ops(source);
        let row = report
            .diagnostics
            .iter()
            .find(|d| d.code.name == "invalid_pattern_variable_in_shared_case_scope")
            .unwrap_or_else(|| panic!("{source:?} rows: {:#?}", rows(&report)));
        assert!(
            row.message
                .contains("doesn't have the same type and/or finality"),
            "{} — {source:?}",
            row.message
        );
    }
}
