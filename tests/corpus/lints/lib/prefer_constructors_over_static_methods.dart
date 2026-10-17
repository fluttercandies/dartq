// prefer_constructors_over_static_methods — official rule faces
// (pkg/linter prefer_constructors_over_static_methods.dart:66-95): a
// static method with no type parameters whose return type equals the
// enclosing non-generic class's own type, and whose body contains an
// instance creation of that type, reports at the method name. A generic
// method, a nullable return, an unrelated or subclass return, an
// extension static, and a factory all pass.
// Expected rows: 1 — PREFER_CONSTRUCTORS_OVER_STATIC_METHODS keyed
// prefer_constructors_over_static_methods.dart, message
// "Static method should be a constructor.":
//   the static method `f` returning `A` whose body creates `A.named()`
class A {
  A.named();

  static A f() => A.named();
}

// GOOD: a nullable return and an unrelated return type pass.
class B {
  B.named();

  static B? maybe() => B.named();

  static Object unrelated() => Object();
}
