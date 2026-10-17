// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableSharedCaseScopeDifferentFinalityOrType (variant 2)
void f(Object? x) {
  switch (x) {
    case int a when a > 0:
    default:
      a;
  }
}
