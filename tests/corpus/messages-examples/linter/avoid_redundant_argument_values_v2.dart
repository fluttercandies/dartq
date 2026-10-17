// messages-example: linter/messages.yaml LinterLintCode.avoidRedundantArgumentValues (variant 2)
void g([int? a, int? b = 1]) {}

void f() {
  g(0, 1);
}
