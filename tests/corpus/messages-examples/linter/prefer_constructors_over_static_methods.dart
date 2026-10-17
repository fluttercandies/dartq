// messages-example: linter/messages.yaml LinterLintCode.preferConstructorsOverStaticMethods
class C {
  final int a, b, c;
  C(this.a, this.b, this.c);
  static C all(int i) => C(i, i, i);
}
