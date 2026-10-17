// Trigger for solid_lints `no_equal_then_else`.
// Shape: `if`/`else` branches with identical bodies — the rule's flagged
// form.
void f(bool flag) {
  if (flag) {
    print('same');
  } else {
    print('same');
  }
}
