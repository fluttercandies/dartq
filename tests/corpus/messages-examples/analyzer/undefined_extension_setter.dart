// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedExtensionSetter
extension E on String {
  set a(String v) {}
}

extension F on String {
  set b(String v) {}
}

void f() {
  E('c').b = 'd';
}
