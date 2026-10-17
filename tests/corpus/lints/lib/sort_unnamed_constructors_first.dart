// sort_unnamed_constructors_first — official rule faces
// (pkg/linter sort_unnamed_constructors_first.dart): an unnamed
// constructor that follows a named constructor in a class, enum, or
// extension-type body reports — a named constructor arms the flag and
// non-constructor members neither arm nor clear it. An unnamed factory
// carries the same shape and reports identically.
// Expected rows: 2 — SORT_UNNAMED_CONSTRUCTORS_FIRST keyed
// sort_unnamed_constructors_first.dart, message
// "Invalid location for the unnamed constructor.":
//   the unnamed `A()` after `A.named()`
//   the unnamed `E()` after `E.named()` in the enum body
class A {
  A.named();
  A();
}

enum E {
  a;
  E.named();
  E();
}

// GOOD: a sorted class passes, and an unnamed constructor after a
// non-constructor member stays silent (only constructor order is judged).
class B {
  B();
  B.named();
}

class C {
  int x = 0;
  void m() {}
  C();
}
