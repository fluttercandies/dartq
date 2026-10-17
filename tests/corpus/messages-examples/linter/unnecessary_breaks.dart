// messages-example: linter/messages.yaml LinterLintCode.unnecessaryBreaks
void f(int x) {
  switch (x) {
    case 1:
      print('one');
      break;
    case 2:
      print('two');
  }
}
