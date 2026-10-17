// messages-example: analyzer/messages.yaml CompileTimeErrorCode.switchExpressionNotAssignable
void f(String s) {
  switch (s) {
    case 0:
      break;
  }
}
