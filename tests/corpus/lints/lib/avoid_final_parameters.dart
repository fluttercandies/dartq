// avoid_final_parameters — official rule faces
// (pkg/linter avoid_final_parameters.dart:41-84): `final` on any
// positional, optional, or named formal parameter is reported — but only
// where the grammar still accepts it. At the settled language versions
// (3.13+) the parser rejects `final`/`var` on ordinary formals
// (`extraneous_modifier`, ERROR tier) and the rule never registers
// (upstream :56-59 keys the registration on primary constructors being
// off), so the oracle emits errors and no lints here; the upstream tests
// pin the lint face under `// @dart=3.12` only.
// Expected rows: none — the `final` keywords at 11/13/15 are the oracle's
// `extraneous_modifier` ERROR rows, outside this INFO gate.
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
