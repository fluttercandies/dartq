// messages-example: analyzer/messages.yaml CompileTimeErrorCode.inconsistentPatternVariableLogicalOr (variant 2)
void f(Object? x) {
  if (x case (final int a) || (int a)) {
    print(a);
  }
}
