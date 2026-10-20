// one_member_abstracts — official rule faces
// (pkg/linter one_member_abstracts.dart): an abstract class with no
// extends/with/implements clauses, no fields, and exactly one abstract
// member reports at the class name with the method name as the argument.
// The member tally is version-bound: the oracle-of-record (Dart 3.13.x)
// counts accessor members toward it, so a getter beside the lone method
// lifts the count to two and the class stays silent; the 3.15-main
// element model's `element.methods` excludes accessors. The rule is
// deprecated upstream since 3.13 but stays registered and fires.
// Expected rows: 1 — ONE_MEMBER_ABSTRACTS keyed one_member_abstracts.dart,
// message "Unnecessary use of an abstract class.":
//   the class name `A` (the lone abstract method `m`)
abstract class A {
  void m();
}

// GOOD under the 3.13.x tally: the getter counts, so `B` holds two
// members and stays silent.
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
