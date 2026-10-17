// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableSharedCaseScopeDifferentFinalityOrType
void f(Object? x) {
  switch (x) {
    case int a when a > 0:
    case 0:
      a;
  }
}
