// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionDeclaresMemberOfObject
extension E on String {
  String toString() => this;
}
