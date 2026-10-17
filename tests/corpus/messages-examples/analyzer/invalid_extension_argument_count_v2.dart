// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidExtensionArgumentCount (variant 2)
extension E on String {
  String join(String other) => '$this $other';
}

void f() {
  E('a', 'b').join('c');
}
