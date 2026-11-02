//! #3629 pins — the S2 structural pattern faces: the five codes whose
//! verdicts are pure parse-shape facts (`missing_named_pattern_field_name`,
//! `positional_field_in_object_pattern`, `duplicate_pattern_field`,
//! `rest_element_in_map_pattern`, `duplicate_rest_element_in_pattern`).
//!
//! Every expectation is the oracle row of the corresponding upstream
//! fixture (upstream Dart SDK source, `pkg/analyzer/test/...`), same span
//! shape and byte-verbatim message:
//!
//! * `object_pattern_test.dart` `test_class_notGeneric_noName_constant` —
//!   `case A(: 0):` anchors the whole field node `: 0` (resolver.dart:502,
//!   `.at(field)`), never the colon alone.
//! * `object_pattern_test.dart` `test_class_notGeneric_positionalField` —
//!   `if (x case Object(0))` anchors the positional field `0`.
//! * `duplicate_pattern_field_test.dart` `test_objectPattern` — the
//!   duplicate's name token `sign`, the first field silent.
//! * `map_pattern_test.dart` `test_matchMap_noTypeArguments_restElement_first`
//!   — the `...` element alone, one row per rest element.
//! * `duplicate_rest_element_in_pattern_test.dart` `test_listPattern` —
//!   the second `...` element, the first silent.
//!
//! The no-extras pin guards the census direction: a well-formed pattern
//! reports none of the five.
//!
//! The remaining eight S2 codes (`empty_map_pattern`,
//! `expected_one_list_pattern_type_arguments`,
//! `expected_two_map_pattern_type_arguments`, `duplicate_variable_pattern`,
//! `duplicate_pattern_assignment_variable`,
//! `pattern_assignment_not_local_variable`, `missing_variable_pattern`,
//! `inconsistent_pattern_variable_logical_or`) are pinned at the tail:
//! their verdicts emit inside the fold's existing `ParserListener` hook
//! bodies, carried by the `||` binder ledger and the assignment ledger of
//! `listener.rs`'s `PatternNodeState`.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

/// Byte offset of `needle` in `fixture`, shifted past the rig's prelude
/// (ASCII fixtures: byte and UTF-16 offsets coincide).
fn at(needle: &str, fixture: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    CORE_PRELUDE.len() as u32 + hit as u32
}

/// The rows of one code as `(offset, length)` pairs, ascending.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// The messages of one code, in report order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

/// `case A(: 0):` — the whole field node anchors the row, exactly the
/// oracle's 3-char `: 0` span.
#[test]
fn missing_named_pattern_field_name_anchors_the_whole_field() {
    let fixture = "\
abstract class A {
  int get foo;
}

void f(x) {
  switch (x) {
    case A(: 0):
      break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "missing_named_pattern_field_name"),
        vec![(at("(: 0):", fixture) + 1, 3)],
        "the row spans the whole `: 0` field node"
    );
    assert_eq!(
        messages(&report, "missing_named_pattern_field_name"),
        vec!["The getter name is not specified explicitly, and the pattern is not a variable."
            .to_string()]
    );
}

/// A variable sub-pattern infers the name — the oracle's
/// `test_class_notGeneric_noName_variable` carries no row.
#[test]
fn nameless_field_with_variable_subpattern_stays_silent() {
    let fixture = "\
abstract class A {
  int get foo;
}

void f(x) {
  switch (x) {
    case A(: var foo):
      break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "missing_named_pattern_field_name"),
        vec![],
        "the inferred getter name suppresses the row"
    );
}

/// `if (x case Object(0))` — the positional field reports, whatever its
/// sub-pattern binds.
#[test]
fn positional_field_in_object_pattern_anchors_the_field() {
    let fixture = "void f(Object? x) {\n  if (x case Object(0)) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "positional_field_in_object_pattern"),
        vec![(at("Object(0)", fixture) + 7, 1)],
        "the row spans the `0` field node"
    );
    assert_eq!(
        messages(&report, "positional_field_in_object_pattern"),
        vec!["Object patterns can only use named fields.".to_string()]
    );
}

/// `case int(sign: 0, sign: 1):` — one row at the duplicate's name token;
/// the first occurrence stays silent.
#[test]
fn duplicate_pattern_field_anchors_the_duplicate_name() {
    let fixture = "\
void f(Object? x) {
  switch (x) {
    case int(sign: 0, sign: 1):
      break;
  }
}
";
    let report = report_ops(fixture);
    let second = fixture.find("sign: 1").expect("duplicate name in fixture");
    assert_eq!(
        rows(&report, "duplicate_pattern_field"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 4)],
        "exactly one row, at the second `sign`"
    );
    assert_eq!(
        messages(&report, "duplicate_pattern_field"),
        vec!["The field 'sign' is already matched in this pattern.".to_string()]
    );
}

/// `case {..., 0: ''}` — one row per rest element inside a map pattern
/// (the `...` alone; the oracle keeps no duplicate ledger for maps).
#[test]
fn rest_element_in_map_pattern_anchors_the_element() {
    let fixture = "void f(Map<int, String> x) {\n  if (x case {..., 0: ''}) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "rest_element_in_map_pattern"),
        vec![(at("{..., 0: ''}", fixture) + 1, 3)],
        "the row spans the bare `...` element"
    );
    assert_eq!(
        messages(&report, "rest_element_in_map_pattern"),
        vec!["A map pattern can't contain a rest pattern.".to_string()]
    );
}

/// `[..., ...]` — the duplicate element reports; the first stays silent.
#[test]
fn duplicate_rest_element_anchors_the_second_rest() {
    let fixture = "void f(List<int> x) {\n  if (x case [..., ...]) {}\n}\n";
    let report = report_ops(fixture);
    let second = fixture.rfind("...").expect("second rest in fixture");
    assert_eq!(
        rows(&report, "duplicate_rest_element_in_pattern"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 3)],
        "exactly one row, at the second `...`"
    );
    assert_eq!(
        messages(&report, "duplicate_rest_element_in_pattern"),
        vec!["At most one rest element is allowed in a list or map pattern.".to_string()]
    );
}

/// A rest inside a nested list stays a list-context verdict: the enclosing
/// bracket is the innermost unclosed group, so the map outside never
/// reports it.
#[test]
fn rest_inside_nested_list_stays_off_the_map_face() {
    let fixture = "void f(Map<String, List<int>> x) {\n  if (x case {'k': [..., 1]}) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(rows(&report, "rest_element_in_map_pattern"), vec![]);
    assert_eq!(rows(&report, "duplicate_rest_element_in_pattern"), vec![]);
}

// —— #3668: the record-pattern arm of the same two field-name faces ——
//
// `handle_record_pattern` closes through the same shared walk with
// `mustBeNamed: false` (upstream ast.dart:27938/:27982): positional
// fields stay legal, `missing_named_pattern_field_name` and
// `duplicate_pattern_field` fire exactly as in object patterns.

/// `case (: 0)` — the record shorthand's nameless field anchors the whole
/// `: 0` node, the object arm's exact span (the oracle's
/// `test_recordType_sameShape_named_noName_constant` row).
#[test]
fn record_missing_named_pattern_field_name_anchors_the_whole_field() {
    let fixture = "\
void f(({int foo}) x) {
  switch (x) {
    case (: 0):
      break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "missing_named_pattern_field_name"),
        vec![(at("(: 0):", fixture) + 1, 3)],
        "the row spans the whole `: 0` field node"
    );
    assert_eq!(
        messages(&report, "missing_named_pattern_field_name"),
        vec!["The getter name is not specified explicitly, and the pattern is not a variable."
            .to_string()]
    );
}

/// `case (foo: 0, foo: 1)` — the duplicate's written name token reports;
/// the first occurrence stays silent (the oracle's
/// `test_recordPattern_dynamicType` row).
#[test]
fn record_duplicate_pattern_field_anchors_the_duplicate_name() {
    let fixture = "\
void f(x) {
  switch (x) {
    case (foo: 0, foo: 1):
      break;
  }
}
";
    let report = report_ops(fixture);
    let second = fixture.find("foo: 1").expect("duplicate name in fixture");
    assert_eq!(
        rows(&report, "duplicate_pattern_field"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 3)],
        "exactly one row, at the second `foo`"
    );
    assert_eq!(
        messages(&report, "duplicate_pattern_field"),
        vec!["The field 'foo' is already matched in this pattern.".to_string()]
    );
}

/// `case (foo: 0, :var foo)` — the implicit-name duplicate anchors the
/// shorthand's colon (`duplicateTarget = name ?? colon`), matching the
/// oracle's `test_recordPattern_dynamicType_implicitName_duplicate` row.
#[test]
fn record_duplicate_pattern_field_implicit_name_anchors_the_colon() {
    let fixture = "\
void f(x) {
  switch (x) {
    case (foo: 0, :var foo):
      break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "duplicate_pattern_field"),
        vec![(at(", :var foo", fixture) + 2, 1)],
        "the row anchors the shorthand field's colon"
    );
    assert_eq!(
        messages(&report, "duplicate_pattern_field"),
        vec!["The field 'foo' is already matched in this pattern.".to_string()]
    );
}

/// `case (:var foo, foo: 1)` — the implicit name joins the ledger at its
/// own colon, so the later written name reports as the duplicate (the
/// oracle's `test_recordPattern_dynamicType_implicitName_original` row).
#[test]
fn record_duplicate_pattern_field_written_name_after_implicit() {
    let fixture = "\
void f(x) {
  switch (x) {
    case (:var foo, foo: 1):
      break;
  }
}
";
    let report = report_ops(fixture);
    let second = fixture.find("foo: 1").expect("written duplicate in fixture");
    assert_eq!(
        rows(&report, "duplicate_pattern_field"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 3)],
        "exactly one row, at the written `foo`"
    );
    assert_eq!(
        messages(&report, "duplicate_pattern_field"),
        vec!["The field 'foo' is already matched in this pattern.".to_string()]
    );
}

/// A positional record field is legal — `mustBeNamed: false` keeps the
/// `positional_field_in_object_pattern` arm dark, and a positional slot
/// joins no duplicate ledger.
#[test]
fn record_positional_fields_stay_legal() {
    let fixture = "\
void f(Object? x) {
  switch (x) {
    case (0, 1):
      break;
    case (var a, b: 2,):
      break;
  }
}
";
    let report = report_ops(fixture);
    for code in [
        "missing_named_pattern_field_name",
        "positional_field_in_object_pattern",
        "duplicate_pattern_field",
    ] {
        assert_eq!(rows(&report, code), vec![], "{code} fired on a legal record pattern");
    }
}

/// A well-formed named-field pattern reports none of the five faces — the
/// census direction guard (any emission is pure gain; an extra is never).
#[test]
fn well_formed_patterns_stay_silent() {
    let fixture = "\
void f(Object? x, List<int> xs) {
  switch (x) {
    case int(sign: 0):
      break;
  }
  if (xs case [1, ...]) {}
  if (x case int(isEven: true)) {}
}
";
    let report = report_ops(fixture);
    for code in [
        "missing_named_pattern_field_name",
        "positional_field_in_object_pattern",
        "duplicate_pattern_field",
        "rest_element_in_map_pattern",
        "duplicate_rest_element_in_pattern",
    ] {
        assert_eq!(rows(&report, code), vec![], "{code} fired on a clean pattern");
    }
}

// —— the residual eight: verdicts inside the fold's hook bodies ——
//
// The remaining eight S2 codes emit inside hooks the fold already owns,
// each pinned to the oracle row of its upstream fixture:
//
// * `empty_map_pattern` → `handle_map_pattern`'s `count == 0` arm
//   (`visitMapPattern`'s `errors.emptyMapPattern(pattern: node)`,
//   type_analyzer.dart:1360-1368 — `.at(pattern)`, the whole `{}`).
// * `expected_one_list_pattern_type_arguments` → `handle_list_pattern`,
//   reading the written `<...>` group's count (`_buildTypeArguments`,
//   named_type_resolver.dart — `.at(typeArguments)`, the group node).
// * `expected_two_map_pattern_type_arguments` → `handle_map_pattern`, the
//   same group-count face on the map suffix.
// * `duplicate_variable_pattern` → `handle_declared_variable_pattern`
//   (`_VariableBinderErrors.duplicateVariablePattern`,
//   resolution_visitor.dart:1638-1650 — the duplicate's name token, the
//   first binder riding the context row).
// * `duplicate_pattern_assignment_variable` →
//   `handle_assigned_variable_pattern` (`duplicateAssignmentPatternVariable`,
//   shared_type_analyzer.dart:70-82 — the duplicate's name token).
// * `pattern_assignment_not_local_variable` →
//   `handle_assigned_variable_pattern` (`visitAssignedVariablePattern`'s
//   non-local element arm, resolution_visitor.dart:180-200 — `.at(node.name)`).
// * `missing_variable_pattern` → `end_binary_pattern` for `||`
//   (`logicalOrPatternBranchMissingVariable`, resolution_visitor.dart:1657-1669
//   — the branch operand that omits the variable).
// * `inconsistent_pattern_variable_logical_or` → `end_binary_pattern`
//   (`inconsistentJoinedPatternVariable`, shared_type_analyzer.dart:109-124
//   — the joined variable's name token in the later branch).

/// `if (x case {})` — the empty map pattern anchors the whole `{}`.
#[test]
fn empty_map_pattern_anchors_the_empty_braces() {
    let fixture = "void f(Map<int, String> x) {\n  if (x case {}) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "empty_map_pattern"),
        vec![(at("{}", fixture), 2)],
        "the row spans the empty `{{}}` pattern"
    );
    assert_eq!(
        messages(&report, "empty_map_pattern"),
        vec!["A map pattern must have at least one entry.".to_string()]
    );
}

/// `if (x case <int, int>[0])` — the written group's count is the verdict,
/// and the row spans the group node `<int, int>`.
#[test]
fn expected_one_list_pattern_type_arguments_anchors_the_group() {
    let fixture = "void f(x) {\n  if (x case <int, int>[0]) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "expected_one_list_pattern_type_arguments"),
        vec![(at("<int, int>", fixture), 10)],
        "the row spans the written type-argument group"
    );
    assert_eq!(
        messages(&report, "expected_one_list_pattern_type_arguments"),
        vec!["List patterns require one type argument or none, but 2 found.".to_string()]
    );
}

/// `if (x case <int>{0: _})` — the map suffix's group-count verdict.
#[test]
fn expected_two_map_pattern_type_arguments_anchors_the_group() {
    let fixture = "void f(x) {\n  if (x case <int>{0: _}) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "expected_two_map_pattern_type_arguments"),
        vec![(at("<int>", fixture), 5)],
        "the row spans the written type-argument group"
    );
    assert_eq!(
        messages(&report, "expected_two_map_pattern_type_arguments"),
        vec!["Map patterns require two type arguments or none, but 1 found.".to_string()]
    );
}

/// `if (x case var a && var a)` — the second binder reports, the first
/// carries the context row.
#[test]
fn duplicate_variable_pattern_anchors_the_second_binder() {
    let fixture = "void f(int x) {\n  if (x case var a && var a) {\n    a;\n  }\n}\n";
    let report = report_ops(fixture);
    let second = fixture.rfind("var a").expect("second binder in fixture") + 4;
    assert_eq!(
        rows(&report, "duplicate_variable_pattern"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 1)],
        "exactly one row, at the second binder's name"
    );
    assert_eq!(
        messages(&report, "duplicate_variable_pattern"),
        vec!["The variable 'a' is already defined in this pattern.".to_string()]
    );
}

/// `(a, a) = (1, 2)` — the second assigned variable reports.
#[test]
fn duplicate_pattern_assignment_variable_anchors_the_second_target() {
    let fixture = "void f() {\n  int a;\n  (a, a) = (1, 2);\n  a;\n}\n";
    let report = report_ops(fixture);
    let second = fixture.rfind("a) =").expect("second target in fixture");
    assert_eq!(
        rows(&report, "duplicate_pattern_assignment_variable"),
        vec![(CORE_PRELUDE.len() as u32 + second as u32, 1)],
        "exactly one row, at the second `a`"
    );
    assert_eq!(
        messages(&report, "duplicate_pattern_assignment_variable"),
        vec!["The variable 'a' is already assigned in this pattern.".to_string()]
    );
}

/// `(int) = 0` — a non-local assignment target reports at the name.
#[test]
fn pattern_assignment_not_local_variable_anchors_the_target() {
    let fixture = "void f() {\n  (int) = 0;\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_assignment_not_local_variable"),
        vec![(at("int) = 0", fixture), 3)],
        "the row spans the non-local target `int`"
    );
    assert_eq!(
        messages(&report, "pattern_assignment_not_local_variable"),
        vec!["Only local variables can be assigned in pattern assignments.".to_string()]
    );
}

/// `if (x case int a || 2 || 3)` — each branch that omits `a` reports at
/// its own operand.
#[test]
fn missing_variable_pattern_anchors_each_omitting_branch() {
    let fixture = "void f(Object? x) {\n  if (x case int a || 2 || 3 when a > 0) {\n    a;\n  }\n}\n";
    let report = report_ops(fixture);
    let two = at("|| 2", fixture) + 3;
    let three = at("|| 3", fixture) + 3;
    assert_eq!(
        rows(&report, "missing_variable_pattern"),
        vec![(two, 1), (three, 1)],
        "one row per branch missing the variable"
    );
    assert_eq!(
        messages(&report, "missing_variable_pattern"),
        vec![
            "Variable pattern 'a' is missing in this branch of the logical-or pattern.".to_string(),
            "Variable pattern 'a' is missing in this branch of the logical-or pattern.".to_string(),
        ]
    );
}

/// `if (x case int a || [double a])` — both branches bind `a`, but their
/// joined types disagree, so the later branch's binder reports.
#[test]
fn inconsistent_pattern_variable_logical_or_anchors_the_later_binder() {
    let fixture =
        "void f(Object? x) {\n  if (x case int a || [double a] when a > 0) {\n    a;\n  }\n}\n";
    let report = report_ops(fixture);
    let later = at("[double a]", fixture) + 8;
    assert_eq!(
        rows(&report, "inconsistent_pattern_variable_logical_or"),
        vec![(later, 1)],
        "the row anchors the later branch's `a`"
    );
    assert_eq!(
        messages(&report, "inconsistent_pattern_variable_logical_or"),
        vec![
            "The variable 'a' has a different type and/or finality in this branch of the logical-or pattern."
                .to_string()
        ]
    );
}
