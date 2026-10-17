// unnecessary_statements — official rule faces
// (pkg/linter unnecessary_statements.dart:39-217): an expression
// statement, a C-style for initializer/updater, or a cascade section
// without a clear effect reports. Invocations, assignments, and awaits
// are clear-effect; every other binary reports at the whole expression;
// a field read through a same-file typed receiver reports at the chain.
// Expected rows: 2 — UNNECESSARY_STATEMENTS keyed
// unnecessary_statements.dart, message "Unnecessary statement.":
//   the `1 + 2` expression statement
//   the `c.x` field read through the typed receiver
class C {
  int x = 1;
}

void f(C c) {
  1 + 2;
  c.x;

  // GOOD: an invocation and an assignment have clear effects.
  print(1);
  c.x = 2;
}
