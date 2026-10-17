// Helper library for the two deferred-import cases of
// prefer_const_constructors.dart (#1160): upstream
// test_deferred_arg writes class A + `const aa = A();` into the test
// package's lib/a.dart, and test_deferredConstructorCall writes class A
// alone; this merged helper carries both faces.
class A {
  const A();
}

const aa = A();
