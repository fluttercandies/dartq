// messages-example: analyzer/messages.yaml CompileTimeErrorCode.uncheckedUseOfNullableValueAsCondition
void f(String? s) {
  if (s.length > 3) {
    // ...
  }
}
