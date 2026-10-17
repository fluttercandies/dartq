// messages-example: analyzer/messages.yaml CompileTimeErrorCode.unqualifiedReferenceToNonLocalStaticMember
class A {
  static int x = 3;
}

class B extends A {
  int get g => x;
}
