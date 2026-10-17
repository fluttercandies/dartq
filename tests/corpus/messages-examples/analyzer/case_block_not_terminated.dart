// messages-example: analyzer/messages.yaml CompileTimeErrorCode.caseBlockNotTerminated
void f(int x) {
  switch (x) {
    case 0:
      x += 2;
    default:
      x += 1;
  }
}
