// messages-example: linter/messages.yaml LinterLintCode.unnecessaryNullAwareAssignments
void f(String? s) {
  s ??= null;
}
