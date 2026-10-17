// messages-example: linter/messages.yaml LinterLintCode.preferConditionalAssignment
int f(String? s) {
  if (s == null) {
    s = '';
  }
  return s.length;
}
