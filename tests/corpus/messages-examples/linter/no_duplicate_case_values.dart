// messages-example: linter/messages.yaml LinterLintCode.noDuplicateCaseValues
// @dart = 2.19
void f(int v) {
  switch (v) {
    case 1:
      break;
    case 1:
      break;
  }
}
