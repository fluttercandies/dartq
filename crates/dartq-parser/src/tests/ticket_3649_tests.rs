//! #3649 pins: parser-emitted diagnostic parity for `expected_token`,
//! `missing_function_body`, `missing_assignable_selector` and siblings
//! (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart` recovery order
//! and `pkg/analyzer/lib/src/fasta/ast_builder.dart` report anchors).

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// `super[i]` reads/writes carry zero `missing_assignable_selector` rows —
/// `IndexExpression.isAssignable` is `true` upstream (ast.dart:18924), so
/// `reportErrorIfSuper` never sees a bare `SuperExpression` on the
/// selector-index path. Oracle: `features/_core/super/language/super/
/// operator_index{2..7}_test` report.tsv rows are empty.
#[test]
fn super_index_read_and_write_are_silent() {
    assert_eq!(
        sorted_rows(
            "class B {\n  test() {\n    Expect.equals(42, super[0]);\n    Expect.equals(42, super[0] = 42);\n    super[0]++;\n    ++super[0];\n    super[0] += 1;\n  }\n}\n",
        ),
        vec![]
    );
}

/// Under `augmentations`, a `;`-bodied static member (or top-level
/// function) with no completing `augment` reports `missing_function_body`
/// at its `;` — the verifier's singleton-incomplete arm
/// (`_checkForFunctionBodyCompleteness`, error_verifier.dart:5286-5311),
/// deferred to `parse_unit`'s end. A same-unit `augment` of the same
/// signature suppresses the row entirely (complete chain) or hands it to
/// the multi-fragment face.
#[test]
fn augmentations_defer_missing_function_body_to_unclaimed_singletons() {
    use super::collector::version_features;
    let (table, text) = scan_ok(
        "class A {\n  static int get foo;\n  int get bar;\n  augment int get bar { return 1; }\n}\nint f();\naugment int f() { return 0; }\nint g();\n",
    );
    let mut collector = super::collector::Collector::default();
    let mut features = version_features(3, 15);
    features.augmentations_enabled = true;
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            // `static int get foo;` — singleton incomplete, static.
            ("missing_function_body".to_owned(), 30, 31),
            // `int g();` — singleton incomplete, top level.
            ("missing_function_body".to_owned(), 131, 132),
        ],
        "the `augment`-claimed `int get bar` and `int f` chains stay silent"
    );
}

/// `PRIMARY_CONSTRUCTOR_BODY_WITH_MODIFIER` anchors on the `async`/`sync`
/// keyword itself (`primaryConstructorBodyWithModifier`,
/// parser_impl.dart:3878-3891), and its `{0}` argument is the keyword
/// lexeme plus a `*` for the generator forms. Oracle:
/// `primary_constructor_body_async*` rows at the modifier, never at `this`.
#[test]
fn primary_constructor_body_modifier_anchors_on_keyword() {
    // `class C(int x) {\n  this async {}` — `async` at utf-16 24..29.
    assert_eq!(
        sorted_rows("class C(int x) {\n  this async {}\n}\n"),
        vec![code("primary_constructor_body_with_modifier", 24, 29)],
    );
    // The generator form widens the argument to `async*` — the report
    // still anchors on the keyword token alone (length 5).
    assert_eq!(
        sorted_rows("class C(int x) {\n  this async* {}\n}\n"),
        vec![code("primary_constructor_body_with_modifier", 24, 29)],
    );
    // `sync*` reads the same arm; `sync` is 4 chars (oracle
    // `test_class_primaryConstructor_modifier_syncStar`: `3:8|4`).
    assert_eq!(
        sorted_rows("class C(int x) {\n  this sync* {}\n}\n"),
        vec![code("primary_constructor_body_with_modifier", 24, 28)],
    );
}

/// An unmodified `this {}` primary-constructor body is legal — the modifier
/// row is oracle-silent (negative pin).
#[test]
fn primary_constructor_body_plain_body_silent() {
    assert_eq!(
        sorted_rows("class C(int x) {\n  this {}\n}\n"),
        Vec::new(),
    );
}

/// The non-assignable assignment LHS reports `missingAssignableSelector`
/// over `lhs.beginToken..lhs.endToken` (ast_builder.dart:3831-3836) and —
/// for the statement's ROOT assignment — `illegalAssignmentToNonAssignable`
/// on the same run (:4206-4223). The last-token proxy admitted every
/// identifier-ending run; the shape flag narrows it to the parsed
/// `Identifier`/`PropertyAccess`/`IndexExpression` shapes
/// (`_isAssignable`, ast_builder.dart:6607-6620).
#[test]
fn non_assignable_lhs_faces_report_over_the_whole_run() {
    // `a + b += c;` — the `+` binary is not assignable: MAS + illegal over
    // `a + b` (utf-16 25..30 inside `class A { test() { a + b += c; } }`).
    let (table, text) = scan_ok("class A {\n  test() {\n    a + b += c;\n  }\n}\n");
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    // `    a + b` — `a` at offset 25, `a + b` spans 25..30.
    assert!(rows
        .iter()
        .any(|(c, s, e)| c == "missing_assignable_selector" && *s == 25 && *e == 30));
    assert!(rows
        .iter()
        .any(|(c, s, e)| c == "illegal_assignment_to_non_assignable" && *s == 25 && *e == 30));
}

/// `PropertyAccess` and `IndexExpression` stay assignable (negative pins):
/// `a.b += c`, `a[0] = 1`, `x!.y = 1` (the `.y` selector rebuilds the run
/// as a property extraction), `f().x = 1`.
#[test]
fn assignable_lhs_shapes_stay_silent() {
    assert_eq!(
        sorted_rows("class A {\n  test() {\n    a.b += c;\n    a[0] = 1;\n    x!.y = 1;\n    f().x = 1;\n  }\n}\n"),
        Vec::new(),
        "PropertyAccess/IndexExpression LHS carries no assignability rows"
    );
}

/// `++ ++x` — the OUTER `++`'s operand is the `PrefixExpression` `++x`,
/// which `_isAssignable` rejects (only Identifier/PropertyAccess/Index-
/// Expression qualify): `missingAssignableSelector` anchors at the
/// operand's endToken — the `x` (ast_builder.dart:5643-5659, oracle
/// `formalParameter_inc_inc` `3:9|1`). The INNER `++` sees the bare `x`,
/// an assignable Identifier — no row from it.
#[test]
fn prefix_increment_of_prefix_reports_at_operand_end() {
    let (table, text) = scan_ok("class A {\n  test() {\n    ++ ++x;\n  }\n}\n");
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    // `    ++ ++x;` — `x` at utf-16 30..31.
    assert_eq!(
        rows.iter()
            .filter(|(c, _, _)| c == "missing_assignable_selector")
            .collect::<Vec<_>>(),
        vec![&("missing_assignable_selector".to_string(), 30u32, 31u32)],
    );
}

/// `++x` / `++a.b` / `++a[0]` operands are the assignable shapes — silent
/// (negative pin).
#[test]
fn prefix_increment_of_assignable_operand_silent() {
    assert_eq!(
        sorted_rows("class A {\n  test() {\n    ++x;\n    ++a.b;\n    ++a[0];\n  }\n}\n"),
        Vec::new(),
    );
}

/// `for (;; super)` — each updater expression runs
/// `reportErrorIfSuper(update)` (ast_builder.dart:4348): a bare `super`
/// updater reports `missingAssignableSelector` at the `super` token
/// (oracle `update_super` `4:14|5`). A wrapped `super` (`super.x`,
/// `super + 1`) fails the bare-expression probe and stays silent.
#[test]
fn for_updater_bare_super_reports_selector() {
    let (table, text) = scan_ok("class A {\n  test() {\n    for (;; super) {}\n  }\n}\n");
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    // `    for (;; super)` — `super` at utf-16 33..38.
    assert!(rows
        .iter()
        .any(|(c, s, e)| c == "missing_assignable_selector" && *s == 33 && *e == 38));
    // Two bare-`super` updaters report once each.
    let (table2, text2) =
        scan_ok("class A {\n  test() {\n    for (;; super, super) {}\n  }\n}\n");
    let mut collector2 = super::collector::Collector::default();
    let mut parser2 =
        crate::Parser::new(text2, table2, latest_features(), &mut collector2);
    parser2.parse_unit();
    let mut rows2 = parser2.listener.errors.clone();
    rows2.sort();
    assert_eq!(
        rows2.iter()
            .filter(|(c, _, _)| c == "missing_assignable_selector")
            .count(),
        2,
    );
    // `for (;; super.x)` — the `.x` selector widens the run, silent.
    assert_eq!(
        sorted_rows("class A {\n  test() {\n    for (;; super.x) {}\n  }\n}\n"),
        Vec::new(),
    );
}
