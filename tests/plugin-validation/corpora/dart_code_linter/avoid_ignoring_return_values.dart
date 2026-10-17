// Trigger for dart_code_linter `avoid-ignoring-return-values`.
// Shape: a non-void return value discarded at the call site — the rule's
// flagged form.
String greeting() => 'hello';

void f() {
  greeting();

  final list = [1, 2, 3];
  list.remove(1);
}
