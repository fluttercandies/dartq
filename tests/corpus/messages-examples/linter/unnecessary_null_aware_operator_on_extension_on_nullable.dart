// messages-example: linter/messages.yaml LinterLintCode.unnecessaryNullAwareOperatorOnExtensionOnNullable
extension E on int? {
  int m() => 1;
}

int? f(int? i) => i?.m();
