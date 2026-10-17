// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedExtensionGetter
extension E on String {
  String get a => 'a';
}

extension F on String {
  String get b => 'b';
}

void f() {
  E('c').b;
}
