// avoid_returning_this — official rule faces
// (pkg/linter avoid_returning_this.dart:68-106): a class/enum/mixin method
// whose written return type names the enclosing declaration and whose body
// hands back `this` (block: first bare `return this;`; expression: at the
// method name). Operators, `@override`, extensions, other return types,
// and a preceding non-`this` return stay silent.
// Expected rows: 3 — AVOID_RETURNING_THIS keyed
// avoid_returning_this.dart, message "Don't return 'this' from a method.":
//   `this` -> block-body face (the `this` expression)
//   `m`    -> expression-body face (the method name)
//   `this` -> enum-member block face
class A {
  A m() {
    return this;
  }

  A arrow() => this;

  A chained() {
    if (1 == 2) return this;
    return null;
  }
}

enum E {
  a;
  E e() {
    return this;
  }
}

// GOOD: operators, overrides, extensions, and non-class members skip.
class B {
  @override
  String toString() => 'B';
}

extension Ext on B {
  B self() => this;
}

class C {
  C operator +(int n) => this;
}

class D {
  Object other() {
    return this;
  }
}
