//! The #3322 oracle-corpus pins: `no_dynamic_casts` and
//! `implicit_call_tearoffs` over every `conformance/` case of the rule
//! (the upstream `pkg/linter/test/rules/<rule>_test.dart` markup cases
//! plus the rule's doc examples), each asserted against the row the
//! pinned `dart analyze` 3.13.7 oracle rendered (`conformance/expected.tsv`,
//! the rule's own rows only). Every case runs before the assertion, so one
//! report names every diverging case with its `(line, col, len)` rows. One
//! composed pin follows the `implicit_call_tearoffs` corpus: shapes one step
//! past its cases, each row taken from the oracle case it composes.
//!
//! The sources mirror the materialized `.case` files byte-for-byte: the
//! upstream markup cases open with an empty first line, the doc examples
//! do not — the oracle's line numbers depend on it.

use crate::engine::Lint;
use crate::testsupport::{Fixture, one_rule};

/// One oracle row: `(line, col, len)` — 1-based line and UTF-16 column,
/// UTF-16 length (the `dart analyze` rendering the corpus pins).
type Row = (u32, u32, u32);

/// One corpus case: its conformance id tail, the analyzed source, and the
/// rule's expected rows.
type Case = (&'static str, &'static str, &'static [Row]);

fn rows_of(f: &Fixture, lints: &[Lint]) -> Vec<Row> {
    let mut rows: Vec<Row> = lints
        .iter()
        .map(|lint| {
            let (line, col) = f.lines.line_col_utf16(&f.source, lint.range.start);
            (line, col, lint.range.end - lint.range.start)
        })
        .collect();
    rows.sort_unstable();
    rows
}

/// Runs `rule` over every case with the semantic face mounted and asserts
/// the rendered rows and message against the oracle's — all cases first,
/// one report.
fn assert_corpus(rule: &str, message: &str, cases: &[Case]) {
    let mut failures = Vec::new();
    for (id, source, want) in cases {
        let f = Fixture::parse(source);
        let lints = f.run_semantic(&one_rule(rule));
        let got = rows_of(&f, &lints);
        let mut want = want.to_vec();
        want.sort_unstable();
        if got != want {
            failures.push(format!("{id}: want {want:?}, got {got:?}\n{source}"));
            continue;
        }
        if let Some(lint) = lints.iter().find(|lint| lint.message != message) {
            failures.push(format!("{id}: message {:?}", lint.message));
        }
    }
    assert!(
        failures.is_empty(),
        "{rule}: {} of {} cases diverged:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// `lints/no_dynamic_casts/**` — the 28 `NoDynamicCastsTest` cases and
/// the two doc examples. Every reported row is the `dynamic` expression
/// itself (the upstream `reportAtNode(expression)`); the loop-variable
/// face reports at the iterable.
#[test]
fn no_dynamic_casts_oracle_corpus() {
    const CASES: &[Case] = &[
        (
            "test_argument#0",
            "\nvoid f(int x) {}\nvoid g(dynamic a) {\n  f(a);\n}\n",
            &[(4, 5, 1)],
        ),
        (
            "test_assignment#0",
            "\nvoid f(dynamic a) {\n  int x = a;\n}\n",
            &[(3, 11, 1)],
        ),
        (
            "test_assignment_ok#0",
            "\nvoid f(dynamic a) {\n  dynamic x = a;\n  Object? y = a;\n}\n",
            &[],
        ),
        (
            "test_condition_conditionalExpression#0",
            "\nvoid f(dynamic a) {\n  a ? 1 : 2;\n}\n",
            &[(3, 3, 1)],
        ),
        (
            "test_condition_doLoop#0",
            "\nvoid f(dynamic a) {\n  do {} while (a);\n}\n",
            &[(3, 16, 1)],
        ),
        (
            "test_condition_forLoop#0",
            "\nvoid f(dynamic a) {\n  for (; a;) {}\n}\n",
            &[(3, 10, 1)],
        ),
        (
            "test_condition_ifExpression#0",
            "\nvoid f(dynamic a) {\n  [if (a) 7];\n}\n",
            &[(3, 8, 1)],
        ),
        (
            "test_condition_ifStatement#0",
            "\nvoid f(dynamic a) {\n  if (a) {}\n}\n",
            &[(3, 7, 1)],
        ),
        (
            "test_condition_whileLoop#0",
            "\nvoid f(dynamic a) {\n  while (a) {}\n}\n",
            &[(3, 10, 1)],
        ),
        (
            "test_explicitCast_ok#0",
            "\nvoid f(dynamic a) {\n  int x = a as int;\n}\n",
            &[],
        ),
        (
            "test_expressionFunctionBody#0",
            "\nint f(dynamic a) => a;\n",
            &[(2, 21, 1)],
        ),
        (
            "test_forEach_iterable#0",
            "\nvoid f(dynamic a) {\n  for (var x in a) {}\n}\n",
            &[(3, 17, 1)],
        ),
        (
            "test_forEach_variable#0",
            "\nvoid f(List<dynamic> list) {\n  for (int x in list) {}\n}\n",
            &[(3, 17, 4)],
        ),
        (
            "test_forEach_variable_objectQuestionTarget#0",
            "\nvoid f(List<dynamic> list) {\n  for (Object? x in list) {}\n}\n",
            &[],
        ),
        (
            "test_listLiteral#0",
            "\nvoid f(dynamic a) {\n  var list = <int>[a];\n}\n",
            &[(3, 20, 1)],
        ),
        (
            "test_logicalBinary_left#0",
            "\nvoid f(dynamic a, bool b) {\n  a && b;\n}\n",
            &[(3, 3, 1)],
        ),
        (
            "test_logicalBinary_right#0",
            "\nvoid f(bool a, dynamic b) {\n  a && b;\n}\n",
            &[(3, 8, 1)],
        ),
        (
            "test_mapLiteral_key#0",
            "\nvoid f(dynamic a) {\n  var map = <int, String>{a: 'x'};\n}\n",
            &[(3, 27, 1)],
        ),
        (
            "test_mapLiteral_spread#0",
            "\nvoid f(dynamic a) {\n  var map = <String, int>{...a};\n}\n",
            &[(3, 30, 1)],
        ),
        (
            "test_mapLiteral_value#0",
            "\nvoid f(dynamic a) {\n  var map = <String, int>{'x': a};\n}\n",
            &[(3, 32, 1)],
        ),
        (
            "test_namedArgument#0",
            "\nvoid f({required int x}) {}\nvoid g(dynamic a) {\n  f(x: a);\n}\n",
            &[(4, 8, 1)],
        ),
        (
            "test_negation#0",
            "\nvoid f(dynamic a) {\n  !a;\n}\n",
            &[(3, 4, 1)],
        ),
        (
            "test_return#0",
            "\nint f(dynamic a) {\n  return a;\n}\n",
            &[(3, 10, 1)],
        ),
        (
            "test_return_async#0",
            "\nFuture<int> f(dynamic a) async {\n  return a;\n}\n",
            &[(3, 10, 1)],
        ),
        (
            "test_setLiteral#0",
            "\nvoid f(dynamic a) {\n  var set = <int>{a};\n}\n",
            &[(3, 19, 1)],
        ),
        (
            "test_spreadList#0",
            "\nvoid f(dynamic a) {\n  var list = <int>[...a];\n}\n",
            &[(3, 23, 1)],
        ),
        (
            "test_yield#0",
            "\nIterable<int> f(dynamic a) sync* {\n  yield a;\n}\n",
            &[(3, 9, 1)],
        ),
        (
            "test_yieldStar#0",
            "\nIterable<int> f(dynamic a) sync* {\n  yield* a;\n}\n",
            &[(3, 10, 1)],
        ),
        (
            "doc_bad_0",
            "void f(dynamic x) {\n  int y = x;\n}\n",
            &[(2, 11, 1)],
        ),
        (
            "doc_good_0",
            "void f(dynamic x) {\n  int y = x as int;\n}\n",
            &[],
        ),
    ];
    assert_corpus("no_dynamic_casts", "Implicit cast from 'dynamic'.", CASES);
}

/// `lints/implicit_call_tearoffs/**` — the 26 `ImplicitCallTearoffsTest`
/// cases and the four doc examples. Every reported row is the resolver's
/// `ImplicitCallReference` node whole: the callable operand, and for a
/// tear-off instantiation its type arguments too (`c<int>` is 6 wide,
/// `C()<int>` 8). An explicit `.call`, a context-free creation, and the
/// top-level-statement doc shapes (parse-error cases) carry no row.
#[test]
fn implicit_call_tearoffs_oracle_corpus() {
    const CASES: &[Case] = &[
        (
            "test_explicitCallTearoff#0",
            "\nvoid Function() f() => C().call;\nclass C {\n  void call() {}\n}\n",
            &[],
        ),
        (
            "test_ifNullExpression_functionTypeContext#0",
            "\nvoid Function() f(C? c1, C c2) {\n  return c1 ?? c2;\n}\nclass C {\n  void call() {}\n}\n",
            &[(3, 10, 8)],
        ),
        (
            "test_instanceCreation_argumentToFunctionTypeParameter#0",
            "\nvoid f() {\n  g(C());\n}\nclass C {\n  void call() {}\n}\nvoid g(void Function() f) {}\n",
            &[(3, 5, 3)],
        ),
        (
            "test_instanceCreation_argumentToFunctionTypeParameter_instantiatedTypeArgument#0",
            "\nvoid f() {\n  g(C());\n}\nvoid g(void Function(int) f) {}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 5, 3)],
        ),
        (
            "test_instanceCreation_cascadeExpression_functionTypeContext#0",
            "\nvoid Function() f() {\n  return C()..other();\n}\nclass C {\n  void call() {}\n  void other() {}\n}\n",
            &[(3, 10, 12)],
        ),
        (
            "test_instanceCreation_functionContext#0",
            "\nFunction f = C();\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 14, 3)],
        ),
        (
            "test_instanceCreation_functionTypeContext#0",
            "\nvoid Function() f = C();\nclass C {\n  void call() {}\n}\n",
            &[(2, 21, 3)],
        ),
        (
            "test_instanceCreation_functionTypeContext_instantiatedTypeArgument#0",
            "\nvoid Function(int) f = C();\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 24, 3)],
        ),
        (
            "test_instanceCreation_genericFunctionTypeContext#0",
            "\nvoid Function<T>(T) f = C();\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 25, 3)],
        ),
        (
            "test_instanceCreation_noContext#0",
            "\nfinal c = C();\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[],
        ),
        (
            "test_simpleIdentifier_argumentToFunctionTypeParameter_instantiatedTypeArgument#0",
            "\nvoid f(C c) {\n  g(c);\n}\nvoid g(void Function(int) f) {}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 5, 1)],
        ),
        (
            "test_simpleIdentifier_argumentToFunctionTypedParameter#0",
            "\nvoid f(C c) {\n  g(c);\n}\nvoid g(void Function() f) {}\nclass C {\n  void call() {}\n}\n",
            &[(3, 5, 1)],
        ),
        (
            "test_simpleIdentifier_functionContext#0",
            "\nvoid f(C c) {\n  Function fn = c;\n}\nclass C {\n  void call() {}\n}\n",
            &[(3, 17, 1)],
        ),
        (
            "test_simpleIdentifier_functionTypeContext#0",
            "\nvoid Function() f(C c) => c;\nclass C {\n  void call() {}\n}\n",
            &[(2, 27, 1)],
        ),
        (
            "test_simpleIdentifier_functionTypeContext_instantiatedTypeArgument#0",
            "\nvoid Function(int) f(C c) => c;\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 30, 1)],
        ),
        (
            "test_simpleIdentifier_functionTypeContext_listTypArgument#0",
            "\nvoid f(C c) {\n  <void Function()>[c];\n}\nclass C {\n  void call() {}\n}\n",
            &[(3, 21, 1)],
        ),
        (
            "test_simpleIdentifier_genericFunctionTypeContext#0",
            "\nvoid f(C c) {\n  void Function<T>(T) fn = c;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 28, 1)],
        ),
        (
            "test_simpleIdentifier_listLiteral_listOfFunctionTypeContext#0",
            "\nList<void Function()> f(C c) {\n  return [c];\n}\nclass C {\n  void call() {}\n}\n",
            &[(3, 11, 1)],
        ),
        (
            "test_tearoffInstantiationOfInstanceCreation_argumentToFunctionTypeParameter#0",
            "\nvoid f() {\n  g(C()<int>);\n}\nvoid g(void Function(int) f) {}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 5, 8)],
        ),
        (
            "test_tearoffInstantiationOfInstanceCreation_functionContext#0",
            "\nFunction f = C()<int>; // LINT\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 14, 8)],
        ),
        (
            "test_tearoffInstantiationOfInstanceCreation_functionTypeContext#0",
            "\nvoid Function(int) f = C()<int>;\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(2, 24, 8)],
        ),
        (
            "test_tearoffInstantiation_argumentToFunctionTypeParameter#0",
            "\nvoid f(C c) {\n  g(c<int>);\n}\nvoid g(void Function(int) f) {}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 5, 6)],
        ),
        (
            "test_tearoffInstantiation_functionContext#0",
            "\nvoid f(C c) {\n  Function fn = c<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 17, 6)],
        ),
        (
            "test_tearoffInstantiation_functionTypeContext#0",
            "\nvoid f(C c) {\n  void Function(int) fn = c<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 27, 6)],
        ),
        (
            "test_tearoffInstantiation_noContext#0",
            "\nvoid f(C c) {\n  c<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 3, 6)],
        ),
        (
            "test_tearoffInstantiationofInstanceCreation_noContext#0",
            "\nvoid f() {\n  C()<int>;\n}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 3, 8)],
        ),
        (
            "doc_bad_0",
            "class Callable {\n  void call() {}\n}\n\nvoid callIt(void Function() f) {\n  f();\n}\n\nvoid f() {\n  callIt(Callable());\n}\n",
            &[(10, 10, 10)],
        ),
        (
            "doc_bad_1",
            "class Callable {\n  void call() {}\n}\nvoid callIt(void Function() f) {\n  f();\n}\n\ncallIt(Callable());\n",
            &[],
        ),
        (
            "doc_good_0",
            "class Callable {\n  void call() {}\n}\n\nvoid callIt(void Function() f) {\n  f();\n}\n\nvoid f() {\n  callIt(Callable().call);\n}\n",
            &[],
        ),
        (
            "doc_good_1",
            "class Callable {\n  void call() {}\n}\nvoid callIt(void Function() f) {\n  f();\n}\n\ncallIt(Callable().call);\n",
            &[],
        ),
    ];
    assert_corpus(
        "implicit_call_tearoffs",
        "Implicit tear-off of the 'call' method.",
        CASES,
    );
}

/// The argument-slot and element-anchor faces one step past the corpus's
/// single-argument / single-element shapes — each expected row is the
/// oracle row of the corpus case it composes:
///
/// - a tear-off instantiation argument holds one formal slot: `c<int>`
///   reports whole and the argument after it reads the next formal
///   (`test_tearoffInstantiation_argumentToFunctionTypeParameter` +
///   `test_simpleIdentifier_argumentToFunctionTypeParameter_instantiatedTypeArgument`);
/// - an untyped literal's first element anchors at the element, not at the
///   bracket, and the later elements keep their own rows
///   (`test_simpleIdentifier_listLiteral_listOfFunctionTypeContext`).
#[test]
fn implicit_call_tearoffs_argument_slots_and_element_anchors() {
    const CASES: &[Case] = &[
        (
            "instantiation_argument_then_value",
            "\nvoid f(C c) {\n  g(c<int>, c);\n}\nvoid g(void Function(int) f, void Function(int) h) {}\nclass C {\n  void call<T>(T arg) {}\n}\n",
            &[(3, 5, 6), (3, 13, 1)],
        ),
        (
            "untyped_list_literal_elements",
            "\nList<void Function()> f(C a, C b) {\n  return [a, b];\n}\nclass C {\n  void call() {}\n}\n",
            &[(3, 11, 1), (3, 14, 1)],
        ),
    ];
    assert_corpus(
        "implicit_call_tearoffs",
        "Implicit tear-off of the 'call' method.",
        CASES,
    );
}
