// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeConstructorWithSuperInvocation
extension type E(int i) {
  E.n() : i = 0, super.n();
}
