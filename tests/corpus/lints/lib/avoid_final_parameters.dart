// avoid_final_parameters — official rule faces
// (pkg/linter avoid_final_parameters.dart:41-84): `final` on any
// positional, optional, or named formal parameter is reported;
// function-type parameters (where `final` belongs to the inner
// signature grammar) and local `final` variables are exempt.
// Expected rows: 3 — AVOID_FINAL_PARAMETERS keyed
// avoid_final_parameters.dart, one per `final` formal:
//   `void f(final int x)`         -> row at the `final` keyword
//   `void g({final int y = 1})`   -> row at the `final` keyword
//   `void h([final int z = 2])`   -> row at the `final` keyword
void f(final int x) {}

void g({final int y = 1}) {}

void h([final int z = 2]) {}

// GOOD: plain formals, function-typed signatures, and local finals.
void i(int x) {}

void j(void Function(int) cb) {}

typedef F = void Function(final int inner);

void k() {
  final int local = 1;
  print(local);
}
