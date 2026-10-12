// always_put_required_named_parameters_first — official rule faces
// (pkg/linter always_put_required_named_parameters_first.dart:31-92):
// every required named parameter must precede all optional named
// parameters; each required parameter appearing after an optional one
// is its own row.
// Expected rows: 3 — ALWAYS_PUT_REQUIRED_NAMED_PARAMETERS_FIRST keyed
// always_put_required_named_parameters_first.dart:
//   `f({int? a, required int b})`         -> row at `required int b`
//   `g({int? a, int? c, required int b, required int d})` -> rows at b, d
void f({int? a, required int b}) {}

void g({int? a, int? c, required int b, required int d}) {}

// GOOD: required named parameters first.
void h({required int a, int? b}) {}

void i({required int a, required int b, int? c, int? d}) {}

void j(int first, {required int a, int? b}) {}
