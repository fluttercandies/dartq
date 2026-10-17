// one_member_abstracts — official rule faces
// (pkg/linter one_member_abstracts.dart): an abstract class with no
// extends/with/implements clauses, no fields, and exactly one abstract
// METHOD (accessors excluded) reports at the class name with the method
// name as the argument. The rule is deprecated upstream since 3.13 but
// stays registered and fires.
// Expected rows: 2 — ONE_MEMBER_ABSTRACTS keyed one_member_abstracts.dart,
// message "Unnecessary use of an abstract class.":
//   the class name `A` (the lone abstract method `m`)
//   the class name `B` (a getter beside the lone method keeps the count
//   at one)
abstract class A {
  void m();
}

abstract class B {
  int get x;
  void n();
}

// GOOD: a concrete class, an accessor-only abstract, a field, and an
// extends clause all pass.
class C {
  void m() {}
}

abstract class D {
  int get x;
}

abstract class E {
  int x = 0;
  void m();
}

class Base {}

abstract class F extends Base {
  void m();
}
