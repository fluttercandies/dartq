// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superInExtensionType
extension type E(String s) {
  void m() {
    super.m();
  }
}
