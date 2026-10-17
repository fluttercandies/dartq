// messages-example: linter/messages.yaml LinterLintCode.preferFunctionDeclarationsOverVariables
void g() {
  var f = (int i) => i * 2;
  f(1);
}
