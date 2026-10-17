// messages-example: linter/messages.yaml LinterLintCode.typeInitFormals (variant 2)
class A {
  A(int a);
}

class B extends A {
  B(int super.a);
}
