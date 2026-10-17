// messages-example: linter/messages.yaml LinterLintCode.unnecessaryLambdas
void f(List<String> strings) {
  strings.forEach((string) {
    print(string);
  });
}
