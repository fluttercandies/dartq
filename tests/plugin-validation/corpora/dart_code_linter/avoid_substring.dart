// Trigger for dart_code_linter `avoid-substring`.
// Shape: `substring` used on a string containing non-ASCII (emoji) content —
// the rule's flagged form.
void f() {
  'hello 😀 world'.substring(7, 8);
}
