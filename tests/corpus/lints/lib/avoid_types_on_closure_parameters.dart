// avoid_types_on_closure_parameters — official rule faces
// (pkg/linter avoid_types_on_closure_parameters.dart): a closure whose
// context type is a function type reports each parameter carrying a
// written non-`dynamic` type (at the type) and function-typed-suffix
// parameters (at the parameter). The lexical context face: written
// function-type variable initializers and assignment targets; SDK
// callback arguments under-report (the documented resolution family).
// Expected rows: 3 — AVOID_TYPES_ON_CLOSURE_PARAMETERS keyed
// avoid_types_on_closure_parameters.dart, message
// "Unnecessary type annotation on a function expression parameter.":
//   `int` -> the initializer face, typed parameter
//   `int p(int x)` -> the initializer face, function-typed suffix
//   `int` -> the assignment-target face
void Function(int) cb = (int e) {};
void f() {
  cb = (int a) {};
}

void Function(int) suffix = (int p(int x)) => p(0);

// GOOD: untyped targets, `dynamic` parameters, and closures outside a
// written function-type position stay silent.
var onFoo = (int p) {};
void Function(dynamic) dyn = (dynamic e) {};
