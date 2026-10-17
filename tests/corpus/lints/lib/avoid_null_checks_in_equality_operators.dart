// avoid_null_checks_in_equality_operators — official rule faces
// (pkg/linter avoid_null_checks_in_equality_operators.dart:11-27,101-115):
// inside an `operator ==` whose single parameter is nullable, every null
// check against that parameter reports — an `==`/`!=` against `null`, a
// `??` with the parameter on the left, and a `?.` access or invocation on
// the parameter. A non-`?` parameter (including written `dynamic`) bails.
// Expected rows: 1 — AVOID_NULL_CHECKS_IN_EQUALITY_OPERATORS keyed
// avoid_null_checks_in_equality_operators.dart, message
// "Unnecessary null comparison in implementation of '=='.":
//   the `other != null` comparison in `operator ==`
class C {
  operator ==(Object? other) => other != null && other is C;
}

// GOOD: a non-`?` parameter is the analyzer's other face, not this lint.
class D {
  operator ==(Object other) => other != null;
}
