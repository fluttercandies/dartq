// @dart = 3.12
// prefer_final_parameters — official rule faces
// (pkg/linter prefer_final_parameters.dart:54-73): a constructor, method,
// or function-expression parameter that is neither final/const nor
// reassigned in its function reports at the parameter. A top-level
// function is not a registered face; a wildcard `_` skips; a write in the
// scope (including an assigned variable pattern) silences.
//
// The rule registers only WITHOUT primary constructors (the feature is a
// compile-time error on declaring parameters), so this fixture pins the
// library to 3.12 — the pre-primary-constructors face — where both sides
// register it.
// Expected rows: 3 — PREFER_FINAL_PARAMETERS keyed
// prefer_final_parameters.dart, message
// "The parameter 'p' should be final." /
// "The parameter 'a' should be final." /
// "The parameter 'b' should be final.":
//   the method parameter `p` of `C.m`
//   the named parameter `a` of `C.n`
//   the named parameter `b` of `C.n`
class C {
  void m(String p) {}

  void n({int a, int b}) {}
}

// GOOD: parameters written in the scope, and a wildcard, skip.
void h(int q, int r) {
  q = 1;
  r = 2;
}

void w(String _) {}
