// messages-example: linter/messages.yaml LinterLintCode.avoidDoubleAndIntChecks
void f(Object x) {
  if (x is double) {
    print('double');
  } else if (x is int) {
    print('int');
  }
}
