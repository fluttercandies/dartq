// Trigger for dart_code_linter `prefer-last`.
// Shape: `iterable[iterable.length - 1]` where `.last` is preferred — the
// rule's flagged form.
int tail(List<int> items) => items[items.length - 1];
