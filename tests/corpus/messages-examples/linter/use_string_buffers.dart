// messages-example: linter/messages.yaml LinterLintCode.useStringBuffers
String f() {
  var result = '';
  for (int i = 0; i < 10; i++) {
    result += 'a';
  }
  return result;
}
