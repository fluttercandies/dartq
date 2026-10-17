// messages-example: analyzer/messages.yaml CompileTimeErrorCode.switchCaseCompletesNormally
void f(int a) {
  switch (a) {
    case 0:
      print(0);
    default:
      return;
  }
}
