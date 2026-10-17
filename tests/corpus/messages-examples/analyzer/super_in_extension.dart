// messages-example: analyzer/messages.yaml CompileTimeErrorCode.superInExtension
extension E on Object {
  String get displayString => super.toString();
}
