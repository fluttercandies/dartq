// messages-example: linter/messages.yaml LinterLintCode.preferForeach
void f(Map<String, int> m) {
  for (final key in m.keys) {
    print(key);
  }
}
