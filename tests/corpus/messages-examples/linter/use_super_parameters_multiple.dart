// messages-example: linter/messages.yaml LinterLintCode.useSuperParametersMultiple
class A {
  A({int? x, int? y});
}

class B extends A {
  B({int? x, int? y}) : super(x: x, y: y);
}
