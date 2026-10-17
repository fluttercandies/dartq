// messages-example: analyzer/messages.yaml CompileTimeErrorCode.throwOfInvalidType
void f(String? s) {
  throw s;
}
