// messages-example: linter/messages.yaml LinterLintCode.preferNullAwareOperators
int? f(List<int>? p) {
  return p == null ? null : p.length;
}
