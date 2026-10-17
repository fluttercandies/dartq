// Trigger for solid_lints `avoid_unrelated_type_assertions`.
// Shape: an `is` assertion between two unrelated types — the rule's
// flagged form.
void f(String value) {
  final check = value is int;
}
