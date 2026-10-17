// messages-example: analyzer/messages.yaml CompileTimeErrorCode.patternVariableSharedCaseScopeDifferentFinalityOrType (variant 4)
void f(Object? x) {
  switch (x) {
    case int a when a < 0:
    case num a when a > 0:
      a;
  }
}
