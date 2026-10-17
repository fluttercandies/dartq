// messages-example: linter/messages.yaml LinterLintCode.annotateRedeclares
class C {
  void f() {}
}

extension type E(C c) implements C {
  void f() {}
}
