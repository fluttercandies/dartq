// messages-example: linter/messages.yaml LinterLintCode.preferIsNotOperator
String f(Object o) {
  if (!(o is String)) {
    return o.toString();
  }
  return o;
}
