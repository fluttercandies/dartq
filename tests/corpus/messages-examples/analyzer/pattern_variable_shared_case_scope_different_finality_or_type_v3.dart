// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableSharedCaseScopeDifferentFinalityOrType (variant 3)
void f(Object? x) {
  switch (x) {
    someLabel:
    case int a when a > 0:
      a;
    case int b when b < 0:
      continue someLabel;
  }
}
