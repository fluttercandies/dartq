// messages-example: analyzer/messages.yaml CompileTimeErrorCode.extensionTypeInheritedMemberConflict
class A {
  void m() {}
}

extension type B(A a) {
  void m() {}
}

extension type C(A a) implements A, B {}
