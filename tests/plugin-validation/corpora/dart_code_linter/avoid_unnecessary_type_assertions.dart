// Trigger for dart_code_linter `avoid-unnecessary-type-assertions`.
// Shape: an `is`/`as` assertion on an expression already of that type —
// the rule's flagged form.
void f(String value) {
  final check = value is String;
}
