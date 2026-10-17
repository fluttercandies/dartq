// messages-example: analyzer/messages.yaml CompileTimeErrorCode.constantPatternWithNonConstantExpression
void f(int e, int i) {
  switch (e) {
    case i:
      break;
  }
}
