// Trigger for dart_code_linter `avoid-global-state`.
// Shape: mutable top-level variables and static fields — the rule's
// flagged form.
var answer = 42;

class Foo {
  static int? counter;
}
