// messages-example: linter/messages.yaml LinterLintCode.voidChecks (variant 2)
class A<T> {
  void m(T p) {}
}

void f(A<void> a) {
  a.m(1);
}
