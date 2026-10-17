// strict_top_level_inference — official rule faces
// (pkg/linter strict_top_level_inference.dart:52-241): a declaration
// whose types inference would have to fill reports "Missing type
// annotation." in three shapes — untyped function/method/getter return
// faces, untyped simple formals, and untyped top-level/field variables
// without initializers. Local declarations never report; a written type
// or a function-typed formal stops its parameter list.
// Expected rows: 3 — STRICT_TOP_LEVEL_INFERENCE keyed
// strict_top_level_inference.dart, message "Missing type annotation.":
//   the untyped top-level function `m`
//   the setter formal `value` in `C`
//   the untyped top-level variable `v` (no initializer)
m() {}

class C {
  set s(value) {}
}

var v;

// GOOD: typed faces and local declarations pass.
void ok(int p) {}

class D {
  set t(int value) {}
}

void local() {
  var x;
  inner(q) {}
}
