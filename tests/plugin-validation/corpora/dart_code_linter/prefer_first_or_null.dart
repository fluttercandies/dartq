// Trigger for dart_code_linter `prefer-first-or-null`.
// Shape from the upstream suite
// (vendor/dart_code_linter-4.4.2/test/src/analyzers/lint_analyzer/rules/rules_list/prefer_first_or_null/examples/example.dart):
// `.first`, `.elementAt(0)` and `list[0]` accesses where `.firstOrNull` is
// preferred — the rule's flagged forms.
int firstOf(List<int> items) => items.first;

int elementAt(List<int> items) => items.elementAt(0);

int subscript(List<int> items) => items[0];
