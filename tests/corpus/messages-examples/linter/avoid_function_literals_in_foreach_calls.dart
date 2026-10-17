// messages-example: linter/messages.yaml LinterLintCode.avoidFunctionLiteralsInForeachCalls
void f(Iterable<String> s) {
  s.forEach((e) => print(e));
}
