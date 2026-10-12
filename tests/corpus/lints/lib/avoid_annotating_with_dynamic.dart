// avoid_annotating_with_dynamic — official rule faces
// (pkg/linter avoid_annotating_with_dynamic.dart:30-76): a parameter
// annotated with the bare `dynamic` type when a more specific type
// works. Nested `dynamic` type arguments are a different face and stay
// out of this fixture.
// Expected rows: 3 — AVOID_ANNOTATING_WITH_DYNAMIC keyed
// avoid_annotating_with_dynamic.dart, one per `dynamic` formal:
//   `void f(dynamic x)`        -> row at the `dynamic` annotation
//   `void g(int a, dynamic b)` -> row at the `dynamic` annotation
//   `void j({dynamic x})`      -> row at the `dynamic` annotation
void f(dynamic x) {}

void g(int a, dynamic b) {}

void j({dynamic x}) {}

// GOOD: specific types instead of `dynamic`.
void h(int a, Object b) {}

void i(List<int> xs) {}
