// always_declare_return_types — official rule faces (pkg/linter
// always_declare_return_types.dart:38-85): every function-shaped
// declaration without an explicit return type, with the setter
// exemption the official rule grants.
// Expected rows: 4 — ALWAYS_DECLARE_RETURN_TYPES keyed
// always_declare_return_types.dart:
//   `f() => 1;`     -> "The function 'f' should have a return type but doesn't."
//   `get p => 2;`   -> "The function 'p' should have a return type but doesn't."
//   `m() {}`        -> "The method 'm' should have a return type but doesn't."
//   `n() => 3;`     -> "The method 'n' should have a return type but doesn't."
f() => 1;

get p => 2;

class C {
  m() {}
  n() => 3;
  set s(int v) {}
}

// GOOD: explicit return types, typed methods, and the setter exemption.
String g() => '';

class D {
  void m() {}
  int n() => 4;
  set s(int v) {}
}
