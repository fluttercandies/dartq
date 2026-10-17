// Trigger for solid_lints `prefer_last`.
// Shape: `iterable[iterable.length - 1]` where `.last` is preferred — the
// rule's flagged form.
int tail(List<int> items) => items[items.length - 1];
