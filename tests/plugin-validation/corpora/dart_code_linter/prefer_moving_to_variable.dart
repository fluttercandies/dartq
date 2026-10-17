// Trigger for dart_code_linter `prefer-moving-to-variable`.
// Shape: a repeated method call inside a chain that should be hoisted to a
// variable — the rule's flagged form.
int f(List<int> items) =>
    items.length + items.length;
