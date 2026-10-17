// messages-example: linter/messages.yaml LinterLintCode.avoidRenamingMethodParameters
class A {
  void m(int a) {}
}

class B extends A {
  @override
  void m(int b) {}
}
