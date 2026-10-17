// Trigger for dart_code_linter `prefer-enums-by-name`.
// Shape from the upstream suite
// (vendor/dart_code_linter-4.4.2/test/src/analyzers/lint_analyzer/rules/rules_list/prefer_enums_by_name/examples/example.dart):
// `SomeEnum.values.firstWhere((e) => e.name == x)` — the rule's flagged
// form (it suggests `values.byName(x)`).
enum Fruit { apple, banana }

Fruit find(String name) =>
    Fruit.values.firstWhere((e) => e.name == name, orElse: () => Fruit.apple);
