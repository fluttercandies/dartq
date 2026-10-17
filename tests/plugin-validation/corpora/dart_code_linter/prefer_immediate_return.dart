// Trigger for dart_code_linter `prefer-immediate-return`.
// Shape: a local variable declared then immediately returned — the rule's
// flagged form.
int f(int value) {
  final result = value * 2;
  return result;
}
