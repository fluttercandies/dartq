// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionOverrideAccessToStaticMember
extension E on String {
  static void m() {}
}

void f() {
  E('').m();
}
