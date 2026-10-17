// prefer_expression_function_bodies — official rule faces
// (pkg/linter prefer_expression_function_bodies.dart:29-37): a block
// function body whose statements are exactly one `return` with an
// expression reports at the body. Every function-like owner reports —
// lambdas and getters included; a plain `return;` and multi-statement
// bodies pass.
// Expected rows: 2 — PREFER_EXPRESSION_FUNCTION_BODIES keyed
// prefer_expression_function_bodies.dart, message
// "Unnecessary use of a block function body.":
//   the `{ return 1; }` body of `f`
//   the getter body `{ return 7; }` in `A`
int f() {
  return 1;
}

class A {
  get g {
    return 7;
  }
}

// GOOD: a bare `return;` and several statements stay silent.
void h() {
  return;
}

int i() {
  var a = 1;
  return a;
}
