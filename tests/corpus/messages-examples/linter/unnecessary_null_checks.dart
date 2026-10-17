// messages-example: linter/messages.yaml LinterLintCode.unnecessaryNullChecks
int? f(int? i) {
  return i!;
}
