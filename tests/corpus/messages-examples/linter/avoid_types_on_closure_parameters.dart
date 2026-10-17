// messages-example: linter/messages.yaml LinterLintCode.avoidTypesOnClosureParameters
void f(List<int> l) {
  l.map((int x) => x.isEven);
}
