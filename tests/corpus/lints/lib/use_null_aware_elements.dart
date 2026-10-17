// use_null_aware_elements — official rule faces
// (pkg/linter use_null_aware_elements.dart:44-135): an if-element whose
// condition null-checks a target and whose then-element is exactly that
// target reports at the `if` keyword — `[if (x != null) x]` becomes
// `[?x]`. An else segment never reports; a mismatched then-element stays
// silent; the face is name-level (a same-file value binding is
// promotable, a same-file getter takes the `!` form).
// Expected rows: 1 — USE_NULL_AWARE_ELEMENTS keyed
// use_null_aware_elements.dart, message
// "Use the null-aware marker '?' rather than a null check via an 'if'.":
//   the `if (x != null) x` list element
List<int> f(int? x) {
  return [if (x != null) x];
}

// GOOD: an else segment and a mismatched then-element pass.
List<int> g(int? x) {
  return [if (x != null) x else 1, if (x != null) 1];
}
