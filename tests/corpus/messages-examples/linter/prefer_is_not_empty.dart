// messages-example: linter/messages.yaml LinterLintCode.preferIsNotEmpty
void f(Iterable<int> p) => !p.isEmpty ? p.first : 0;
