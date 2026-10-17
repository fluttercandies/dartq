// messages-example: linter/messages.yaml LinterLintCode.preferSpreadCollections (variant 2)
List<int> f(List<int>? numbers) {
  return [0, 1]..addAll(numbers ?? const []);
}
