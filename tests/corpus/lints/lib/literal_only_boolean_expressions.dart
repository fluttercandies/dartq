// literal_only_boolean_expressions — official rule faces
// (pkg/linter literal_only_boolean_expressions.dart:14-41,69-113): a
// do/for/if/while condition (or a `when` guard) composed only of literals
// reports at the statement. `while (true)` passes (dart-lang/linter#453).
// Expected rows: 1 — LITERAL_ONLY_BOOLEAN_EXPRESSIONS keyed
// literal_only_boolean_expressions.dart, message
// "The Boolean expression has a constant value.":
//   the `1 == 1` condition under `if`
void f() {
  if (1 == 1) {}

  // GOOD: a written variable breaks the all-literal walk.
  bool b = true;
  if (b) {}
}
