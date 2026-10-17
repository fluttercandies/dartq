// Trigger for solid_lints `avoid_duplicate_code`.
// Shape: two structurally identical function bodies — the rule's flagged
// form.
int addOne(int value) {
  final result = value + 1;
  return result;
}

int addTwo(int value) {
  final result = value + 1;
  return result;
}
