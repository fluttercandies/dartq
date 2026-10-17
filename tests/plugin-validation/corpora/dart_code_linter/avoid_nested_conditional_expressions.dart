// Trigger for dart_code_linter `avoid-nested-conditional-expressions`.
// Shape: a conditional expression nested inside another — the rule's
// flagged form.
void f(bool a, bool b) {
  final result = a ? b ? 'x' : 'y' : 'z';
}
