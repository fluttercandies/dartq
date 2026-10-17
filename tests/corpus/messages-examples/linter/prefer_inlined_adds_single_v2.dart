// messages-example: linter/messages.yaml LinterLintCode.preferInlinedAddsSingle (variant 2)
List<String> f(String a, List<String> b) {
  return [a]..addAll(b);
}
