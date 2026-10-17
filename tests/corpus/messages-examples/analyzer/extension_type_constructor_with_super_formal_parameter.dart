// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeConstructorWithSuperFormalParameter
extension type E(int i) {
  E.n(this.i, super.foo);
}
