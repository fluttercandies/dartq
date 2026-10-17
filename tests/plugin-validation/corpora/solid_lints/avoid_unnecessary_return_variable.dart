// Trigger for solid_lints `avoid_unnecessary_return_variable`.
// Shape: a local variable declared then immediately returned — the rule's
// flagged form.
int f(int value) {
  final result = value * 2;
  return result;
}
