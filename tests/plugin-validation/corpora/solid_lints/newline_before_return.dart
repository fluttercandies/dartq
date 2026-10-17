// Trigger for solid_lints `newline_before_return`.
// Shape: a `return` immediately preceded by a non-blank statement — the
// rule's flagged form.
int f(int value) {
  final doubled = value * 2;
  return doubled;
}
