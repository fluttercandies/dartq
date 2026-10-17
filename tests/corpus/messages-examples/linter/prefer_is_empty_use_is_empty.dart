// messages-example: linter/messages.yaml LinterLintCode.preferIsEmptyUseIsEmpty
int f(Iterable<int> p) => p.length == 0 ? 0 : p.first;
