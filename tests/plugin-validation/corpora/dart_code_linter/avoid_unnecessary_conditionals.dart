// Trigger for dart_code_linter `avoid-unnecessary-conditionals`.
// Shape: a conditional whose branches both evaluate to the same constant —
// the rule's flagged form.
void f(bool flag) {
  final value = flag ? true : false;
}
