// messages-example: linter/messages.yaml LinterLintCode.voidChecks
class A<T> {
  T? value;
}

void f(A<void> a) {
  a.value = 1;
}
