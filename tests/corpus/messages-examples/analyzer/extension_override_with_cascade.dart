// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionOverrideWithCascade
extension E on int {
  void m() {}
}

void f() {
  E(3)..m();
}
