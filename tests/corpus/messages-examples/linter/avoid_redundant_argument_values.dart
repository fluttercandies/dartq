// messages-example: linter/messages.yaml LinterLintCode.avoidRedundantArgumentValues
void g({bool b = true}) {}

void f() {
  g(b: true);
}
