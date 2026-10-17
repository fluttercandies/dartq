// messages-example: linter/messages.yaml LinterLintCode.simplifyVariablePattern
void f(Object o) {
  if (o case String(length: var length)) {
    print('string is $length characters long');
  }
}
