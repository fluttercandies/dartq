// messages-example: linter/messages.yaml LinterLintCode.overriddenFields
class A {
  int f = 1;
}

class B extends A {
  @override
  int f = 2;
}
