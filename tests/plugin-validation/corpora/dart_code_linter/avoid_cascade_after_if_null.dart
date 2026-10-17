// Trigger for dart_code_linter `avoid-cascade-after-if-null`.
// Shape: a cascade applied to the `??` result without parentheses — the
// rule's flagged form.
void f(Cow? nullableCow) {
  final cow = nullableCow ?? Cow()..moo();
}

class Cow {
  void moo() {}
}
