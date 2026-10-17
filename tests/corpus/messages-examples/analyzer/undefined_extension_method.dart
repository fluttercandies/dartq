// messages-example: analyzer/messages.yaml CompileTimeErrorCode.undefinedExtensionMethod
extension E on String {
  String a() => 'a';
}

extension F on String {
  String b() => 'b';
}

void f() {
  E('c').b();
}
