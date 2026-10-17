// messages-example: linter/messages.yaml LinterLintCode.useDeclaringParameters
class C(int i) {
  final int i;

  this : i = i;
}
