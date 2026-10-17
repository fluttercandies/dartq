// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableSharedCaseScopeDifferentFinalityOrType (variant 5)
void f(Object? x) {
  switch (x) {
    case final int a when a < 0:
    case int a when a > 0:
      a;
  }
}
