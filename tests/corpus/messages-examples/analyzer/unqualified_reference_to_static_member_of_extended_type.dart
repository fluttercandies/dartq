// messages-example: analyzer/messages.yaml CompileTimeErrorCode.unqualifiedReferenceToStaticMemberOfExtendedType
class C {
  static void m() {}
}

extension E on C {
  void f() {
    m();
  }
}
