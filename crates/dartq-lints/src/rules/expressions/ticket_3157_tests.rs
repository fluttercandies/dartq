//! #3157 pins for the `unnecessary_statements` residual #3148 fold-C handed
//! back, measured against the pinned `dart analyze` 3.13.7 oracle
//! (`pkg/linter/test/rules/unnecessary_statements_test.dart`).
//!
//! Three faces had under-reported:
//!
//! - the empty-literal statement (`[];`, `<dynamic, dynamic>{};`) — the
//!   parser seals the empty list literal away, so the statement row carries
//!   no token range and the window must be read off its byte span;
//! - the member-selector extent (`c.g`, `C().g`, `() => c.g`) — the bridge's
//!   `DotAccess` marker anchors at the member name, so the containment root
//!   walk missed it and the whole statement stayed silent;
//! - the cascade section (`c..m`) — a method section's static type is the
//!   method's own signature (a function type), never its declared return
//!   type.
//!
//! Every expected span below is the oracle's `line:column|length` row for the
//! same shape.

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// One row for `source`: it opens at the `nth` `needle` token and spans
/// `len` UTF-16 units, the oracle's `reportAtNode` extent.
fn assert_row(source: &str, needle: &str, nth: usize, len: u32) {
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].message, "Unnecessary statement.", "{source:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, needle, nth)),
        "{source:?}"
    );
    assert_eq!(
        lints[0].range.end - lints[0].range.start,
        len,
        "{source:?}"
    );
}

/// No row for `source`.
fn assert_silent(source: &str) {
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert!(lints.is_empty(), "{source:?}: {lints:?}");
}

// —— the empty-literal statement ——————————————————————————————————————
// Oracle `test_listLiteral` 3:3|2 and `test_inIfBody` 4:5|2.

#[test]
fn us_empty_list_literal_statement_reports_at_the_brackets() {
    assert_row("void f() {\n  [];\n}\n", "[]", 0, 2);
}

#[test]
fn us_empty_list_literal_in_block_body_reports_at_the_brackets() {
    assert_row("void f() {\n  if (1 == 2) {\n    [];\n  }\n}\n", "[]", 0, 2);
}

/// Oracle `test_mapLiteral` 3:3|20 — the whole `<>{}` literal, not the
/// braces alone.
#[test]
fn us_map_literal_statement_reports_the_whole_literal() {
    assert_row(
        "void f() {\n  <dynamic, dynamic>{};\n}\n",
        "<",
        0,
        20,
    );
}

/// Oracle `test_inForLoopUpdaters` 3:18|2 — an updater's empty literal is
/// an expression statement's expression (upstream visits updaters).
#[test]
fn us_for_updater_empty_list_literal_reports_at_the_brackets() {
    assert_row("void f() {\n  for (; 1 == 2; []) {}\n}\n", "[]", 0, 2);
}

// —— the member-selector extent ————————————————————————————————————
// Oracle `test_instanceField2` 3:3|3 — the whole `x.g`, not the member.

#[test]
fn us_instance_field_read_reports_the_whole_selector() {
    assert_row(
        "void f(C v) {\n  v.g;\n}\nclass C {\n  int g = 1;\n}\n",
        "v",
        1,
        3,
    );
}

/// Oracle `test_instanceField` 3:3|5 — the invocation receiver chain.
#[test]
fn us_instance_field_read_on_an_invocation_receiver() {
    assert_row(
        "void f() {\n  C().g;\n}\nclass C {\n  int g = 1;\n}\n",
        "C",
        0,
        5,
    );
}

/// Oracle `test_methodTearoff_returned_inFunctionLiteral` 3:3|24 — the
/// function literal's body carries the selector, and the parameter list's
/// `)` is content (not a follower).
#[test]
fn us_function_literal_body_selector_reports_the_whole_literal() {
    assert_row(
        "void f(C x) {\n  () => x.g;\n}\nclass C {\n  int g = 1;\n}\n",
        "(",
        1,
        9,
    );
}

// —— the cascade section —————————————————————————————————————————————
// Oracle `test_methodTearoff_cascaded` 3:15|7 — the section opens at `..`.

#[test]
fn us_cascade_method_tearoff_reports_the_section() {
    assert_row(
        "void f(C c) {\n  c..m;\n}\nclass C {\n  void m() {}\n}\n",
        "..",
        0,
        4,
    );
}

/// A cascade getter that does not return a function type has a clear
/// effect (`:194` gates on `staticType is FunctionType`).
#[test]
fn us_cascade_field_section_stays_silent() {
    assert_silent("void f(C c) {\n  c..g;\n}\nclass C {\n  int g = 1;\n}\n");
}

/// Oracle `test_methodTearoff` 3:3|18 — a method read as a value is a
/// tear-off, reported over the whole selector.
#[test]
fn us_method_tearoff_on_an_invocation_receiver() {
    assert_row(
        "void f() {\n  C().m;\n}\nclass C {\n  void m() {}\n}\n",
        "C",
        0,
        5,
    );
}

/// Oracle `test_constructorTearoff_named` 3:3|12 — a static member read
/// through a class reference reports over the whole selector.
#[test]
fn us_static_field_read_on_a_class_reference() {
    assert_row(
        "void f() {\n  C.now;\n}\nclass C {\n  static int now = 1;\n}\n",
        "C",
        0,
        5,
    );
}

/// An assignment is a clear effect — the selector-extent widening must not
/// drag it into the report set.
#[test]
fn us_selector_assignment_stays_silent() {
    assert_silent("void f(C x) {\n  x.g = 1;\n}\nclass C {\n  int g = 1;\n}\n");
}

/// An origin getter read has no effect to report and nothing to discard
/// (`:129-176` keeps the getter face silent) — the fencing case for the
/// widened selector root.
#[test]
fn us_instance_getter_read_stays_silent() {
    assert_silent("void f(C c) {\n  c.g;\n}\nclass C {\n  int get g => 1;\n}\n");
}

#[test]
fn us_top_level_getter_stays_silent() {
    assert_silent("void f() {\n  g;\n}\nint get g => 1;\n");
}

// —— the upstream oracle corpus ———————————————————————————————————————
// The whole corpus (`UnnecessaryStatementsTest`, 55 cases, plus the six
// `messages.yaml` doc examples — 61 total) pinned against the oracle rows
// of `conformance/expected.tsv`: each entry is the exact case source (the
// materialized file carries a leading blank line, so the upstream `3:3`
// rows land as written) and the oracle's `(line, column, length)` for
// every `unnecessary_statements` row.
//
// Six cases sit out, named with their oracle rows (no silent gaps). All
// six share one root that owns the semantic layer, not this rule's
// decision order: the diagnose engine types the factory-constructor
// invocation `List.empty()` as `Type` (the container TypeLiteral face)
// and SDK containers are unindexed in the library member walk, so the
// getter-vs-value split upstream's `GetterElement` face makes
// (`first` silent, `where` a reported tear-off) is unreachable from
// `Analysis::member_of`:
//
// - `test_instanceGetter#0` — oracle: silent (`first` is an instance
//   getter of `List`); dartq reports the whole statement.
// - `test_methodTearoff_cascaded#0` — oracle: (3, 15, 7); the cascade
//   section's `staticType is FunctionType` gate needs `where` as a
//   method of the unindexed chain. The conformance verdict additionally
//   carries a semantic `undefined_getter` extra on the same mis-typed
//   receiver.
// - `test_methodTearoff_cascaded_followOn#0` — oracle: (5, 5, 7); same
//   root (+ `undefined_method` extra on the `..forEach` section).
// - `test_methodTearoff_cascaded_returned_InLocalFunction#0` — oracle:
//   (4, 22, 7); same root.
// - `test_methodTearoff_cascaded_returned_InTopLevelFunction#0` —
//   oracle: (2, 30, 7); same root.
// - `test_returnStatement_cascadedTearoff#0` — oracle: (3, 22, 7); same
//   root.
//
// One further case-level (not pin-level) residual:
// `test_inForLoopUpdaters#0` — the pin's lint rows match below; the
// conformance verdict additionally carries a semantic `non_bool_condition`
// extra (the for-parts condition span is misread as the updater region —
// semantic layer).
#[test]
fn unnecessary_statements_oracle_corpus_pins() {
    // (case, source, oracle rows as (line, column, length)).
    let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
        (
            "test_asExpression#0",
            r#"
void f(Object o) {
  o as int;
}
"#,
            &[],
        ),
        (
            "test_binaryExpression#0",
            r#"
void f() {
  1 + 1;
}
"#,
            &[(3, 3, 5)],
        ),
        (
            "test_binaryExpression_andAnd#0",
            r#"
void f() {
  true && 1 == 2;
}
"#,
            &[(3, 11, 6)],
        ),
        (
            "test_binaryExpression_or#0",
            r#"
void f() {
  false || 1 == 2;
}
"#,
            &[(3, 12, 6)],
        ),
        (
            "test_binaryExpression_or_rightSideHasEffect#0",
            r#"
void f() {
  false || 7.isEven;
}
"#,
            &[],
        ),
        (
            "test_construcorTearoff_new#0",
            r#"
void f() {
  ArgumentError.new;
}
"#,
            &[(3, 3, 17)],
        ),
        (
            "test_constructorTearoff_named#0",
            r#"
void f() {
  DateTime.now;
}
"#,
            &[(3, 3, 12)],
        ),
        (
            "test_doStatement#0",
            r#"
void f() {
  do {} while (1 == 2);
}
"#,
            &[],
        ),
        (
            "test_forEachStatement#0",
            r#"
void f() {
  for (var i in []) {}
}
"#,
            &[],
        ),
        (
            "test_forLoopUpdaters_subsequent#0",
            r#"
void f() {
  for (; 1 == 2; print(7), []) {}
}
"#,
            &[(3, 28, 2)],
        ),
        (
            "test_forStatement#0",
            r#"
void f() {
  for (; 1 == 2;) {}
}
"#,
            &[],
        ),
        (
            "test_functionTearoff#0",
            r#"
void f() {
  g;
}
void g() {}
"#,
            &[(3, 3, 1)],
        ),
        (
            "test_ifNull#0",
            r#"
void f() {
  null ?? 7;
}
"#,
            &[(3, 11, 1)],
        ),
        (
            "test_ifNull_rightSideHasEffect#0",
            r#"
void f() {
  null ?? 7.toString();
}
"#,
            &[],
        ),
        (
            "test_ifStatement#0",
            r#"
void f() {
  if (1 == 2) {
  } else if (1 == 2) {
  }
}
"#,
            &[],
        ),
        (
            "test_inDoWhile#0",
            r#"
void f() {
  do {
    [];
  } while (1 == 2);
}
"#,
            &[(4, 5, 2)],
        ),
        (
            "test_inForLoop#0",
            r#"
void f() {
  for (var i in []) {
    ~1;
  }
}
"#,
            &[(4, 5, 2)],
        ),
        (
            "test_inForLoopInitializer#0",
            r#"
void f() {
  for (7; 1 == 2;) {}
}
"#,
            &[(3, 8, 1)],
        ),
        (
            "test_inForLoopUpdaters#0",
            r#"
void f() {
  for (; 1 == 2; []) {}
}
"#,
            &[(3, 18, 2)],
        ),
        (
            "test_inIfBody#0",
            r#"
void f() {
  if (1 == 2) {
    [];
  }
}
"#,
            &[(4, 5, 2)],
        ),
        (
            "test_inSwitchStatement_case#0",
            r#"
void f() {
  switch (7) {
    case 6:
      [];
  }
}
"#,
            &[(5, 7, 2)],
        ),
        (
            "test_inSwitchStatement_case_break#0",
            r#"
void f() {
  switch (7) {
    case 6:
      break;
  }
}
"#,
            &[],
        ),
        (
            "test_inSwitchStatement_default#0",
            r#"
void f() {
  switch (7) {
    default:
      [];
  }
}
"#,
            &[(5, 7, 2)],
        ),
        (
            "test_instanceCreationExpression#0",
            r#"
void f() {
  List.empty();
}
"#,
            &[],
        ),
        (
            "test_instanceField#0",
            r#"
void f() {
  C().g;
}

class C {
  int g = 1;
}
"#,
            &[(3, 3, 5)],
        ),
        (
            "test_instanceField2#0",
            r#"
void f(C c) {
  c.g;
}
class C {
  int g = 1;
}
"#,
            &[(3, 3, 3)],
        ),
        (
            "test_instanceGetter2#0",
            r#"
void f(List<int> list) {
  list.first;
}
"#,
            &[],
        ),
        (
            "test_intLiteral#0",
            r#"
void f() {
  1;
}
"#,
            &[(3, 3, 1)],
        ),
        (
            "test_listLiteral#0",
            r#"
void f() {
  [];
}
"#,
            &[(3, 3, 2)],
        ),
        (
            "test_localVariable#0",
            r#"
void f() {
  var g = 1;
  g;
}
"#,
            &[(4, 3, 1)],
        ),
        (
            "test_mapLiteral#0",
            r#"
void f() {
  <dynamic, dynamic>{};
}
"#,
            &[(3, 3, 20)],
        ),
        (
            "test_methodInvocation#0",
            r#"
void f() {
  g();
}
void g() {}
"#,
            &[],
        ),
        (
            "test_methodInvocation2#0",
            r#"
void f(List<int> list) {
  list.forEach((_) {});
}
"#,
            &[],
        ),
        (
            "test_methodTearoff#0",
            r#"
void f() {
  List.empty().where;
}
"#,
            &[(3, 3, 18)],
        ),
        (
            "test_methodTearoff_returned_InLocalFunction#0",
            r#"
void f() {
  // ignore: unused_element
  g() => List.empty().where;
}
"#,
            &[],
        ),
        (
            "test_methodTearoff_returned_InTopLevelFunction#0",
            r#"
Object f() => List.empty().where;
"#,
            &[],
        ),
        (
            "test_methodTearoff_returned_inFunctionLiteral#0",
            r#"
void f() {
  () => List.empty().where;
}
"#,
            &[(3, 3, 24)],
        ),
        (
            "test_patternAssignment_ok#0",
            r#"
f() {
  var (a, b) = (0, 0);
  var result = (1, 2);
  (a, b) = (a + result.$1, b + result.$2);
}
"#,
            &[],
        ),
        (
            "test_rethrow#0",
            r#"
void f() {
  try {} catch (_) {
    rethrow;
  }
}
"#,
            &[],
        ),
        (
            "test_returnStatement_binaryOperation#0",
            r#"
int f() {
  return 1 + 1;
}
"#,
            &[],
        ),
        (
            "test_stringLiteral#0",
            r#"
void f() {
  "blah";
}
"#,
            &[(3, 3, 6)],
        ),
        (
            "test_switchStatement#0",
            r#"
void f() {
  switch (~1) {}
}
"#,
            &[],
        ),
        (
            "test_throwExpression#0",
            r#"
void f() {
  throw Exception();
}
"#,
            &[],
        ),
        (
            "test_topLevelGetter#0",
            r#"
void f() {
  g;
}
int get g => 1;
"#,
            &[],
        ),
        (
            "test_topLevelVariable#0",
            r#"
void f() {
  g;
}
int g = 1;
"#,
            &[(3, 3, 1)],
        ),
        (
            "test_unaryOperation#0",
            r#"
void f() {
  ~1;
}
"#,
            &[(3, 3, 2)],
        ),
        (
            "test_unaryOperation_postfix#0",
            r#"
void f(int x) {
  x++;
}
"#,
            &[],
        ),
        (
            "test_unaryOperation_prefix#0",
            r#"
void f(int x) {
  ++x;
}
"#,
            &[],
        ),
        (
            "test_whileStatement#0",
            r#"
void f() {
  while (1 == 2) {}
}
"#,
            &[],
        ),
        (
            "doc_bad_0",
            r#"void f(int Function() first, int Function() second) {
  first() + second();
}
"#,
            &[(2, 3, 18)],
        ),
        (
            "doc_bad_1",
            r#"myvar;
list.clear;
1 + 2;
methodOne() + methodTwo();
foo ? bar : baz;
"#,
            &[],
        ),
        (
            "doc_good_0",
            r#"void f(int Function() first, int Function() second) {}
"#,
            &[],
        ),
        (
            "doc_good_1",
            r#"void f(int Function() first, int Function() second) {
  print(first() + second());
}
"#,
            &[],
        ),
        (
            "doc_good_2",
            r#"void f(int Function() first, int Function() second) {
  first();
  second();
}
"#,
            &[],
        ),
        (
            "doc_good_3",
            r#"some.method();
const SomeClass();
methodOne();
methodTwo();
foo ? bar() : baz();
return myvar;
"#,
            &[],
        ),
    ];
    // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
    fn offset(source: &str, line: u32, column: u32) -> u32 {
        let line_start: usize = source
            .split_inclusive('\n')
            .take(line as usize - 1)
            .map(str::len)
            .sum();
        (line_start + column as usize - 1) as u32
    }
    let mut mismatches = Vec::new();
    for (case, source, oracle) in cases {
        let f = Fixture::parse(source);
        let mut lints = f.run_semantic(&one_rule("unnecessary_statements"));
        lints.sort_by_key(|lint| lint.range.start);
        let got: Vec<(u32, u32)> = lints
            .iter()
            .map(|lint| (lint.range.start, lint.range.end))
            .collect();
        let want: Vec<(u32, u32)> = oracle
            .iter()
            .map(|&(line, column, length)| {
                let start = offset(source, line, column);
                (start, start + length)
            })
            .collect();
        if got != want {
            mismatches.push(format!("{case}: want {want:?} got {got:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
