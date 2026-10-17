// messages-example: analyzer/messages.yaml CompileTimeErrorCode.nonConstantCaseExpression
void f(int i, int j) {
  switch (i) {
    case j:
      // ...
      break;
  }
}
