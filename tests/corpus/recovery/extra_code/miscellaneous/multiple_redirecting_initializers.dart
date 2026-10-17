// recovery: extra_code_test.dart/test_multipleRedirectingInitializers
class A {
  A() : this.a(), this.b();
  A.a() {}
  A.b() {}
}
