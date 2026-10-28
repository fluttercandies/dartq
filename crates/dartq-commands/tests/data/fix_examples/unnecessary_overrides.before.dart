class A {
  void f() {}
}
class B extends A {
  @override
  void f() {
    super.f();
  }
}
