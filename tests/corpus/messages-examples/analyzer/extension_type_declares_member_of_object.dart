// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeDeclaresMemberOfObject
extension type E(int i) {
  int get hashCode => 0;
}
