// prefer_int_literals — official rule faces
// (pkg/linter prefer_int_literals.dart:36-75): a double literal whose
// value is an exact integer reports when the context demands a double —
// a written `double` variable, a `double` return face, and a typed list
// element. Argument and named-argument positions under-report (the
// documented element-model gap).
// Expected rows: 2 — PREFER_INT_LITERALS keyed prefer_int_literals.dart,
// message "Unnecessary use of a 'double' literal.":
//   `1.0` in the `double x` declaration
//   `2.0` returned from the `double`-typed function
double x = 1.0;

double m() {
  return 2.0;
}

// GOOD: a non-integral double and an untyped context stay silent.
var g1 = 1.5;
var g2 = 3.0;
