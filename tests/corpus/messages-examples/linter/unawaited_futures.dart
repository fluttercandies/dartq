// messages-example: linter/messages.yaml LinterLintCode.unawaitedFutures
Future<void> f() async {
  g();
}

Future<int> g() => Future.value(0);
