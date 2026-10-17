// unnecessary_const_in_enum_constructor — official rule faces
// (pkg/linter unnecessary_const_in_enum_constructor.dart:30-46): an
// explicit `const` on an enum constructor — the body form and the primary
// form — reports at the keyword. A class constructor's `const` is outside
// the face.
// Expected rows: 2 — UNNECESSARY_CONST_IN_ENUM_CONSTRUCTOR keyed
// unnecessary_const_in_enum_constructor.dart, message
// "Unnecessary 'const' keyword in an enum constructor.":
//   the `const` of `E()` in the enum body
//   the `const` of the enum primary constructor `const F(int x)`
enum E {
  a;
  const E();
}

enum const F(int x) {
  a;
}

// GOOD: a class constructor keeps its `const`.
class C {
  const C();
}
