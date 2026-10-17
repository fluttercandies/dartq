// unnecessary_underscores — official rule faces
// (pkg/linter unnecessary_underscores.dart:33-106): a formal parameter or
// local variable whose name is two or more underscores reports at the
// name when the body never references it — with the wildcard-variables
// feature (Dart 3.7) a single `_` binds nothing, so the extra
// underscores carry no meaning. Fields, top-level variables, enum
// elements, and declaration names are not the face; receiver formals and
// primary-constructor declaring formals skip.
// Expected rows: 2 — UNNECESSARY_UNDERSCORES keyed
// unnecessary_underscores.dart, message
// "Unnecessary use of multiple underscores.":
//   the `__` parameter of `f`
//   the `__` local declaration in `g`
void f(int __) {}

void g() {
  var __ = 0;
}

// GOOD: a referenced local and a lone wildcard parameter pass.
void h() {
  var __ = 0;
  print(__);
}

void i(int _) {}
