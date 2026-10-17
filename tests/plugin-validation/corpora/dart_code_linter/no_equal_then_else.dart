// Trigger for dart_code_linter `no-equal-then-else`.
// Shape: `if`/`else` branches with identical bodies — the rule's flagged
// form.
void f(bool flag) {
  if (flag) {
    print('same');
  } else {
    print('same');
  }
}
