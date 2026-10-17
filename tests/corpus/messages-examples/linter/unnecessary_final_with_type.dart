// messages-example: linter/messages.yaml LinterLintCode.unnecessaryFinalWithType
void f(int a, int b) {
  final c = a + b;
  print(c);
}
