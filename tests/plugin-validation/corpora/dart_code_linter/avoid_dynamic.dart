// Trigger for dart_code_linter `avoid-dynamic`.
// Shape from the vendored rule's suite
// (vendor/dart_code_linter-4.4.2/test/src/analyzers/lint_analyzer/rules/rules_list/avoid_dynamic/avoid_dynamic_rule_test.dart,
// `startLines: [2, 6, 10, ...]`): `dynamic` annotations on declarations and
// members are flagged.
dynamic flag = true;

void f(dynamic value) => print(value);

class Holder {
  dynamic field;

  Map<String, dynamic> map() => {};
}
