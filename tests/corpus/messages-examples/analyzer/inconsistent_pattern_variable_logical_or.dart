// messages-example: analyzer/messages.yaml CompileTimeErrorCode.inconsistentPatternVariableLogicalOr
void f(Object? x) {
  if (x case (int a) || (double a)) {
    print(a);
  }
}
