// messages-example: linter/messages.yaml LinterLintCode.unnecessaryStatements
void f(int Function() first, int Function() second) {
  first() + second();
}
