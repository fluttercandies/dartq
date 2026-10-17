// messages-example: linter/messages.yaml LinterLintCode.useNullAwareElements
void f(int? x) => [if (x != null) x];
