// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionOverrideArgumentNotAssignable
extension E on String {
  void method() {}
}

void f() {
  E(3).method();
}
