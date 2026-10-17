// messages-example: linter/messages.yaml LinterLintCode.asyncReturnWithNoAwait
Future<String> futureString(Future<String> value) async {
  return value;
}
