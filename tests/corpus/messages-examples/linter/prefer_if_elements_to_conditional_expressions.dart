// messages-example: linter/messages.yaml LinterLintCode.preferIfElementsToConditionalExpressions
List<String> f(bool b) {
  return ['a', b ? 'c' : 'd', 'e'];
}
