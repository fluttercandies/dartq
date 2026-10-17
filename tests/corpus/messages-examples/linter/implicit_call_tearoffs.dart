// messages-example: linter/messages.yaml LinterLintCode.implicitCallTearoffs
class Callable {
  void call() {}
}

void callIt(void Function() f) {
  f();
}

void f() {
  callIt(Callable());
}
