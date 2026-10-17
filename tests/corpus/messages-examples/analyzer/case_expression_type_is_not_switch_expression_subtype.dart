// messages-example: analyzer/messages.yaml CompileTimeErrorCode.caseExpressionTypeIsNotSwitchExpressionSubtype
void f(String s) {
  switch (s) {
    case 1:
      break;
  }
}
