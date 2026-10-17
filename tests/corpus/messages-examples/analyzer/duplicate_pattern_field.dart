// messages-example: analyzer/messages.yaml CompileTimeErrorCode.duplicatePatternField
void f(({int a, int b}) r) {
  switch (r) {
    case (a: 1, a: 2):
      return;
  }
}
