// Trigger for solid_lints `prefer_conditional_expressions`.
// Shape: an `if`/`else` assigning the same variable that could be a
// conditional expression — the rule's flagged form.
int f(bool flag) {
  int value;
  if (flag) {
    value = 1;
  } else {
    value = 2;
  }
  return value;
}
