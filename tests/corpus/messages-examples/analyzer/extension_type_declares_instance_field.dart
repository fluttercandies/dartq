// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeDeclaresInstanceField
extension type E(int i) {
  final int f = 0;
}
