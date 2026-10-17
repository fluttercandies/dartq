// Trigger for dart_code_linter `avoid-unnecessary-type-casts`.
// Shape: an `as` cast to a type the expression already has — the rule's
// flagged form.
void f(String value) {
  final cast = value as String;
}
