// messages-example: linter/messages.yaml LinterLintCode.unnecessaryAwaitInReturn
Future<int> f(Future<int> future) async {
  return await future;
}
