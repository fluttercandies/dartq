// messages-example: analyzer/messages.yaml CompileTimeErrorCode.invalidOverride
class A {
  void m(int p) {}
}

class B extends A {
  @override
  void m(String p) {}
}
