// sort_constructors_first — official rule faces
// (pkg/linter sort_constructors_first.dart): a constructor that follows a
// non-constructor member in a class, enum, or extension-type body
// reports — the first non-constructor member arms the flag and every
// constructor past it reports. Factories join the constructor family.
// Expected rows: 2 — SORT_CONSTRUCTORS_FIRST keyed
// sort_constructors_first.dart, message
// "Constructor declarations should be before non-constructor declarations.":
//   the `A()` constructor after the field `x`
//   the `E.named()` constructor after the field `x` in the enum body
class A {
  int x = 0;
  A();
}

enum E {
  a;
  E();
  int x = 0;
  E.named();
}

// GOOD: a sorted class passes.
class B {
  B();
  int x = 0;
}
