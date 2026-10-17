// Trigger for dart_code_linter `no-empty-block`.
// Shape: `if` and `catch` blocks with no statements — the rule's flagged
// form.
void m(int value) {
  if (value > 0) {
  }

  try {
    value++;
  } catch (e) {
  }
}
