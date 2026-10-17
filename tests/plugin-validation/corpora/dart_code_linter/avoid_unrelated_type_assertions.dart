// Trigger for dart_code_linter `avoid-unrelated-type-assertions`.
// Shape: an `is` assertion between two unrelated types — the rule's
// flagged form.
void f(String value) {
  final check = value is int;
}
