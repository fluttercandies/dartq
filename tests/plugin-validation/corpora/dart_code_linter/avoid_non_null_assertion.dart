// Trigger for dart_code_linter `avoid-non-null-assertion`.
// Shape: a `!` non-null assertion — the rule's flagged form.
void f(String? value) {
  value!.contains('x');
}
