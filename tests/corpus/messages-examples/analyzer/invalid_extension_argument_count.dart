// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidExtensionArgumentCount
extension E on String {
  String join(String other) => '$this $other';
}

void f() {
  E().join('b');
}
