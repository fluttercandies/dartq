// messages-example: linter/messages.yaml LinterLintCode.unnecessaryOverrides
class C {
  int m(int x) => x;
}

class D extends C {
  @override
  int m(int x) => super.m(x);
}
